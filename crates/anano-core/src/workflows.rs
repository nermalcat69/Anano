//! GitHub Actions through `gh`: a repository's workflows, their runs, the jobs of a run with
//! their steps, and a job's log parsed into lines that know what they are (a step header,
//! an error, a warning, a command), so the UI can highlight them.
//!
//! Everything here only reads; nothing re-runs, cancels or edits a workflow.

use crate::external::{gh_repo, gh_stdout, parse_time, text};

/// Runs per page.
pub const RUNS_PER_PAGE: usize = 30;
/// Workflows are listed in pages of 100, up to this many pages.
const MAX_WORKFLOW_PAGES: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workflow {
    pub id: u64,
    pub name: String,
    /// `.github/workflows/ci.yml`
    pub path: String,
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowRun {
    pub id: u64,
    pub workflow_id: u64,
    /// The workflow's name.
    pub workflow: String,
    /// What the run is called on GitHub: the pull request title, the commit subject, …
    pub title: String,
    pub number: u64,
    pub event: String,
    /// `queued`, `in_progress` or `completed`, as GitHub spells them (a few more exist).
    pub status: String,
    /// Empty until the run has completed.
    pub conclusion: String,
    pub branch: String,
    pub actor: String,
    pub sha: String,
    /// Unix seconds.
    pub created_at: i64,
    pub started_at: i64,
    pub updated_at: i64,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunStep {
    pub number: u64,
    pub name: String,
    pub status: String,
    pub conclusion: String,
    pub started_at: i64,
    pub completed_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunJob {
    pub id: u64,
    pub name: String,
    pub status: String,
    pub conclusion: String,
    pub started_at: i64,
    pub completed_at: i64,
    pub steps: Vec<RunStep>,
}

/// What the status chips over the run list ask GitHub for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RunStatus {
    #[default]
    All,
    Failed,
    Succeeded,
    Running,
}

impl RunStatus {
    fn api(self) -> Option<&'static str> {
        match self {
            RunStatus::All => None,
            RunStatus::Failed => Some("failure"),
            RunStatus::Succeeded => Some("success"),
            RunStatus::Running => Some("in_progress"),
        }
    }
}

/// One page of runs and how many there are in all.
pub struct RunPage {
    pub items: Vec<WorkflowRun>,
    pub total: usize,
}

fn count(v: &serde_json::Value) -> u64 {
    v.as_u64().unwrap_or(0)
}

fn time(v: &serde_json::Value) -> i64 {
    parse_time(v.as_str().unwrap_or_default())
}

fn parse_workflows(json: &str) -> Result<(Vec<Workflow>, usize), String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let list = v["workflows"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|w| {
            Some(Workflow {
                id: w["id"].as_u64()?,
                name: text(&w["name"]),
                path: text(&w["path"]),
                active: w["state"].as_str() == Some("active"),
            })
        })
        .collect();
    Ok((list, count(&v["total_count"]) as usize))
}

fn parse_runs(json: &str) -> Result<RunPage, String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let items = v["workflow_runs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| {
            Some(WorkflowRun {
                id: r["id"].as_u64()?,
                workflow_id: count(&r["workflow_id"]),
                workflow: text(&r["name"]),
                title: match text(&r["display_title"]) {
                    t if t.is_empty() => text(&r["name"]),
                    t => t,
                },
                number: count(&r["run_number"]),
                event: text(&r["event"]),
                status: text(&r["status"]),
                conclusion: text(&r["conclusion"]),
                branch: text(&r["head_branch"]),
                actor: text(&r["actor"]["login"]),
                sha: text(&r["head_sha"]),
                created_at: time(&r["created_at"]),
                started_at: time(&r["run_started_at"]),
                updated_at: time(&r["updated_at"]),
                url: text(&r["html_url"]),
            })
        })
        .collect();
    Ok(RunPage {
        items,
        total: count(&v["total_count"]) as usize,
    })
}

fn parse_jobs(json: &str) -> Result<Vec<RunJob>, String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    Ok(v["jobs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|j| {
            Some(RunJob {
                id: j["id"].as_u64()?,
                name: text(&j["name"]),
                status: text(&j["status"]),
                conclusion: text(&j["conclusion"]),
                started_at: time(&j["started_at"]),
                completed_at: time(&j["completed_at"]),
                steps: j["steps"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|s| RunStep {
                        number: count(&s["number"]),
                        name: text(&s["name"]),
                        status: text(&s["status"]),
                        conclusion: text(&s["conclusion"]),
                        started_at: time(&s["started_at"]),
                        completed_at: time(&s["completed_at"]),
                    })
                    .collect(),
            })
        })
        .collect())
}

/// `gh api --method GET <path> -F k=v …` for the repository behind `origin`.
fn api(origin: &str, path: &str, fields: &[(&str, String)]) -> Result<String, String> {
    let (mut gh, slug) = gh_repo(origin)?;
    gh.args(["api", "--method", "GET", &format!("repos/{slug}/{path}")]);
    for (key, value) in fields {
        gh.arg("-F").arg(format!("{key}={value}"));
    }
    gh_stdout(gh)
}

/// Every workflow of the repository (up to 300), in the order GitHub lists them.
pub fn list_workflows(origin: &str) -> Result<Vec<Workflow>, String> {
    let mut all = Vec::new();
    for page in 1..=MAX_WORKFLOW_PAGES {
        let json = api(
            origin,
            "actions/workflows",
            &[("per_page", "100".into()), ("page", page.to_string())],
        )?;
        let (list, total) = parse_workflows(&json)?;
        let got = list.len();
        all.extend(list);
        if got < 100 || all.len() >= total {
            break;
        }
    }
    Ok(all)
}

/// One page (1-based) of runs, newest first: of every workflow or of one, narrowed by status.
pub fn list_runs(
    origin: &str,
    workflow: Option<u64>,
    status: RunStatus,
    page: usize,
) -> Result<RunPage, String> {
    let path = match workflow {
        Some(id) => format!("actions/workflows/{id}/runs"),
        None => "actions/runs".to_string(),
    };
    let mut fields = vec![
        ("per_page", RUNS_PER_PAGE.to_string()),
        ("page", page.to_string()),
    ];
    if let Some(status) = status.api() {
        fields.push(("status", status.to_string()));
    }
    parse_runs(&api(origin, &path, &fields)?)
}

/// The jobs of a run (the latest attempt), each with its steps.
pub fn run_jobs(origin: &str, run: u64) -> Result<Vec<RunJob>, String> {
    parse_jobs(&api(
        origin,
        &format!("actions/runs/{run}/jobs"),
        &[("per_page", "100".into()), ("filter", "latest".into())],
    )?)
}

/// What a line of a job log is, for highlighting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogKind {
    Plain,
    /// `##[group]…`: where a step (or a folded block of output) starts.
    Group,
    Error,
    Warning,
    Notice,
    /// `##[command]…`: the command a step ran.
    Command,
    Debug,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
    pub kind: LogKind,
    /// `07:51:27`: the time of day GitHub stamped the line with.
    pub time: Option<String>,
    pub text: String,
}

/// A job's log, fetched and parsed. Logs expire on GitHub (and need access to the
/// repository's Actions); its message comes back as the error.
pub fn job_log(origin: &str, job: u64) -> Result<Vec<LogLine>, String> {
    let (mut gh, slug) = gh_repo(origin)?;
    // Logs carry terminal colour codes, which gh refuses to print unless told it may;
    // `parse_log` strips them.
    gh.args(["api", "--method", "GET", "--allow-escape-sequences"])
        .arg(format!("repos/{slug}/actions/jobs/{job}/logs"));
    Ok(parse_log(&gh_stdout(gh)?))
}

/// Remove terminal colour / cursor escape sequences.
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        // `ESC [ … <letter>`; a bare ESC is simply dropped.
        if chars.peek() == Some(&'[') {
            chars.next();
            for next in chars.by_ref() {
                if ('@'..='~').contains(&next) {
                    break;
                }
            }
        }
    }
    out
}

/// `2026-11-17T07:51:27.7969050Z rest` → (`07:51:27`, `rest`).
fn split_timestamp(line: &str) -> (Option<String>, &str) {
    let b = line.as_bytes();
    let stamped = b.len() > 21
        && b[4] == b'-'
        && b[7] == b'-'
        && b[10] == b'T'
        && b[13] == b':'
        && b[16] == b':';
    if !stamped {
        return (None, line);
    }
    match line.find("Z ") {
        Some(end) if end >= 19 => (Some(line[11..19].to_string()), &line[end + 2..]),
        _ => (None, line),
    }
}

fn classify(rest: &str) -> Option<(LogKind, &str)> {
    for (marker, kind) in [
        ("##[group]", LogKind::Group),
        ("##[error]", LogKind::Error),
        ("##[warning]", LogKind::Warning),
        ("##[notice]", LogKind::Notice),
        ("##[command]", LogKind::Command),
        ("##[debug]", LogKind::Debug),
    ] {
        if let Some(text) = rest.strip_prefix(marker) {
            return Some((kind, text));
        }
    }
    // Output no marker covers: tools that print their own errors. Narrow on purpose, so
    // "0 failed" and the like stay plain.
    let low = rest.trim_start().to_ascii_lowercase();
    let starts = |prefixes: &[&str]| prefixes.iter().any(|p| low.starts_with(p));
    if starts(&["error:", "error[", "fatal:", "npm err!"]) || rest.contains("panicked at") {
        return Some((LogKind::Error, rest));
    }
    if starts(&["warning:", "warning[", "npm warn"]) {
        return Some((LogKind::Warning, rest));
    }
    None
}

/// Split a raw job log into lines that know what they are. A leading byte-order mark and
/// colour codes are removed; `##[endgroup]` lines (which only close a block) are dropped.
pub fn parse_log(raw: &str) -> Vec<LogLine> {
    let raw = raw.strip_prefix('\u{feff}').unwrap_or(raw);
    let mut lines = Vec::new();
    for line in raw.lines() {
        let line = strip_ansi(line.trim_end_matches('\r'));
        let (time, rest) = split_timestamp(&line);
        if rest.starts_with("##[endgroup]") {
            continue;
        }
        let (kind, text) = classify(rest).unwrap_or((LogKind::Plain, rest));
        lines.push(LogLine {
            kind,
            time,
            text: text.to_string(),
        });
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_lines_know_what_they_are() {
        let raw = "\u{feff}2026-11-17T07:51:27.7969050Z Current runner version: '2.338.2'\r
2026-11-17T07:51:27.7984377Z ##[group]GITHUB_TOKEN Permissions
2026-11-17T07:51:27.7987726Z Actions: read
2026-11-17T07:51:27.7990000Z ##[endgroup]
2026-11-17T07:51:28.1000000Z ##[command]cargo build
2026-11-17T07:51:28.2000000Z \u{1b}[31mwarning: unused variable\u{1b}[0m
2026-11-17T07:51:28.3000000Z thread 'main' panicked at src/lib.rs:1:1
2026-11-17T07:51:28.4000000Z test result: ok. 3 passed; 0 failed
2026-11-17T07:51:28.8381545Z ##[error]Process completed with exit code 1.
no timestamp here
";
        let lines = parse_log(raw);
        let summary: Vec<_> = lines
            .iter()
            .map(|l| (l.kind, l.time.as_deref(), l.text.as_str()))
            .collect();
        assert_eq!(
            summary,
            [
                (
                    LogKind::Plain,
                    Some("07:51:27"),
                    "Current runner version: '2.338.2'"
                ),
                (LogKind::Group, Some("07:51:27"), "GITHUB_TOKEN Permissions"),
                (LogKind::Plain, Some("07:51:27"), "Actions: read"),
                (LogKind::Command, Some("07:51:28"), "cargo build"),
                (
                    LogKind::Warning,
                    Some("07:51:28"),
                    "warning: unused variable"
                ),
                (
                    LogKind::Error,
                    Some("07:51:28"),
                    "thread 'main' panicked at src/lib.rs:1:1"
                ),
                // "0 failed" is a summary, not a failure.
                (
                    LogKind::Plain,
                    Some("07:51:28"),
                    "test result: ok. 3 passed; 0 failed"
                ),
                (
                    LogKind::Error,
                    Some("07:51:28"),
                    "Process completed with exit code 1."
                ),
                (LogKind::Plain, None, "no timestamp here"),
            ]
        );
    }

    #[test]
    fn runs_jobs_and_workflows_parse() {
        let runs = r#"{"total_count":412,"workflow_runs":[
          {"id":9,"workflow_id":3,"name":"CI","display_title":"Fix the thing","run_number":57,
           "event":"pull_request","status":"completed","conclusion":"failure",
           "head_branch":"feat","head_sha":"abcdef0123","actor":{"login":"me"},
           "created_at":"2026-09-14T23:16:15Z","run_started_at":"2026-09-14T23:16:20Z",
           "updated_at":"2026-09-14T23:20:00Z","html_url":"https://github.com/o/r/actions/runs/9"},
          {"id":8,"workflow_id":3,"name":"CI","display_title":"","run_number":56,"event":"push",
           "status":"in_progress","conclusion":null,"head_branch":"main","head_sha":"1234567",
           "actor":{"login":"you"},"created_at":"2026-09-14T00:00:00Z",
           "run_started_at":"2026-09-14T00:00:01Z","updated_at":"2026-09-14T00:00:02Z",
           "html_url":"u"}]}"#;
        let page = parse_runs(runs).unwrap();
        assert_eq!(page.total, 412);
        assert_eq!(page.items[0].title, "Fix the thing");
        assert_eq!(page.items[0].conclusion, "failure");
        assert_eq!(page.items[0].created_at, 1_789_427_775);
        // No title falls back to the workflow's name; no conclusion yet is empty.
        assert_eq!(page.items[1].title, "CI");
        assert_eq!(page.items[1].conclusion, "");

        let jobs = r#"{"jobs":[{"id":5,"name":"build","status":"completed","conclusion":"success",
          "started_at":"2026-09-14T00:00:00Z","completed_at":"2026-09-14T00:01:00Z",
          "steps":[{"number":1,"name":"Set up job","status":"completed","conclusion":"success",
                    "started_at":"2026-09-14T00:00:00Z","completed_at":"2026-09-14T00:00:02Z"}]}]}"#;
        let jobs = parse_jobs(jobs).unwrap();
        assert_eq!((jobs[0].name.as_str(), jobs[0].steps.len()), ("build", 1));
        assert_eq!(jobs[0].completed_at - jobs[0].started_at, 60);

        let wf = r#"{"total_count":2,"workflows":[
          {"id":1,"name":"CI","path":".github/workflows/ci.yml","state":"active"},
          {"id":2,"name":"Old","path":".github/workflows/old.yml","state":"disabled_manually"}]}"#;
        let (list, total) = parse_workflows(wf).unwrap();
        assert_eq!(total, 2);
        assert!(list[0].active && !list[1].active);
    }
}
