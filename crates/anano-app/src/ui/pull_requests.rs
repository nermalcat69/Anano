//! The Pull Requests view (toolbar → Pull Requests): the repository's pull requests down
//! the left; the open one on the right with its description, its history (commits,
//! comments, reviews, oldest first) and, at the end, GitHub's verdict on merging it, with
//! a Merge button when GitHub says it can be merged.

use anano_core::external::{PrEvent, PrEventKind, PullRequest, PullRequestDetail};
use gpui_kit::component::{
    ActiveTheme, Disableable, Icon, IconName, Sizable, WindowExt, button::ButtonVariants,
    dialog::DialogFooter, h_flex, text::markdown, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};

use crate::assets::ICON_REFRESH;
use crate::i18n::tr;
use crate::state::workspace::Workspace;

const LIST_WIDTH: f32 = 340.;

/// What the Pull Requests view is showing. `None` in a slot means "loading".
#[derive(Default)]
pub struct PullRequests {
    pub list: Option<Result<Vec<PullRequest>, String>>,
    pub open: Option<u64>,
    pub detail: Option<Result<PullRequestDetail, String>>,
    pub merging: bool,
    /// What GitHub said when the last merge attempt was refused.
    pub merge_error: Option<String>,
}

/// GitHub's merge verdict in words, and whether the Merge button should be live.
///
/// `mergeable` / `merge_state` are passed through from the API; this is the one place that
/// decides what they mean for the user.
pub(crate) fn merge_verdict(pr: &PullRequestDetail) -> (SharedString, bool) {
    match (
        pr.summary.state.as_str(),
        pr.mergeable.as_str(),
        pr.merge_state.as_str(),
    ) {
        ("MERGED", ..) => (tr!("repo.pulls.verdict.merged"), false),
        ("CLOSED", ..) => (tr!("repo.pulls.verdict.closed"), false),
        _ if pr.summary.draft => (tr!("repo.pulls.verdict.draft"), false),
        (_, "CONFLICTING", _) | (.., "DIRTY") => (tr!("repo.pulls.verdict.conflicts"), false),
        (.., "BLOCKED") => (tr!("repo.pulls.verdict.blocked"), false),
        (.., "BEHIND") => (tr!("repo.pulls.verdict.behind"), false),
        (_, "MERGEABLE", "UNSTABLE") => (tr!("repo.pulls.verdict.unstable"), true),
        (_, "MERGEABLE", _) => (tr!("repo.pulls.verdict.clean"), true),
        // GitHub computes mergeability lazily; right after opening it often says UNKNOWN.
        _ => (tr!("repo.pulls.verdict.unknown"), false),
    }
}

fn state_badge(pr: &PullRequest, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let (label, color) = match pr.state.as_str() {
        "MERGED" => (tr!("repo.pulls.state.merged"), theme.primary),
        "CLOSED" => (tr!("repo.pulls.state.closed"), theme.danger),
        _ if pr.draft => (tr!("repo.pulls.state.draft"), theme.muted_foreground),
        _ => (tr!("repo.pulls.state.open"), theme.success),
    };
    div()
        .flex_none()
        .px_1p5()
        .py_0p5()
        .rounded(px(999.))
        .text_xs()
        .text_color(color)
        .bg(color.opacity(0.15))
        .child(label)
}

fn note(text: SharedString, color: Hsla) -> AnyElement {
    div()
        .p_4()
        .text_sm()
        .text_color(color)
        .child(text)
        .into_any_element()
}

fn event_row(event: &PrEvent, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let what = match event.kind {
        PrEventKind::Commit => tr!("repo.pulls.event.commit"),
        PrEventKind::Comment => tr!("repo.pulls.event.comment"),
        PrEventKind::Review => tr!("repo.pulls.event.review"),
    };
    let body = match event.kind {
        // A commit is one mono line: short SHA + subject.
        PrEventKind::Commit => div()
            .text_sm()
            .font_family(theme.mono_font_family.clone())
            .child(event.text.clone())
            .into_any_element(),
        _ => markdown(event.text.clone()).into_any_element(),
    };
    v_flex()
        .gap_1()
        .pl_3()
        .border_l_2()
        .border_color(theme.border)
        .child(
            h_flex()
                .gap_1p5()
                .text_xs()
                .text_color(muted)
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(event.author.clone()),
                )
                .child(what)
                .child("•")
                .child(crate::ui::time_ago(event.at)),
        )
        .child(body)
}

impl Workspace {
    /// Toolbar → Pull Requests: show the view and (re)load the list.
    pub fn open_pull_requests(&mut self, cx: &mut Context<Self>) {
        if self.pulls.is_none() {
            self.pulls = Some(PullRequests::default());
        }
        self.reload_pull_requests(cx);
        cx.notify();
    }

    pub fn close_pull_requests(&mut self, cx: &mut Context<Self>) {
        self.pulls = None;
        cx.notify();
    }

    fn pulls_origin(&self) -> Option<String> {
        self.active_repo().and_then(|r| r.origin.clone())
    }

    pub fn reload_pull_requests(&mut self, cx: &mut Context<Self>) {
        let Some(origin) = self.pulls_origin() else {
            if let Some(pulls) = &mut self.pulls {
                pulls.list = Some(Err(tr!("repo.pulls.no_origin").to_string()));
            }
            return;
        };
        let task = crate::bridge::pull_requests(cx, origin);
        cx.spawn(async move |ws, cx| {
            let list = task.await.map_err(|e| e.to_string());
            let _ = ws.update(cx, |ws, cx| {
                if let Some(pulls) = &mut ws.pulls {
                    pulls.list = Some(list);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Open one pull request (or refresh the one that's open).
    pub fn open_pull_request(&mut self, number: u64, cx: &mut Context<Self>) {
        let (Some(origin), Some(pulls)) = (self.pulls_origin(), &mut self.pulls) else {
            return;
        };
        if pulls.open != Some(number) {
            pulls.detail = None;
            pulls.merge_error = None;
        }
        pulls.open = Some(number);
        cx.notify();
        let task = crate::bridge::pull_request(cx, origin, number);
        cx.spawn(async move |ws, cx| {
            let detail = task.await.map_err(|e| e.to_string());
            let _ = ws.update(cx, |ws, cx| {
                // Another one may have been clicked while this was loading.
                if let Some(pulls) = &mut ws.pulls
                    && pulls.open == Some(number)
                {
                    pulls.detail = Some(detail);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Merging changes the repository on GitHub for everyone, so it is confirmed first.
    fn confirm_merge(
        &mut self,
        number: u64,
        title: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.has_active_dialog(cx) {
            return;
        }
        let workspace = cx.entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let (workspace, title) = (workspace.clone(), title.clone());
            dialog
                .title(tr!("repo.pulls.merge_title", number = number.to_string()))
                .w(px(440.))
                .content(move |content, _, _| content.child(div().text_sm().child(title.clone())))
                .footer(
                    DialogFooter::new()
                        .child(
                            crate::ui::button("merge-cancel")
                                .outline()
                                .label(tr!("common.cancel"))
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            crate::ui::button("merge-confirm")
                                .primary()
                                .label(tr!("repo.pulls.merge"))
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    workspace
                                        .update(cx, |ws, cx| ws.merge_pull_request(number, cx));
                                }),
                        ),
                )
        });
    }

    fn merge_pull_request(&mut self, number: u64, cx: &mut Context<Self>) {
        let (Some(origin), Some(pulls)) = (self.pulls_origin(), &mut self.pulls) else {
            return;
        };
        pulls.merging = true;
        pulls.merge_error = None;
        cx.notify();
        let task = crate::bridge::merge_pull_request(cx, origin, number);
        cx.spawn(async move |ws, cx| {
            let result = task.await;
            let _ = ws.update(cx, |ws, cx| {
                if let Some(pulls) = &mut ws.pulls {
                    pulls.merging = false;
                    pulls.merge_error = result.err().map(|e| e.to_string());
                }
                // Either way, show what GitHub now says about it.
                ws.open_pull_request(number, cx);
                ws.reload_pull_requests(cx);
                // A merge moved the base branch on the remote.
                ws.fetch(cx);
            });
        })
        .detach();
    }

    fn render_pull_list(&self, pulls: &PullRequests, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, danger, hover, active, border) = (
            theme.muted_foreground,
            theme.danger,
            theme.list_hover,
            theme.list_active,
            theme.border,
        );
        let rows = match &pulls.list {
            None => return note(tr!("repo.pulls.loading"), muted),
            Some(Err(err)) => return note(err.clone().into(), danger),
            Some(Ok(list)) if list.is_empty() => return note(tr!("repo.pulls.none"), muted),
            Some(Ok(list)) => list,
        };
        v_flex()
            .id("pull-list")
            .size_full()
            .overflow_y_scroll()
            .children(rows.iter().map(|pr| {
                let number = pr.number;
                let selected = pulls.open == Some(number);
                v_flex()
                    .id(("pull", number as usize))
                    .px_3()
                    .py_2()
                    .gap_1()
                    .border_b_1()
                    .border_color(border)
                    .cursor_pointer()
                    .when(selected, |row| row.bg(active))
                    .hover(move |style| style.bg(hover))
                    .on_click(cx.listener(move |this, _, _, cx| this.open_pull_request(number, cx)))
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_sm()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(pr.title.clone()),
                            )
                            .child(state_badge(pr, cx)),
                    )
                    .child(div().text_xs().text_color(muted).truncate().child(format!(
                        "#{} • {} • {}",
                        pr.number,
                        pr.author,
                        crate::ui::time_ago(pr.created_at)
                    )))
            }))
            .into_any_element()
    }

    fn render_pull_detail(&self, pulls: &PullRequests, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, danger, border) = (theme.muted_foreground, theme.danger, theme.border);
        let pr = match (&pulls.open, &pulls.detail) {
            (None, _) => return note(tr!("repo.pulls.pick_one"), muted),
            (_, None) => return note(tr!("repo.pulls.loading_one"), muted),
            (_, Some(Err(err))) => return note(err.clone().into(), danger),
            (_, Some(Ok(pr))) => pr,
        };
        let (verdict, can_merge) = merge_verdict(pr);
        let section = |title: SharedString| {
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .child(title)
        };
        let (number, title, url) = (pr.summary.number, pr.summary.title.clone(), pr.url.clone());
        v_flex()
            .id("pull-detail")
            .size_full()
            .overflow_y_scroll()
            .p_4()
            .gap_4()
            .child(
                v_flex()
                    .gap_1p5()
                    .child(
                        div()
                            .text_base()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(format!("{} #{}", pr.summary.title, pr.summary.number)),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .flex_wrap()
                            .text_xs()
                            .text_color(muted)
                            .child(state_badge(&pr.summary, cx))
                            .child(tr!(
                                "repo.pulls.wants_to_merge",
                                author = pr.summary.author.clone(),
                                head = pr.summary.head.clone(),
                                base = pr.summary.base.clone()
                            ))
                            .child(format!(
                                "+{} −{} • {}",
                                pr.additions,
                                pr.deletions,
                                tr!(
                                    "repo.history.changed_files",
                                    count = pr.changed_files.to_string()
                                )
                            ))
                            .child(
                                crate::ui::button("pull-open-web")
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::ExternalLink)
                                    .label(tr!("repo.history.view_on_github"))
                                    .on_click(move |_, _, cx| cx.open_url(&url)),
                            ),
                    ),
            )
            .child(if pr.body.trim().is_empty() {
                div()
                    .text_sm()
                    .text_color(muted)
                    .child(tr!("repo.pulls.no_description"))
                    .into_any_element()
            } else {
                markdown(pr.body.clone()).into_any_element()
            })
            .child(section(tr!("repo.pulls.history")))
            .child(
                v_flex()
                    .gap_3()
                    .children(pr.events.iter().map(|event| event_row(event, cx))),
            )
            // The end of the page: can it be merged, and the button if so.
            .child(
                v_flex()
                    .gap_2()
                    .p_3()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(border)
                    .child(div().text_sm().child(verdict))
                    .children(
                        pulls
                            .merge_error
                            .clone()
                            .map(|err| div().text_sm().text_color(danger).child(err)),
                    )
                    .when(pr.summary.state == "OPEN", |merge_box| {
                        merge_box.child(
                            h_flex().debug_selector(|| "pull-merge".into()).child(
                                crate::ui::button("pull-merge")
                                    .primary()
                                    .label(tr!("repo.pulls.merge"))
                                    .loading(pulls.merging)
                                    .disabled(!can_merge || pulls.merging)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.confirm_merge(number, title.clone(), window, cx)
                                    })),
                            ),
                        )
                    }),
            )
            .into_any_element()
    }

    pub fn render_pull_requests(
        &self,
        pulls: &PullRequests,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let border = cx.theme().border;
        let slug = self
            .pulls_origin()
            .and_then(|o| anano_core::external::repo_slug(&o))
            .unwrap_or_default();
        v_flex()
            .id("pull-requests")
            .size_full()
            .child(
                h_flex()
                    .flex_none()
                    .h_10()
                    .px_3()
                    .gap_2()
                    .items_center()
                    .border_b_1()
                    .border_color(border)
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(tr!("repo.pulls.title")),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(slug),
                    )
                    .child(
                        crate::ui::button("pulls-refresh")
                            .ghost()
                            .xsmall()
                            .icon(Icon::empty().path(ICON_REFRESH))
                            .tooltip(tr!("repo.clone.refresh"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.reload_pull_requests(cx);
                                if let Some(open) = this.pulls.as_ref().and_then(|p| p.open) {
                                    this.open_pull_request(open, cx);
                                }
                            })),
                    )
                    .child(
                        crate::ui::button("pulls-close")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Close)
                            .tooltip(tr!("repo.close"))
                            .on_click(cx.listener(|this, _, _, cx| this.close_pull_requests(cx))),
                    ),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_start()
                    .child(
                        div()
                            .flex_none()
                            .w(px(LIST_WIDTH))
                            .h_full()
                            .border_r_1()
                            .border_color(border)
                            .child(self.render_pull_list(pulls, cx)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .child(self.render_pull_detail(pulls, cx)),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr(state: &str, draft: bool, mergeable: &str, merge_state: &str) -> PullRequestDetail {
        PullRequestDetail {
            summary: PullRequest {
                number: 1,
                title: "t".into(),
                author: "a".into(),
                state: state.into(),
                draft,
                head: "h".into(),
                base: "main".into(),
                created_at: 0,
            },
            body: String::new(),
            url: String::new(),
            additions: 0,
            deletions: 0,
            changed_files: 0,
            mergeable: mergeable.into(),
            merge_state: merge_state.into(),
            events: Vec::new(),
        }
    }

    /// The Merge button is only live when GitHub itself says the pull request can merge.
    #[test]
    fn merge_button_follows_githubs_verdict() {
        let can = |state, draft, mergeable, merge_state| {
            merge_verdict(&pr(state, draft, mergeable, merge_state)).1
        };
        assert!(can("OPEN", false, "MERGEABLE", "CLEAN"));
        assert!(can("OPEN", false, "MERGEABLE", "UNSTABLE"));
        assert!(!can("OPEN", false, "CONFLICTING", "DIRTY"));
        assert!(!can("OPEN", false, "MERGEABLE", "BLOCKED"));
        assert!(!can("OPEN", false, "MERGEABLE", "BEHIND"));
        assert!(!can("OPEN", true, "MERGEABLE", "CLEAN"));
        assert!(!can("OPEN", false, "UNKNOWN", "UNKNOWN"));
        assert!(!can("MERGED", false, "MERGEABLE", "CLEAN"));
        assert!(!can("CLOSED", false, "MERGEABLE", "CLEAN"));
    }

    /// The whole view draws: list, a Markdown description, each kind of history entry, and
    /// the merge box with its button at the end.
    #[gpui_kit::test]
    fn pull_request_view_draws_list_detail_history_and_merge_box(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use gpui_kit::AppContext as _;
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
        let cx = &mut gpui_kit::VisualTestContext::from_window(window.into(), cx);
        let mut detail = pr("OPEN", false, "MERGEABLE", "CLEAN");
        detail.body = "# Heading\n\nSome **bold** text and a list:\n\n- one\n- two\n".into();
        detail.events = vec![
            PrEvent {
                kind: PrEventKind::Commit,
                author: "me".into(),
                at: 1,
                text: "abc1234 do it".into(),
            },
            PrEvent {
                kind: PrEventKind::Review,
                author: "you".into(),
                at: 2,
                text: "APPROVED".into(),
            },
            PrEvent {
                kind: PrEventKind::Comment,
                author: "you".into(),
                at: 3,
                text: "looks *good*".into(),
            },
        ];
        workspace.update(cx, |ws, cx| {
            ws.open_repo_path("/tmp/anano-pulls-test".into(), cx);
            ws.pulls = Some(PullRequests {
                list: Some(Ok(vec![detail.summary.clone()])),
                open: Some(1),
                detail: Some(Ok(detail)),
                ..Default::default()
            });
            cx.notify();
        });
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("pull-merge").is_some(),
            "merge box wasn't drawn"
        );
        // Selecting something in the sidebar goes back to the diff.
        workspace.update(cx, |ws, cx| {
            ws.select(
                crate::state::workspace::Selected::Commit("0".repeat(40)),
                cx,
            )
        });
        assert!(workspace.read_with(cx, |ws, _| ws.pulls.is_none()));
    }
}
