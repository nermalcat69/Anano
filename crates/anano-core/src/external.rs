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

/// Every repository the account can reach (its own, collaborations, organisations) as
/// `owner/name`, most recently pushed first.
pub fn list_repos(login: &str) -> Result<Vec<String>, String> {
    let mut gh = gh_as(login)?;
    gh.args(["api", "user/repos?per_page=100&sort=pushed", "--paginate"])
        .args(["-q", ".[].full_name"]);
    Ok(gh_stdout(gh)?.lines().map(str::to_string).collect())
}

/// Create an empty repository on GitHub under the account; returns its HTTPS clone URL.
pub fn create_repo(login: &str, name: &str, private: bool) -> Result<String, String> {
    let mut gh = gh_as(login)?;
    gh.args(["api", "-X", "POST", "user/repos", "-f"])
        .arg(format!("name={name}"))
        .arg("-F")
        .arg(format!("private={private}"))
        .args(["-q", ".clone_url"]);
    let url = gh_stdout(gh)?.trim().to_string();
    if url.is_empty() {
        return Err("GitHub didn't return a clone URL".to_string());
    }
    Ok(url)
}

/// `true` for `https://[user@]github.com/...` only. Guards the token: it must never be
/// offered to another host just because "github.com" appears somewhere in the URL.
pub fn is_github_https(url: &str) -> bool {
    url.strip_prefix("https://")
        .and_then(|rest| rest.split('/').next())
        .and_then(|authority| authority.rsplit('@').next())
        .is_some_and(|host| host.eq_ignore_ascii_case(GITHUB_HOST))
}

/// Pin a GitHub HTTPS URL to one account by putting the login in the URL
/// (`https://login@github.com/owner/repo`). That's how a repository remembers its account:
/// the remote URL carries it, and `git::remote::auth_callbacks` looks up that login's token.
/// Anything else (SSH, other hosts, already pinned) comes back unchanged.
pub fn with_account(url: &str, login: &str) -> String {
    match url.strip_prefix("https://github.com/") {
        Some(rest) => format!("https://{login}@{GITHUB_HOST}/{rest}"),
        None => url.to_string(),
    }
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
    fn with_account_pins_only_plain_github_https_urls() {
        assert_eq!(
            with_account("https://github.com/o/r.git", "me"),
            "https://me@github.com/o/r.git"
        );
        for untouched in [
            "git@github.com:o/r.git",
            "https://other@github.com/o/r.git",
            "https://gitlab.com/o/r.git",
        ] {
            assert_eq!(with_account(untouched, "me"), untouched);
        }
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
