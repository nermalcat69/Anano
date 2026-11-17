//! Running Claude Code (the `claude` CLI) on a repository and following what it does.
//!
//! It runs non-interactively (`claude -p`) with `--output-format stream-json`, so every
//! step arrives as one line of JSON: what it says, which tool it uses, how it ended. This
//! module starts it, turns those lines into [`ClaudeLine`]s for the UI to show, and can stop
//! it. It is a log you can read, not a terminal you can type into: Claude Code cannot ask
//! questions here, so it is told what to do up front and what it may run.

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// What one step of a run looks like on screen.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ClaudeLine {
    /// Something Claude said.
    Text(String),
    /// A tool it used: `Bash` with the command, `Edit` with the file, …
    Tool { name: String, detail: String },
    /// A status note from here (started, stopped).
    Note(String),
    /// The run is over.
    Done {
        ok: bool,
        text: String,
        seconds: f64,
    },
}

/// One agent session as kept between runs of the app: what it was asked, where it ran and
/// everything it said, so a repository's past sessions can be read again.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SessionRecord {
    /// Unix milliseconds when it was created: unique, and orders sessions by recency.
    pub id: u64,
    pub title: String,
    pub repo: PathBuf,
    pub slug: Option<String>,
    pub isolated: bool,
    pub dir: PathBuf,
    pub branch: Option<String>,
    pub lines: Vec<ClaudeLine>,
}

fn sessions_file() -> Option<PathBuf> {
    Some(
        directories::ProjectDirs::from("", "", "Anano")?
            .data_dir()
            .join("agents.json"),
    )
}

/// Every saved session (empty when there is no file or it can't be read).
pub fn load_sessions() -> Vec<SessionRecord> {
    sessions_file()
        .and_then(|f| std::fs::read(f).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Replace the saved sessions. Written to a side file and renamed, so a crash mid-write
/// can't leave half a history.
pub fn save_sessions(sessions: &[SessionRecord]) {
    let Some(file) = sessions_file() else { return };
    let Ok(json) = serde_json::to_vec(sessions) else {
        return;
    };
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let part = file.with_extension("json.part");
    if std::fs::write(&part, json).is_ok() {
        let _ = std::fs::rename(&part, &file);
    }
}

/// Where the `claude` CLI is: `$ANANO_CLAUDE` if set (for tests), then `PATH`, then the
/// places it is usually installed (a GUI app's `PATH` is bare).
pub fn find_claude() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("ANANO_CLAUDE") {
        return Some(PathBuf::from(path));
    }
    let exe = if cfg!(windows) { "claude.exe" } else { "claude" };
    let on_path = std::env::var_os("PATH")
        .iter()
        .flat_map(std::env::split_paths)
        .map(|dir| dir.join(exe))
        .find(|p| p.is_file());
    // Windows has no HOME; USERPROFILE is the equivalent
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from);
    on_path.or_else(|| {
        [
            home.as_ref().map(|h| h.join(".local/bin").join(exe)),
            home.as_ref().map(|h| h.join(".claude/local").join(exe)),
            Some(PathBuf::from("/opt/homebrew/bin/claude")),
            Some(PathBuf::from("/usr/local/bin/claude")),
        ]
        .into_iter()
        .flatten()
        .find(|p| p.is_file())
    })
}

fn shorten(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match flat.char_indices().nth(max) {
        Some((cut, _)) => format!("{}…", &flat[..cut]),
        None => flat,
    }
}

/// The part of a tool call worth showing: the command, the file, the pattern.
fn tool_detail(input: &serde_json::Value) -> String {
    for key in [
        "command",
        "file_path",
        "path",
        "pattern",
        "description",
        "url",
    ] {
        if let Some(value) = input[key].as_str() {
            return shorten(value, 200);
        }
    }
    shorten(&input.to_string(), 120)
}

/// One line of `--output-format stream-json` as what to show (nothing for the events that
/// are bookkeeping: hooks, rate limits, tool results).
pub fn parse_event(line: &str) -> Vec<ClaudeLine> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
        return Vec::new();
    };
    match v["type"].as_str() {
        Some("system") if v["subtype"].as_str() == Some("init") => {
            let model = v["model"].as_str().unwrap_or("Claude");
            vec![ClaudeLine::Note(format!("Claude Code started ({model})"))]
        }
        Some("assistant") => v["message"]["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|item| match item["type"].as_str() {
                Some("text") => {
                    let text = item["text"].as_str()?.trim();
                    (!text.is_empty()).then(|| ClaudeLine::Text(text.to_string()))
                }
                Some("tool_use") => Some(ClaudeLine::Tool {
                    name: item["name"].as_str().unwrap_or("tool").to_string(),
                    detail: tool_detail(&item["input"]),
                }),
                _ => None,
            })
            .collect(),
        Some("result") => vec![ClaudeLine::Done {
            ok: !v["is_error"].as_bool().unwrap_or(false),
            text: v["result"].as_str().unwrap_or_default().trim().to_string(),
            seconds: v["duration_ms"].as_f64().unwrap_or(0.0) / 1000.0,
        }],
        _ => Vec::new(),
    }
}

/// A new branch in its own working copy (`git worktree`), so an agent can change files,
/// commit and push without touching the checkout the user is looking at, and several agents
/// can work on the same repository at once. Returns the new folder.
pub fn create_worktree(repo: &Path, branch: &str) -> Result<PathBuf, String> {
    let name = repo.file_name().and_then(|n| n.to_str()).unwrap_or("repo");
    let dir = crate::external::cache_dir()
        .ok_or("no cache folder")?
        .join("agents")
        .join(format!("{name}-{}", branch.replace('/', "-")));
    std::fs::create_dir_all(dir.parent().ok_or("bad path")?).map_err(|e| e.to_string())?;
    let out = Command::new("git")
        .current_dir(repo)
        .args(["worktree", "add", "-b", branch])
        .arg(&dir)
        .arg("HEAD")
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(dir)
    } else {
        Err(shorten(&String::from_utf8_lossy(&out.stderr), 300))
    }
}

/// What every agent started from Anano is told about where it is and what to deliver.
pub fn agent_system_prompt(repo_slug: Option<&str>, branch: &str) -> String {
    let repo = repo_slug.unwrap_or("this repository");
    format!(
        "You were started from Anano, a native Git client for the desktop (Rust + GPUI): it \
         shows changes, history, branches, pull requests, GitHub Actions runs and connected \
         GitHub accounts, and starts agents like you on a repository.\n\
         You work on {repo} in an isolated git worktree (your current directory) on the new \
         branch `{branch}`, created from the repository's HEAD. Stay inside this directory: \
         never touch the user's main checkout or other folders.\n\
         Do what the user asks, then: commit with clear messages, `git push -u origin \
         {branch}`, and open a pull request with `gh pr create` (a short title and a body \
         saying what changed and why). If the task splits into independent parts, you may \
         use subagents; give each part its own branch and worktree (`git worktree add`) so \
         they never edit the same files, and open one pull request per part. Never force \
         push and never merge. Finish by listing every pull request URL you opened."
    )
}

/// A running Claude Code. Dropping it does not stop it; call [`Self::stop`].
pub struct ClaudeHandle {
    child: Arc<Mutex<Child>>,
    stopped: Arc<AtomicBool>,
}

impl ClaudeHandle {
    /// Kill the run. The reader then reports it as stopped.
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        if let Ok(mut child) = self.child.lock() {
            // Its own process group (see `spawn`): kill what Claude Code started too, or a
            // grandchild keeps the output pipe open and the run never reports as stopped.
            #[cfg(unix)]
            let _ = Command::new("kill")
                .args(["-KILL", &format!("-{}", child.id())])
                .status();
            let _ = child.kill();
        }
    }
}

/// Start Claude Code in `dir` with `prompt`, calling `on_line` (from a background thread)
/// for each step and finally with [`ClaudeLine::Done`].
///
/// It may edit files (`acceptEdits`) and run `git` and `gh`; anything else it would have to
/// ask about is refused, since there is nobody to ask.
pub fn spawn(
    dir: &Path,
    prompt: &str,
    system: Option<&str>,
    on_line: impl Fn(ClaudeLine) + Send + 'static,
) -> Result<ClaudeHandle, String> {
    run(
        dir,
        prompt,
        system,
        "Bash(git:*) Bash(gh:*)",
        "acceptEdits",
        None,
        on_line,
    )
}

/// What Claude Code is asked when you press "Review with Claude" on a pull request.
pub fn review_prompt(slug: &str, number: u64) -> String {
    format!(
        "Review pull request #{number} of {slug}. Read it with `gh pr view {number} --repo \
         {slug}` and `gh pr diff {number} --repo {slug}`; if you need context, read files in \
         the current directory. Do not change anything. Write a concise review in Markdown: \
         one line saying what the change does, then the concrete problems you found (bugs, \
         risks, missing tests or edge cases) most important first, each with the file and \
         line, then whether it looks ready to merge. No preamble, and nothing about how you \
         worked. Begin the answer with exactly one line, `VERDICT: APPROVE`, `VERDICT: \
         REQUEST_CHANGES` or `VERDICT: COMMENT`, the review you would sign."
    )
}

/// Split a review into the verdict Claude signed with (its first `VERDICT:` line) and the
/// text to post. No recognisable verdict line: the whole text, and no verdict.
pub fn parse_review(text: &str) -> (Option<crate::external::ReviewVerdict>, String) {
    use crate::external::ReviewVerdict;
    let trimmed = text.trim();
    let (first, rest) = trimmed.split_once('\n').unwrap_or((trimmed, ""));
    let verdict = first
        .trim()
        .trim_matches('*')
        .strip_prefix("VERDICT:")
        .map(|v| v.trim().trim_matches('*').to_ascii_uppercase())
        .and_then(|v| match v.as_str() {
            "APPROVE" | "APPROVED" => Some(ReviewVerdict::Approve),
            "REQUEST_CHANGES" | "CHANGES_REQUESTED" => Some(ReviewVerdict::RequestChanges),
            "COMMENT" => Some(ReviewVerdict::Comment),
            _ => None,
        });
    match verdict {
        Some(v) => (Some(v), rest.trim().to_string()),
        None => (None, trimmed.to_string()),
    }
}

/// "Claudify": read what people said on a pull request and, if another commit can fix it,
/// make that commit on the pull request's own branch.
pub fn claudify_prompt(slug: &str, number: u64) -> String {
    format!(
        "People have commented on or reviewed pull request #{number} of {slug}. Read it with \
         `gh pr view {number} --repo {slug} --comments` and `gh pr diff {number} --repo \
         {slug}`. Decide whether their feedback can be fixed with another commit. If it can: \
         `gh pr checkout {number}` in this working copy, make the fixes, check that the \
         result still builds or passes the project's tests if there is a quick way to run \
         them, commit with a clear message, and `git push` to the pull request's branch \
         (never force push, never merge). Finish by saying what you changed and which \
         comments it answers. If it can't (the feedback needs a decision, is unclear, or is \
         about something outside the code), change nothing and finish with a short \
         explanation of what a person needs to decide."
    )
}

/// Review a pull request, read-only: it can read the pull request and the files in `dir`,
/// run `gh pr view` / `gh pr diff` and look at git history, and nothing else. Uses the
/// repository's GitHub account for `gh`. The review arrives as the final [`ClaudeLine::Done`].
pub fn review_pull_request(
    origin: &str,
    dir: &Path,
    number: u64,
    on_line: impl Fn(ClaudeLine) + Send + 'static,
) -> Result<ClaudeHandle, String> {
    let slug = crate::external::repo_slug(origin).ok_or("not a GitHub repository")?;
    let token = crate::external::accounts_for(origin, &crate::external::list_accounts())
        .first()
        .and_then(|a| crate::external::token(&a.login));
    run(
        dir,
        &review_prompt(&slug, number),
        None,
        "Read Grep Glob Bash(gh pr view:*) Bash(gh pr diff:*) Bash(git log:*) Bash(git show:*)",
        "default",
        token.map(|t| ("GH_TOKEN", t)),
        on_line,
    )
}

fn run(
    dir: &Path,
    prompt: &str,
    system: Option<&str>,
    tools: &str,
    permission_mode: &str,
    env: Option<(&str, String)>,
    on_line: impl Fn(ClaudeLine) + Send + 'static,
) -> Result<ClaudeHandle, String> {
    let claude = find_claude().ok_or("Claude Code (the `claude` command) isn't installed")?;
    let mut command = Command::new(claude);
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let mut child = command
        .current_dir(dir)
        .arg("-p")
        .arg(prompt)
        .args(["--output-format", "stream-json", "--verbose"])
        .args([
            "--no-session-persistence",
            "--permission-mode",
            permission_mode,
        ])
        .args(["--allowedTools", tools])
        .envs(env)
        .args(
            system
                .into_iter()
                .flat_map(|s| ["--append-system-prompt", s]),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("couldn't start Claude Code: {e}"))?;
    let stdout = child.stdout.take().expect("stdout is piped");
    let mut stderr = child.stderr.take().expect("stderr is piped");
    let child = Arc::new(Mutex::new(child));
    let stopped = Arc::new(AtomicBool::new(false));
    let handle = ClaudeHandle {
        child: child.clone(),
        stopped: stopped.clone(),
    };
    std::thread::spawn(move || {
        // Drained on the side so a chatty stderr can never fill its pipe and stall the run.
        let errors = std::thread::spawn(move || {
            let mut text = String::new();
            let _ = stderr.read_to_string(&mut text);
            text
        });
        let mut finished = false;
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            for step in parse_event(&line) {
                finished |= matches!(step, ClaudeLine::Done { .. });
                on_line(step);
            }
        }
        let status = child.lock().ok().and_then(|mut c| c.wait().ok());
        let stderr = errors.join().unwrap_or_default();
        if !finished {
            on_line(ClaudeLine::Done {
                ok: false,
                text: if stopped.load(Ordering::SeqCst) {
                    "Stopped.".to_string()
                } else if stderr.trim().is_empty() {
                    format!(
                        "Claude Code exited ({})",
                        status.map_or("?".into(), |s| s.to_string())
                    )
                } else {
                    shorten(&stderr, 400)
                },
                seconds: 0.0,
            });
        }
    });
    Ok(handle)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An agent's branch gets its own folder, and the checkout it came from is untouched.
    #[test]
    fn a_worktree_is_a_new_branch_in_its_own_folder() {
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str], cwd: &Path| {
            let out = Command::new("git")
                .args(args)
                .current_dir(cwd)
                .output()
                .unwrap();
            assert!(out.status.success(), "{args:?}");
            String::from_utf8(out.stdout).unwrap()
        };
        git(&["init", "-q", "-b", "main"], dir.path());
        git(&["config", "user.email", "t@t"], dir.path());
        git(&["config", "user.name", "t"], dir.path());
        std::fs::write(dir.path().join("a.txt"), "x").unwrap();
        git(&["add", "."], dir.path());
        git(&["commit", "-qm", "init"], dir.path());
        let branch = format!("claude/test-{}", std::process::id());
        let work = create_worktree(dir.path(), &branch).unwrap();
        assert!(work.join("a.txt").is_file());
        assert_eq!(git(&["branch", "--show-current"], &work).trim(), branch);
        assert_eq!(
            git(&["branch", "--show-current"], dir.path()).trim(),
            "main"
        );
        // Same branch twice fails with a readable reason instead of clobbering.
        assert!(create_worktree(dir.path(), &branch).is_err());
        let prompt = agent_system_prompt(Some("me/repo"), &branch);
        assert!(
            prompt.contains("Anano")
                && prompt.contains("me/repo")
                && prompt.contains("gh pr create")
        );
        let _ = std::fs::remove_dir_all(&work);
    }

    #[test]
    fn a_session_survives_being_saved_and_read_back() {
        let record = SessionRecord {
            id: 1_789_427_775_000,
            title: "Add a logo".into(),
            repo: PathBuf::from("/tmp/repo"),
            slug: Some("me/repo".into()),
            isolated: true,
            dir: PathBuf::from("/tmp/agents/repo-claude-task-1"),
            branch: Some("claude/task-1".into()),
            lines: vec![
                ClaudeLine::Text("Working".into()),
                ClaudeLine::Tool {
                    name: "Bash".into(),
                    detail: "git status".into(),
                },
                ClaudeLine::Done {
                    ok: true,
                    text: "PR: https://x/1".into(),
                    seconds: 12.0,
                },
            ],
        };
        let json = serde_json::to_vec(std::slice::from_ref(&record)).unwrap();
        assert_eq!(
            serde_json::from_slice::<Vec<SessionRecord>>(&json).unwrap(),
            [record]
        );
    }

    #[test]
    fn a_reviews_first_line_is_its_verdict() {
        use crate::external::ReviewVerdict::*;
        assert_eq!(
            parse_review("VERDICT: APPROVE\n\nLooks good."),
            (Some(Approve), "Looks good.".into())
        );
        assert_eq!(
            parse_review("**VERDICT: REQUEST_CHANGES**\nBug in a.rs:3"),
            (Some(RequestChanges), "Bug in a.rs:3".into())
        );
        assert_eq!(
            parse_review("verdict: nonsense\nx"),
            (None, "verdict: nonsense\nx".into())
        );
        assert_eq!(
            parse_review("No verdict here"),
            (None, "No verdict here".into())
        );
        assert!(claudify_prompt("me/r", 5).contains("gh pr checkout 5"));
    }

    #[test]
    fn the_review_prompt_names_the_pull_request_and_forbids_changes() {
        let p = review_prompt("me/repo", 42);
        assert!(p.contains("gh pr diff 42 --repo me/repo") && p.contains("Do not change anything"));
    }

    #[test]
    fn stream_events_become_steps() {
        let init = r#"{"type":"system","subtype":"init","model":"claude-x"}"#;
        assert_eq!(
            parse_event(init),
            [ClaudeLine::Note("Claude Code started (claude-x)".into())]
        );
        let assistant = r#"{"type":"assistant","message":{"content":[
            {"type":"text","text":"  Merging now.  "},
            {"type":"tool_use","name":"Bash","input":{"command":"git merge origin/main"}},
            {"type":"tool_use","name":"Edit","input":{"file_path":"src/a.rs","old_string":"x"}},
            {"type":"thinking","thinking":"hmm"}]}}"#;
        assert_eq!(
            parse_event(&assistant.replace('\n', "")),
            [
                ClaudeLine::Text("Merging now.".into()),
                ClaudeLine::Tool {
                    name: "Bash".into(),
                    detail: "git merge origin/main".into()
                },
                ClaudeLine::Tool {
                    name: "Edit".into(),
                    detail: "src/a.rs".into()
                },
            ]
        );
        let done = r#"{"type":"result","subtype":"success","is_error":false,"result":"Resolved.","duration_ms":3500}"#;
        assert_eq!(
            parse_event(done),
            [ClaudeLine::Done {
                ok: true,
                text: "Resolved.".into(),
                seconds: 3.5
            }]
        );
        // Bookkeeping and junk produce nothing.
        for quiet in [
            r#"{"type":"system","subtype":"hook_started"}"#,
            r#"{"type":"rate_limit_event"}"#,
            r#"{"type":"user","message":{"content":[]}}"#,
            "not json",
            "",
        ] {
            assert!(parse_event(quiet).is_empty(), "{quiet}");
        }
    }

    /// The whole path against a stand-in `claude` script: lines stream out in order, the run
    /// ends with Done, and stopping a long run reports it as stopped.
    #[cfg(unix)]
    #[test]
    fn a_run_streams_its_steps_and_can_be_stopped() {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::mpsc;
        use std::time::Duration;
        // ANANO_CLAUDE is process-global: one test owns it.
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("claude");
        std::fs::write(
            &script,
            "#!/bin/sh\n\
             echo '{\"type\":\"system\",\"subtype\":\"init\",\"model\":\"m\"}'\n\
             echo '{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"working\"}]}}'\n\
             if [ \"$1\" = \"-p\" ] && [ \"$2\" = \"slow\" ]; then sleep 30; fi\n\
             echo '{\"type\":\"result\",\"is_error\":false,\"result\":\"ok\",\"duration_ms\":1000}'\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        unsafe { std::env::set_var("ANANO_CLAUDE", &script) };

        let (tx, rx) = mpsc::channel();
        spawn(dir.path(), "quick", None, move |line| {
            let _ = tx.send(line);
        })
        .unwrap();
        let steps: Vec<_> = rx.iter().take(3).collect();
        assert!(matches!(steps[0], ClaudeLine::Note(_)));
        assert_eq!(steps[1], ClaudeLine::Text("working".into()));
        assert!(matches!(steps[2], ClaudeLine::Done { ok: true, .. }));

        let (tx, rx) = mpsc::channel();
        let handle = spawn(dir.path(), "slow", None, move |line| {
            let _ = tx.send(line);
        })
        .unwrap();
        // The first two steps arrive while it is still "working"; then we stop it.
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(10)).unwrap(),
            ClaudeLine::Note(_)
        ));
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(10)).unwrap(),
            ClaudeLine::Text(_)
        ));
        handle.stop();
        match rx.recv_timeout(Duration::from_secs(10)).unwrap() {
            ClaudeLine::Done {
                ok: false, text, ..
            } => assert_eq!(text, "Stopped."),
            other => panic!("expected a stopped run, got {other:?}"),
        }
    }
}
