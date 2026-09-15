//! 内容区：当前激活仓库的标题行与最近提交历史。
//!
//! 未 stage / 已 stage 的文件列表连同暂存按钮已经挪到侧栏（见 `ui::sidebar`）——
//! 用户反馈这些信息该跟着仓库条目直接看到，不该单独占一块主内容区。
//! 这里只留下"当前是哪个仓库 / 刷新按钮 / 出错提示"的标题行，与提交历史。

use gpui_kit::component::{
    ActiveTheme, Icon, IconName, Sizable,
    button::{Button, ButtonVariants},
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, FontWeight, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, div, px,
};

use crate::i18n::tr;
use crate::state::repos::OpenRepo;
use crate::state::workspace::Workspace;

/// Unix 秒 → `YYYY-MM-DD HH:MM`，不拉时区/本地化库：历史列表只需要一眼看出先后顺序。
fn format_commit_time(seconds: i64) -> String {
    const DAY: i64 = 86_400;
    let days = seconds.div_euclid(DAY);
    let secs_of_day = seconds.rem_euclid(DAY);
    let (h, m) = (secs_of_day / 3600, (secs_of_day % 3600) / 60);

    // 公历儒略日算法（civil_from_days，Howard Hinnant 的公开算法）：把「1970-01-01 起的
    // 天数」转成年 / 月 / 日，不依赖任何时区数据库——本地提交历史用 UTC 展示足够。
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m_num = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m_num <= 2 { y + 1 } else { y };

    format!("{y:04}-{m_num:02}-{d:02} {h:02}:{m:02}")
}

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
                .child(div().text_xs().child(tr!("repo.none_open_hint")))
                .into_any_element();
        };

        v_flex()
            .id("status-pane")
            .size_full()
            .child(self.render_status_header(repo, cx))
            .child(
                v_flex()
                    .id("status-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_3()
                    .gap_4()
                    .child(self.render_history(repo, cx)),
            )
            .into_any_element()
    }

    fn render_status_header(&self, repo: &OpenRepo, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .flex_none()
            .h_10()
            .px_3()
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .truncate()
                            .child(repo.entry.display_name.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .truncate()
                            .child(repo.entry.path.display().to_string()),
                    ),
            )
            .when_some(repo.error.clone(), |d, err| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().danger)
                        .child(tr!("repo.status.error", error = err)),
                )
            })
            .child(
                Button::new("refresh-status")
                    .ghost()
                    .small()
                    .icon(Icon::new(IconName::RotateCw))
                    .loading(repo.loading)
                    .tooltip(tr!("repo.status.refresh"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.refresh_active_status(cx);
                        this.refresh_active_log(cx);
                    })),
            )
    }

    /// 最近提交历史：短 hash + 摘要 + 作者 + 时间，纯展示，不支持点进去看 diff（后续阶段）。
    fn render_history(&self, repo: &OpenRepo, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        if repo.commits.is_empty() {
            if repo.log_loading {
                return div().into_any_element();
            }
            return v_flex()
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(muted)
                        .child(tr!("repo.history.title")),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(muted)
                        .child(tr!("repo.history.empty")),
                )
                .into_any_element();
        }

        let rows: Vec<AnyElement> = repo
            .commits
            .iter()
            .map(|commit| {
                h_flex()
                    .h_7()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .w(px(64.))
                            .flex_none()
                            .text_xs()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_color(muted)
                            .child(commit.id[..7.min(commit.id.len())].to_string()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .truncate()
                            .child(commit.summary.clone()),
                    )
                    .child(div().flex_none().text_xs().text_color(muted).child(format!(
                        "{} · {}",
                        commit.author,
                        format_commit_time(commit.time)
                    )))
                    .into_any_element()
            })
            .collect();

        v_flex()
            .gap_1()
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(muted)
                    .child(tr!("repo.history.title")),
            )
            .children(rows)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_known_unix_timestamps() {
        assert_eq!(format_commit_time(0), "1970-01-01 00:00");
        // 2026-09-15T00:26:27Z，出现在本任务的一次真实运行日志里
        assert_eq!(format_commit_time(1_789_431_987), "2026-09-15 00:26");
        assert_eq!(format_commit_time(946_684_800), "2000-01-01 00:00");
    }
}
