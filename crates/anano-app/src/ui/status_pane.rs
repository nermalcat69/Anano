//! Main content area: the diff of whatever is selected in the sidebar — a changed file
//! (Changes tab) or a commit (History tab). Rows follow the Nermal IDE's unified diff view:
//! old/new line-number gutters, a +/- marker, tinted added and deleted lines.

use std::rc::Rc;

use anano_core::git::{DiffLine, DiffView, LineKind};
use gpui_kit::component::{
    ActiveTheme, IconName, Sizable,
    button::{Button, ButtonVariants},
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, FontWeight, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, div, px, uniform_list,
};

use crate::i18n::tr;
use crate::state::workspace::{Selected, Workspace};

const ROW_HEIGHT: f32 = 20.;
const GUTTER_WIDTH: f32 = 44.;
const FILES_WIDTH: f32 = 240.;

pub(crate) fn diff_row(ix: usize, line: &DiffLine, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    if line.kind == LineKind::File {
        return h_flex()
            .h(px(ROW_HEIGHT))
            .w_full()
            .px_3()
            .bg(theme.secondary)
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .child(
                div()
                    .truncate()
                    .child(crate::ui::selectable(("diff-file", ix), line.text.clone())),
            )
            .into_any_element();
    }
    let (marker, tint) = match line.kind {
        LineKind::Add => ("+", Some(theme.success.opacity(0.16))),
        LineKind::Del => ("-", Some(theme.danger.opacity(0.16))),
        _ => (" ", None),
    };
    let number = |n: Option<u32>| {
        div()
            .w(px(GUTTER_WIDTH))
            .flex_none()
            .flex()
            .justify_end()
            .pr_2()
            .text_color(muted)
            .children(n.map(|n| n.to_string()))
    };
    h_flex()
        .h(px(ROW_HEIGHT))
        .w_full()
        .font_family(theme.mono_font_family.clone())
        .text_xs()
        .when_some(tint, |row, tint| row.bg(tint))
        .when(line.kind == LineKind::Hunk, |row| row.text_color(muted))
        .child(number(line.old_no))
        .child(number(line.new_no))
        .child(div().w(px(14.)).flex_none().text_color(muted).child(marker))
        // ponytail: long lines are clipped, no horizontal scroll. Add a shared horizontal
        // scroll handle across rows when that starts to hurt.
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .child(crate::ui::selectable(
                    ("diff-text", ix),
                    line.text.replace('\t', "    "),
                )),
        )
        .into_any_element()
}

impl Workspace {
    fn render_open_in_nermal(&self, cx: &mut Context<Self>) -> Option<Button> {
        self.nermal_available.then(|| {
            crate::ui::button("open-in-nermal")
                .outline()
                .small()
                .icon(IconName::ExternalLink)
                .label(tr!("repo.open_in_nermal"))
                .tooltip(tr!("repo.open_in_nermal_tooltip"))
                .on_click(cx.listener(|this, _, _, cx| this.open_in_nermal(cx)))
        })
    }

    /// Title + subtitle for the header above the diff.
    fn diff_heading(&self, selected: &Selected) -> (SharedString, SharedString) {
        match selected {
            Selected::File { path } => (path.clone().into(), SharedString::default()),
            Selected::Commit(id) => {
                let commit = self
                    .active_repo()
                    .and_then(|r| r.commits.iter().find(|c| &c.id == id));
                let short = &id[..7.min(id.len())];
                match commit {
                    Some(c) => (
                        c.summary.clone().into(),
                        format!("{short} · {}", c.author).into(),
                    ),
                    None => (short.to_string().into(), SharedString::default()),
                }
            }
        }
    }

    pub fn render_status_pane(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let pane = v_flex().id("status-pane").size_full();
        let centered = |pane: gpui_kit::Stateful<gpui_kit::Div>| {
            pane.items_center().justify_center().gap_1().text_sm()
        };
        let muted = cx.theme().muted_foreground;
        if let Some(busy) = self.busy.clone() {
            return centered(pane).gap_3().child(busy).child(
                gpui_kit::component::progress::Progress::new("busy-progress")
                    .loading(true)
                    .w(px(280.)),
            );
        }
        if let Some(pulls) = &self.pulls {
            return pane.child(self.render_pull_requests(pulls, cx));
        }
        if let Some(workflows) = &self.workflows {
            return pane.child(self.render_workflows(workflows, cx));
        }
        if let Some(account) = &self.account {
            return pane.child(self.render_account(account, cx));
        }
        let Some(repo) = self.active_repo() else {
            return centered(pane)
                .text_color(muted)
                .child(tr!("repo.none_open_title"))
                .child(div().text_xs().child(tr!("repo.none_open_hint")));
        };
        let Some(selected) = self.selected() else {
            // Nothing to commit: say so, and suggest what to do next.
            if let Some(panel) = self.render_no_changes(repo, cx) {
                return pane.child(panel);
            }
            return centered(pane)
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(repo.entry.display_name.clone()),
                )
                .child(div().text_color(muted).child(tr!("repo.main_placeholder")))
                .children(
                    self.render_open_in_nermal(cx)
                        .map(|button| div().mt_2().child(button)),
                );
        };

        let (title, subtitle) = self.diff_heading(selected);
        // A commit gets GitHub Desktop's header: who made it, and its SHA with a copy button.
        let commit = match selected {
            Selected::Commit(id) => repo.commits.iter().find(|c| &c.id == id),
            Selected::File { .. } => None,
        };
        let byline = commit.map(|c| {
            let picture = self.avatars.get(&c.email).and_then(|p| p.as_ref());
            let short: SharedString = c.id[..7.min(c.id.len())].to_string().into();
            let full = c.id.clone();
            h_flex()
                .flex_none()
                .gap_2()
                .items_center()
                .text_xs()
                .text_color(muted)
                .child(crate::ui::avatar(picture, &c.author, px(18.), cx))
                .child(c.author.clone())
                .child(
                    div()
                        .font_family(cx.theme().mono_font_family.clone())
                        .child(short),
                )
                .child(
                    crate::ui::button("copy-sha")
                        .ghost()
                        .xsmall()
                        .icon(IconName::Copy)
                        .tooltip(tr!("repo.history.copy_sha"))
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(
                                full.clone(),
                            ));
                        }),
                )
        });
        let subtitle = if byline.is_some() {
            SharedString::default()
        } else {
            subtitle
        };
        let truncated = matches!(&self.diff, Some(Ok(view)) if view.truncated);
        let header = h_flex()
            .flex_none()
            .h_10()
            .px_3()
            .gap_3()
            .items_center()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_2()
                    .items_center()
                    .text_sm()
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(crate::ui::selectable("diff-title", title)),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_xs()
                            .text_color(muted)
                            .child(subtitle),
                    )
                    .children(byline)
                    .when(truncated, |h| {
                        h.child(
                            div()
                                .flex_none()
                                .text_xs()
                                .text_color(cx.theme().warning)
                                .child(tr!("repo.diff.truncated")),
                        )
                    }),
            )
            .children(self.render_open_in_nermal(cx).map(|b| b.xsmall()));

        let message = |text: SharedString, color| {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(color)
                .child(text)
                .into_any_element()
        };
        let body = match &self.diff {
            None => message(tr!("repo.diff.loading"), muted),
            Some(Err(err)) => message(err.clone().into(), cx.theme().danger),
            Some(Ok(view)) if view.lines.is_empty() => message(tr!("repo.diff.empty"), muted),
            Some(Ok(view)) => {
                // For a commit: the files it touched down the left; clicking one jumps the
                // diff to that file's section.
                let files = commit.is_some().then(|| {
                    let entries: Vec<(usize, &DiffLine)> = view
                        .lines
                        .iter()
                        .enumerate()
                        .filter(|(_, line)| line.kind == LineKind::File)
                        .collect();
                    let hover = cx.theme().list_hover;
                    v_flex()
                        .id("changed-files")
                        .flex_none()
                        .w(px(FILES_WIDTH))
                        .h_full()
                        .border_r_1()
                        .border_color(cx.theme().border)
                        .overflow_y_scroll()
                        .child(div().px_3().py_2().text_sm().text_color(muted).child(tr!(
                            "repo.history.changed_files",
                            count = entries.len().to_string()
                        )))
                        .children(entries.into_iter().map(|(ix, line)| {
                            let scroll = self.diff_scroll.clone();
                            div()
                                .id(("changed-file", ix))
                                .flex_none()
                                .px_3()
                                .py_1p5()
                                .text_sm()
                                .truncate()
                                .cursor_pointer()
                                .hover(move |style| style.bg(hover))
                                .child(line.text.clone())
                                .on_click(move |_, _, _| {
                                    scroll.scroll_to_item_strict(ix, gpui_kit::ScrollStrategy::Top)
                                })
                        }))
                });
                let view: Rc<DiffView> = view.clone();
                let lines = uniform_list("diff-lines", view.lines.len(), move |range, _, cx| {
                    range.map(|ix| diff_row(ix, &view.lines[ix], cx)).collect()
                })
                .track_scroll(&self.diff_scroll)
                .flex_1()
                .min_w_0()
                .h_full();
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_start()
                    .children(files)
                    .child(lines)
                    .into_any_element()
            }
        };
        pane.child(header).child(body)
    }
}
