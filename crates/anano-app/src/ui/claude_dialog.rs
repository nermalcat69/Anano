//! "Fix conflicts with Claude Code": a dialog that runs Claude Code on the open repository
//! with a prompt to resolve merge conflicts, and shows what it does as it goes.
//!
//! The prompt is shown (and editable) before anything runs, so starting it is a decision,
//! not a side effect. It runs non-interactively: Claude Code can edit files and run `git`
//! and `gh`, and is told not to push. It cannot ask questions; the run can be stopped.

use std::path::PathBuf;

use anano_core::claude::{ClaudeHandle, ClaudeLine};
use gpui_kit::component::{
    ActiveTheme, Disableable, WindowExt,
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

const DIALOG_WIDTH: f32 = 820.;
const LOG_HEIGHT: f32 = 380.;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaudePhase {
    /// The prompt is on screen, waiting to be run.
    Ready,
    Running,
    Finished,
}

pub struct ClaudeSession {
    /// What this run is for, in the dialog's title.
    pub title: SharedString,
    pub dir: PathBuf,
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

/// The dialog's content, rebuilt from the session each frame.
fn claude_body(workspace: &Entity<Workspace>, cx: &mut App) -> AnyElement {
    let ws = workspace.read(cx);
    let Some(session) = &ws.claude else {
        return div().into_any_element();
    };
    let (phase, error) = (session.phase, session.error.clone());
    let lines = session.lines.clone();
    let dir = session.dir.display().to_string();
    let installed = anano_core::claude::find_claude().is_some();
    let (input, scroll) = (ws.claude_input.clone(), ws.claude_scroll.clone());
    let theme = cx.theme();
    let (muted, danger, border) = (theme.muted_foreground, theme.danger, theme.border);

    let place = div()
        .text_xs()
        .text_color(muted)
        .child(tr!("repo.claude.where", path = dir));
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
        .justify_end()
        .child(match phase {
            ClaudePhase::Running => crate::ui::button("claude-stop")
                .outline()
                .label(tr!("repo.claude.stop"))
                .on_click(move |_, _, cx| ws_stop.update(cx, |ws, cx| ws.stop_claude(cx)))
                .into_any_element(),
            _ => crate::ui::button("claude-close")
                .outline()
                .label(if phase == ClaudePhase::Finished {
                    tr!("repo.claude.close")
                } else {
                    tr!("common.cancel")
                })
                .on_click(|_, window, cx| window.close_dialog(cx))
                .into_any_element(),
        })
        .when(phase == ClaudePhase::Ready, |h| {
            h.child(
                crate::ui::button("claude-run")
                    .primary()
                    .label(tr!("repo.claude.run"))
                    .disabled(!installed)
                    .on_click(move |_, _, cx| ws_run.update(cx, |ws, cx| ws.run_claude(cx))),
            )
        });
    v_flex()
        .gap_3()
        .py_1()
        .child(place)
        .child(main)
        .child(buttons)
        .into_any_element()
}

impl Workspace {
    /// Open the dialog with `prompt` ready to run in the open repository. A run still going
    /// from before is shown instead of being replaced.
    pub fn open_claude_dialog(
        &mut self,
        title: SharedString,
        prompt: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.has_active_dialog(cx) {
            return;
        }
        let Some(dir) = self.active_repo().map(|r| r.entry.path.clone()) else {
            return;
        };
        let running = self
            .claude
            .as_ref()
            .is_some_and(|s| s.phase == ClaudePhase::Running);
        let title = if running {
            self.claude
                .as_ref()
                .map(|s| s.title.clone())
                .unwrap_or(title)
        } else {
            self.claude = Some(ClaudeSession {
                title: title.clone(),
                dir,
                phase: ClaudePhase::Ready,
                lines: Vec::new(),
                handle: None,
                error: None,
            });
            self.claude_input
                .update(cx, |input, cx| input.set_value(prompt, window, cx));
            title
        };
        let workspace = cx.entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let workspace = workspace.clone();
            dialog
                .title(title.clone())
                .w(px(DIALOG_WIDTH))
                // Closing it must be a choice: a run in progress has Stop.
                .overlay_closable(false)
                .content(move |content, _, cx| content.child(claude_body(&workspace, cx)))
        });
    }

    /// "Run Claude Code": start it with whatever the prompt box now says.
    pub fn run_claude(&mut self, cx: &mut Context<Self>) {
        let prompt = self.claude_input.read(cx).value().trim().to_string();
        let Some(session) = &mut self.claude else {
            return;
        };
        if prompt.is_empty() || session.phase != ClaudePhase::Ready {
            return;
        }
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<ClaudeLine>();
        match anano_core::claude::spawn(&session.dir, &prompt, move |line| {
            let _ = tx.send(line);
        }) {
            Ok(handle) => {
                session.phase = ClaudePhase::Running;
                session.handle = Some(handle);
                session.error = None;
            }
            Err(err) => {
                session.error = Some(err);
                cx.notify();
                return;
            }
        }
        cx.notify();
        cx.spawn(async move |ws, cx| {
            while let Some(line) = rx.recv().await {
                let done = matches!(line, ClaudeLine::Done { .. });
                let _ = ws.update(cx, |ws, cx| ws.push_claude_line(line, cx));
                if done {
                    break;
                }
            }
        })
        .detach();
    }

    fn push_claude_line(&mut self, line: ClaudeLine, cx: &mut Context<Self>) {
        let Some(session) = &mut self.claude else {
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
        }
        self.claude_scroll.scroll_to_bottom();
        cx.notify();
    }

    pub fn stop_claude(&mut self, cx: &mut Context<Self>) {
        if let Some(handle) = self.claude.as_ref().and_then(|s| s.handle.as_ref()) {
            handle.stop();
        }
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
