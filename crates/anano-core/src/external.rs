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
pub(crate) fn gh_as(login: &str) -> Result<Command, String> {
    let token = token(login).ok_or_else(|| format!("{login} isn't signed in to gh"))?;
    let mut gh = gh()?;
    gh.env("GH_TOKEN", token);
    Ok(gh)
}

pub(crate) fn gh_stdout(mut gh: Command) -> Result<String, String> {
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
pub(crate) fn parse_time(iso: &str) -> i64 {
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

pub(crate) fn text(v: &serde_json::Value) -> String {
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

/// `gh` for the repository behind `origin`, as the account that repository uses, plus the
/// repository's `owner/name`.
pub(crate) fn gh_repo(origin: &str) -> Result<(Command, String), String> {
    let slug = repo_slug(origin).ok_or("this repository's origin isn't on GitHub")?;
    // Same account the repository syncs as (pin, then owner, then default).
    let gh = match accounts_for(origin, &list_accounts()).first() {
        Some(account) => gh_as(&account.login)?,
        None => gh()?,
    };
    Ok((gh, slug))
}

/// `gh pr …` for the repository behind `origin`.
fn gh_pr(origin: &str) -> Result<(Command, String), String> {
    let (mut gh, slug) = gh_repo(origin)?;
    gh.arg("pr");
    Ok((gh, slug))
}

const PR_FIELDS: &str = "number,title,author,state,isDraft,headRefName,baseRefName,createdAt";

/// Pull requests per page.
pub const PULLS_PER_PAGE: usize = 30;
/// GitHub's search never returns more than this many results for one query.
pub const SEARCH_CAP: usize = 1000;
/// The most pages one revalidation re-reads; deeper ones are loaded on demand.
const MAX_REVALIDATE_PAGES: usize = 10;

/// Which pull requests a query wants. "Closed" means closed without being merged; a draft
/// counts as open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PullState {
    #[default]
    Open,
    Merged,
    Closed,
    All,
}

impl PullState {
    fn qualifier(self) -> &'static str {
        match self {
            PullState::Open => " is:open",
            PullState::Merged => " is:merged",
            PullState::Closed => " is:closed -is:merged",
            PullState::All => "",
        }
    }

    fn file_tag(self) -> &'static str {
        match self {
            PullState::Open => "open",
            PullState::Merged => "merged",
            PullState::Closed => "closed",
            PullState::All => "all",
        }
    }
}

/// A pull request query: state, optionally one author, optionally free text (matched
/// against titles and descriptions by GitHub's search).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PullSearch {
    pub state: PullState,
    pub author: Option<String>,
    pub text: String,
}

impl PullSearch {
    /// No author, no text: the one query whose results are cached on disk.
    pub fn is_plain(&self) -> bool {
        self.author.is_none() && self.text.trim().is_empty()
    }

    /// The query string for GitHub's search, scoped to one repository. A typed
    /// `repo:` / `org:` / `user:` is dropped so a search can't wander out of it.
    fn query(&self, slug: &str) -> String {
        let mut q = format!("repo:{slug} is:pr{}", self.state.qualifier());
        if let Some(author) = &self.author {
            // Bots are `name[bot]` in lists but `app/name` to the search.
            let author = match author.strip_suffix("[bot]") {
                Some(name) => format!("app/{name}"),
                None => author.clone(),
            };
            q.push_str(&format!(" author:{author}"));
        }
        for word in self.text.split_whitespace() {
            let lower = word.to_ascii_lowercase();
            if ["repo:", "org:", "user:"]
                .iter()
                .any(|p| lower.starts_with(p))
            {
                continue;
            }
            q.push(' ');
            q.push_str(word);
        }
        q
    }
}

/// How many pull requests each state holds for a query: what the tabs show, and the real
/// totals (a page of the list only knows about the pages loaded so far).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PullCounts {
    pub open: usize,
    pub merged: usize,
    pub closed: usize,
}

impl PullCounts {
    pub fn all(&self) -> usize {
        self.open + self.merged + self.closed
    }
}

/// One page of results and how many there are in all.
pub struct PullPage {
    pub items: Vec<PullRequest>,
    pub total: usize,
}

/// One item of GitHub's issue search, which pull requests are a kind of. Search reports
/// merged pull requests as `closed`; `pull_request.merged_at` is what tells them apart.
/// It carries no branch names, which only the detail view needs.
fn parse_search_item(v: &serde_json::Value) -> Option<PullRequest> {
    let state = match (
        v["state"].as_str()?,
        v["pull_request"]["merged_at"].is_string(),
    ) {
        ("open", _) => "OPEN",
        (_, true) => "MERGED",
        _ => "CLOSED",
    };
    Some(PullRequest {
        number: v["number"].as_u64()?,
        title: text(&v["title"]),
        author: text(&v["user"]["login"]),
        state: state.to_string(),
        draft: v["draft"].as_bool().unwrap_or(false),
        head: String::new(),
        base: String::new(),
        created_at: parse_time(v["created_at"].as_str().unwrap_or_default()),
    })
}

fn parse_search_page(json: &str) -> Result<PullPage, String> {
    let value: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    Ok(PullPage {
        items: value["items"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(parse_search_item)
            .collect(),
        total: value["total_count"].as_u64().unwrap_or(0) as usize,
    })
}

fn search_issues(
    origin: &str,
    query: &str,
    per_page: usize,
    page: usize,
) -> Result<String, String> {
    let (mut gh, _) = gh_repo(origin)?;
    gh.args(["api", "--method", "GET", "search/issues"])
        .arg("-f")
        .arg(format!("q={query}"))
        .args(["-f", "sort=created", "-f", "order=desc"])
        .arg("-F")
        .arg(format!("per_page={per_page}"))
        .arg("-F")
        .arg(format!("page={page}"));
    gh_stdout(gh)
}

/// One page (1-based) of the pull requests matching `search`, newest first.
pub fn search_pull_requests(
    origin: &str,
    search: &PullSearch,
    page: usize,
) -> Result<PullPage, String> {
    let slug = repo_slug(origin).ok_or("this repository's origin isn't on GitHub")?;
    parse_search_page(&search_issues(
        origin,
        &search.query(&slug),
        PULLS_PER_PAGE,
        page,
    )?)
}

/// Open / merged / closed totals for an author and text, three cheap searches run side by
/// side.
pub fn pull_counts(origin: &str, author: Option<&str>, text: &str) -> Result<PullCounts, String> {
    let total = |state: PullState| -> Result<usize, String> {
        let search = PullSearch {
            state,
            author: author.map(str::to_string),
            text: text.to_string(),
        };
        search_pull_requests_total(origin, &search)
    };
    std::thread::scope(|scope| {
        let open = scope.spawn(|| total(PullState::Open));
        let merged = scope.spawn(|| total(PullState::Merged));
        let closed = total(PullState::Closed);
        let joined = |h: std::thread::ScopedJoinHandle<'_, Result<usize, String>>| {
            h.join()
                .unwrap_or_else(|_| Err("a count thread panicked".to_string()))
        };
        Ok(PullCounts {
            open: joined(open)?,
            merged: joined(merged)?,
            closed: closed?,
        })
    })
}

fn search_pull_requests_total(origin: &str, search: &PullSearch) -> Result<usize, String> {
    let slug = repo_slug(origin).ok_or("this repository's origin isn't on GitHub")?;
    Ok(parse_search_page(&search_issues(origin, &search.query(&slug), 1, 1)?)?.total)
}

/// Re-read the first `pages` pages from GitHub. For a plain query this also replaces
/// what was cached: the "revalidate" half of stale-while-revalidate, where the caller
/// shows the cached list at once and swaps this one in, so a pull request that merged
/// since is updated wherever it sits in the pages already loaded. Returns the list and the
/// total number of matches.
pub fn refresh_pull_requests(
    origin: &str,
    search: &PullSearch,
    pages: usize,
) -> Result<(Vec<PullRequest>, usize), String> {
    let mut all = Vec::new();
    let mut total = 0;
    for page in 1..=pages.clamp(1, MAX_REVALIDATE_PAGES) {
        let result = search_pull_requests(origin, search, page)?;
        total = result.total;
        let got = result.items.len();
        all.extend(result.items);
        if got < PULLS_PER_PAGE || all.len() >= total.min(SEARCH_CAP) {
            break;
        }
    }
    if search.is_plain() {
        save_pull_cache(origin, search.state, &all, total);
    }
    Ok((all, total))
}

/// Fetch page `page` and append what isn't in `current` yet (a pull request opened since
/// the first page was read shifts later ones down a page, so duplicates are expected).
pub fn more_pull_requests(
    origin: &str,
    search: &PullSearch,
    page: usize,
    current: Vec<PullRequest>,
) -> Result<(Vec<PullRequest>, usize), String> {
    let result = search_pull_requests(origin, search, page)?;
    let mut all = current;
    for item in result.items {
        if !all.iter().any(|pr| pr.number == item.number) {
            all.push(item);
        }
    }
    if search.is_plain() {
        save_pull_cache(origin, search.state, &all, result.total);
    }
    Ok((all, result.total))
}

fn pull_cache_file(origin: &str, state: PullState) -> Option<PathBuf> {
    // owner/name → a filename; never trust a repository name into a path.
    let safe: String = repo_slug(origin)?
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    Some(
        cache_dir()?
            .join("pulls")
            .join(format!("{safe}-{}.json", state.file_tag())),
    )
}

fn pulls_to_json(list: &[PullRequest], total: usize) -> String {
    let prs: Vec<_> = list
        .iter()
        .map(|pr| {
            serde_json::json!({
                "number": pr.number, "title": pr.title, "author": pr.author,
                "state": pr.state, "draft": pr.draft, "head": pr.head,
                "base": pr.base, "created_at": pr.created_at,
            })
        })
        .collect();
    serde_json::json!({ "total": total, "prs": prs }).to_string()
}

fn pulls_from_json(json: &str) -> Option<(Vec<PullRequest>, usize)> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let list = v["prs"]
        .as_array()?
        .iter()
        .filter_map(|p| {
            Some(PullRequest {
                number: p["number"].as_u64()?,
                title: text(&p["title"]),
                author: text(&p["author"]),
                state: text(&p["state"]),
                draft: p["draft"].as_bool().unwrap_or(false),
                head: text(&p["head"]),
                base: text(&p["base"]),
                created_at: p["created_at"].as_i64().unwrap_or(0),
            })
        })
        .collect();
    Some((list, v["total"].as_u64().unwrap_or(0) as usize))
}

fn save_pull_cache(origin: &str, state: PullState, list: &[PullRequest], total: usize) {
    if let Some(file) = pull_cache_file(origin, state)
        && let Some(dir) = file.parent()
        && std::fs::create_dir_all(dir).is_ok()
    {
        // Write-then-rename: a crash mid-write must not leave half a cache behind.
        let part = file.with_extension("part");
        if std::fs::write(&part, pulls_to_json(list, total)).is_ok() {
            let _ = std::fs::rename(part, file);
        }
    }
}

/// What the last successful plain fetch saved for this repository and state: shown at once
/// while a fresh copy is fetched behind it. May be stale, never wrong about a pull
/// request's existence.
pub fn cached_pull_requests(origin: &str, state: PullState) -> Option<(Vec<PullRequest>, usize)> {
    pulls_from_json(&read_fresh(&pull_cache_file(origin, state)?)?)
}

/// How long a cached pull request page stays usable. Past it the cache is ignored (and
/// then replaced by the next successful fetch).
pub const PULL_CACHE_DAYS: u64 = 30;

/// The file's text, unless it is missing or older than [`PULL_CACHE_DAYS`].
fn read_fresh(file: &Path) -> Option<String> {
    let age = std::fs::metadata(file)
        .ok()?
        .modified()
        .ok()?
        .elapsed()
        .unwrap_or_default();
    (age <= std::time::Duration::from_secs(PULL_CACHE_DAYS * 24 * 3600))
        .then(|| std::fs::read_to_string(file).ok())?
}

fn pull_detail_file(origin: &str, number: u64) -> Option<PathBuf> {
    let safe: String = repo_slug(origin)?
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    Some(
        cache_dir()?
            .join("pull-details")
            .join(format!("{safe}-{number}.json")),
    )
}

/// A pull request opened before (within [`PULL_CACHE_DAYS`]): shown at once while the
/// current version is fetched behind it.
pub fn cached_pull_request(origin: &str, number: u64) -> Option<PullRequestDetail> {
    let json: serde_json::Value =
        serde_json::from_str(&read_fresh(&pull_detail_file(origin, number)?)?).ok()?;
    parse_pull_request_detail(&json)
}

fn save_pull_detail(origin: &str, number: u64, raw: &str) {
    if let Some(file) = pull_detail_file(origin, number)
        && let Some(dir) = file.parent()
        && std::fs::create_dir_all(dir).is_ok()
    {
        let part = file.with_extension("part");
        if std::fs::write(&part, raw).is_ok() {
            let _ = std::fs::rename(part, file);
        }
    }
}

fn authors_cache_file(origin: &str) -> Option<PathBuf> {
    let safe: String = repo_slug(origin)?
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    Some(cache_dir()?.join("authors").join(format!("{safe}.txt")))
}

/// The people the author filter offers: the repository's contributors, most active first
/// (up to 300), saved for next time.
pub fn pull_authors(origin: &str) -> Result<Vec<String>, String> {
    let (_, slug) = gh_repo(origin)?;
    let mut logins: Vec<String> = Vec::new();
    for page in 1..=3 {
        let (mut gh, _) = gh_repo(origin)?;
        gh.args([
            "api",
            "--method",
            "GET",
            &format!("repos/{slug}/contributors"),
        ])
        .args(["-F", "per_page=100", "-F"])
        .arg(format!("page={page}"))
        .args(["-q", ".[].login"]);
        let got: Vec<String> = gh_stdout(gh)?.lines().map(str::to_string).collect();
        let full = got.len() == 100;
        logins.extend(got);
        if !full {
            break;
        }
    }
    if let Some(file) = authors_cache_file(origin)
        && let Some(dir) = file.parent()
        && std::fs::create_dir_all(dir).is_ok()
    {
        let _ = std::fs::write(file, logins.join("\n"));
    }
    Ok(logins)
}

/// The author list saved by the last visit (instant; may be stale).
pub fn cached_pull_authors(origin: &str) -> Option<Vec<String>> {
    let text = read_fresh(&authors_cache_file(origin)?)?;
    Some(text.lines().map(str::to_string).collect())
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
    let raw = gh_stdout(gh)?;
    let json: serde_json::Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    let detail = parse_pull_request_detail(&json)
        .ok_or_else(|| "GitHub returned no pull request".to_string())?;
    save_pull_detail(origin, number, &raw);
    Ok(detail)
}

/// The whole pull request as a unified diff, already split into numbered files.
pub fn pull_request_files(origin: &str, number: u64) -> Result<Vec<crate::git::PatchFile>, String> {
    let (mut gh, slug) = gh_pr(origin)?;
    gh.args([
        "diff",
        &number.to_string(),
        "--repo",
        &slug,
        "--color",
        "never",
    ]);
    Ok(crate::git::parse_patch(&gh_stdout(gh)?))
}

/// How a review ends, as GitHub's three buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReviewVerdict {
    /// Feedback with no verdict.
    #[default]
    Comment,
    Approve,
    RequestChanges,
}

/// Submit a review of a pull request as the account the repository uses. GitHub requires
/// words for a plain comment or a request for changes (an approval may be silent), and
/// refuses reviews it doesn't allow (your own pull request, no access): its message comes
/// back as the error.
pub fn submit_review(
    origin: &str,
    number: u64,
    verdict: ReviewVerdict,
    body: &str,
) -> Result<(), String> {
    let body = body.trim();
    if body.is_empty() && verdict != ReviewVerdict::Approve {
        return Err("Write a message first: GitHub needs one for this kind of review.".into());
    }
    let (mut gh, slug) = gh_pr(origin)?;
    gh.args(["review", &number.to_string(), "--repo", &slug]);
    gh.arg(match verdict {
        ReviewVerdict::Comment => "--comment",
        ReviewVerdict::Approve => "--approve",
        ReviewVerdict::RequestChanges => "--request-changes",
    });
    if !body.is_empty() {
        gh.args(["--body", body]);
    }
    gh_stdout(gh).map(|_| ())
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
    if email.is_empty() {
        return None;
    }
    let escaped: String = email
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect();
    cached_image(
        &email.to_lowercase(),
        &format!("https://avatars.githubusercontent.com/u/e?email={escaped}&s=64"),
    )
}

/// A commit email's picture if it has been downloaded before (no network).
pub fn cached_avatar(email: &str) -> Option<PathBuf> {
    cached_file(&email.to_lowercase())
}

/// A connected account's own profile picture (toolbar account button), cached the same way.
pub fn account_avatar(login: &str) -> Option<PathBuf> {
    // Bots come as `dependabot[bot]` (REST) or `app/dependabot` (gh); the picture lives
    // under the bare name.
    let login = login.strip_prefix("app/").unwrap_or(login);
    let login = login.strip_suffix("[bot]").unwrap_or(login);
    // Logins are [A-Za-z0-9-]; anything else has no business in a URL path.
    if login.is_empty() || !login.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return None;
    }
    cached_image(
        &format!("login:{}", login.to_lowercase()),
        &format!("https://github.com/{login}.png?size=64"),
    )
}

/// Where the picture for `key` lives in the avatar cache: the folder and the file stem.
fn image_slot(key: &str) -> Option<(PathBuf, String)> {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut hasher);
    Some((
        cache_dir()?.join("avatars"),
        format!("{:016x}", hasher.finish()),
    ))
}

/// The picture for `key` if it is already on disk. Stored with the extension matching its
/// real format (the image loader goes by it).
fn cached_file(key: &str) -> Option<PathBuf> {
    let (dir, stem) = image_slot(key)?;
    ["png", "jpg", "gif"]
        .into_iter()
        .map(|ext| dir.join(format!("{stem}.{ext}")))
        .find(|file| file.is_file())
}

/// A login's picture if it has been downloaded before: no network, safe to call while
/// drawing a list, so logos that are already known show up on the first frame.
pub fn cached_account_avatar(login: &str) -> Option<PathBuf> {
    let login = login.strip_prefix("app/").unwrap_or(login);
    let login = login.strip_suffix("[bot]").unwrap_or(login);
    if login.is_empty() || !login.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return None;
    }
    cached_file(&format!("login:{}", login.to_lowercase()))
}

/// Download `url` once into the avatar cache under a name derived from `key`, and hand
/// back the local file from then on.
fn cached_image(key: &str, url: &str) -> Option<PathBuf> {
    if let Some(found) = cached_file(key) {
        return Some(found);
    }
    let (dir, stem) = image_slot(key)?;
    std::fs::create_dir_all(&dir).ok()?;
    let part = dir.join(format!("{stem}.part"));
    let fetched = Command::new("curl")
        .args(["-sfL", "--max-time", "10", "-o"])
        .arg(&part)
        .arg(url)
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
pub(crate) fn cache_dir() -> Option<PathBuf> {
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

/// The connected accounts in the order a repository would try them: the one it is pinned
/// to (login in the URL); otherwise the one that owns it (`github.com/<login>/…`), since
/// that is almost always the account you want for your own repositories; then the
/// terminal's default; then the rest. The first entry is "the account this repository uses"
/// as far as the UI is concerned.
pub fn accounts_for(url: &str, accounts: &[Account]) -> Vec<Account> {
    let pinned = account_of(url);
    let owner = repo_slug(url).and_then(|slug| slug.split('/').next().map(str::to_string));
    let rank = |a: &Account| {
        let is = |login: Option<&str>| login.is_some_and(|l| l.eq_ignore_ascii_case(&a.login));
        if is(pinned) {
            0
        } else if is(owner.as_deref()) {
            1
        } else if a.active {
            2
        } else {
            3
        }
    };
    let mut ordered = accounts.to_vec();
    ordered.sort_by_key(rank);
    // A pin names exactly one account; don't fall through to others behind its back.
    if pinned.is_some() {
        ordered.truncate(1);
    }
    ordered
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
    fn a_repository_uses_its_pin_then_its_owner_then_the_default_account() {
        let accounts = [
            Account {
                login: "arjunowo".into(),
                active: true,
            },
            Account {
                login: "nermalcat69".into(),
                active: false,
            },
        ];
        let first = |url: &str| accounts_for(url, &accounts)[0].login.clone();
        // Unpinned: whoever owns the repository, even if another account is the default.
        assert_eq!(
            first("https://github.com/nermalcat69/Anano.git"),
            "nermalcat69"
        );
        assert_eq!(first("https://github.com/arjunowo/site.git"), "arjunowo");
        assert_eq!(first("git@github.com:NermalCat69/Anano.git"), "nermalcat69");
        // Someone else's repository: the default account.
        assert_eq!(first("https://github.com/rust-lang/rust.git"), "arjunowo");
        // A pin always wins, and nothing else is tried behind it.
        let pinned = accounts_for(
            "https://arjunowo@github.com/nermalcat69/Anano.git",
            &accounts,
        );
        assert_eq!(pinned.len(), 1);
        assert_eq!(pinned[0].login, "arjunowo");
    }

    #[test]
    fn cached_pull_pages_expire_after_thirty_days() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("x.json");
        std::fs::write(&file, "{}").unwrap();
        assert_eq!(read_fresh(&file).as_deref(), Some("{}"));
        let age = |days: u64| {
            let f = std::fs::File::options().write(true).open(&file).unwrap();
            f.set_modified(
                std::time::SystemTime::now() - std::time::Duration::from_secs(days * 86_400 + 60),
            )
            .unwrap();
        };
        age(29);
        assert!(read_fresh(&file).is_some());
        age(31);
        assert!(read_fresh(&file).is_none());
        assert!(read_fresh(&dir.path().join("missing")).is_none());
    }

    #[test]
    fn cached_logos_are_looked_up_without_the_network_and_only_for_real_logins() {
        // Nothing here may touch a path or a URL: odd names are refused outright.
        for bad in ["", "a/b", "a b", "../x", "a?b"] {
            assert_eq!(cached_account_avatar(bad), None, "{bad:?}");
        }
        // A well-formed login nobody has downloaded is simply absent, not an error.
        assert_eq!(
            cached_account_avatar("anano-test-nobody-has-this-login-xyz"),
            None
        );
        // Bots resolve to the same file as their bare name.
        assert_eq!(
            cached_account_avatar("app/anano-test-nobody-bot"),
            cached_account_avatar("anano-test-nobody-bot[bot]")
        );
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
    fn search_results_parse_merged_closed_and_open_apart() {
        let page = r#"{"total_count":57,"items":[
          {"number":3,"title":"c","state":"open","draft":true,"user":{"login":"me"},
           "created_at":"2026-09-14T23:16:15Z","pull_request":{"merged_at":null}},
          {"number":2,"title":"b","state":"closed","draft":false,"user":{"login":"you"},
           "created_at":"2026-09-13T00:00:00Z","pull_request":{"merged_at":"2026-09-15T00:00:00Z"}},
          {"number":1,"title":"a","state":"closed","draft":false,"user":{"login":"you"},
           "created_at":"2026-09-12T00:00:00Z","pull_request":{"merged_at":null}}]}"#;
        let result = parse_search_page(page).unwrap();
        assert_eq!(result.total, 57, "the real total, not just this page");
        let states: Vec<_> = result
            .items
            .iter()
            .map(|p| (p.number, p.state.as_str(), p.draft))
            .collect();
        assert_eq!(
            states,
            [
                (3, "OPEN", true),
                (2, "MERGED", false),
                (1, "CLOSED", false)
            ]
        );
        assert_eq!(result.items[0].created_at, 1_789_427_775);
    }

    #[test]
    fn search_queries_are_scoped_to_the_repository() {
        let q = |state, author: Option<&str>, text: &str| {
            PullSearch {
                state,
                author: author.map(str::to_string),
                text: text.to_string(),
            }
            .query("o/r")
        };
        assert_eq!(q(PullState::Open, None, ""), "repo:o/r is:pr is:open");
        assert_eq!(q(PullState::Merged, None, ""), "repo:o/r is:pr is:merged");
        assert_eq!(
            q(PullState::Closed, None, ""),
            "repo:o/r is:pr is:closed -is:merged"
        );
        assert_eq!(
            q(PullState::All, Some("me"), "fix  bug"),
            "repo:o/r is:pr author:me fix bug"
        );
        assert_eq!(
            q(PullState::Open, Some("dependabot[bot]"), ""),
            "repo:o/r is:pr is:open author:app/dependabot"
        );
        // A typed repo: / org: / user: can't take the search somewhere else.
        assert_eq!(
            q(PullState::All, None, "fix repo:evil/x ORG:evil"),
            "repo:o/r is:pr fix"
        );
        assert!(PullSearch::default().is_plain());
        assert!(
            !PullSearch {
                text: "x".into(),
                ..Default::default()
            }
            .is_plain()
        );
    }

    #[test]
    fn the_pull_request_cache_round_trips() {
        let list = vec![PullRequest {
            number: 7,
            title: "T \"quoted\"".into(),
            author: "me".into(),
            state: "MERGED".into(),
            draft: false,
            head: String::new(),
            base: String::new(),
            created_at: 1_789_427_775,
        }];
        let (back, total) = pulls_from_json(&pulls_to_json(&list, 412)).unwrap();
        assert_eq!(back, list);
        assert_eq!(total, 412);
        assert_eq!(pulls_from_json("not json"), None);
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
