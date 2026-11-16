//! External CLIs the app leans on instead of reimplementing them (same idea as `git/lfs.rs`
//! shelling out to `git-lfs`):
//!
//! - `gh`: GitHub accounts. `gh` already does multi-account sign-in and keeps the tokens in
//!   the OS keychain, so "connected accounts" are simply the accounts `gh` is signed into.
//!   Nothing secret is ever written to our own settings files.
//! - `nermal`: the Nermal IDE's CLI, used to open a repository's directory in a new IDE window.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, Command, Stdio};

const GITHUB_HOST: &str = "github.com";
/// Where the user types the one-time code from [`PendingLogin::code`].
pub const GITHUB_DEVICE_URL: &str = "https://github.com/login/device";

/// Find a CLI on `PATH`, then in `fallbacks`. Apps launched from Finder/Dock get a bare
/// `PATH` without Homebrew, hence the fallbacks.
fn tool_path(name: &str, fallbacks: &[&str]) -> Option<PathBuf> {
    let file = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    std::env::var_os("PATH")
        .iter()
        .flat_map(std::env::split_paths)
        .map(|dir| dir.join(&file))
        .chain(fallbacks.iter().map(PathBuf::from))
        .find(|p| p.is_file())
}

fn gh() -> Result<Command, String> {
    tool_path("gh", &["/opt/homebrew/bin/gh", "/usr/local/bin/gh"])
        .map(Command::new)
        .ok_or_else(|| "the GitHub CLI (gh) isn't installed".to_string())
}

fn nermal() -> Result<Command, String> {
    tool_path(
        "nermal",
        &[
            "/Applications/nermal.app/Contents/MacOS/nermal",
            "/opt/homebrew/bin/nermal",
        ],
    )
    .map(|path| {
        // Started from a Nermal terminal, we'd inherit its pane/workspace address and the
        // CLI would act on that workspace instead of the repository's.
        let mut cmd = Command::new(path);
        cmd.env_remove("nermal_WS").env_remove("nermal_PANE");
        cmd
    })
    .ok_or_else(|| "the Nermal CLI (nermal) isn't installed".to_string())
}

pub fn gh_available() -> bool {
    gh().is_ok()
}

pub fn nermal_available() -> bool {
    nermal().is_ok()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub login: String,
    /// The account plain `gh` / `git` commands use by default.
    pub active: bool,
}

/// GitHub accounts `gh` is signed into. Empty when `gh` is missing or signed out.
pub fn list_accounts() -> Vec<Account> {
    // `gh auth status` exits non-zero if any account's token went bad; stdout is still valid.
    gh().ok()
        .and_then(|mut gh| gh.args(["auth", "status", "--json", "hosts"]).output().ok())
        .map(|out| parse_accounts(&out.stdout))
        .unwrap_or_default()
}

fn parse_accounts(json: &[u8]) -> Vec<Account> {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(json) else {
        return Vec::new();
    };
    value["hosts"][GITHUB_HOST]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|a| a["state"] == "success")
        .filter_map(|a| {
            Some(Account {
                login: a["login"].as_str()?.to_string(),
                active: a["active"].as_bool().unwrap_or(false),
            })
        })
        .collect()
}

/// The OAuth token of one signed-in account, straight from `gh`'s keychain entry.
pub fn token(login: &str) -> Option<String> {
    let out = gh()
        .ok()?
        .args(["auth", "token", "--hostname", GITHUB_HOST, "--user", login])
        .output()
        .ok()?;
    let token = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (out.status.success() && !token.is_empty()).then_some(token)
}

/// `gh`, acting as one specific connected account.
fn gh_as(login: &str) -> Result<Command, String> {
    let token = token(login).ok_or_else(|| format!("{login} isn't signed in to gh"))?;
    let mut gh = gh()?;
    gh.env("GH_TOKEN", token);
    Ok(gh)
}

fn gh_stdout(mut gh: Command) -> Result<String, String> {
    let out = gh.output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    /// `owner/name`.
    pub name: String,
    pub private: bool,
}

fn parse_repos(lines: &str) -> Vec<Repo> {
    lines
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .map(|(private, name)| Repo {
            name: name.to_string(),
            private: private == "true",
        })
        .collect()
}

/// Every repository the account can reach (its own, collaborations, organisations),
/// most recently pushed first.
pub fn list_repos(login: &str) -> Result<Vec<Repo>, String> {
    let mut gh = gh_as(login)?;
    gh.args(["api", "user/repos?per_page=100&sort=pushed", "--paginate"])
        .args(["-q", r#".[] | "\(.private)\t\(.full_name)""#]);
    let lines = gh_stdout(gh)?;
    if let Some(file) = repos_cache_file(login)
        && let Some(dir) = file.parent()
        && std::fs::create_dir_all(dir).is_ok()
    {
        let _ = std::fs::write(file, &lines);
    }
    Ok(parse_repos(&lines))
}

/// Create an empty repository on GitHub under the account; returns its HTTPS clone URL.
pub fn create_repo(
    login: &str,
    name: &str,
    description: &str,
    private: bool,
) -> Result<String, String> {
    let mut gh = gh_as(login)?;
    gh.args(["api", "-X", "POST", "user/repos", "-f"])
        .arg(format!("name={name}"))
        .arg("-f")
        .arg(format!("description={description}"))
        .arg("-F")
        .arg(format!("private={private}"))
        .args(["-q", ".clone_url"]);
    let url = gh_stdout(gh)?.trim().to_string();
    if url.is_empty() {
        return Err("GitHub didn't return a clone URL".to_string());
    }
    Ok(url)
}

/// What the "Create repository" dialog offers under Git Ignore and License. Both lists
/// come from GitHub's own template API, the same source github.com's "new repository"
/// page uses, so nothing is bundled or goes stale here.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Templates {
    /// Template names, e.g. `Rust`, `Node`.
    pub gitignores: Vec<String>,
    /// `(key, display name)`, e.g. `("mit", "MIT License")`.
    pub licenses: Vec<(String, String)>,
}

fn gh_api(path: &str, query: &str) -> Result<String, String> {
    let mut gh = gh()?;
    gh.args(["api", path, "-q", query]);
    gh_stdout(gh)
}

pub fn templates() -> Result<Templates, String> {
    let gitignores = gh_api("gitignore/templates", ".[]")?;
    let licenses = gh_api("licenses", r#".[] | "\(.key)\t\(.name)""#)?;
    Ok(Templates {
        gitignores: gitignores.lines().map(str::to_string).collect(),
        licenses: licenses
            .lines()
            .filter_map(|l| l.split_once('\t'))
            .map(|(key, name)| (key.to_string(), name.to_string()))
            .collect(),
    })
}

/// The contents of one `.gitignore` template.
pub fn gitignore_source(name: &str) -> Result<String, String> {
    gh_api(&format!("gitignore/templates/{name}"), ".source")
}

/// The text of one license, with GitHub's `[year]` / `[fullname]` placeholders filled in.
pub fn license_body(key: &str, year: i64, holder: &str) -> Result<String, String> {
    Ok(gh_api(&format!("licenses/{key}"), ".body")?
        .replace("[year]", &year.to_string())
        .replace("[fullname]", holder))
}

/// `owner/name` for a GitHub remote: what `gh --repo` and the REST API call a repository.
pub fn repo_slug(origin: &str) -> Option<String> {
    let path = github_repo_path(origin)?;
    let path = path
        .strip_suffix(".git")
        .unwrap_or(path)
        .trim_end_matches('/');
    (!path.is_empty()).then(|| path.to_string())
}

/// One row of the pull request list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    pub author: String,
    /// `OPEN`, `MERGED` or `CLOSED`, as GitHub spells them.
    pub state: String,
    pub draft: bool,
    pub head: String,
    pub base: String,
    /// Unix seconds.
    pub created_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrEventKind {
    Commit,
    Comment,
    /// A review: `text` starts with its verdict (`APPROVED`, `CHANGES_REQUESTED`, …).
    Review,
}

/// One entry of a pull request's history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrEvent {
    pub kind: PrEventKind,
    pub author: String,
    pub at: i64,
    pub text: String,
}

/// Everything shown when a pull request is opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequestDetail {
    pub summary: PullRequest,
    /// The description, as Markdown.
    pub body: String,
    pub url: String,
    pub additions: u64,
    pub deletions: u64,
    pub changed_files: u64,
    /// GitHub's own verdicts, passed through untouched: `mergeable` is `MERGEABLE`,
    /// `CONFLICTING` or `UNKNOWN`; `merge_state` is `CLEAN`, `BLOCKED`, `BEHIND`, `DIRTY`,
    /// `DRAFT`, `UNSTABLE`, `HAS_HOOKS` or `UNKNOWN`.
    pub mergeable: String,
    pub merge_state: String,
    /// Commits, comments and reviews, oldest first.
    pub events: Vec<PrEvent>,
}

/// `2026-09-14T23:16:15Z` → Unix seconds (GitHub always answers in UTC, to the second).
fn parse_time(iso: &str) -> i64 {
    let num = |range: std::ops::Range<usize>| -> i64 {
        iso.get(range).and_then(|s| s.parse().ok()).unwrap_or(0)
    };
    let (y, m, d) = (num(0..4), num(5..7), num(8..10));
    if m == 0 || d == 0 {
        return 0;
    }
    // Days from civil (Howard Hinnant's algorithm).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    days * 86_400 + num(11..13) * 3600 + num(14..16) * 60 + num(17..19)
}

fn text(v: &serde_json::Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

fn parse_pull_request(v: &serde_json::Value) -> Option<PullRequest> {
    Some(PullRequest {
        number: v["number"].as_u64()?,
        title: text(&v["title"]),
        author: text(&v["author"]["login"]),
        state: text(&v["state"]),
        draft: v["isDraft"].as_bool().unwrap_or(false),
        head: text(&v["headRefName"]),
        base: text(&v["baseRefName"]),
        created_at: parse_time(v["createdAt"].as_str().unwrap_or_default()),
    })
}

fn parse_pull_request_detail(v: &serde_json::Value) -> Option<PullRequestDetail> {
    let list = |key: &str| v[key].as_array().cloned().unwrap_or_default();
    let mut events: Vec<PrEvent> = Vec::new();
    for c in list("commits") {
        let author = &c["authors"][0];
        let oid = c["oid"].as_str().unwrap_or_default();
        events.push(PrEvent {
            kind: PrEventKind::Commit,
            author: text(if author["login"].is_string() {
                &author["login"]
            } else {
                &author["name"]
            }),
            at: parse_time(c["committedDate"].as_str().unwrap_or_default()),
            text: format!(
                "{} {}",
                &oid[..7.min(oid.len())],
                text(&c["messageHeadline"])
            ),
        });
    }
    for c in list("comments") {
        events.push(PrEvent {
            kind: PrEventKind::Comment,
            author: text(&c["author"]["login"]),
            at: parse_time(c["createdAt"].as_str().unwrap_or_default()),
            text: text(&c["body"]),
        });
    }
    for r in list("reviews") {
        let body = text(&r["body"]);
        let verdict = text(&r["state"]);
        events.push(PrEvent {
            kind: PrEventKind::Review,
            author: text(&r["author"]["login"]),
            at: parse_time(r["submittedAt"].as_str().unwrap_or_default()),
            text: if body.is_empty() {
                verdict
            } else {
                format!("{verdict}\n{body}")
            },
        });
    }
    events.sort_by_key(|e| e.at);
    Some(PullRequestDetail {
        summary: parse_pull_request(v)?,
        body: text(&v["body"]),
        url: text(&v["url"]),
        additions: v["additions"].as_u64().unwrap_or(0),
        deletions: v["deletions"].as_u64().unwrap_or(0),
        changed_files: v["changedFiles"].as_u64().unwrap_or(0),
        mergeable: text(&v["mergeable"]),
        merge_state: text(&v["mergeStateStatus"]),
        events,
    })
}

/// `gh pr …` for the repository behind `origin`, as the account that remote is pinned to
/// (or gh's default account when it isn't pinned).
fn gh_pr(origin: &str) -> Result<(Command, String), String> {
    let slug = repo_slug(origin).ok_or("this repository's origin isn't on GitHub")?;
    let mut gh = match account_of(origin) {
        Some(login) => gh_as(login)?,
        None => gh()?,
    };
    gh.arg("pr");
    Ok((gh, slug))
}

const PR_FIELDS: &str = "number,title,author,state,isDraft,headRefName,baseRefName,createdAt";

/// The repository's pull requests, newest first, open and closed alike.
pub fn list_pull_requests(origin: &str) -> Result<Vec<PullRequest>, String> {
    let (mut gh, slug) = gh_pr(origin)?;
    gh.args(["list", "--repo", &slug, "--state", "all", "--limit", "100"])
        .args(["--json", PR_FIELDS]);
    let json: serde_json::Value =
        serde_json::from_str(&gh_stdout(gh)?).map_err(|e| e.to_string())?;
    Ok(json
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(parse_pull_request)
        .collect())
}

pub fn pull_request(origin: &str, number: u64) -> Result<PullRequestDetail, String> {
    let (mut gh, slug) = gh_pr(origin)?;
    let fields = format!(
        "{PR_FIELDS},body,url,additions,deletions,changedFiles,mergeable,mergeStateStatus,\
         commits,comments,reviews"
    );
    gh.args([
        "view",
        &number.to_string(),
        "--repo",
        &slug,
        "--json",
        &fields,
    ]);
    let json: serde_json::Value =
        serde_json::from_str(&gh_stdout(gh)?).map_err(|e| e.to_string())?;
    parse_pull_request_detail(&json).ok_or_else(|| "GitHub returned no pull request".to_string())
}

/// Merge a pull request with a merge commit. Whatever GitHub answers when it refuses
/// (conflicts, required reviews, failing checks, no permission) comes back as the error.
pub fn merge_pull_request(origin: &str, number: u64) -> Result<(), String> {
    let (mut gh, slug) = gh_pr(origin)?;
    gh.args(["merge", &number.to_string(), "--repo", &slug, "--merge"]);
    gh_stdout(gh).map(|_| ())
}

/// The page for one commit on github.com, when `origin` is a GitHub remote.
pub fn commit_web_url(origin: &str, sha: &str) -> Option<String> {
    let path = github_repo_path(origin)?;
    let path = path
        .strip_suffix(".git")
        .unwrap_or(path)
        .trim_end_matches('/');
    Some(format!("https://{GITHUB_HOST}/{path}/commit/{sha}"))
}

/// A commit author's GitHub profile picture, as a local file.
///
/// GitHub resolves avatars by commit email (the lookup GitHub Desktop uses); an email it
/// doesn't know gets its generic identicon. Each email is downloaded once into the user
/// cache directory and reused from then on. Note this sends the email to GitHub.
pub fn avatar(email: &str) -> Option<PathBuf> {
    use std::hash::{Hash, Hasher};
    if email.is_empty() {
        return None;
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    email.to_lowercase().hash(&mut hasher);
    let dir = cache_dir()?.join("avatars");
    let stem = format!("{:016x}", hasher.finish());
    // Stored with the extension matching its real format (the image loader goes by it).
    for ext in ["png", "jpg", "gif"] {
        let cached = dir.join(format!("{stem}.{ext}"));
        if cached.is_file() {
            return Some(cached);
        }
    }
    std::fs::create_dir_all(&dir).ok()?;
    let escaped: String = email
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect();
    let part = dir.join(format!("{stem}.part"));
    let fetched = Command::new("curl")
        .args(["-sfL", "--max-time", "10", "-o"])
        .arg(&part)
        .arg(format!(
            "https://avatars.githubusercontent.com/u/e?email={escaped}&s=64"
        ))
        .status()
        .is_ok_and(|s| s.success());
    let bytes = std::fs::read(&part).unwrap_or_default();
    let ext = match bytes.as_slice() {
        [0x89, b'P', b'N', b'G', ..] => Some("png"),
        [0xFF, 0xD8, ..] => Some("jpg"),
        [b'G', b'I', b'F', b'8', ..] => Some("gif"),
        _ => None,
    };
    if let (true, Some(ext)) = (fetched, ext) {
        let file = dir.join(format!("{stem}.{ext}"));
        if std::fs::rename(&part, &file).is_ok() {
            return Some(file);
        }
    }
    let _ = std::fs::remove_file(&part);
    None
}

/// Where downloaded things that can always be fetched again live.
fn cache_dir() -> Option<PathBuf> {
    Some(directories::BaseDirs::new()?.cache_dir().join("Anano"))
}

fn repos_cache_file(login: &str) -> Option<PathBuf> {
    // Logins are [A-Za-z0-9-], but never trust a name into a path.
    let safe: String = login
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    Some(cache_dir()?.join("repos").join(format!("{safe}.tsv")))
}

/// The repository list saved by the last successful [`list_repos`] for this account, so
/// the clone dialog can show something at once and refresh behind it.
pub fn cached_repos(login: &str) -> Option<Vec<Repo>> {
    let text = std::fs::read_to_string(repos_cache_file(login)?).ok()?;
    Some(parse_repos(&text))
}

/// `true` for `https://[user@]github.com/...` only. Guards the token: it must never be
/// offered to another host just because "github.com" appears somewhere in the URL.
pub fn is_github_https(url: &str) -> bool {
    url.strip_prefix("https://")
        .and_then(|rest| rest.split('/').next())
        .and_then(|authority| authority.rsplit('@').next())
        .is_some_and(|host| host.eq_ignore_ascii_case(GITHUB_HOST))
}

/// `owner/repo.git` out of any GitHub remote URL: `https://[user@]github.com/…`,
/// `git@github.com:…` or `ssh://git@github.com/…`.
fn github_repo_path(url: &str) -> Option<&str> {
    if is_github_https(url) {
        return url["https://".len()..]
            .split_once('/')
            .map(|(_, path)| path);
    }
    url.strip_prefix("git@github.com:")
        .or_else(|| url.strip_prefix("ssh://git@github.com/"))
}

/// Pin a GitHub remote to one account by putting the login in an HTTPS URL
/// (`https://login@github.com/owner/repo`). That's how a repository remembers its account:
/// the remote URL carries it, and `git::remote::auth_callbacks` looks up that login's token.
/// SSH remotes are switched to HTTPS (the token is an HTTPS credential), an existing pin is
/// replaced, and non-GitHub URLs come back unchanged.
pub fn with_account(url: &str, login: &str) -> String {
    match github_repo_path(url) {
        Some(path) => format!("https://{login}@{GITHUB_HOST}/{path}"),
        None => url.to_string(),
    }
}

/// The login a GitHub HTTPS remote is pinned to, if any.
pub fn account_of(url: &str) -> Option<&str> {
    if !is_github_https(url) {
        return None;
    }
    let authority = url["https://".len()..].split('/').next()?;
    authority.rsplit_once('@').map(|(login, _)| login)
}

/// Whether `url` is a GitHub remote an account can be picked for.
pub fn is_github(url: &str) -> bool {
    github_repo_path(url).is_some()
}

/// A `gh auth login` device flow waiting for the user to enter [`Self::code`] in the browser.
pub struct PendingLogin {
    pub code: String,
    child: Child,
    stderr: BufReader<ChildStderr>,
    previously_active: Option<String>,
}

/// Start signing in one more GitHub account. Returns as soon as `gh` prints the one-time
/// code; [`PendingLogin::finish`] then blocks until the browser step is done.
pub fn start_login() -> Result<PendingLogin, String> {
    let previously_active = list_accounts()
        .into_iter()
        .find(|a| a.active)
        .map(|a| a.login);
    // No TTY on stdin => gh runs non-interactively: prints the code + URL, then polls.
    let mut child = gh()?
        .args(["auth", "login", "--hostname", GITHUB_HOST])
        .args(["--git-protocol", "https", "--web", "--skip-ssh-key"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut stderr = BufReader::new(child.stderr.take().expect("stderr is piped"));
    let mut seen = String::new();
    let mut line = String::new();
    while stderr.read_line(&mut line).map_err(|e| e.to_string())? > 0 {
        if let Some(code) = line.split("one-time code: ").nth(1) {
            return Ok(PendingLogin {
                code: code.trim().to_string(),
                child,
                stderr,
                previously_active,
            });
        }
        seen.push_str(&line);
        line.clear();
    }
    let _ = child.wait();
    Err(format!("gh auth login failed: {}", seen.trim()))
}

impl PendingLogin {
    // ponytail: no cancel. If the user walks away, gh gives up by itself when the device
    // code expires (~15 min). Keep the `Child` in the UI state and kill it if that matters.
    pub fn finish(mut self) -> Result<(), String> {
        let mut rest = String::new();
        let _ = std::io::Read::read_to_string(&mut self.stderr, &mut rest);
        let status = self.child.wait().map_err(|e| e.to_string())?;
        if !status.success() {
            return Err(format!("gh auth login failed: {}", rest.trim()));
        }
        // gh makes the newest login the active one. Put the old default back so adding an
        // account here doesn't silently change which account the user's terminal uses.
        if let (Some(login), Ok(mut gh)) = (self.previously_active, gh()) {
            let _ = gh
                .args([
                    "auth",
                    "switch",
                    "--hostname",
                    GITHUB_HOST,
                    "--user",
                    &login,
                ])
                .output();
        }
        Ok(())
    }
}

/// Nermal quietly falls back to the home directory when handed a folder that doesn't exist
/// (a repository that was moved or deleted), so refuse those up front.
fn check_dir(path: &Path) -> Result<&Path, String> {
    if path.is_dir() {
        Ok(path)
    } else {
        Err(format!("{} no longer exists", path.display()))
    }
}

/// Open `path` in a new Nermal IDE window. In Nermal a window shows one workspace, so this
/// is `nermal new <path> --open`: a workspace rooted at the directory plus a window on it.
/// The workspace is left unnamed so Nermal labels it by its folder.
pub fn open_in_nermal(path: &Path) -> Result<(), String> {
    let out = nermal()?
        .arg("new")
        .arg(check_dir(path)?)
        .args(["--open", "--json"])
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    // `--open` needs a running GUI; if there was none, launch it at the directory instead.
    let created: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_default();
    if created["opened"] != true {
        nermal()?.arg(path).spawn().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_accounts_reads_gh_status_json_and_skips_broken_logins() {
        let json = br#"{"hosts":{"github.com":[
            {"state":"success","active":true,"login":"one"},
            {"state":"success","active":false,"login":"two"},
            {"state":"error","active":false,"login":"expired"}],
            "ghe.example.com":[{"state":"success","active":true,"login":"work"}]}}"#;
        let accounts = parse_accounts(json);
        assert_eq!(
            accounts,
            vec![
                Account {
                    login: "one".into(),
                    active: true
                },
                Account {
                    login: "two".into(),
                    active: false
                },
            ]
        );
        assert!(parse_accounts(b"not json").is_empty());
    }

    #[test]
    fn commit_web_url_handles_every_github_remote_shape() {
        for origin in [
            "https://github.com/o/r.git",
            "https://me@github.com/o/r",
            "git@github.com:o/r.git",
        ] {
            assert_eq!(
                commit_web_url(origin, "abc").as_deref(),
                Some("https://github.com/o/r/commit/abc"),
                "{origin}"
            );
        }
        assert_eq!(commit_web_url("https://gitlab.com/o/r.git", "abc"), None);
    }

    #[test]
    fn pull_request_json_becomes_a_detail_with_its_history_in_order() {
        assert_eq!(parse_time("1970-01-02T00:00:01Z"), 86_401);
        assert_eq!(parse_time("2026-09-14T23:16:15Z"), 1_789_427_775);
        let json: serde_json::Value = serde_json::from_str(
            r#"{"number":6,"title":"T","author":{"login":"me"},"state":"OPEN","isDraft":false,
            "headRefName":"feat","baseRefName":"main","createdAt":"2026-09-14T23:16:15Z",
            "body":"hello","url":"https://github.com/o/r/pull/6","additions":4,"deletions":1,
            "changedFiles":2,"mergeable":"MERGEABLE","mergeStateStatus":"CLEAN",
            "commits":[{"oid":"abcdef0123","messageHeadline":"do it",
                "committedDate":"2026-09-14T10:00:00Z","authors":[{"login":"me","name":"Me"}]}],
            "comments":[{"author":{"login":"you"},"body":"nice","createdAt":"2026-09-15T10:00:00Z"}],
            "reviews":[{"author":{"login":"you"},"body":"","state":"APPROVED",
                "submittedAt":"2026-09-14T12:00:00Z"}]}"#,
        )
        .unwrap();
        let pr = parse_pull_request_detail(&json).unwrap();
        assert_eq!((pr.summary.number, pr.summary.head.as_str()), (6, "feat"));
        assert_eq!(
            (pr.mergeable.as_str(), pr.merge_state.as_str()),
            ("MERGEABLE", "CLEAN")
        );
        let kinds: Vec<_> = pr.events.iter().map(|e| e.kind).collect();
        assert_eq!(
            kinds,
            [
                PrEventKind::Commit,
                PrEventKind::Review,
                PrEventKind::Comment
            ]
        );
        assert_eq!(pr.events[0].text, "abcdef0 do it");
        assert_eq!(pr.events[1].text, "APPROVED");
        assert_eq!(
            repo_slug("https://me@github.com/o/r.git").as_deref(),
            Some("o/r")
        );
    }

    #[test]
    fn parse_repos_reads_the_private_flag_and_full_name() {
        assert_eq!(
            parse_repos("true\tme/secret\nfalse\torg/open\n"),
            vec![
                Repo {
                    name: "me/secret".into(),
                    private: true
                },
                Repo {
                    name: "org/open".into(),
                    private: false
                },
            ]
        );
    }

    #[test]
    fn with_account_pins_any_github_remote_and_nothing_else() {
        for url in [
            "https://github.com/o/r.git",
            "https://other@github.com/o/r.git",
            "git@github.com:o/r.git",
            "ssh://git@github.com/o/r.git",
        ] {
            assert_eq!(
                with_account(url, "me"),
                "https://me@github.com/o/r.git",
                "{url}"
            );
        }
        assert_eq!(
            with_account("https://gitlab.com/o/r.git", "me"),
            "https://gitlab.com/o/r.git"
        );
        assert_eq!(account_of("https://me@github.com/o/r.git"), Some("me"));
        assert_eq!(account_of("https://github.com/o/r.git"), None);
        assert_eq!(account_of("https://me@gitlab.com/o/r.git"), None);
    }

    #[test]
    fn github_token_is_only_offered_to_github_over_https() {
        assert!(is_github_https("https://github.com/o/r"));
        assert!(is_github_https("https://me@github.com/o/r.git"));
        assert!(!is_github_https("http://github.com/o/r"));
        assert!(!is_github_https("https://github.com.evil.io/o/r"));
        assert!(!is_github_https("https://evil.io/github.com/r"));
        assert!(!is_github_https("https://github.com@evil.io/o/r"));
    }
}
