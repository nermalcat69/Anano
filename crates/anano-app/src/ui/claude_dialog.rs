//! "Fix conflicts with Claude Code": a dialog that runs Claude Code on the open repository
//! with a prompt to resolve merge conflicts, and shows what it does as it goes.
//!
//! The prompt is shown (and editable) before anything runs, so starting it is a decision,
//! not a side effect. It runs non-interactively: Claude Code can edit files and run `git`
//! and `gh`, and is told not to push. It cannot ask questions; the run can be stopped.

use std::path::PathBuf;

use anano_core::claude::{ClaudeHandle, ClaudeLine};
use gpui_kit::AppContext as _;
use gpui_kit::component::{
    ActiveTheme, Disableable, Sizable, WindowExt,
    button::ButtonVariants,
    h_flex,
    input::{InputEvent, Textarea},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, Entity, FontWeight, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};

use crate::i18n::tr;
use crate::state::workspace::Workspace;

const DIALOG_WIDTH: f32 = 1120.;
const SIDEBAR_WIDTH: f32 = 250.;
const LOG_HEIGHT: f32 = 470.;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaudePhase {
    /// The prompt is on screen, waiting to be run.
    Ready,
    Running,
    Finished,
}

pub struct ClaudeSession {
    pub id: u64,
    /// What this run is for: the tab's name and the dialog's title.
    pub title: SharedString,
    /// The repository it was started from.
    pub repo: PathBuf,
    pub slug: Option<String>,
    /// Runs in its own branch and working copy (made when it starts) and opens pull
    /// requests; otherwise it works in the repository's own folder.
    pub isolated: bool,
    /// Where it runs: the repository, or its own working copy once started.
    pub dir: PathBuf,
    pub branch: Option<String>,
    pub phase: ClaudePhase,
    pub lines: Vec<ClaudeLine>,
    pub handle: Option<ClaudeHandle>,
    /// Why it couldn't start.
    pub error: Option<String>,
}

/// The default branch to merge from: `main`, else `master`, else whichever of the two
/// exists; `None` when neither does (the user can still edit the prompt).
pub(crate) fn default_branch(branches: &[String]) -> Option<&str> {
    ["main", "master"]
        .into_iter()
        .find(|name| branches.iter().any(|b| b == name))
}

/// What to tell Claude Code when a merge or rebase has stopped on conflicts in the open
/// repository.
pub(crate) fn conflicts_prompt() -> String {
    "This repository has unresolved merge conflicts (a merge, rebase, cherry-pick or stash \\
     pop stopped on them). Resolve every conflicted file: read both sides, keep the intent \\
     of both changes, and remove all conflict markers. Check that the result still builds \\
     or passes the project's tests if there is a quick way to run them. Then stage the \\
     files and finish the operation that was interrupted (for a merge: `git commit` with a \\
     sensible message; for a rebase: `git rebase --continue`). Do not push anything. \\
     Finish with a short summary of what you changed in each file."
        .to_string()
}

/// Merge `base` into `branch` and fix what conflicts.
pub(crate) fn branch_conflicts_prompt(branch: &str, base: &str) -> String {
    format!(
        "Bring the branch `{branch}` up to date with `{base}` and fix any conflicts. First \\
         make sure the working tree is clean: if there are uncommitted changes, stop and \\
         say so instead of touching them. Then `git switch {branch}`, `git fetch origin`, \\
         and `git merge origin/{base}`. If the merge stops on conflicts, resolve every \\
         conflicted file: read both sides, keep the intent of both changes, remove all \\
         conflict markers, and check that the result still builds or passes the project's \\
         tests if there is a quick way to run them. Stage the files and complete the merge \\
         with `git commit`. Do not push anything. Finish with a short summary of what you \\
         changed in each file."
    )
}

/// The same for a pull request: it is checked out with `gh`, which also handles forks.
pub(crate) fn pull_request_conflicts_prompt(number: u64, head: &str, base: &str) -> String {
    format!(
        "Pull request #{number} (`{head}` into `{base}`) has merge conflicts with `{base}`. \\
         First make sure the working tree is clean: if there are uncommitted changes, stop \\
         and say so instead of touching them. Then check the pull request out with \\
         `gh pr checkout {number}`, run `git fetch origin`, and `git merge origin/{base}`. \\
         Resolve every conflicted file: read both sides, keep the intent of both changes, \\
         remove all conflict markers, and check that the result still builds or passes the \\
         project's tests if there is a quick way to run them. Stage the files and complete \\
         the merge with `git commit`. Do not push anything. Finish with a short summary of \\
         what you changed in each file."
    )
}

fn line_row(ix: usize, line: &ClaudeLine, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let (muted, success, danger) = (theme.muted_foreground, theme.success, theme.danger);
    match line {
        ClaudeLine::Text(text) => div()
            .text_sm()
            .child(crate::ui::selectable(("claude-text", ix), text.clone()))
            .into_any_element(),
        ClaudeLine::Tool { name, detail } => h_flex()
            .gap_2()
            .items_center()
            .text_xs()
            .child(
                div()
                    .flex_none()
                    .px_1p5()
                    .py_0p5()
                    .rounded(px(4.))
                    .bg(theme.secondary)
                    .text_color(muted)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(name.clone()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_family(theme.mono_font_family.clone())
                    .text_color(muted)
                    .child(crate::ui::selectable(("claude-tool", ix), detail.clone())),
            )
            .into_any_element(),
        ClaudeLine::Note(text) => div()
            .text_xs()
            .text_color(muted)
            .child(text.clone())
            .into_any_element(),
        ClaudeLine::Done { ok, text, seconds } => {
            let color = if *ok { success } else { danger };
            v_flex()
                .gap_1()
                .p_3()
                .rounded(px(8.))
                .border_1()
                .border_color(color.opacity(0.6))
                .bg(color.opacity(0.1))
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(color)
                        .child(if *ok {
                            tr!("repo.claude.done", seconds = format!("{seconds:.0}"))
                        } else {
                            tr!("repo.claude.failed")
                        }),
                )
                .when(!text.is_empty(), |v| {
                    v.child(
                        div()
                            .text_sm()
                            .child(crate::ui::selectable(("claude-done", ix), text.clone())),
                    )
                })
                .into_any_element()
        }
    }
}

/// The dialog's content, rebuilt from the sessions each frame: a tab per agent, then the
/// selected agent's prompt (before it runs) or its log.
fn claude_body(workspace: &Entity<Workspace>, cx: &mut App) -> AnyElement {
    let ws = workspace.read(cx);
    // This repository's sessions, newest first. The selected one, if it belongs to
    // another repository, gives way to this one's newest.
    let repo_path = ws.active_repo().map(|r| r.entry.path.clone());
    let mut mine: Vec<usize> = (0..ws.claude.len())
        .filter(|&i| Some(&ws.claude[i].repo) == repo_path.as_ref())
        .collect();
    mine.sort_by_key(|&i| std::cmp::Reverse(ws.claude[i].id));
    let shown = if mine.contains(&ws.claude_active) {
        Some(ws.claude_active)
    } else {
        mine.first().copied()
    };
    let Some(session) = shown.and_then(|i| ws.claude.get(i)) else {
        return div().into_any_element();
    };
    let (phase, error, id) = (session.phase, session.error.clone(), session.id);
    let lines = session.lines.clone();
    let place = match &session.branch {
        Some(branch) => tr!("repo.claude.where_branch", branch = branch.clone()),
        None if session.isolated => tr!("repo.claude.where_new_branch"),
        None => tr!(
            "repo.claude.where",
            path = session.dir.display().to_string()
        ),
    };
    let tabs: Vec<(u64, SharedString, ClaudePhase, SharedString, bool)> = mine
        .iter()
        .map(|&i| {
            let s = &ws.claude[i];
            let what = match (&s.branch, s.phase) {
                (_, ClaudePhase::Ready) => tr!("repo.claude.draft"),
                (Some(branch), _) => branch.clone().into(),
                _ => crate::ui::time_ago((s.id / 1000) as i64),
            };
            (s.id, s.title.clone(), s.phase, what, Some(i) == shown)
        })
        .collect();
    let finished_any = mine
        .iter()
        .any(|&i| ws.claude[i].phase == ClaudePhase::Finished);
    let installed = anano_core::claude::find_claude().is_some();
    let (input, scroll) = (ws.claude_input.clone(), ws.claude_scroll.clone());
    let theme = cx.theme();
    let (muted, danger, border, active, success) = (
        theme.muted_foreground,
        theme.danger,
        theme.border,
        theme.list_active,
        theme.success,
    );

    let ws_new = workspace.clone();
    let ws_clear = workspace.clone();
    let sidebar = v_flex()
        .flex_none()
        .w(px(SIDEBAR_WIDTH))
        .h_full()
        .gap_2()
        .pr_3()
        .border_r_1()
        .border_color(border)
        .child(
            crate::ui::button("claude-new-agent")
                .outline()
                .small()
                .w_full()
                .label(tr!("repo.claude.new_agent"))
                .on_click(move |_, window, cx| {
                    ws_new.update(cx, |ws, cx| ws.new_claude_agent(window, cx))
                }),
        )
        .child(
            v_flex()
                .id("claude-sessions")
                .flex_1()
                .min_h_0()
                .gap_1()
                .overflow_y_scroll()
                .children(
                    tabs.into_iter()
                        .map(|(tab_id, title, phase, what, selected)| {
                            let ws_tab = workspace.clone();
                            let dot = match phase {
                                ClaudePhase::Running => theme.warning,
                                ClaudePhase::Finished => success,
                                ClaudePhase::Ready => muted,
                            };
                            h_flex()
                                .id(("claude-tab", tab_id as usize))
                                .gap_2()
                                .px_2()
                                .py_1p5()
                                .items_start()
                                .rounded(px(6.))
                                .cursor_pointer()
                                .when(selected, |t| t.bg(active))
                                .hover(|t| t.bg(active.opacity(0.6)))
                                .child(div().flex_none().mt_1().size(px(8.)).rounded_full().bg(dot))
                                .child(
                                    v_flex()
                                        .min_w_0()
                                        .child(div().truncate().text_sm().child(title))
                                        .child(
                                            div()
                                                .truncate()
                                                .text_xs()
                                                .text_color(muted)
                                                .child(what),
                                        ),
                                )
                                .on_click(move |_, window, cx| {
                                    ws_tab.update(cx, |ws, cx| {
                                        ws.select_claude_tab(tab_id, window, cx)
                                    })
                                })
                        }),
                ),
        )
        .when(finished_any, |v| {
            v.child(
                crate::ui::button("claude-clear")
                    .ghost()
                    .xsmall()
                    .w_full()
                    .label(tr!("repo.claude.clear_finished"))
                    .on_click(move |_, window, cx| {
                        ws_clear.update(cx, |ws, cx| ws.clear_finished_claude(window, cx))
                    }),
            )
        });
    let place = div().text_xs().text_color(muted).child(place);
    let ws_run = workspace.clone();
    let ws_stop = workspace.clone();

    let main: AnyElement = match phase {
        ClaudePhase::Ready => v_flex()
            .gap_3()
            .child(
                div()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(border)
                    .child(Textarea::new(&input)),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(tr!("repo.claude.warning")),
            )
            .when(!installed, |v| {
                v.child(
                    div()
                        .text_sm()
                        .text_color(danger)
                        .child(tr!("repo.claude.not_installed")),
                )
            })
            .children(error.map(|e| div().text_sm().text_color(danger).child(e)))
            .into_any_element(),
        _ => v_flex()
            .id("claude-log")
            .h(px(LOG_HEIGHT))
            .gap_2()
            .p_3()
            .rounded(px(6.))
            .border_1()
            .border_color(border)
            .overflow_y_scroll()
            .track_scroll(&scroll)
            .children(lines.iter().enumerate().map(|(ix, l)| line_row(ix, l, cx)))
            .when(phase == ClaudePhase::Running, |v| {
                v.child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(tr!("repo.claude.working")),
                )
            })
            .into_any_element(),
    };
    let buttons = h_flex()
        .gap_2()
        .items_center()
        .child(
            div()
                .flex_1()
                .text_xs()
                .text_color(muted)
                .when(phase == ClaudePhase::Running, |d| {
                    d.child(tr!("repo.claude.keeps_running"))
                }),
        )
        .when(phase == ClaudePhase::Running, |h| {
            h.child(
                crate::ui::button("claude-stop")
                    .outline()
                    .label(tr!("repo.claude.stop"))
                    .on_click(move |_, window, cx| {
                        ws_stop.update(cx, |ws, cx| ws.stop_claude(id, window, cx))
                    }),
            )
        })
        .child(
            crate::ui::button("claude-close")
                .outline()
                .label(tr!("repo.claude.close"))
                .on_click(|_, window, cx| window.close_dialog(cx)),
        )
        .when(phase == ClaudePhase::Ready, |h| {
            h.child(
                crate::ui::button("claude-run")
                    .primary()
                    .label(tr!("repo.claude.run"))
                    .disabled(!installed)
                    .on_click(move |_, window, cx| {
                        ws_run.update(cx, |ws, cx| ws.run_claude(window, cx))
                    }),
            )
        });
    h_flex()
        .gap_4()
        .items_start()
        .child(sidebar.h(px(LOG_HEIGHT + 130.)))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_3()
                .child(place)
                .child(main)
                .child(buttons),
        )
        .into_any_element()
}

impl Workspace {
    /// The one agent whose prompt is still being written, if any.
    fn claude_draft(&self) -> Option<usize> {
        let repo = self.active_repo().map(|r| &r.entry.path)?;
        self.claude
            .iter()
            .position(|s| s.phase == ClaudePhase::Ready && &s.repo == repo)
    }

    /// Save every session that has been run (drafts are not history).
    pub(crate) fn save_claude_sessions(&self) {
        let records: Vec<_> = self
            .claude
            .iter()
            .filter(|s| s.phase != ClaudePhase::Ready)
            .map(|s| anano_core::claude::SessionRecord {
                id: s.id,
                title: s.title.to_string(),
                repo: s.repo.clone(),
                slug: s.slug.clone(),
                isolated: s.isolated,
                dir: s.dir.clone(),
                branch: s.branch.clone(),
                lines: s.lines.clone(),
            })
            .collect();
        anano_core::claude::save_sessions(&records);
    }

    /// The saved sessions as they were left. One that was still running when the app closed
    /// is over: its process went with it.
    pub(crate) fn load_claude_sessions() -> Vec<ClaudeSession> {
        anano_core::claude::load_sessions()
            .into_iter()
            .map(|r| {
                let mut lines = r.lines;
                if !matches!(lines.last(), Some(ClaudeLine::Done { .. })) {
                    lines.push(ClaudeLine::Done {
                        ok: false,
                        text: "Anano was closed before this finished.".into(),
                        seconds: 0.0,
                    });
                }
                ClaudeSession {
                    id: r.id,
                    title: r.title.into(),
                    repo: r.repo,
                    slug: r.slug,
                    isolated: r.isolated,
                    dir: r.dir,
                    branch: r.branch,
                    phase: ClaudePhase::Finished,
                    lines,
                    handle: None,
                    error: None,
                }
            })
            .collect()
    }

    fn next_claude_id(&self) -> u64 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        self.claude.iter().map(|s| s.id + 1).fold(now, u64::max)
    }

    /// "Clear finished": forget this repository's finished sessions.
    pub fn clear_finished_claude(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let repo = self.active_repo().map(|r| r.entry.path.clone());
        self.claude
            .retain(|s| s.phase != ClaudePhase::Finished || Some(&s.repo) != repo.as_ref());
        self.claude_active = 0;
        self.save_claude_sessions();
        window.refresh();
        cx.notify();
    }

    /// Show the dialog with a prompt ready to run (replacing a draft not yet run; agents
    /// already running or finished keep their tabs).
    fn show_claude_draft(
        &mut self,
        title: SharedString,
        prompt: String,
        isolated: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.prepare_claude_draft(title, isolated) {
            return;
        }
        self.claude_input
            .update(cx, |input, cx| input.set_value(prompt, window, cx));
        self.open_claude_window(window, cx);
    }

    /// Make (or reuse) this repository's draft session. `false` when no repository is open.
    fn prepare_claude_draft(&mut self, title: SharedString, isolated: bool) -> bool {
        let Some(repo) = self.active_repo() else {
            return false;
        };
        let (dir, slug) = (
            repo.entry.path.clone(),
            repo.origin
                .as_deref()
                .and_then(anano_core::external::repo_slug),
        );
        let ix = match self.claude_draft() {
            Some(ix) => ix,
            None => {
                let id = self.next_claude_id();
                self.claude.push(ClaudeSession {
                    id,
                    title: SharedString::default(),
                    repo: dir.clone(),
                    slug: slug.clone(),
                    isolated,
                    dir: dir.clone(),
                    branch: None,
                    phase: ClaudePhase::Ready,
                    lines: Vec::new(),
                    handle: None,
                    error: None,
                });
                self.claude.len() - 1
            }
        };
        let session = &mut self.claude[ix];
        (
            session.title,
            session.isolated,
            session.repo,
            session.slug,
            session.dir,
        ) = (title, isolated, dir.clone(), slug, dir);
        session.error = None;
        self.claude_active = ix;
        true
    }

    /// Start an agent right away, with no dialog: it runs in the background in its own
    /// branch and working copy, shows up in the sessions list, and says so when it is done.
    pub fn start_background_agent(
        &mut self,
        title: SharedString,
        prompt: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.prepare_claude_draft(title, true) {
            self.run_claude_with(prompt, window, cx);
        }
    }

    /// Open the dialog itself (once); it follows `self.claude` from then on.
    fn open_claude_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !window.has_active_dialog(cx) {
            let workspace = cx.entity();
            window.open_dialog(cx, move |dialog, window, _| {
                let workspace = workspace.clone();
                crate::ui::centered(dialog, window, 660.)
                    .title(tr!("repo.claude.dialog_title"))
                    .w(px(DIALOG_WIDTH))
                    // Closing it is a choice (Close): agents keep running behind it.
                    .overlay_closable(false)
                    .content(move |content, _, cx| content.child(claude_body(&workspace, cx)))
            });
        }
        window.refresh();
        cx.notify();
    }

    pub fn open_claude_dialog(
        &mut self,
        title: SharedString,
        prompt: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_claude_draft(title, prompt, false, window, cx);
    }

    /// "Open Claude Code" on the No local changes page: an agent that makes its own branch,
    /// does what you ask and opens a pull request.
    pub fn open_claude_agent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.claude_draft().is_none() {
            self.show_claude_draft(
                tr!("repo.claude.agent_title"),
                String::new(),
                true,
                window,
                cx,
            );
        } else if let Some(ix) = self.claude_draft() {
            // A draft is waiting (maybe a conflict fix): the new agent is its own tab.
            if !self.claude[ix].isolated {
                self.claude[ix].phase = ClaudePhase::Finished;
                self.claude[ix]
                    .lines
                    .push(ClaudeLine::Note("Not started".into()));
                self.show_claude_draft(
                    tr!("repo.claude.agent_title"),
                    String::new(),
                    true,
                    window,
                    cx,
                );
            } else {
                self.claude_active = ix;
                self.open_claude_window(window, cx);
            }
        }
    }

    pub fn new_claude_agent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_claude_agent(window, cx);
    }

    pub fn select_claude_tab(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.claude.iter().position(|s| s.id == id) {
            self.claude_active = ix;
            window.refresh();
            cx.notify();
        }
    }

    /// "Run Claude Code": start the selected agent with whatever the prompt box says. An
    /// isolated agent first gets its own branch and working copy.
    pub fn run_claude(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let prompt = self.claude_input.read(cx).value().trim().to_string();
        self.run_claude_with(prompt, window, cx);
    }

    fn run_claude_with(&mut self, prompt: String, window: &mut Window, cx: &mut Context<Self>) {
        // Only a draft runs, and a repository has at most one: the one on screen.
        let Some(session) = self.claude_draft().and_then(|ix| self.claude.get_mut(ix)) else {
            return;
        };
        if prompt.is_empty() || session.phase != ClaudePhase::Ready {
            return;
        }
        session.phase = ClaudePhase::Running;
        session.error = None;
        session.title = prompt
            .lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(40)
            .collect::<String>()
            .into();
        let (id, isolated, repo, slug) = (
            session.id,
            session.isolated,
            session.repo.clone(),
            session.slug.clone(),
        );
        let branch = format!(
            "claude/task-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs())
        );
        if isolated {
            session.lines.push(ClaudeLine::Note(format!(
                "Creating branch {branch} in its own working copy…"
            )));
        }
        self.save_claude_sessions();
        window.refresh();
        cx.notify();
        let make = isolated.then(|| {
            let (repo, branch) = (repo.clone(), branch.clone());
            cx.background_spawn(async move { anano_core::claude::create_worktree(&repo, &branch) })
        });
        cx.spawn_in(window, async move |ws, cx| {
            let dir = match make {
                Some(task) => match task.await {
                    Ok(dir) => dir,
                    Err(err) => {
                        let _ = ws.update_in(cx, |ws, window, cx| {
                            ws.finish_claude_failed(id, err, window, cx)
                        });
                        return;
                    }
                },
                None => repo.clone(),
            };
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<ClaudeLine>();
            let system =
                isolated.then(|| anano_core::claude::agent_system_prompt(slug.as_deref(), &branch));
            let started =
                anano_core::claude::spawn(&dir, &prompt, system.as_deref(), move |line| {
                    let _ = tx.send(line);
                });
            match started {
                Ok(handle) => {
                    let _ = ws.update_in(cx, |ws, window, cx| {
                        if let Some(s) = ws.claude.iter_mut().find(|s| s.id == id) {
                            s.handle = Some(handle);
                            s.dir = dir;
                            s.branch = isolated.then(|| branch.clone());
                        }
                        window.refresh();
                        cx.notify();
                    });
                }
                Err(err) => {
                    let _ = ws.update_in(cx, |ws, window, cx| {
                        ws.finish_claude_failed(id, err, window, cx)
                    });
                    return;
                }
            }
            while let Some(line) = rx.recv().await {
                let done = matches!(line, ClaudeLine::Done { .. });
                let _ = ws.update_in(cx, |ws, window, cx| {
                    ws.push_claude_line(id, line, window, cx)
                });
                if done {
                    break;
                }
            }
        })
        .detach();
    }

    fn finish_claude_failed(
        &mut self,
        id: u64,
        err: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.push_claude_line(
            id,
            ClaudeLine::Done {
                ok: false,
                text: err,
                seconds: 0.0,
            },
            window,
            cx,
        );
    }

    fn push_claude_line(
        &mut self,
        id: u64,
        line: ClaudeLine,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.claude.iter_mut().find(|s| s.id == id) else {
            return;
        };
        let done = matches!(line, ClaudeLine::Done { .. });
        session.lines.push(line);
        if done {
            session.phase = ClaudePhase::Finished;
            session.handle = None;
            // Whatever it did to the repository: show it.
            self.refresh_active_status(cx);
            self.refresh_active_log(cx);
            self.refresh_active_branches(cx);
            self.save_claude_sessions();
            // Finished while you were elsewhere (the dialog is closed): say so.
            if !window.has_active_dialog(cx)
                && let Some(s) = self.claude.iter().find(|s| s.id == id)
            {
                crate::ui::notify(
                    window,
                    cx,
                    tr!("repo.claude.agent_done", title = s.title.to_string()),
                );
            }
        }
        self.claude_scroll.scroll_to_bottom();
        window.refresh();
        cx.notify();
    }

    pub fn stop_claude(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(handle) = self
            .claude
            .iter()
            .find(|s| s.id == id)
            .and_then(|s| s.handle.as_ref())
        {
            handle.stop();
        }
        window.refresh();
        cx.notify();
    }

    /// The Changes list's "Fix with Claude Code": conflicts are in the working tree now.
    pub fn fix_conflicts_in_tree(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_claude_dialog(
            tr!("repo.claude.title_tree"),
            conflicts_prompt(),
            window,
            cx,
        );
    }

    /// A branch's right-click menu: merge the default branch into it and fix conflicts.
    pub fn merge_default_with_claude(
        &mut self,
        branch: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let names: Vec<String> = self
            .active_repo()
            .map(|r| {
                r.branches
                    .iter()
                    .filter(|b| !b.is_remote)
                    .map(|b| b.name.clone())
                    .collect()
            })
            .unwrap_or_default();
        let base = default_branch(&names).unwrap_or("main").to_string();
        self.open_claude_dialog(
            tr!("repo.claude.title_branch", branch = branch.clone()),
            branch_conflicts_prompt(&branch, &base),
            window,
            cx,
        );
    }

    /// The pull request's "Fix conflicts with Claude Code".
    pub fn fix_pull_request_conflicts(
        &mut self,
        number: u64,
        head: String,
        base: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_claude_dialog(
            tr!("repo.claude.title_pr", number = number.to_string()),
            pull_request_conflicts_prompt(number, &head, &base),
            window,
            cx,
        );
    }

    /// Typing in the prompt box redraws the dialog.
    pub(crate) fn on_claude_input_event(&mut self, event: &InputEvent, cx: &mut Context<Self>) {
        if matches!(event, InputEvent::Change) {
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompts_name_what_to_do_and_forbid_pushing() {
        assert_eq!(
            default_branch(&["feature".into(), "master".into()]),
            Some("master")
        );
        assert_eq!(
            default_branch(&["main".into(), "master".into()]),
            Some("main")
        );
        assert_eq!(default_branch(&["feature".into()]), None);

        let pr = pull_request_conflicts_prompt(42, "topic", "main");
        assert!(pr.contains("gh pr checkout 42") && pr.contains("origin/main"));
        let branch = branch_conflicts_prompt("topic", "main");
        assert!(branch.contains("git switch topic") && branch.contains("git merge origin/main"));
        for prompt in [conflicts_prompt(), pr, branch] {
            assert!(prompt.contains("Do not push"), "{prompt}");
            assert!(prompt.contains("conflict markers"));
        }
    }
}
