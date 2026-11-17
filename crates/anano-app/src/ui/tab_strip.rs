//! 仓库标签栏：每个打开的仓库一个标签，单行横向滚动。
//!
//! Phase 1 有意不做 HTTP 版本里的多行分页——一次同时开十几个仓库并不常见，
//! 装不下时靠系统的横向滚动（触控板 / 滚轮）就够了，犯不上再搭一套分页算法。

use gpui_kit::component::{
    ActiveTheme, Icon, IconName, Sizable,
    button::ButtonVariants,
    h_flex,
    menu::{ContextMenuExt, PopupMenu, PopupMenuItem},
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, ClickEvent, ClipboardItem, Context, Entity, FontWeight, InteractiveElement,
    IntoElement, MouseButton, ParentElement, Role, SharedString, StatefulInteractiveElement,
    Styled, div, px,
};

use crate::assets::ICON_FOLDER_GIT;
use crate::i18n::tr;
use crate::state::workspace::Workspace;

const TAB_HEIGHT: f32 = 30.;
const TAB_WIDTH: f32 = 200.;

/// The right-click menu of a repository, shared by its tab and its row in the repository
/// list. Nothing destructive is a visible button anywhere: closing, removing and the rest
/// live here, and removing asks first.
pub(crate) fn repo_menu(
    menu: PopupMenu,
    workspace: &Entity<Workspace>,
    ix: usize,
    path: SharedString,
    is_tab: bool,
    nermal: bool,
) -> PopupMenu {
    let item =
        |label: SharedString,
         ws: &Entity<Workspace>,
         act: fn(&mut Workspace, usize, &mut gpui_kit::Window, &mut Context<Workspace>)| {
            let ws = ws.clone();
            PopupMenuItem::new(label).on_click(move |_, window, cx| {
                let ws = ws.clone();
                // Next frame: the menu has closed by then and can't take what opens with it.
                window.defer(cx, move |window, cx| {
                    ws.update(cx, |ws, cx| act(ws, ix, window, cx));
                });
            })
        };
    let mut menu = menu;
    if is_tab {
        menu = menu
            .item(item(
                tr!("repo.tab_menu.close"),
                workspace,
                |ws, ix, _, cx| ws.close_tab(ix, cx),
            ))
            .item(item(
                tr!("repo.tab_menu.close_others"),
                workspace,
                |ws, ix, _, cx| ws.close_other_tabs(ix, cx),
            ))
            .separator();
    }
    menu = menu
        .item(item(
            tr!("repo.tab_menu.remove"),
            workspace,
            |ws, ix, window, cx| ws.confirm_remove_repo(ix, window, cx),
        ))
        .separator()
        .item(
            PopupMenuItem::new(if cfg!(target_os = "macos") {
                tr!("settings.reveal_finder")
            } else {
                tr!("settings.reveal_folder")
            })
            .on_click({
                let path = path.clone();
                move |_, _, cx| cx.reveal_path(std::path::Path::new(path.as_ref()))
            }),
        )
        .item(
            PopupMenuItem::new(tr!("repo.tab_menu.copy_path")).on_click(move |_, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(path.to_string()))
            }),
        );
    if nermal {
        menu = menu.item(item(
            tr!("repo.open_in_nermal"),
            workspace,
            |ws, ix, _, cx| ws.open_repo_in_nermal(ix, cx),
        ));
    }
    menu
}

impl Workspace {
    pub fn render_tab_strip(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.active_index();
        let tabs: Vec<(SharedString, SharedString)> = self.repos_meta(cx);

        let mut children = Vec::with_capacity(tabs.len());
        for (ix, (name, path)) in tabs.into_iter().enumerate() {
            // A repository whose tab was closed is still in the app, just not up here.
            if !self.is_tab(ix) {
                continue;
            }
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
            // The sidebar's own collapse button disappears with it; this is the way back.
            .when(self.sidebar_collapsed(), |strip| {
                strip.child(
                    div().flex_none().pl_1().py_1().child(
                        crate::ui::button("expand-sidebar")
                            .ghost()
                            .small()
                            .icon(Icon::new(IconName::PanelLeftOpen).size_4())
                            .tooltip_with_action(tr!("sidebar.expand"), &crate::ToggleSidebar, None)
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_sidebar(cx))),
                    ),
                )
            })
            .child(
                div().flex_none().px_1().py_1().child(
                    crate::ui::button("open-repo")
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
        let path_for_menu = path.clone();
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
            // Middle-click closes a tab, as in a browser.
            .on_mouse_up(
                MouseButton::Middle,
                cx.listener(move |this, _, _, cx| this.close_tab(ix, cx)),
            )
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.activate(ix, cx)))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _, _, cx| this.activate(ix, cx)),
            )
            .context_menu({
                let (workspace, nermal) = (cx.entity(), self.nermal_available);
                let path_for_menu = path_for_menu.clone();
                move |menu, _, _| {
                    repo_menu(menu, &workspace, ix, path_for_menu.clone(), true, nermal)
                }
            })
            .into_any_element()
    }
}
