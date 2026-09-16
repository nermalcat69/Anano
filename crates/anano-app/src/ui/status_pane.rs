//! 主内容区：暂时只是个占位。改动列表和提交历史都挪到侧栏了（见 `ui::sidebar`）——
//! 用户反馈这些信息该跟着仓库条目直接看到。这里留给以后的 diff 视图（这次任务不做）。

use gpui_kit::component::{ActiveTheme, v_flex};
use gpui_kit::{Context, FontWeight, InteractiveElement, IntoElement, ParentElement, Styled, div};

use crate::i18n::tr;
use crate::state::workspace::Workspace;

impl Workspace {
    pub fn render_status_pane(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(repo) = self.active_repo() else {
            return v_flex()
                .id("status-pane")
                .size_full()
                .items_center()
                .justify_center()
                .gap_1()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(tr!("repo.none_open_title"))
                .child(div().text_xs().child(tr!("repo.none_open_hint")));
        };

        v_flex()
            .id("status-pane")
            .size_full()
            .items_center()
            .justify_center()
            .gap_1()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(repo.entry.display_name.clone()),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(tr!("repo.main_placeholder")),
            )
    }
}
