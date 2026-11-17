//! The Review dialog (Pull Requests → a pull request → Review): every file the pull request
//! changes down the left, the selected file's changes on the right, before and after
//! (side by side, or one column), and a "Review changes" form at the bottom where you
//! write a message and submit it as a comment, an approval or a request for changes.

use std::rc::Rc;

use anano_core::external::ReviewVerdict;
use anano_core::git::{DiffLine, LineKind, PatchFile, PatchStatus};
use gpui_kit::component::{
    ActiveTheme, Disableable, Sizable, WindowExt,
    button::ButtonVariants,
    h_flex,
    input::{InputEvent, Textarea},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, Entity, FontWeight, Hsla, InteractiveElement, IntoElement,
    ParentElement, ScrollStrategy, SharedString, StatefulInteractiveElement, Styled, Window, div,
    px, uniform_list,
};

use crate::bridge;
use crate::i18n::tr;
use crate::state::workspace::Workspace;
use crate::ui::status_pane::diff_row;

const DIALOG_WIDTH: f32 = 1000.;
const CONTENT_HEIGHT: f32 = 560.;
const FILES_WIDTH: f32 = 260.;
const ROW_HEIGHT: f32 = 20.;
const GUTTER_WIDTH: f32 = 44.;

/// What the dialog is showing. `files` is `None` while the diff is being fetched.
pub struct ReviewState {
    pub number: u64,
    pub files: Option<Result<Rc<Vec<PatchFile>>, String>>,
    pub selected: usize,
    /// Before and after side by side, rather than one column.
    pub split: bool,
    /// The "Review changes" form (message + verdict) is open.
    pub form_open: bool,
    pub verdict: ReviewVerdict,
    pub submitting: bool,
    /// Why the last submit failed, in GitHub's words.
    pub error: Option<String>,
}

/// One row of the side-by-side view: a hunk header across both sides, or the line as it was
/// (left) next to the line as it is (right).
pub(crate) enum SplitRow {
    Hunk(String),
    Pair(Option<DiffLine>, Option<DiffLine>),
}

/// Lay a file's lines out for the side-by-side view: context on both sides, and each run of
/// removed lines next to the run of added lines that replaced it, row by row.
pub(crate) fn split_rows(lines: &[DiffLine]) -> Vec<SplitRow> {
    fn flush(rows: &mut Vec<SplitRow>, dels: &mut Vec<DiffLine>, adds: &mut Vec<DiffLine>) {
        for i in 0..dels.len().max(adds.len()) {
            rows.push(SplitRow::Pair(dels.get(i).cloned(), adds.get(i).cloned()));
        }
        dels.clear();
        adds.clear();
    }
    let mut rows = Vec::new();
    let (mut dels, mut adds) = (Vec::new(), Vec::new());
    for line in lines {
        match line.kind {
            LineKind::Del => dels.push(line.clone()),
            LineKind::Add => adds.push(line.clone()),
            LineKind::Hunk | LineKind::File => {
                flush(&mut rows, &mut dels, &mut adds);
                rows.push(SplitRow::Hunk(line.text.clone()));
            }
            LineKind::Context => {
                flush(&mut rows, &mut dels, &mut adds);
                rows.push(SplitRow::Pair(Some(line.clone()), Some(line.clone())));
            }
        }
    }
    flush(&mut rows, &mut dels, &mut adds);
    rows
}

/// One half of a side-by-side row: line number, marker, text; tinted when it is the removed
/// (left) or added (right) side, greyed when there is nothing on this side.
fn split_cell(ix: usize, line: Option<&DiffLine>, left: bool, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let cell = h_flex()
        .flex_1()
        .min_w_0()
        .h(px(ROW_HEIGHT))
        .font_family(theme.mono_font_family.clone())
        .text_xs();
    let Some(line) = line else {
        return cell.bg(theme.muted.opacity(0.35)).into_any_element();
    };
    let (tint, marker) = match line.kind {
        LineKind::Add => (Some(theme.success.opacity(0.16)), "+"),
        LineKind::Del => (Some(theme.danger.opacity(0.16)), "-"),
        _ => (None, " "),
    };
    let number = if left { line.old_no } else { line.new_no };
    cell.when_some(tint, |c, tint| c.bg(tint))
        .child(
            div()
                .w(px(GUTTER_WIDTH))
                .flex_none()
                .flex()
                .justify_end()
                .pr_2()
                .text_color(muted)
                .children(number.map(|n| n.to_string())),
        )
        .child(div().w(px(14.)).flex_none().text_color(muted).child(marker))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .child(crate::ui::selectable(
                    (if left { "split-old" } else { "split-new" }, ix),
                    line.text.replace('\t', "    "),
                )),
        )
        .into_any_element()
}

fn split_row(ix: usize, row: &SplitRow, cx: &App) -> AnyElement {
    match row {
        SplitRow::Hunk(text) => h_flex()
            .h(px(ROW_HEIGHT))
            .w_full()
            .px_3()
            .bg(cx.theme().secondary)
            .text_xs()
            .font_family(cx.theme().mono_font_family.clone())
            .text_color(cx.theme().muted_foreground)
            .child(
                div()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(text.clone()),
            )
            .into_any_element(),
        SplitRow::Pair(old, new) => h_flex()
            .w_full()
            .child(split_cell(ix, old.as_ref(), true, cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .border_l_1()
                    .border_color(cx.theme().border)
                    .child(split_cell(ix, new.as_ref(), false, cx)),
            )
            .into_any_element(),
    }
}

/// A / D / M / R in the colour of what happened to the file.
fn status_badge(status: PatchStatus, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let (glyph, color) = match status {
        PatchStatus::Added => ("A", theme.success),
        PatchStatus::Deleted => ("D", theme.danger),
        PatchStatus::Renamed => ("R", theme.primary),
        PatchStatus::Modified => ("M", theme.warning),
    };
    div()
        .flex_none()
        .size_4()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(3.))
        .border_1()
        .border_color(color)
        .text_color(color)
        .text_xs()
        .font_weight(FontWeight::BOLD)
        .child(glyph)
}

fn note(text: SharedString, color: Hsla) -> AnyElement {
    div()
        .p_4()
        .text_sm()
        .text_color(color)
        .child(text)
        .into_any_element()
}

fn verdict_text(verdict: ReviewVerdict) -> (SharedString, SharedString) {
    match verdict {
        ReviewVerdict::Comment => (
            tr!("repo.pulls.verdict_pick.comment"),
            tr!("repo.pulls.verdict_pick.comment_desc"),
        ),
        ReviewVerdict::Approve => (
            tr!("repo.pulls.verdict_pick.approve"),
            tr!("repo.pulls.verdict_pick.approve_desc"),
        ),
        ReviewVerdict::RequestChanges => (
            tr!("repo.pulls.verdict_pick.request"),
            tr!("repo.pulls.verdict_pick.request_desc"),
        ),
    }
}

/// Everything inside the dialog, rebuilt from the workspace's review state each frame.
fn review_body(workspace: &Entity<Workspace>, cx: &mut App) -> AnyElement {
    let ws = workspace.read(cx);
    let Some(review) = ws.pulls.as_ref().and_then(|p| p.review.as_ref()) else {
        return div().into_any_element();
    };
    let (files, selected, split) = (review.files.clone(), review.selected, review.split);
    let (form_open, verdict, submitting) = (review.form_open, review.verdict, review.submitting);
    let error = review.error.clone();
    let message_empty = ws.review_input.read(cx).value().trim().is_empty();
    let (input, scroll) = (ws.review_input.clone(), ws.review_scroll.clone());
    let theme = cx.theme();
    let (muted, danger, border, active, hover, success) = (
        theme.muted_foreground,
        theme.danger,
        theme.border,
        theme.list_active,
        theme.list_hover,
        theme.success,
    );

    // ---- the files and the diff
    let main = match &files {
        None => note(tr!("repo.pulls.review_loading"), muted),
        Some(Err(err)) => note(err.clone().into(), danger),
        Some(Ok(files)) if files.is_empty() => note(tr!("repo.pulls.review_no_files"), muted),
        Some(Ok(files)) => {
            let selected = selected.min(files.len() - 1);
            let list = v_flex()
                .id("review-files")
                .flex_none()
                .w(px(FILES_WIDTH))
                .h_full()
                .border_r_1()
                .border_color(border)
                .overflow_y_scroll()
                .children(files.iter().enumerate().map(|(ix, file)| {
                    let ws = workspace.clone();
                    h_flex()
                        .id(("review-file", ix))
                        .flex_none()
                        .gap_2()
                        .px_3()
                        .py_1p5()
                        .items_center()
                        .border_b_1()
                        .border_color(border)
                        .cursor_pointer()
                        .when(ix == selected, |row| row.bg(active))
                        .hover(move |style| style.bg(hover))
                        .child(status_badge(file.status, cx))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_sm()
                                .child(file.path.clone()),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_xs()
                                .text_color(muted)
                                .child(format!("+{} −{}", file.additions, file.deletions)),
                        )
                        .on_click(move |_, _, cx| {
                            ws.update(cx, |ws, cx| ws.set_review_file(ix, cx))
                        })
                }));
            let file = &files[selected];
            let rows: AnyElement = if file.lines.is_empty() {
                note(tr!("repo.pulls.review_file_empty"), muted)
            } else if split {
                let rows = Rc::new(split_rows(&file.lines));
                uniform_list("review-diff", rows.len(), move |range, _, cx| {
                    range.map(|i| split_row(i, &rows[i], cx)).collect()
                })
                .track_scroll(&scroll)
                .flex_1()
                .min_h_0()
                .into_any_element()
            } else {
                let files = files.clone();
                uniform_list("review-diff", file.lines.len(), move |range, _, cx| {
                    range
                        .map(|i| diff_row(i, &files[selected].lines[i], cx))
                        .collect()
                })
                .track_scroll(&scroll)
                .flex_1()
                .min_h_0()
                .into_any_element()
            };
            let heading = match &file.old_path {
                Some(old) => format!("{old} → {}", file.path),
                None => file.path.clone(),
            };
            let pane = v_flex()
                .flex_1()
                .min_w_0()
                .h_full()
                .child(
                    h_flex()
                        .flex_none()
                        .h_9()
                        .px_3()
                        .gap_2()
                        .items_center()
                        .border_b_1()
                        .border_color(border)
                        .text_sm()
                        .child(div().flex_1().min_w_0().truncate().child(heading))
                        .child(
                            div()
                                .flex_none()
                                .text_xs()
                                .text_color(muted)
                                .child(format!("+{} −{}", file.additions, file.deletions)),
                        ),
                )
                .child(rows);
            h_flex()
                .flex_1()
                .min_h_0()
                .items_start()
                .child(list)
                .child(pane)
                .into_any_element()
        }
    };

    // ---- the line above it: totals, and Unified / Split
    let (count, additions, deletions) = match &files {
        Some(Ok(files)) => (
            files.len(),
            files.iter().map(|f| f.additions).sum::<usize>(),
            files.iter().map(|f| f.deletions).sum::<usize>(),
        ),
        _ => (0, 0, 0),
    };
    let mode_button = |id: &'static str, label: SharedString, on: bool, to_split: bool| {
        let ws = workspace.clone();
        crate::ui::button(id)
            .ghost()
            .xsmall()
            .label(label)
            .when(on, |b| b.bg(active))
            .on_click(move |_, _, cx| ws.update(cx, |ws, cx| ws.set_review_split(to_split, cx)))
    };
    let top = h_flex()
        .flex_none()
        .h_10()
        .px_3()
        .gap_3()
        .items_center()
        .border_b_1()
        .border_color(border)
        .child(
            div()
                .text_sm()
                .child(tr!("repo.history.changed_files", count = count.to_string())),
        )
        .child(
            h_flex()
                .gap_1()
                .text_sm()
                .child(div().text_color(success).child(format!("+{additions}")))
                .child(div().text_color(danger).child(format!("−{deletions}"))),
        )
        .child(div().flex_1())
        .child(
            h_flex()
                .gap_1()
                .child(mode_button(
                    "review-split",
                    tr!("repo.pulls.review_split"),
                    split,
                    true,
                ))
                .child(mode_button(
                    "review-unified",
                    tr!("repo.pulls.review_unified"),
                    !split,
                    false,
                )),
        );

    // ---- the review form
    let form = form_open.then(|| {
        let (_, hint) = verdict_text(verdict);
        let pills = h_flex().gap_2().children(
            [
                ReviewVerdict::Comment,
                ReviewVerdict::Approve,
                ReviewVerdict::RequestChanges,
            ]
            .into_iter()
            .enumerate()
            .map(|(ix, option)| {
                let ws = workspace.clone();
                let button = crate::ui::button(("review-verdict", ix))
                    .small()
                    .label(verdict_text(option).0);
                if option == verdict {
                    button.primary()
                } else {
                    button.outline()
                }
                .on_click(move |_, _, cx| ws.update(cx, |ws, cx| ws.set_review_verdict(option, cx)))
            }),
        );
        let ws = workspace.clone();
        v_flex()
            .flex_none()
            .gap_2()
            .p_3()
            .border_t_1()
            .border_color(border)
            .child(
                div()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(border)
                    .child(Textarea::new(&input)),
            )
            .child(pills)
            .child(div().text_xs().text_color(muted).child(hint))
            .children(error.map(|err| div().text_sm().text_color(danger).child(err)))
            .child(
                h_flex().justify_end().child(
                    crate::ui::button("review-submit")
                        .primary()
                        .label(tr!("repo.pulls.review_submit"))
                        .loading(submitting)
                        .disabled(
                            submitting || (verdict != ReviewVerdict::Approve && message_empty),
                        )
                        .on_click(move |_, window, cx| {
                            ws.update(cx, |ws, cx| ws.submit_review(window, cx))
                        }),
                ),
            )
    });

    // ---- the bar at the bottom
    let toggle_ws = workspace.clone();
    let bar = h_flex()
        .flex_none()
        .gap_2()
        .p_3()
        .justify_end()
        .border_t_1()
        .border_color(border)
        .child(
            crate::ui::button("review-close")
                .outline()
                .label(tr!("common.cancel"))
                .on_click(|_, window, cx| window.close_dialog(cx)),
        )
        .child({
            let button =
                crate::ui::button("review-changes").label(tr!("repo.pulls.review_changes"));
            if form_open {
                button.outline()
            } else {
                button.primary()
            }
            .on_click(move |_, _, cx| toggle_ws.update(cx, |ws, cx| ws.toggle_review_form(cx)))
        });

    v_flex()
        .h(px(CONTENT_HEIGHT))
        .child(top)
        .child(main)
        .children(form)
        .child(bar)
        .into_any_element()
}

impl Workspace {
    /// The header's Review button: open the dialog for the pull request being read and
    /// fetch its changes.
    pub fn open_review_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if window.has_active_dialog(cx) {
            return;
        }
        let (Some(origin), Some(pulls)) = (self.pulls_origin(), &mut self.pulls) else {
            return;
        };
        let Some(number) = pulls.open else {
            return;
        };
        pulls.review = Some(ReviewState {
            number,
            files: None,
            selected: 0,
            split: true,
            form_open: false,
            verdict: ReviewVerdict::default(),
            submitting: false,
            error: None,
        });
        self.review_input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.review_scroll.scroll_to_item(0, ScrollStrategy::Top);
        let task = bridge::pull_request_files(cx, origin, number);
        cx.spawn(async move |ws, cx| {
            let result = task.await.map(Rc::new).map_err(|e| e.to_string());
            let _ = ws.update(cx, |ws, cx| {
                if let Some(review) = ws.pulls.as_mut().and_then(|p| p.review.as_mut())
                    && review.number == number
                {
                    review.files = Some(result);
                    cx.notify();
                }
            });
        })
        .detach();
        self.show_review_dialog(number, window, cx);
    }

    /// The dialog itself; its content is `review_body`, read from the review state.
    fn show_review_dialog(&mut self, number: u64, window: &mut Window, cx: &mut Context<Self>) {
        let workspace = cx.entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let workspace = workspace.clone();
            dialog
                .title(tr!("repo.pulls.review_title", number = number.to_string()))
                .w(px(DIALOG_WIDTH))
                .content(move |content, _, cx| content.child(review_body(&workspace, cx)))
        });
    }

    fn review_mut(&mut self) -> Option<&mut ReviewState> {
        self.pulls.as_mut().and_then(|p| p.review.as_mut())
    }

    pub fn set_review_file(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(review) = self.review_mut() {
            review.selected = ix;
        }
        self.review_scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    pub fn set_review_split(&mut self, split: bool, cx: &mut Context<Self>) {
        if let Some(review) = self.review_mut() {
            review.split = split;
        }
        self.review_scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    pub fn toggle_review_form(&mut self, cx: &mut Context<Self>) {
        if let Some(review) = self.review_mut() {
            review.form_open = !review.form_open;
            review.error = None;
        }
        cx.notify();
    }

    pub fn set_review_verdict(&mut self, verdict: ReviewVerdict, cx: &mut Context<Self>) {
        if let Some(review) = self.review_mut() {
            review.verdict = verdict;
            review.error = None;
        }
        cx.notify();
    }

    /// "Submit review": send the message and verdict to GitHub. On success the dialog
    /// closes and the pull request is re-read so the review shows in its history; on
    /// failure GitHub's message is shown in the form and nothing is lost.
    pub fn submit_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(origin) = self.pulls_origin() else {
            return;
        };
        let body = self.review_input.read(cx).value().to_string();
        let Some(review) = self.review_mut() else {
            return;
        };
        if review.submitting {
            return;
        }
        review.submitting = true;
        review.error = None;
        let (number, verdict) = (review.number, review.verdict);
        let task = bridge::submit_review(cx, origin, number, verdict, body);
        cx.notify();
        cx.spawn_in(window, async move |ws, cx| {
            let result = task.await.map_err(|e| e.to_string());
            let _ = ws.update_in(cx, |ws, window, cx| {
                match result {
                    Ok(()) => {
                        if let Some(pulls) = &mut ws.pulls {
                            pulls.review = None;
                        }
                        window.close_dialog(cx);
                        ws.open_pull_request(number, cx);
                    }
                    Err(err) => {
                        if let Some(review) = ws.review_mut() {
                            review.submitting = false;
                            review.error = Some(err);
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Typing in the review message must re-enable / disable Submit.
    pub(crate) fn on_review_input_event(&mut self, event: &InputEvent, cx: &mut Context<Self>) {
        if matches!(event, InputEvent::Change) {
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::pull_requests::PullRequests;
    use gpui_kit::AppContext as _;

    fn line(kind: LineKind, old: Option<u32>, new: Option<u32>, text: &str) -> DiffLine {
        DiffLine {
            kind,
            old_no: old,
            new_no: new,
            text: text.into(),
        }
    }

    /// Removed and added lines meet row by row; the longer run spills onto its own side.
    #[test]
    fn side_by_side_pairs_removed_lines_with_what_replaced_them() {
        let lines = vec![
            line(LineKind::Hunk, None, None, "@@ -1,4 +1,5 @@"),
            line(LineKind::Context, Some(1), Some(1), "keep"),
            line(LineKind::Del, Some(2), None, "old a"),
            line(LineKind::Del, Some(3), None, "old b"),
            line(LineKind::Add, None, Some(2), "new a"),
            line(LineKind::Add, None, Some(3), "new b"),
            line(LineKind::Add, None, Some(4), "new c"),
            line(LineKind::Context, Some(4), Some(5), "tail"),
        ];
        let rows = split_rows(&lines);
        let text = |l: &Option<DiffLine>| l.as_ref().map(|l| l.text.clone());
        let shape: Vec<_> = rows
            .iter()
            .map(|r| match r {
                SplitRow::Hunk(h) => (Some(h.clone()), None),
                SplitRow::Pair(l, r) => (text(l), text(r)),
            })
            .collect();
        let s = |x: &str| Some(x.to_string());
        assert_eq!(
            shape,
            [
                (s("@@ -1,4 +1,5 @@"), None),
                (s("keep"), s("keep")),
                (s("old a"), s("new a")),
                (s("old b"), s("new b")),
                (None, s("new c")),
                (s("tail"), s("tail")),
            ]
        );
    }

    /// The dialog opens and draws in every state: loading, loaded side by side, loaded in
    /// one column, with the review form open, and after a failed submit.
    #[gpui_kit::test]
    fn the_review_dialog_draws_in_every_state(cx: &mut gpui_kit::TestAppContext) {
        cx.executor().allow_parking();
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::bridge::init(cx);
        });
        let mut workspace = None;
        let window = cx.add_window(|window, cx| {
            let ws = cx.new(|cx| Workspace::new(window, cx));
            workspace = Some(ws.clone());
            gpui_kit::component::Root::new(ws, window, cx)
        });
        let workspace = workspace.unwrap();
        let file = PatchFile {
            path: "src/a.rs".into(),
            old_path: None,
            status: PatchStatus::Modified,
            additions: 2,
            deletions: 1,
            binary: false,
            lines: vec![
                line(LineKind::Hunk, None, None, "@@ -1,2 +1,3 @@"),
                line(LineKind::Context, Some(1), Some(1), "keep"),
                line(LineKind::Del, Some(2), None, "old"),
                line(LineKind::Add, None, Some(2), "new"),
                line(LineKind::Add, None, Some(3), "more"),
            ],
        };
        let draw = |cx: &mut gpui_kit::VisualTestContext| {
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
        };
        let cx = &mut gpui_kit::VisualTestContext::from_window(window.into(), cx);
        cx.update(|window, cx| {
            workspace.update(cx, |ws, cx| {
                ws.pulls = Some(PullRequests {
                    open: Some(7),
                    review: Some(ReviewState {
                        number: 7,
                        files: None,
                        selected: 0,
                        split: true,
                        form_open: false,
                        verdict: ReviewVerdict::Comment,
                        submitting: false,
                        error: None,
                    }),
                    ..Default::default()
                });
                ws.show_review_dialog(7, window, cx);
                assert!(window.has_active_dialog(cx));
            });
        });
        draw(cx); // loading

        workspace.update(cx, |ws, cx| {
            ws.review_mut().unwrap().files = Some(Ok(Rc::new(vec![file])));
            cx.notify();
        });
        draw(cx); // side by side
        workspace.update(cx, |ws, cx| ws.set_review_split(false, cx));
        draw(cx); // one column
        workspace.update(cx, |ws, cx| {
            ws.toggle_review_form(cx);
            ws.set_review_verdict(ReviewVerdict::RequestChanges, cx);
            ws.review_mut().unwrap().error = Some("GitHub said no".into());
        });
        draw(cx); // form open, with an error
        workspace.read_with(cx, |ws, _| {
            let review = ws.pulls.as_ref().unwrap().review.as_ref().unwrap();
            assert!(review.form_open && !review.split);
            assert_eq!(review.verdict, ReviewVerdict::RequestChanges);
        });
    }
}
