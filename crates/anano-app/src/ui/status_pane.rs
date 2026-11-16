//! Main content area: the diff of whatever is selected in the sidebar — a changed file
//! (Changes tab) or a commit (History tab). Rows follow the Nermal IDE's unified diff view:
//! old/new line-number gutters, a +/- marker, tinted added and deleted lines.

use std::rc::Rc;

use anano_core::git::{DiffLine, DiffView, LineKind};
use gpui_kit::component::{ActiveTheme, IconName, Sizable, button::Button, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, FontWeight, InteractiveElement, IntoElement, ParentElement,
    SharedString, Styled, div, px, uniform_list,
};

use crate::i18n::tr;
use crate::state::workspace::{Selected, Workspace};

const ROW_HEIGHT: f32 = 20.;
const GUTTER_WIDTH: f32 = 44.;

fn diff_row(line: &DiffLine, cx: &App) -> AnyElement {
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
            .child(div().truncate().child(line.text.clone()))
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
                .child(line.text.replace('\t', "    ")),
        )
        .into_any_element()
}

impl Workspace {
    fn render_open_in_nermal(&self, cx: &mut Context<Self>) -> Option<Button> {
        self.nermal_available.then(|| {
            Button::new("open-in-nermal")
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
            Selected::File { path, staged } => (
                path.clone().into(),
                if *staged {
                    tr!("repo.status.staged_title")
                } else {
                    tr!("repo.status.unstaged_title")
                },
            ),
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
        let Some(repo) = self.active_repo() else {
            return centered(pane)
                .text_color(muted)
                .child(tr!("repo.none_open_title"))
                .child(div().text_xs().child(tr!("repo.none_open_hint")));
        };
        let Some(selected) = self.selected() else {
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
                            .child(title),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_xs()
                            .text_color(muted)
                            .child(subtitle),
                    )
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
                let view: Rc<DiffView> = view.clone();
                uniform_list("diff-lines", view.lines.len(), move |range, _, cx| {
                    view.lines[range]
                        .iter()
                        .map(|line| diff_row(line, cx))
                        .collect()
                })
                .track_scroll(&self.diff_scroll)
                .flex_1()
                .min_h_0()
                .into_any_element()
            }
        };
        pane.child(header).child(body)
    }
}
