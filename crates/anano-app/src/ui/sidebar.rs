//! 左侧栏 = 固定图标栏 + 可展开的面板：仓库切换器（GitHub Desktop 风格的下拉，替代原来
//! 带完整路径的仓库列表）→ 分支 / 同步（常驻）→ Changes / History 两个标签。
//!
//! Changes 标签是原来的改动列表 + 提交框；History 标签是从 `ui::status_pane` 挪过来的
//! 提交历史渲染——用户反馈这些都该在侧栏跟着仓库条目直接看到，主内容区腾出来给以后的
//! diff 视图（这次不做）。

use anano_core::git::{Branch, ChangeKind, FileStatus, LfsSync};
use gpui_kit::component::{
    ActiveTheme, Disableable, Icon, IconName, Sizable,
    button::{Button, ButtonVariants},
    h_flex,
    input::Input,
    menu::{ContextMenuExt, DropdownMenu, PopupMenuItem},
    popover::Popover,
    tooltip::Tooltip,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use anano_core::model::ThemePref;

use crate::ToggleSidebar;
use crate::assets::{ICON_FOLDER_GIT, ICON_HISTORY, logo_path};
use crate::brand::APP_NAME;
use crate::i18n::tr;
use crate::state::repos::SyncKind;
use crate::state::workspace::{SidebarTab, Workspace};
use crate::ui::text::theme_label;

fn theme_icon(pref: ThemePref) -> IconName {
    match pref {
        ThemePref::System => IconName::Palette,
        ThemePref::Light => IconName::Sun,
        ThemePref::Dark => IconName::Moon,
    }
}

fn kind_label(kind: ChangeKind) -> SharedString {
    match kind {
        ChangeKind::New => tr!("repo.status.new"),
        ChangeKind::Modified => tr!("repo.status.modified"),
        ChangeKind::Deleted => tr!("repo.status.deleted"),
        ChangeKind::Renamed => tr!("repo.status.renamed"),
        ChangeKind::TypeChange => tr!("repo.status.type_change"),
        ChangeKind::Conflicted => tr!("repo.status.conflicted"),
    }
}

fn kind_color(kind: ChangeKind, cx: &App) -> Hsla {
    match kind {
        ChangeKind::New => cx.theme().success,
        ChangeKind::Deleted | ChangeKind::Conflicted => cx.theme().danger,
        ChangeKind::Modified | ChangeKind::Renamed | ChangeKind::TypeChange => cx.theme().warning,
    }
}

/// Unix 秒 → `YYYY-MM-DD HH:MM`；从 `ui::status_pane` 原样搬过来（历史列表挪到这里）。
fn format_commit_time(seconds: i64) -> String {
    const DAY: i64 = 86_400;
    let days = seconds.div_euclid(DAY);
    let secs_of_day = seconds.rem_euclid(DAY);
    let (h, m) = (secs_of_day / 3600, (secs_of_day % 3600) / 60);

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
    /// 固定图标栏。
    pub fn render_sidebar_rail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme();
        v_flex()
            .id("sidebar-rail")
            .role(Role::Group)
            .aria_label(tr!("sidebar.rail_aria"))
            .w_12()
            .h_full()
            .flex_none()
            .items_center()
            .py_2()
            .gap_1()
            .bg(cx.theme().sidebar)
            .border_r_1()
            .border_color(cx.theme().sidebar_border)
            .child(
                div()
                    .id("logo")
                    .size_9()
                    .flex()
                    .items_center()
                    .justify_center()
                    .tooltip(|window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(APP_NAME).build(window, cx)
                    })
                    .child(img(logo_path(cx)).size(px(26.)).flex_none()),
            )
            .child(div().h(px(1.)).w_6().my_1().bg(cx.theme().sidebar_border))
            .when(self.sidebar_collapsed(), |rail| {
                // 侧栏收起后，展开面板里的「收起」按钮跟着一起被藏起来了——之前没有
                // 任何鼠标可点的地方能再展开它，只能靠不好发现的快捷键。这里补一个
                // 常驻在窄栏里的「展开」按钮，跟收起按钮用同一个图标反过来（PanelLeftOpen）。
                rail.child(
                    Button::new("rail-expand-sidebar")
                        .ghost()
                        .icon(Icon::new(IconName::PanelLeftOpen).size_4())
                        .tooltip_with_action(tr!("sidebar.expand"), &ToggleSidebar, None)
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_sidebar(cx))),
                )
            })
            .child(
                Button::new("rail-open-repo")
                    .ghost()
                    .icon(Icon::empty().path(ICON_FOLDER_GIT).size_4())
                    .tooltip(tr!("repo.open"))
                    .on_click(cx.listener(|this, _, window, cx| this.open_repo_dialog(window, cx))),
            )
            .child(div().flex_1())
            .child(
                Button::new("rail-theme")
                    .ghost()
                    .icon(Icon::new(theme_icon(theme)).size_4())
                    .tooltip(tr!("sidebar.theme_tooltip", theme = theme_label(theme)))
                    .on_click(cx.listener(|this, _, window, cx| this.cycle_theme(window, cx))),
            )
            .child(
                Button::new("rail-settings")
                    .ghost()
                    .icon(Icon::new(IconName::Settings).size_4())
                    .tooltip_with_action(tr!("sidebar.settings"), &crate::OpenSettings, None)
                    .on_click(cx.listener(|this, _, window, cx| this.open_settings(window, cx))),
            )
    }

    /// 展开的功能面板：仓库切换器 + 分支/同步（常驻）+ Changes/History 标签。
    pub fn render_sidebar(&self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("sidebar-panel")
            .role(Role::Group)
            .aria_label(tr!("repo.sidebar_title"))
            .debug_selector(|| "sidebar-panel".into())
            .size_full()
            .bg(cx.theme().sidebar)
            .border_r_1()
            .border_color(cx.theme().sidebar_border)
            .child(
                h_flex()
                    .h_8()
                    .flex_none()
                    .justify_end()
                    .pr_2()
                    .items_center()
                    .child(
                        Button::new("collapse-sidebar")
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(IconName::PanelLeftClose).size_4())
                            .tooltip_with_action(tr!("sidebar.collapse"), &ToggleSidebar, None)
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_sidebar(cx))),
                    ),
            )
            .child(self.render_repo_switcher(cx))
            .when(self.active_repo().is_some(), |d| {
                d.child(self.render_branch_section(cx))
                    .child(self.render_sync_toolbar(cx))
                    .child(self.render_sidebar_tabs(cx))
            })
    }

    /// GitHub Desktop 风格的仓库切换器：当前仓库名 + 分支，点开一个锚定下拉
    /// （`gpui_kit::component::popover::Popover`，这个依赖里已有的组件，不用再手搓一个
    /// 覆盖层）。下拉里是过滤输入框、打开的仓库列表（只显示名字，路径改放 tooltip），
    /// 以及一个「Add」下拉（Clone / Create New / Add Existing）。
    fn render_repo_switcher(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        if self.repo_count() == 0 {
            return v_flex()
                .flex_none()
                .items_center()
                .justify_center()
                .gap_2()
                .px_4()
                .py_6()
                .text_sm()
                .text_center()
                .text_color(muted)
                .child(tr!("repo.none_open_title"))
                .child(div().text_xs().child(tr!("repo.none_open_hint")))
                .child(self.render_add_repo_menu(cx))
                .into_any_element();
        }

        let repo = self.active_repo().expect("repo_count > 0");
        let name = SharedString::from(repo.entry.display_name.clone());
        let branch_label = repo
            .current_branch()
            .map(|b| SharedString::from(b.name.clone()))
            .unwrap_or_else(|| tr!("repo.switcher.no_branch"));
        let workspace = cx.entity();
        let search_input = self.repo_search_input.clone();
        let active_ix = self.active_index();
        let repos_meta = self.repos_meta(cx);
        let hover_bg = cx.theme().list_hover;
        let active_bg = cx.theme().list_active;
        let radius = cx.theme().radius;

        div()
            .id("repo-switcher")
            .flex_none()
            .px_2()
            .pt_1()
            .pb_2()
            .child(
                Popover::new("repo-switcher-popover")
                    .trigger_style(gpui::StyleRefinement::default())
                    .trigger(
                        Button::new("repo-switcher-trigger")
                            .ghost()
                            .w_full()
                            .justify_between()
                            .tooltip(tr!("repo.switcher.open_aria"))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .truncate()
                                            .child(name.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(muted)
                                            .truncate()
                                            .child(branch_label),
                                    ),
                            )
                            .child(Icon::new(IconName::ChevronDown).size_4()),
                    )
                    .content(move |_, _, cx| {
                        let workspace = workspace.clone();
                        let query = search_input.read(cx).value().to_lowercase();
                        let rows: Vec<AnyElement> = repos_meta
                            .iter()
                            .enumerate()
                            .filter(|(_, (n, _))| {
                                query.is_empty() || n.to_lowercase().contains(&query)
                            })
                            .map(|(ix, (n, p))| {
                                let selected = ix == active_ix;
                                let workspace_row = workspace.clone();
                                let workspace_close = workspace.clone();
                                let path_tooltip = p.clone();
                                h_flex()
                                    .id(("switcher-row", ix))
                                    .gap_2()
                                    .items_center()
                                    .px_2()
                                    .py_1()
                                    .rounded(radius)
                                    .when(selected, |row| row.bg(active_bg))
                                    .hover(|style| style.bg(hover_bg))
                                    .aria_selected(selected)
                                    .tooltip(move |window, cx| {
                                        Tooltip::new(path_tooltip.clone()).build(window, cx)
                                    })
                                    .on_click(move |_, _, cx| {
                                        workspace_row
                                            .update(cx, |ws, cx| ws.activate_from_switcher(ix, cx));
                                    })
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .text_sm()
                                            .truncate()
                                            .child(n.clone()),
                                    )
                                    .child(
                                        Button::new(("switcher-close", ix))
                                            .ghost()
                                            .xsmall()
                                            .icon(IconName::Close)
                                            .tooltip(tr!("repo.close"))
                                            .on_click(move |_, window, cx| {
                                                cx.stop_propagation();
                                                workspace_close.update(cx, |ws, cx| {
                                                    ws.close_repo(ix, window, cx)
                                                });
                                            }),
                                    )
                                    .into_any_element()
                            })
                            .collect();

                        v_flex()
                            .w(px(280.))
                            .p_2()
                            .gap_2()
                            .child(
                                div()
                                    .rounded(radius)
                                    .border_1()
                                    .child(Input::new(&search_input)),
                            )
                            .child(
                                v_flex()
                                    .id("switcher-recent")
                                    .gap_0p5()
                                    .max_h(px(240.))
                                    .overflow_y_scroll()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(muted)
                                            .child(tr!("repo.switcher.recent")),
                                    )
                                    .children(rows),
                            )
                            .child(div().h(px(1.)).bg(hover_bg))
                            .child(render_add_repo_dropdown(workspace.clone()))
                    }),
            )
            .into_any_element()
    }

    /// 打开任何仓库前（切换器为空态）用的「Add」按钮，跟切换器下拉里的那个共用同一个
    /// 下拉菜单构建函数。
    fn render_add_repo_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        render_add_repo_dropdown(cx.entity()).into_any_element()
    }

    /// 当前分支 + 切换 / 新建分支。放在改动列表上面：分支是「在哪改」，
    /// 改动是「改了什么」，先后顺序跟着看仓库时的自然思路走。
    fn render_branch_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(repo) = self.active_repo() else {
            return div().into_any_element();
        };
        let current = repo.current_branch();
        let branches: Vec<Branch> = repo
            .branches
            .iter()
            .filter(|b| !b.is_remote)
            .cloned()
            .collect();
        let branch_label = current
            .map(|b| SharedString::from(b.name.clone()))
            .unwrap_or_else(|| tr!("repo.branch.detached"));
        let ahead_behind = current.and_then(|b| b.ahead_behind);
        let workspace = cx.entity();

        v_flex()
            .id("branch-section")
            .flex_none()
            .gap_1p5()
            .px_2()
            .py_2()
            .border_t_1()
            .border_color(cx.theme().sidebar_border)
            .child(
                h_flex()
                    .gap_1p5()
                    .items_center()
                    .child(Icon::empty().path(crate::assets::ICON_GIT_BRANCH).size_4())
                    .child(
                        Button::new("branch-switcher")
                            .ghost()
                            .xsmall()
                            .flex_1()
                            .label(branch_label.clone())
                            .tooltip(tr!("repo.branch.switch", name = branch_label))
                            .dropdown_menu(move |menu, _, _cx| {
                                let workspace = workspace.clone();
                                branches.iter().fold(menu, |menu, branch| {
                                    let name = branch.name.clone();
                                    let is_head = branch.is_head;
                                    let workspace = workspace.clone();
                                    let menu = menu.item(
                                        PopupMenuItem::new(name.clone()).checked(is_head).on_click(
                                            {
                                                let name = name.clone();
                                                let workspace = workspace.clone();
                                                move |_, _, cx| {
                                                    if !is_head {
                                                        workspace.update(cx, |ws, cx| {
                                                            ws.switch_branch(name.clone(), cx)
                                                        });
                                                    }
                                                }
                                            },
                                        ),
                                    );
                                    if is_head {
                                        menu
                                    } else {
                                        menu.item(
                                            PopupMenuItem::new(tr!(
                                                "repo.branch.delete",
                                                name = name.clone()
                                            ))
                                            .icon(Icon::empty().path(crate::assets::ICON_TRASH))
                                            .on_click(move |_, _, cx| {
                                                workspace.update(cx, |ws, cx| {
                                                    ws.delete_branch(name.clone(), cx)
                                                });
                                            }),
                                        )
                                    }
                                })
                            }),
                    )
                    .when_some(ahead_behind, |d, (ahead, behind)| {
                        d.child(
                            div()
                                .flex_none()
                                .text_xs()
                                .font_family(cx.theme().mono_font_family.clone())
                                .text_color(cx.theme().muted_foreground)
                                .child(tr!(
                                    "repo.branch.ahead_behind",
                                    ahead = ahead.to_string(),
                                    behind = behind.to_string()
                                )),
                        )
                    })
                    .child(
                        Button::new("new-branch")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Plus)
                            .tooltip(tr!("repo.branch.new"))
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_new_branch(cx))),
                    ),
            )
            .when(self.new_branch_open, |d| {
                d.child(self.render_new_branch_row(cx))
            })
            .into_any_element()
    }

    fn render_new_branch_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let has_name = !self.new_branch_input.read(cx).value().trim().is_empty();
        h_flex()
            .id("new-branch-row")
            .gap_1p5()
            .items_center()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .rounded(cx.theme().radius)
                    .border_1()
                    .border_color(cx.theme().border)
                    .child(
                        Input::new(&self.new_branch_input)
                            .aria_label(tr!("repo.branch.new_placeholder")),
                    ),
            )
            .child(
                Button::new("new-branch-create")
                    .primary()
                    .xsmall()
                    .disabled(!has_name)
                    .label(tr!("repo.branch.create"))
                    .on_click(
                        cx.listener(|this, _, window, cx| this.submit_new_branch(window, cx)),
                    ),
            )
            .child(
                Button::new("new-branch-cancel")
                    .ghost()
                    .xsmall()
                    .label(tr!("repo.branch.cancel"))
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_new_branch(cx))),
            )
    }

    /// Fetch / Pull / Push：三个独立按钮，不并进提交框——commit 是本地操作，
    /// 这三个是跟远程打交道，混在一起会让人搞不清「提交」到底有没有联网。
    fn render_sync_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(repo) = self.active_repo() else {
            return div().into_any_element();
        };
        let syncing = repo.syncing;
        let action_error = repo.action_error.clone();
        let lfs_status = repo.lfs_status;

        v_flex()
            .id("sync-toolbar")
            .flex_none()
            .gap_1()
            .px_2()
            .pb_2()
            .child(
                h_flex()
                    .gap_1p5()
                    .child(
                        Button::new("sync-fetch")
                            .ghost()
                            .xsmall()
                            .icon(Icon::empty().path(crate::assets::ICON_DOWNLOAD))
                            .label(tr!("repo.sync.fetch"))
                            .loading(syncing == Some(SyncKind::Fetch))
                            .disabled(syncing.is_some())
                            .on_click(cx.listener(|this, _, _, cx| this.fetch(cx))),
                    )
                    .child(
                        Button::new("sync-pull")
                            .ghost()
                            .xsmall()
                            .icon(Icon::empty().path(crate::assets::ICON_DOWNLOAD_CLOUD))
                            .label(tr!("repo.sync.pull"))
                            .loading(syncing == Some(SyncKind::Pull))
                            .disabled(syncing.is_some())
                            .on_click(cx.listener(|this, _, _, cx| this.pull(cx))),
                    )
                    .child(
                        Button::new("sync-push")
                            .ghost()
                            .xsmall()
                            .icon(Icon::empty().path(crate::assets::ICON_UPLOAD_CLOUD))
                            .label(tr!("repo.sync.push"))
                            .loading(syncing == Some(SyncKind::Push))
                            .disabled(syncing.is_some())
                            .on_click(cx.listener(|this, _, _, cx| this.push(cx))),
                    ),
            )
            .when_some(action_error, |d, err| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().danger)
                        .child(tr!("repo.sync.error", error = err)),
                )
            })
            .when(lfs_status == Some(LfsSync::NotInstalled), |d| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().warning)
                        .child(tr!("repo.sync.lfs_not_installed")),
                )
            })
            .into_any_element()
    }

    /// Changes / History 两个标签的头 + 对应内容。
    fn render_sidebar_tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(repo) = self.active_repo() else {
            return div().into_any_element();
        };
        let tab = self.sidebar_tab();
        let changes_count = repo.statuses.len();
        let changes_label = if changes_count > 0 {
            tr!(
                "repo.tabs.changes_with_count",
                count = changes_count.to_string()
            )
        } else {
            tr!("repo.tabs.changes")
        };
        let border = cx.theme().sidebar_border;
        let active_border = cx.theme().primary;
        let muted = cx.theme().muted_foreground;
        let fg = cx.theme().sidebar_foreground;

        v_flex()
            .id("sidebar-tabs")
            .flex_1()
            .min_h_0()
            .child(
                h_flex()
                    .flex_none()
                    .h_9()
                    .border_t_1()
                    .border_color(border)
                    .child(
                        Button::new("tab-changes")
                            .ghost()
                            .compact()
                            .flex_1()
                            .label(changes_label)
                            .text_color(if tab == SidebarTab::Changes {
                                fg
                            } else {
                                muted
                            })
                            .when(tab == SidebarTab::Changes, |b| {
                                b.border_b_2().border_color(active_border)
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_sidebar_tab(SidebarTab::Changes, cx)
                            })),
                    )
                    .child(
                        Button::new("tab-history")
                            .ghost()
                            .compact()
                            .flex_1()
                            .icon(Icon::empty().path(ICON_HISTORY).size_4())
                            .label(tr!("repo.tabs.history"))
                            .text_color(if tab == SidebarTab::History {
                                fg
                            } else {
                                muted
                            })
                            .when(tab == SidebarTab::History, |b| {
                                b.border_b_2().border_color(active_border)
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_sidebar_tab(SidebarTab::History, cx)
                            })),
                    ),
            )
            .child(match tab {
                SidebarTab::Changes => v_flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_changes_section(cx))
                    .child(self.render_commit_box(cx))
                    .into_any_element(),
                SidebarTab::History => self.render_history_tab(cx),
            })
            .into_any_element()
    }

    /// History 标签：最近提交历史，从 `ui::status_pane` 挪过来的原逻辑，纯展示。
    fn render_history_tab(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(repo) = self.active_repo() else {
            return div().into_any_element();
        };
        let muted = cx.theme().muted_foreground;

        if repo.commits.is_empty() {
            if repo.log_loading {
                return div().flex_1().into_any_element();
            }
            return v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(muted)
                .child(tr!("repo.history.empty"))
                .into_any_element();
        }

        let rows: Vec<AnyElement> = repo
            .commits
            .iter()
            .map(|commit| {
                h_flex()
                    .h_7()
                    .px_2()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .w(px(56.))
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
            .id("history-list")
            .flex_1()
            .min_h_0()
            .py_1()
            .overflow_y_scroll()
            .children(rows)
            .into_any_element()
    }

    /// 当前激活仓库的改动：已 stage / 未 stage 两组，每行一个切换暂存的按钮 + 右键菜单。
    fn render_changes_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(repo) = self.active_repo() else {
            return div().into_any_element();
        };
        let muted = cx.theme().muted_foreground;
        let staged: Vec<FileStatus> = repo.staged().cloned().collect();
        let unstaged: Vec<FileStatus> = repo.unstaged().cloned().collect();
        let clean =
            staged.is_empty() && unstaged.is_empty() && !repo.loading && repo.error.is_none();

        v_flex()
            .id("changes")
            .flex_1()
            .min_h_0()
            .gap_3()
            .px_2()
            .py_2()
            .overflow_y_scroll()
            .child(self.render_file_list(tr!("repo.status.staged_title"), staged, true, cx))
            .child(self.render_file_list(tr!("repo.status.unstaged_title"), unstaged, false, cx))
            .when(clean, |d| {
                d.child(
                    div()
                        .text_sm()
                        .text_color(muted)
                        .child(tr!("repo.status.clean")),
                )
            })
            .into_any_element()
    }

    /// 一组文件（已 stage 或未 stage），每行带一个切换暂存状态的按钮 + 右键菜单
    /// （Discard Changes / Ignore File / Ignore All *.ext / Copy Path / Copy Relative Path）。
    fn render_file_list(
        &self,
        title: SharedString,
        entries: Vec<FileStatus>,
        staged: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if entries.is_empty() {
            return div().into_any_element();
        }
        let workspace = cx.entity();
        let rows: Vec<AnyElement> = entries
            .into_iter()
            .enumerate()
            .map(|(ix, status)| {
                let file = status.path.clone();
                let kind = status.kind;
                let menu_workspace = workspace.clone();
                let menu_file = file.clone();
                h_flex()
                    .h_7()
                    .gap_2()
                    .items_center()
                    .context_menu(move |menu, _, _| {
                        let workspace = menu_workspace.clone();
                        let file = menu_file.clone();
                        let ext = std::path::Path::new(&file)
                            .extension()
                            .and_then(|e| e.to_str())
                            .map(str::to_string);
                        let menu = menu
                            .item(
                                PopupMenuItem::new(tr!("repo.context_menu.discard")).on_click({
                                    let workspace = workspace.clone();
                                    let file = file.clone();
                                    move |_, _, cx| {
                                        workspace.update(cx, |ws, cx| {
                                            ws.discard_file_change(file.clone(), kind, cx)
                                        });
                                    }
                                }),
                            )
                            .item(
                                PopupMenuItem::new(tr!("repo.context_menu.ignore_file")).on_click(
                                    {
                                        let workspace = workspace.clone();
                                        let file = file.clone();
                                        move |_, _, cx| {
                                            workspace.update(cx, |ws, cx| {
                                                ws.ignore_file(file.clone(), cx)
                                            });
                                        }
                                    },
                                ),
                            );
                        let menu = if let Some(ext) = ext {
                            menu.item(
                                PopupMenuItem::new(tr!("repo.context_menu.ignore_ext", ext = ext))
                                    .on_click({
                                        let workspace = workspace.clone();
                                        let file = file.clone();
                                        move |_, _, cx| {
                                            workspace.update(cx, |ws, cx| {
                                                ws.ignore_file_extension(file.clone(), cx)
                                            });
                                        }
                                    }),
                            )
                        } else {
                            menu
                        };
                        menu.separator()
                            .item(
                                PopupMenuItem::new(tr!("repo.context_menu.copy_path")).on_click({
                                    let workspace = workspace.clone();
                                    let file = file.clone();
                                    move |_, _, cx| {
                                        workspace.update(cx, |ws, cx| ws.copy_file_path(&file, cx));
                                    }
                                }),
                            )
                            .item(
                                PopupMenuItem::new(tr!("repo.context_menu.copy_relative_path"))
                                    .on_click({
                                        let workspace = workspace.clone();
                                        let file = file.clone();
                                        move |_, _, cx| {
                                            workspace.update(cx, |ws, cx| {
                                                ws.copy_relative_file_path(&file, cx)
                                            });
                                        }
                                    }),
                            )
                    })
                    .child(
                        div()
                            .w(px(72.))
                            .flex_none()
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(kind_color(status.kind, cx))
                            .child(kind_label(status.kind)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .truncate()
                            .child(status.path.clone()),
                    )
                    .child(
                        Button::new(("toggle-stage", staged as usize * 10_000 + ix))
                            .ghost()
                            .xsmall()
                            .icon(if staged {
                                IconName::Minus
                            } else {
                                IconName::Plus
                            })
                            .tooltip(if staged {
                                tr!("repo.status.unstage")
                            } else {
                                tr!("repo.status.stage")
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.toggle_stage(file.clone(), staged, cx)
                            })),
                    )
                    .into_any_element()
            })
            .collect();
        v_flex()
            .gap_1()
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(cx.theme().muted_foreground)
                    .child(title),
            )
            .children(rows)
            .into_any_element()
    }

    /// 侧栏底部常驻的提交区：一行输入框 + 一个「提交」按钮。点「提交」时若索引里还
    /// 没有任何暂存内容，先把当前显示的全部改动暂存一遍再提交（`bridge::commit`
    /// 内部串联 stage-all → commit）；已经手动挑过 stage/unstage 的文件则原样尊重，
    /// 不会被覆盖。设置里开了「提交后自动 push」时，`commit_active` 会在提交成功后
    /// 自己接一次 push，这里不用管。
    fn render_commit_box(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let has_changes = self.active_repo().is_some_and(|r| !r.statuses.is_empty());
        let has_message = !self.commit_input.read(cx).value().trim().is_empty();
        v_flex()
            .id("commit-box")
            .flex_none()
            .gap_1p5()
            .p_2()
            .border_t_1()
            .border_color(cx.theme().sidebar_border)
            .child(
                div()
                    .id("commit-message")
                    .rounded(cx.theme().radius)
                    .border_1()
                    .border_color(cx.theme().border)
                    .child(
                        Input::new(&self.commit_input).aria_label(tr!("repo.commit.placeholder")),
                    ),
            )
            .child(
                Button::new("commit")
                    .primary()
                    .small()
                    .w_full()
                    .disabled(!has_changes || !has_message)
                    .label(tr!("repo.commit.commit"))
                    .on_click(cx.listener(|this, _, window, cx| this.commit_active(window, cx))),
            )
    }
}

/// 「Add」下拉：Clone / Create New / Add Existing，三个都不接 GitHub OAuth
/// （纯 git URL / 本地路径），跟切换器空态、切换器下拉共用同一个构建函数。
fn render_add_repo_dropdown(workspace: Entity<Workspace>) -> impl IntoElement {
    Button::new("repo-add")
        .outline()
        .small()
        .w_full()
        .icon(IconName::Plus)
        .label(tr!("repo.switcher.add"))
        .dropdown_menu(move |menu, _, _| {
            let clone_ws = workspace.clone();
            let create_ws = workspace.clone();
            let existing_ws = workspace.clone();
            menu.item(PopupMenuItem::new(tr!("repo.switcher.add_clone")).on_click(
                move |_, window, cx| {
                    clone_ws.update(cx, |ws, cx| ws.open_clone_dialog(window, cx));
                },
            ))
            .item(
                PopupMenuItem::new(tr!("repo.switcher.add_create")).on_click(
                    move |_, window, cx| {
                        create_ws.update(cx, |ws, cx| ws.open_create_repo_dialog(window, cx));
                    },
                ),
            )
            .item(
                PopupMenuItem::new(tr!("repo.switcher.add_existing")).on_click(
                    move |_, window, cx| {
                        existing_ws.update(cx, |ws, cx| ws.add_existing_repo(window, cx));
                    },
                ),
            )
        })
}
