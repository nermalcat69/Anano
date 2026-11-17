//! Browsing GitHub as one of the connected accounts, without any repository being cloned:
//! the account's repositories, the organizations it belongs to and their repositories, and a
//! repository's commit history with the changes of each commit. Everything only reads.

use crate::external::{gh_as, gh_stdout, parse_time, text};
use crate::git::{PatchFile, parse_patch};

/// Items per page.
pub const PAGE: usize = 30;

/// A repository as GitHub lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteRepo {
    /// `owner/name`
    pub full_name: String,
    pub name: String,
    pub owner: String,
    pub description: String,
    pub private: bool,
    pub fork: bool,
    pub archived: bool,
    pub language: String,
    pub stars: u64,
    /// Unix seconds of the last push.
    pub pushed_at: i64,
}

impl RemoteRepo {
    pub fn clone_url(&self) -> String {
        format!("https://github.com/{}.git", self.full_name)
    }
}

/// An organization the account belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Org {
    pub login: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteCommit {
    pub sha: String,
    /// The whole message: subject, blank line, body.
    pub message: String,
    pub author: String,
    /// The author's GitHub login, when GitHub knows who they are.
    pub login: String,
    pub at: i64,
}

fn parse_repos(json: &str) -> Result<Vec<RemoteRepo>, String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    Ok(v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| {
            Some(RemoteRepo {
                full_name: r["full_name"].as_str()?.to_string(),
                name: text(&r["name"]),
                owner: text(&r["owner"]["login"]),
                description: text(&r["description"]),
                private: r["private"].as_bool().unwrap_or(false),
                fork: r["fork"].as_bool().unwrap_or(false),
                archived: r["archived"].as_bool().unwrap_or(false),
                language: text(&r["language"]),
                stars: r["stargazers_count"].as_u64().unwrap_or(0),
                pushed_at: parse_time(r["pushed_at"].as_str().unwrap_or_default()),
            })
        })
        .collect())
}

fn parse_orgs(json: &str) -> Result<Vec<Org>, String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    Ok(v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|o| {
            Some(Org {
                login: o["login"].as_str()?.to_string(),
                description: text(&o["description"]),
            })
        })
        .collect())
}

fn parse_commits(json: &str) -> Result<Vec<RemoteCommit>, String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    Ok(v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| {
            Some(RemoteCommit {
                sha: c["sha"].as_str()?.to_string(),
                message: text(&c["commit"]["message"]),
                author: text(&c["commit"]["author"]["name"]),
                login: text(&c["author"]["login"]),
                at: parse_time(c["commit"]["author"]["date"].as_str().unwrap_or_default()),
            })
        })
        .collect())
}

/// `owner/name` and nothing else: it ends up in an API path.
fn safe_slug(slug: &str) -> Result<&str, String> {
    let ok = slug.split('/').count() == 2
        && slug
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/'))
        && !slug.contains("..");
    if ok {
        Ok(slug)
    } else {
        Err(format!("not a repository name: {slug}"))
    }
}

/// A bare login, as the API wants it.
fn safe_login(login: &str) -> Result<&str, String> {
    let ok = !login.is_empty()
        && login
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'));
    if ok {
        Ok(login)
    } else {
        Err(format!("not a login: {login}"))
    }
}

/// `gh api` as `login`, with `-F key=value` fields (and extra headers).
fn api(
    login: &str,
    path: &str,
    fields: &[(&str, String)],
    accept: Option<&str>,
) -> Result<String, String> {
    let mut gh = gh_as(login)?;
    gh.args(["api", "--method", "GET", path]);
    for (key, value) in fields {
        gh.arg("-F").arg(format!("{key}={value}"));
    }
    if let Some(accept) = accept {
        gh.args(["-H", &format!("Accept: {accept}")]);
    }
    gh_stdout(gh)
}

fn page_fields(page: usize) -> [(&'static str, String); 2] {
    [("per_page", PAGE.to_string()), ("page", page.to_string())]
}

/// One page (1-based) of the repositories the account owns, most recently pushed first.
pub fn account_repos(login: &str, page: usize) -> Result<Vec<RemoteRepo>, String> {
    let [a, b] = page_fields(page);
    parse_repos(&api(
        login,
        "user/repos",
        &[
            ("affiliation", "owner".into()),
            ("sort", "pushed".into()),
            a,
            b,
        ],
        None,
    )?)
}

/// The organizations the account belongs to.
pub fn orgs(login: &str) -> Result<Vec<Org>, String> {
    parse_orgs(&api(
        login,
        "user/orgs",
        &[("per_page", "100".into())],
        None,
    )?)
}

/// One page (1-based) of an organization's repositories, as far as `login` can see them.
pub fn org_repos(login: &str, org: &str, page: usize) -> Result<Vec<RemoteRepo>, String> {
    let org = safe_login(org)?;
    let [a, b] = page_fields(page);
    parse_repos(&api(
        login,
        &format!("orgs/{org}/repos"),
        &[("type", "all".into()), ("sort", "pushed".into()), a, b],
        None,
    )?)
}

/// One page (1-based) of a repository's commits on its default branch, newest first.
pub fn commits(login: &str, slug: &str, page: usize) -> Result<Vec<RemoteCommit>, String> {
    let slug = safe_slug(slug)?;
    parse_commits(&api(
        login,
        &format!("repos/{slug}/commits"),
        &page_fields(page),
        None,
    )?)
}

/// What one commit changed, file by file, with its lines numbered on both sides.
pub fn commit_files(login: &str, slug: &str, sha: &str) -> Result<Vec<PatchFile>, String> {
    let slug = safe_slug(slug)?;
    if !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("not a commit: {sha}"));
    }
    Ok(parse_patch(&api(
        login,
        &format!("repos/{slug}/commits/{sha}"),
        &[],
        Some("application/vnd.github.diff"),
    )?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repositories_organizations_and_commits_parse() {
        let repos = r#"[{"full_name":"o/r","name":"r","owner":{"login":"o"},"description":null,
            "private":true,"fork":false,"archived":false,"language":"Rust","stargazers_count":3,
            "pushed_at":"2026-09-14T23:16:15Z"},
            {"full_name":"o/s","name":"s","owner":{"login":"o"},"description":"A thing",
            "private":false,"fork":true,"archived":true,"language":null,"stargazers_count":0,
            "pushed_at":null}]"#;
        let repos = parse_repos(repos).unwrap();
        assert_eq!(repos[0].description, "");
        assert!(repos[0].private && !repos[0].fork);
        assert_eq!((repos[0].language.as_str(), repos[0].stars), ("Rust", 3));
        assert_eq!(repos[0].pushed_at, 1_789_427_775);
        assert!(repos[1].fork && repos[1].archived);
        assert_eq!(repos[1].clone_url(), "https://github.com/o/s.git");

        let orgs =
            parse_orgs(r#"[{"login":"Ornn-AI","description":"Compute"},{"login":"x"}]"#).unwrap();
        assert_eq!(
            (orgs[0].login.as_str(), orgs[0].description.as_str()),
            ("Ornn-AI", "Compute")
        );
        assert_eq!(orgs[1].description, "");

        let commits = r#"[{"sha":"abc123","commit":{"message":"Fix it\n\nLonger story",
            "author":{"name":"Me","date":"2026-09-14T23:16:15Z"}},"author":{"login":"me"}},
            {"sha":"def456","commit":{"message":"Orphan","author":{"name":"Them","date":"2026-09-14T00:00:00Z"}},
            "author":null}]"#;
        let commits = parse_commits(commits).unwrap();
        assert_eq!(commits[0].message, "Fix it\n\nLonger story");
        assert_eq!(commits[0].login, "me");
        assert_eq!(commits[1].login, "");
    }

    #[test]
    fn names_that_reach_an_api_path_are_checked() {
        assert!(safe_slug("o/r.git-x_1").is_ok());
        for bad in ["o", "o/r/x", "o/../x", "o/r?x=1", "o/r x", ""] {
            assert!(safe_slug(bad).is_err(), "{bad:?}");
        }
        assert!(safe_login("Ornn-AI").is_ok());
        for bad in ["", "a/b", "a b", "a?b"] {
            assert!(safe_login(bad).is_err(), "{bad:?}");
        }
    }
}
