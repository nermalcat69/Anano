//! 左侧栏 = 固定图标栏 + 可展开的面板（仓库列表 + 当前仓库的改动 + 提交框）。
//!
//! 改动列表原来画在主内容区（见 `ui::status_pane`），用户反馈这信息应该跟着仓库
//! 条目直接看到，不该单独占一块主内容区——现在挪到这里，紧跟在仓库列表下面，
//! 提交框常驻最底部。

use anano_core::git::{Branch, ChangeKind, FileStatus, LfsSync};
use gpui_kit::component::{
    ActiveTheme, Disableable, Icon, IconName, Sizable,
    button::{Button, ButtonVariants},
    h_flex,
    input::Input,
    menu::{DropdownMenu, PopupMenuItem},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use anano_core::model::ThemePref;

use crate::ToggleSidebar;
use crate::assets::{ICON_FOLDER_GIT, logo_path};
use crate::brand::APP_NAME;
use crate::i18n::tr;
use crate::state::repos::SyncKind;
use crate::state::workspace::Workspace;
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

    /// 展开的功能面板：标题行 + 仓库列表 + 当前仓库的改动 + 提交框。
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
                    .h_10()
                    .flex_none()
                    .pl_4()
                    .pr_2()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(cx.theme().sidebar_foreground)
                            .truncate()
                            .child(tr!("repo.sidebar_title")),
                    )
                    .child(
                        Button::new("collapse-sidebar")
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(IconName::PanelLeftClose).size_4())
                            .tooltip_with_action(tr!("sidebar.collapse"), &ToggleSidebar, None)
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_sidebar(cx))),
                    ),
            )
            .child(self.render_repo_list(cx))
            .when(self.active_repo().is_some(), |d| {
                d.child(self.render_branch_section(cx))
                    .child(self.render_sync_toolbar(cx))
                    .child(self.render_changes_section(cx))
                    .child(self.render_commit_box(cx))
            })
    }

    /// 当前分支 + 切换 / 新建分支。放在改动列表上面：分支是「在哪改」，
    /// 改动是「改了什么」，先后顺序跟着看仓库时的自然思路走。
    fn render_branch_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(repo) = self.active_repo() else {
            return div().into_any_element();
        };
        let current = repo.current_branch();
        let branches: Vec<Branch> = repo.branches.iter().filter(|b| !b.is_remote).cloned().collect();
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
                    .child(Icon::new(IconName::GitBranch).size_4())
                    .child(
                        Button::new("branch-switcher")
                            .ghost()
                            .xsmall()
                            .flex_1()
                            .label(branch_label.clone())
                            .tooltip(tr!("repo.branch.switch", name = branch_label))
                            .dropdown_menu(move |menu, _, cx| {
                                branches.iter().fold(menu, |menu, branch| {
                                    let name = branch.name.clone();
                                    let is_head = branch.is_head;
                                    menu.item(
                                        PopupMenuItem::new(name.clone())
                                            .checked(is_head)
                                            .on_click(cx.listener_for(
                                                &cx.entity(),
                                                move |ws: &mut Workspace, _, _, cx| {
                                                    if !is_head {
                                                        ws.switch_branch(name.clone(), cx);
                                                    }
                                                },
                                            )),
                                    )
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
                            .icon(IconName::GitBranchPlus)
                            .tooltip(tr!("repo.branch.new"))
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_new_branch(cx))),
                    ),
            )
            .when(self.new_branch_open, |d| d.child(self.render_new_branch_row(cx)))
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
                    .on_click(cx.listener(|this, _, window, cx| this.submit_new_branch(window, cx))),
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
                            .icon(IconName::Download)
                            .label(tr!("repo.sync.fetch"))
                            .loading(syncing == Some(SyncKind::Fetch))
                            .disabled(syncing.is_some())
                            .on_click(cx.listener(|this, _, _, cx| this.fetch(cx))),
                    )
                    .child(
                        Button::new("sync-pull")
                            .ghost()
                            .xsmall()
                            .icon(IconName::CloudDownload)
                            .label(tr!("repo.sync.pull"))
                            .loading(syncing == Some(SyncKind::Pull))
                            .disabled(syncing.is_some())
                            .on_click(cx.listener(|this, _, _, cx| this.pull(cx))),
                    )
                    .child(
                        Button::new("sync-push")
                            .ghost()
                            .xsmall()
                            .icon(IconName::CloudUpload)
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

    /// 仓库列表：高度封顶、自己滚动，把主要空间让给下面的改动列表。
    fn render_repo_list(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.repo_count() == 0 {
            return v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_1()
                .px_4()
                .text_sm()
                .text_center()
                .text_color(cx.theme().muted_foreground)
                .child(tr!("repo.none_open_title"))
                .child(div().text_xs().child(tr!("repo.none_open_hint")))
                .into_any_element();
        }

        let hover_bg = cx.theme().list_hover;
        let active_bg = cx.theme().list_active;
        let muted = cx.theme().muted_foreground;
        let radius = cx.theme().radius;
        let active_ix = self.active_index();
        let rows: Vec<AnyElement> = self
            .repos_meta(cx)
            .into_iter()
            .enumerate()
            .map(|(ix, (name, path))| {
                let selected = ix == active_ix;
                div()
                    .h_11()
                    .w_full()
                    .py_0p5()
                    .child(
                        h_flex()
                            .id(("repo-row", ix))
                            .size_full()
                            .px_2()
                            .gap_2()
                            .items_center()
                            .rounded(radius)
                            .when(selected, |row| row.bg(active_bg))
                            .hover(|style| style.bg(hover_bg))
                            .aria_selected(selected)
                            .aria_label(tr!("repo.row_aria", name = name.clone()))
                            .on_click(cx.listener(move |this, _, _, cx| this.activate(ix, cx)))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .child(div().text_sm().truncate().child(name))
                                    .child(
                                        div().text_xs().text_color(muted).truncate().child(path),
                                    ),
                            )
                            .child(
                                Button::new(("repo-close", ix))
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::Close)
                                    .tooltip(tr!("repo.close"))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        cx.stop_propagation();
                                        this.close_repo(ix, window, cx);
                                    })),
                            ),
                    )
                    .into_any_element()
            })
            .collect();

        // 高度封顶（约 3.5 行）：仓库通常只开几个，不该占掉整个侧栏，
        // 改动列表才是这个面板里最常被盯着看的内容。
        div()
            .id("repos")
            .flex_none()
            .max_h(px(160.))
            .px_2()
            .pt_1()
            .overflow_y_scroll()
            .child(v_flex().children(rows))
            .into_any_element()
    }

    /// 当前激活仓库的改动：已 stage / 未 stage 两组，每行一个切换暂存的按钮。
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
            .border_t_1()
            .border_color(cx.theme().sidebar_border)
            .overflow_y_scroll()
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(muted)
                    .child(tr!("repo.status.changes_title")),
            )
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

    /// 一组文件（已 stage 或未 stage），每行带一个切换暂存状态的按钮。
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
        let rows: Vec<AnyElement> = entries
            .into_iter()
            .enumerate()
            .map(|(ix, status)| {
                let file = status.path.clone();
                h_flex()
                    .h_7()
                    .gap_2()
                    .items_center()
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
    /// 不会被覆盖。
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
