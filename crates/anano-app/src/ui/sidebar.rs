//! 左侧栏 = 固定图标栏 + 可展开的面板：仓库切换器（GitHub Desktop 风格的下拉，替代原来
//! 带完整路径的仓库列表）→ 分支 / 同步（常驻）→ Changes / History 两个标签。
//!
//! Changes 标签是原来的改动列表 + 提交框；History 标签是从 `ui::status_pane` 挪过来的
//! 提交历史渲染——用户反馈这些都该在侧栏跟着仓库条目直接看到，主内容区腾出来给以后的
//! diff 视图（这次不做）。

use anano_core::git::ChangeKind;
use gpui_kit::component::{
    ActiveTheme, Disableable, Icon, IconName, Sizable,
    button::ButtonVariants,
    checkbox::Checkbox,
    h_flex,
    input::{Input, Textarea},
    menu::{ContextMenuExt, PopupMenuItem},
    popover::Popover,
    tooltip::Tooltip,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::assets::{ICON_HISTORY, logo_path};
use crate::brand::APP_NAME;
use crate::i18n::tr;
use crate::state::workspace::{Selected, SidebarTab, Workspace};

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

/// How many repositories the list repeats under "Recent".
const RECENT_COUNT: usize = 3;

impl Workspace {
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
            // Top: the logo and name, with Settings at the far end.
            .child(
                h_flex()
                    .h_10()
                    .flex_none()
                    .gap_2()
                    .pl_3()
                    .pr_2()
                    .items_center()
                    .child(img(logo_path(cx)).size(px(22.)).flex_none())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(APP_NAME),
                    )
                    .child(
                        // Settings sits here now, where the collapse button was. The sidebar
                        // still collapses with its shortcut, and the tab strip shows an
                        // expand button while it is collapsed.
                        crate::ui::button("sidebar-settings")
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(IconName::Settings).size_4())
                            .tooltip_with_action(
                                tr!("sidebar.settings"),
                                &crate::OpenSettings,
                                None,
                            )
                            .on_click(
                                cx.listener(|this, _, window, cx| this.open_settings(window, cx)),
                            ),
                    ),
            )
            .child(self.render_repo_switcher(cx))
            .when(self.active_repo().is_some(), |d| {
                // Branch and sync live in the toolbar above the tabs; the sidebar is just
                // "which repository" and "what changed / what happened".
                d.child(self.render_sidebar_tabs(cx))
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
            .branch_label()
            .map(SharedString::from)
            .unwrap_or_else(|| tr!("repo.switcher.no_branch"));
        let workspace = cx.entity();
        let search_input = self.repo_search_input.clone();
        let active_ix = self.active_index();
        let entries = self.switcher_entries();
        let nermal = self.nermal_available;
        let hover_bg = cx.theme().list_hover;
        let active_bg = cx.theme().list_active;
        let radius = cx.theme().radius;

        div()
            .id("repo-switcher")
            .debug_selector(|| "repo-switcher".into())
            .flex_none()
            .px_2()
            .pt_1()
            .pb_2()
            .child(
                Popover::new("repo-switcher-popover")
                    .open(self.switcher_open)
                    .on_open_change(cx.listener(|this, open: &bool, _, cx| {
                        this.switcher_open = *open;
                        cx.notify();
                    }))
                    .trigger_style(gpui::StyleRefinement::default())
                    .trigger(
                        crate::ui::button("repo-switcher-trigger")
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
                        // One row; `section` keeps ids unique when a repository is
                        // listed twice (under Recent and under its owner).
                        let row = |section: &'static str,
                                   (ix, name, path, _, _): &(
                            usize,
                            SharedString,
                            SharedString,
                            String,
                            i64,
                        )| {
                            let ix = *ix;
                            let selected = ix == active_ix;
                            let open_ws = workspace.clone();
                            let menu_ws = workspace.clone();
                            let (path, menu_path) = (path.clone(), path.clone());
                            h_flex()
                                .id((section, ix))
                                .flex_none()
                                .gap_2()
                                .items_center()
                                .px_2()
                                .py_1p5()
                                .rounded(radius)
                                .cursor_pointer()
                                .when(selected, |row| row.bg(active_bg))
                                .hover(|style| style.bg(hover_bg))
                                .aria_selected(selected)
                                .tooltip(move |window, cx| {
                                    Tooltip::new(path.clone()).build(window, cx)
                                })
                                .on_click(move |_, _, cx| {
                                    open_ws.update(cx, |ws, cx| ws.activate_from_switcher(ix, cx));
                                })
                                // Remove / reveal / copy path live in the right-click menu.
                                .context_menu(move |menu, _, _| {
                                    crate::ui::tab_strip::repo_menu(
                                        menu,
                                        &menu_ws,
                                        ix,
                                        menu_path.clone(),
                                        false,
                                        nermal,
                                    )
                                })
                                .child(
                                    Icon::empty()
                                        .path(crate::assets::ICON_BOOK)
                                        .size_4()
                                        .text_color(muted),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_sm()
                                        .truncate()
                                        .child(name.clone()),
                                )
                                .into_any_element()
                        };
                        let heading = |title: SharedString| {
                            div()
                                .px_2()
                                .pt_2()
                                .pb_1()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(title)
                                .into_any_element()
                        };
                        let matching: Vec<_> = entries
                            .iter()
                            .filter(|e| e.1.to_lowercase().contains(&query))
                            .collect();
                        let mut rows: Vec<AnyElement> = Vec::new();
                        // Recent: the last few opened, when not searching and there are
                        // enough repositories for a shortcut to be worth it.
                        if query.is_empty() && matching.len() > RECENT_COUNT {
                            let mut recent = matching.clone();
                            recent.sort_by_key(|e| std::cmp::Reverse(e.4));
                            rows.push(heading(tr!("repo.switcher.recent")));
                            rows.extend(
                                recent
                                    .into_iter()
                                    .take(RECENT_COUNT)
                                    .map(|e| row("switcher-recent-row", e)),
                            );
                        }
                        // Then everything, grouped by GitHub owner (alphabetical; the ones
                        // with no GitHub origin last).
                        let mut groups: std::collections::BTreeMap<(bool, String), Vec<_>> =
                            Default::default();
                        for entry in matching {
                            let owner = entry.3.clone();
                            groups
                                .entry((owner.is_empty(), owner))
                                .or_default()
                                .push(entry);
                        }
                        for ((no_owner, owner), mut members) in groups {
                            members.sort_by_key(|e| e.1.to_lowercase());
                            rows.push(heading(if no_owner {
                                tr!("repo.switcher.other")
                            } else {
                                owner.into()
                            }));
                            rows.extend(members.into_iter().map(|e| row("switcher-row", e)));
                        }

                        v_flex()
                            .w(px(320.))
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
                                    .id("switcher-list")
                                    .max_h(px(420.))
                                    .overflow_y_scroll()
                                    .children(rows),
                            )
                            .child(div().h(px(1.)).bg(hover_bg))
                            .child(render_add_repo_actions(workspace.clone()))
                    }),
            )
            .into_any_element()
    }

    /// 打开任何仓库前（切换器为空态）用的「Add」按钮，跟切换器下拉里的那个共用同一个
    /// 下拉菜单构建函数。
    fn render_add_repo_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        render_add_repo_actions(cx.entity()).into_any_element()
    }

    /// Changes / History 两个标签的头 + 对应内容。
    fn render_sidebar_tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(repo) = self.active_repo() else {
            return div().into_any_element();
        };
        let tab = self.sidebar_tab();
        let changes_count = repo.changed_files().len();
        let changes_label = if changes_count > 0 {
            tr!(
                "repo.tabs.changes_with_count",
                count = changes_count.to_string()
            )
        } else {
            tr!("repo.tabs.changes")
        };
        let border = cx.theme().sidebar_border;
        let active_bg = cx.theme().list_active;
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
                        crate::ui::button("tab-changes")
                            .ghost()
                            .compact()
                            .flex_1()
                            .label(changes_label)
                            .text_color(if tab == SidebarTab::Changes {
                                fg
                            } else {
                                muted
                            })
                            .when(tab == SidebarTab::Changes, |b| b.bg(active_bg))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_sidebar_tab(SidebarTab::Changes, cx)
                            })),
                    )
                    .child(
                        crate::ui::button("tab-history")
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
                            .when(tab == SidebarTab::History, |b| b.bg(active_bg))
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

        let hover_bg = cx.theme().list_hover;
        let active_bg = cx.theme().list_active;
        let border = cx.theme().sidebar_border;
        let origin = repo.origin.clone();
        let workspace = cx.entity();
        // GitHub Desktop's row: the summary, then who and how long ago under it.
        let rows: Vec<AnyElement> = repo
            .commits
            .iter()
            .map(|commit| {
                let target = Selected::Commit(commit.id.clone());
                let selected = self.selected() == Some(&target);
                let picture = self.avatars.get(&commit.email).and_then(|p| p.as_ref());
                let (sha, menu_ws) = (commit.id.clone(), workspace.clone());
                let web_url = origin
                    .as_deref()
                    .and_then(|o| anano_core::external::commit_web_url(o, &commit.id));
                v_flex()
                    .id(SharedString::from(format!("commit-{}", commit.id)))
                    .flex_none()
                    .px_3()
                    .py_2()
                    .gap_1()
                    .border_b_1()
                    .border_color(border)
                    .cursor_pointer()
                    .when(selected, |row| row.bg(active_bg))
                    .hover(move |style| style.bg(hover_bg))
                    .aria_selected(selected)
                    .on_click(cx.listener(move |this, _, _, cx| this.select(target.clone(), cx)))
                    .context_menu(move |menu, _, _| {
                        let (branch_ws, branch_sha) = (menu_ws.clone(), sha.clone());
                        let copy_sha = sha.clone();
                        let menu = menu
                            .item(
                                PopupMenuItem::new(tr!("repo.history.branch_from_commit"))
                                    .on_click(move |_, window, cx| {
                                        let sha = branch_sha.clone();
                                        branch_ws.update(cx, |ws, cx| {
                                            ws.open_new_branch_dialog(Some(sha), window, cx)
                                        });
                                    }),
                            )
                            .separator()
                            .item(PopupMenuItem::new(tr!("repo.history.copy_sha")).on_click(
                                move |_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        copy_sha.clone(),
                                    ));
                                },
                            ));
                        match web_url.clone() {
                            Some(url) => menu.item(
                                PopupMenuItem::new(tr!("repo.history.view_on_github"))
                                    .on_click(move |_, _, cx| cx.open_url(&url)),
                            ),
                            None => menu,
                        }
                    })
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .truncate()
                            .child(commit.summary.clone()),
                    )
                    .child(
                        h_flex()
                            .gap_1p5()
                            .items_center()
                            .text_xs()
                            .text_color(muted)
                            .child(crate::ui::avatar(picture, &commit.author, px(16.), cx))
                            .child(div().min_w_0().truncate().child(format!(
                                "{} • {}",
                                commit.author,
                                crate::ui::time_ago(commit.time)
                            ))),
                    )
                    .into_any_element()
            })
            .collect();

        v_flex()
            .id("history-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .children(rows)
            .into_any_element()
    }

    /// The Changes tab's list, GitHub Desktop style: one row per changed file with a
    /// checkbox (ticked = goes into the next commit; everything is ticked by default), the
    /// path, and a symbol for what happened to it. The header checkbox ticks or unticks
    /// them all. Clicking a row shows its diff; right-click for discard / ignore / copy.
    fn render_changes_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(repo) = self.active_repo() else {
            return div().into_any_element();
        };
        let muted = cx.theme().muted_foreground;
        let border = cx.theme().sidebar_border;
        let hover_bg = cx.theme().list_hover;
        let active_bg = cx.theme().list_active;
        let files = repo.changed_files();
        let checked = files.len() - repo.unchecked.len().min(files.len());
        let clean = files.is_empty() && !repo.loading && repo.error.is_none();
        // A repository whose folder was moved or deleted used to look like an empty one.
        let error = repo.error.clone().map(|error| {
            let path = repo.entry.path.display().to_string();
            v_flex()
                .p_3()
                .gap_1()
                .text_sm()
                .text_color(cx.theme().danger)
                .child(tr!("repo.status.unreadable", path = path))
                .child(div().text_xs().text_color(muted).child(error))
        });
        let workspace = cx.entity();

        let header = (!files.is_empty()).then(|| {
            h_flex()
                .flex_none()
                .h_9()
                .px_3()
                .gap_2()
                .items_center()
                .border_b_1()
                .border_color(border)
                .child(
                    Checkbox::new("changes-all")
                        .checked(checked == files.len())
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_all_files(cx))),
                )
                .child(div().text_sm().child(tr!(
                    "repo.status.changed_files",
                    count = files.len().to_string()
                )))
        });

        let rows: Vec<AnyElement> = files
            .iter()
            .enumerate()
            .map(|(ix, (file, kind))| {
                let (file, kind) = (file.clone(), *kind);
                let menu_workspace = workspace.clone();
                let menu_file = file.clone();
                let toggle_file = file.clone();
                let target = Selected::File { path: file.clone() };
                let selected = self.selected() == Some(&target);
                let ticked = !repo.unchecked.contains(&file);
                h_flex()
                    .id(("file-row", ix))
                    .when(ix == 0, |row| {
                        row.debug_selector(|| "first-file-row".into())
                    })
                    // Fixed height: a row in a scrolling list must not be squeezed when
                    // the window gets short (flex children shrink by default), it scrolls.
                    .flex_none()
                    .h_9()
                    .px_3()
                    .gap_2()
                    .items_center()
                    .border_b_1()
                    .border_color(border)
                    .cursor_pointer()
                    .when(selected, |row| row.bg(active_bg))
                    .hover(move |style| style.bg(hover_bg))
                    .aria_selected(selected)
                    .on_click(cx.listener(move |this, _, _, cx| this.select(target.clone(), cx)))
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
                        // Its own click target: ticking a file shouldn't also open it.
                        div()
                            .id(("file-tick", ix))
                            .flex_none()
                            .on_click(|_, _, cx| cx.stop_propagation())
                            .child(Checkbox::new(("file-check", ix)).checked(ticked).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.toggle_file(toggle_file.clone(), cx)
                                }),
                            )),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .truncate()
                            .child(file.clone()),
                    )
                    .child(kind_badge(kind, cx))
                    .into_any_element()
            })
            .collect();

        v_flex()
            .id("changes")
            .flex_1()
            .min_h_0()
            .children(error)
            .children(header)
            .child(
                v_flex()
                    .id("changes-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(rows)
                    .when(clean, |d| {
                        d.child(
                            div()
                                .p_3()
                                .text_sm()
                                .text_color(muted)
                                .child(tr!("repo.status.clean")),
                        )
                    }),
            )
            .into_any_element()
    }

    /// The commit area at the bottom of the Changes tab: your picture, a one-line summary
    /// (required), an optional description, and "Commit N files to <branch>". The button is
    /// live once there is a summary and at least one ticked file. With "Commit and push
    /// with one button" on, it also pushes.
    fn render_commit_box(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let repo = self.active_repo();
        let count = repo.map_or(0, |r| r.checked_files().len());
        let has_summary = !self.commit_input.read(cx).value().trim().is_empty();
        let border = cx.theme().border;
        let radius = cx.theme().radius;
        // Whoever this repository commits and pushes as.
        let me = repo
            .and_then(|r| r.origin.as_deref())
            .and_then(|url| {
                anano_core::external::accounts_for(url, &self.accounts)
                    .into_iter()
                    .next()
            })
            .or_else(|| self.accounts.iter().find(|a| a.active).cloned());
        let avatar = me.map(|account| {
            let picture = self
                .account_avatars
                .get(&account.login)
                .and_then(|p| p.as_ref());
            crate::ui::avatar(picture, &account.login, px(28.), cx)
        });
        // "Commit & Push", or plain "Commit" when there is no remote to push to.
        let label = if repo.is_some_and(|r| r.origin.is_some()) {
            tr!("repo.commit.commit_and_push")
        } else {
            tr!("repo.commit.commit")
        };
        v_flex()
            .id("commit-box")
            .flex_none()
            .gap_2()
            .p_3()
            .border_t_1()
            .border_color(cx.theme().sidebar_border)
            .child(
                h_flex().gap_2().items_center().children(avatar).child(
                    div()
                        .id("commit-message")
                        .flex_1()
                        .min_w_0()
                        .rounded(radius)
                        .border_1()
                        .border_color(border)
                        .child(
                            Input::new(&self.commit_input)
                                .aria_label(tr!("repo.commit.placeholder")),
                        ),
                ),
            )
            .child(
                div()
                    .id("commit-description")
                    .rounded(radius)
                    .border_1()
                    .border_color(border)
                    .child(Textarea::new(&self.commit_description)),
            )
            .child(
                crate::ui::button("commit")
                    .primary()
                    .w_full()
                    .disabled(count == 0 || !has_summary)
                    .label(label)
                    .on_click(cx.listener(|this, _, window, cx| this.commit_active(window, cx))),
            )
    }
}

/// What happened to a file, as GitHub Desktop's little square: a dot for modified, plus
/// for new, minus for deleted, arrow for renamed, bang for a conflict.
fn kind_badge(kind: ChangeKind, cx: &App) -> impl IntoElement {
    let glyph = match kind {
        ChangeKind::Modified | ChangeKind::TypeChange => "•",
        ChangeKind::New => "+",
        ChangeKind::Deleted => "−",
        ChangeKind::Renamed => "→",
        ChangeKind::Conflicted => "!",
    };
    let color = kind_color(kind, cx);
    let label = kind_label(kind);
    div()
        .id(SharedString::from(format!("kind-{glyph}")))
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
        .line_height(px(12.))
        .child(glyph)
        // The word is still there for anyone who hovers (and for screen readers).
        .tooltip(move |window, cx| Tooltip::new(label.clone()).build(window, cx))
}

/// The three ways to add a repository, as plain buttons. They used to be a dropdown menu
/// nested inside the switcher popover; a click on one of its items counted as a click
/// outside the popover, which tore the menu down before the item's handler ran, so none of
/// the dialogs ever opened from there.
fn render_add_repo_actions(workspace: Entity<Workspace>) -> impl IntoElement {
    type Open = fn(&mut Workspace, &mut Window, &mut Context<Workspace>);
    let actions: [(&'static str, SharedString, Open); 3] = [
        (
            "repo-add-clone",
            tr!("repo.switcher.add_clone"),
            Workspace::open_clone_dialog,
        ),
        (
            "repo-add-create",
            tr!("repo.switcher.add_create"),
            Workspace::open_create_repo_dialog,
        ),
        (
            "repo-add-existing",
            tr!("repo.switcher.add_existing"),
            Workspace::add_existing_repo,
        ),
    ];
    v_flex()
        .w_full()
        .gap_0p5()
        .children(actions.into_iter().map(|(id, label, open)| {
            let workspace = workspace.clone();
            let button = crate::ui::button(id)
                .ghost()
                .small()
                .w_full()
                .justify_start()
                .icon(IconName::Plus)
                .label(label)
                .on_click(move |_, window, cx| {
                    // Next frame: by then the popover has finished closing and can't take
                    // the dialog (or its focus) down with it.
                    let workspace = workspace.clone();
                    window.defer(cx, move |window, cx| {
                        workspace.update(cx, |ws, cx| open(ws, window, cx));
                    });
                });
            div()
                .w_full()
                .debug_selector(move || id.into())
                .child(button)
        }))
}
