//! 仓库标签栏：每个打开的仓库一个标签，单行横向滚动。
//!
//! Phase 1 有意不做 HTTP 版本里的多行分页——一次同时开十几个仓库并不常见，
//! 装不下时靠系统的横向滚动（触控板 / 滚轮）就够了，犯不上再搭一套分页算法。

use gpui_kit::component::{
    ActiveTheme, Icon, IconName, Sizable,
    button::{Button, ButtonVariants},
    h_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, ClickEvent, Context, FontWeight, InteractiveElement, IntoElement, MouseButton,
    ParentElement, Role, SharedString, StatefulInteractiveElement, Styled, div, px,
};

use crate::assets::ICON_FOLDER_GIT;
use crate::i18n::tr;
use crate::state::workspace::Workspace;

const TAB_HEIGHT: f32 = 30.;
const TAB_WIDTH: f32 = 200.;

impl Workspace {
    pub fn render_tab_strip(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.active_index();
        let tabs: Vec<(SharedString, SharedString)> = self.repos_meta(cx);

        let mut children = Vec::with_capacity(tabs.len());
        for (ix, (name, path)) in tabs.into_iter().enumerate() {
            children.push(self.render_tab(ix, name, path, ix == active, cx));
        }

        h_flex()
            .id("tab-strip")
            .role(Role::TabList)
            .aria_label(tr!("tab.strip_aria"))
            .w_full()
            .flex_none()
            .bg(cx.theme().tokens.tab_bar)
            .border_b_1()
            .border_color(cx.theme().border)
            .items_start()
            .child(
                div().flex_none().px_1().py_1().child(
                    Button::new("open-repo")
                        .outline()
                        .small()
                        .icon(Icon::empty().path(ICON_FOLDER_GIT))
                        .tooltip(tr!("repo.open"))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.open_repo_dialog(window, cx)),
                        ),
                ),
            )
            .child(
                h_flex()
                    .id("tab-row")
                    .flex_1()
                    .min_w_0()
                    .overflow_x_scroll()
                    .track_scroll(self.tab_scroll())
                    .children(children),
            )
    }

    fn render_tab(
        &self,
        ix: usize,
        name: SharedString,
        path: SharedString,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let active_bg = *cx.theme().tokens.tab_active;
        div()
            .id(("tab", ix))
            .role(Role::Tab)
            .aria_label(tr!("tab.aria", title = name.clone()))
            .aria_selected(selected)
            .relative()
            .flex()
            .items_center()
            .gap_1()
            .flex_none()
            .h(px(TAB_HEIGHT))
            .px_2()
            .max_w(px(TAB_WIDTH))
            .border_l_1()
            .border_r_1()
            .map(|d| {
                if selected {
                    d.bg(active_bg)
                        .border_color(cx.theme().border)
                        .text_color(cx.theme().tab_active_foreground)
                } else {
                    d.border_color(cx.theme().transparent)
                        .text_color(cx.theme().tab_foreground)
                        .hover(|s| s.bg(active_bg.opacity(0.5)))
                }
            })
            .tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(path.clone()).build(window, cx)
            })
            .child(div().flex_1().min_w_0().text_sm().truncate().child(name))
            .child(
                Button::new(("close-tab", ix))
                    .ghost()
                    .xsmall()
                    .icon(IconName::Close)
                    .tooltip(tr!("tab.close"))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.close_repo(ix, window, cx)
                    })),
            )
            .when(selected, |d| {
                d.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .h(px(2.))
                        .bg(cx.theme().primary),
                )
            })
            .font_weight(FontWeight::MEDIUM)
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.activate(ix, cx)))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _, _, cx| this.activate(ix, cx)),
            )
            .into_any_element()
    }
}
