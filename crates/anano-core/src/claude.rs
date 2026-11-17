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
#[derive(Debug, Clone, PartialEq)]
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

/// Where the `claude` CLI is: `$ANANO_CLAUDE` if set (for tests), then `PATH`, then the
/// places it is usually installed (a GUI app's `PATH` is bare).
pub fn find_claude() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("ANANO_CLAUDE") {
        return Some(PathBuf::from(path));
    }
    let on_path = std::env::var_os("PATH")
        .iter()
        .flat_map(std::env::split_paths)
        .map(|dir| dir.join("claude"))
        .find(|p| p.is_file());
    let home = std::env::var_os("HOME").map(PathBuf::from);
    on_path.or_else(|| {
        [
            home.as_ref().map(|h| h.join(".local/bin/claude")),
            home.as_ref().map(|h| h.join(".claude/local/claude")),
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
            "acceptEdits",
        ])
        .args(["--allowedTools", "Bash(git:*) Bash(gh:*)"])
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
        spawn(dir.path(), "quick", move |line| {
            let _ = tx.send(line);
        })
        .unwrap();
        let steps: Vec<_> = rx.iter().take(3).collect();
        assert!(matches!(steps[0], ClaudeLine::Note(_)));
        assert_eq!(steps[1], ClaudeLine::Text("working".into()));
        assert!(matches!(steps[2], ClaudeLine::Done { ok: true, .. }));

        let (tx, rx) = mpsc::channel();
        let handle = spawn(dir.path(), "slow", move |line| {
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
