//! The permanent toolbar above the repository tabs, after GitHub Desktop's: one big
//! "Current Branch" button (click to switch) and one big sync button that names the next
//! sensible step — Fetch, Pull when the branch is behind, Push when it is ahead, Publish
//! when it has no upstream yet — with when the repository was last fetched underneath.

use std::time::SystemTime;

use anano_core::git::Branch;
use gpui_kit::component::{
    ActiveTheme, Icon, IconName, Sizable,
    button::{Button, ButtonVariants},
    h_flex,
    input::{Input, InputState},
    menu::{ContextMenuExt, DropdownMenu, PopupMenuItem},
    popover::Popover,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, Context, Entity, FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, div, px,
};

use crate::assets::{
    ICON_GIT_BRANCH, ICON_PULL_REQUEST, ICON_REFRESH, ICON_UPLOAD_CLOUD, ICON_WORKFLOW,
};
use crate::i18n::tr;
use crate::state::repos::SyncKind;
use crate::state::workspace::Workspace;
use crate::ui::account_view::AccountPage;

const TOOLBAR_HEIGHT: f32 = 52.;
/// Each big button is as wide as its content needs and no wider. A width is the button's
/// fixed chrome (padding, icon, gap: 46px) plus the widest text it ever shows, and real
/// fonts run wider than a character count suggests, so these are generous: a label that
/// is cut off is worse than a few spare pixels.
///   - branch: "Current Branch" over a typical branch name, and the chevron;
///   - sync: "Fetched 10hr ago" / "Publish to GitHub", and "Push origin ↑2" on the title line;
///   - Pull Requests: "Pull Requests" over "Review & merge";
///   - Workflows: "Workflows" over "Runs & logs".
const BRANCH_WIDTH: f32 = 196.;
const SYNC_WIDTH: f32 = 190.;
const PULLS_WIDTH: f32 = 190.;
const WORKFLOWS_WIDTH: f32 = 160.;
/// The account button on the right: picture, name (cut with an ellipsis if very long), chevron.
const ACCOUNT_WIDTH: f32 = 172.;
/// The toolbar's width when everything is showing: the window must be at least this much
/// wider than the sidebar, or the account button falls off the right edge.
pub const MIN_TOOLBAR_WIDTH: f32 =
    BRANCH_WIDTH + SYNC_WIDTH + PULLS_WIDTH + WORKFLOWS_WIDTH + ACCOUNT_WIDTH;
/// The branch panel is wider than its button: names plus "2 months ago" need the room.
const PANEL_WIDTH: f32 = 380.;

/// The "Current Branch" panel: a filter box and a New Branch button, then the branches
/// (default branch first), each with a check when current and how long ago it last moved.
/// Clicking one switches to it.
fn branch_panel(
    branches: &[Branch],
    filter: &Entity<InputState>,
    workspace: &Entity<Workspace>,
    cx: &App,
) -> impl IntoElement + use<> {
    let theme = cx.theme();
    let (muted, border, hover, active, radius) = (
        theme.muted_foreground,
        theme.border,
        theme.list_hover,
        theme.list_active,
        theme.radius,
    );
    let query = filter.read(cx).value().trim().to_lowercase();
    // ponytail: "default" is by name. Read origin/HEAD instead if a repository ever uses
    // another default branch name and this section matters.
    let is_default = |b: &Branch| b.name == "main" || b.name == "master";
    let row = |ix: usize, branch: &Branch| {
        let (name, is_head) = (branch.name.clone(), branch.is_head);
        let workspace = workspace.clone();
        let menu_workspace = workspace.clone();
        h_flex()
            .id(("branch-row", ix))
            .flex_none()
            .gap_2()
            .px_2()
            .py_1p5()
            .rounded(radius)
            .items_center()
            .text_sm()
            .cursor_pointer()
            .when(is_head, |row| row.bg(active))
            .hover(move |style| style.bg(hover))
            .child(
                div()
                    .w_4()
                    .flex_none()
                    .when(is_head, |d| d.child(Icon::new(IconName::Check).size_4())),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(branch.name.clone()),
            )
            .children(branch.last_commit_time.map(|at| {
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(muted)
                    .child(crate::ui::time_ago(at))
            }))
            .on_click(move |_, window, cx| {
                let name = name.clone();
                workspace.update(cx, |ws, cx| {
                    ws.close_branch_popover(cx);
                    if !is_head {
                        ws.request_switch_branch(name, window, cx);
                    }
                });
            })
            // Rename and delete are in the right-click menu, never a button on the row.
            .context_menu({
                let (menu_ws, menu_name) = (menu_workspace, branch.name.clone());
                move |menu, _, _| {
                    let item = |label: SharedString,
                                act: fn(
                        &mut Workspace,
                        String,
                        &mut gpui_kit::Window,
                        &mut Context<Workspace>,
                    )| {
                        let (ws, name) = (menu_ws.clone(), menu_name.clone());
                        PopupMenuItem::new(label).on_click(move |_, window, cx| {
                            let (ws, name) = (ws.clone(), name.clone());
                            // Next frame: the menu and the branch panel have closed by then.
                            window.defer(cx, move |window, cx| {
                                ws.update(cx, |ws, cx| {
                                    ws.close_branch_popover(cx);
                                    act(ws, name, window, cx)
                                });
                            });
                        })
                    };
                    let copy_name = menu_name.clone();
                    let menu = menu.item(item(
                        tr!("repo.branch.rename_menu"),
                        |ws, name, window, cx| ws.open_rename_branch_dialog(name, window, cx),
                    ));
                    let menu = if is_head {
                        menu
                    } else {
                        menu.item(item(
                            tr!("repo.branch.delete_menu"),
                            |ws, name, window, cx| ws.confirm_delete_branch(name, window, cx),
                        ))
                    };
                    menu.separator().item(
                        PopupMenuItem::new(tr!("repo.branch.copy_name")).on_click(
                            move |_, _, cx| {
                                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(
                                    copy_name.clone(),
                                ))
                            },
                        ),
                    )
                }
            })
    };
    let section = |title: SharedString, default: bool| {
        let rows: Vec<_> = branches
            .iter()
            .enumerate()
            .filter(|(_, b)| is_default(b) == default && b.name.to_lowercase().contains(&query))
            .map(|(ix, b)| row(ix, b))
            .collect();
        (!rows.is_empty()).then(|| {
            v_flex()
                .gap_0p5()
                .child(
                    div()
                        .px_2()
                        .py_1()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title),
                )
                .children(rows)
        })
    };
    let new_branch_ws = workspace.clone();
    v_flex()
        .w(px(PANEL_WIDTH))
        .p_2()
        .gap_2()
        .child(
            h_flex()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .rounded(px(6.))
                        .border_1()
                        .border_color(border)
                        .child(
                            Input::new(filter)
                                .prefix(Icon::new(IconName::Search).size_4().text_color(muted)),
                        ),
                )
                .child(
                    div()
                        .flex_none()
                        .debug_selector(|| "branch-new".into())
                        .child(
                            crate::ui::button("branch-new")
                                .outline()
                                .label(tr!("repo.branch.new_button"))
                                .on_click(move |_, window, cx| {
                                    let workspace = new_branch_ws.clone();
                                    // Next frame, once the panel has closed (same reason as the
                                    // repository switcher's Add entries).
                                    window.defer(cx, move |window, cx| {
                                        workspace.update(cx, |ws, cx| {
                                            ws.open_new_branch_dialog(None, window, cx)
                                        });
                                    });
                                }),
                        ),
                ),
        )
        .child(
            v_flex()
                .id("branch-list")
                .gap_2()
                .max_h(px(360.))
                .overflow_y_scroll()
                .children(section(tr!("repo.branch.default_section"), true))
                .children(section(tr!("repo.branch.other_section"), false)),
        )
}

/// "Last fetched 15m ago": worked out from the fetch time whenever the toolbar is drawn
/// (not a ticking clock). Minutes up to an hour, hours up to two days, then days.
fn last_fetched_label(at: Option<SystemTime>) -> SharedString {
    match at.and_then(|t| t.elapsed().ok()) {
        Some(elapsed) => tr!("repo.toolbar.last_fetched", when = ago(elapsed.as_secs())),
        None => tr!("repo.toolbar.never_fetched"),
    }
}

fn ago(secs: u64) -> SharedString {
    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;
    if secs < MINUTE {
        tr!("repo.toolbar.just_now")
    } else if secs < HOUR {
        tr!(
            "repo.toolbar.minutes_ago",
            count = (secs / MINUTE).to_string()
        )
    } else if secs < 2 * DAY {
        tr!("repo.toolbar.hours_ago", count = (secs / HOUR).to_string())
    } else {
        tr!("repo.toolbar.days_ago", count = (secs / DAY).to_string())
    }
}

/// What the sync button should do next, GitHub Desktop's order of precedence.
pub(crate) fn next_sync(branch: Option<&Branch>) -> (SyncKind, SharedString) {
    match branch {
        Some(b) if b.upstream.is_none() => (SyncKind::Push, tr!("repo.toolbar.publish")),
        Some(Branch {
            ahead_behind: Some((_, behind)),
            ..
        }) if *behind > 0 => (SyncKind::Pull, tr!("repo.toolbar.pull")),
        Some(Branch {
            ahead_behind: Some((ahead, _)),
            ..
        }) if *ahead > 0 => (SyncKind::Push, tr!("repo.toolbar.push")),
        _ => (SyncKind::Fetch, tr!("repo.toolbar.fetch")),
    }
}

/// The two-line label of a big toolbar button.
fn big_label(
    top: SharedString,
    bottom: SharedString,
    bold_top: bool,
    muted: Hsla,
) -> impl IntoElement {
    let line = |text: SharedString, bold: bool| {
        div()
            .truncate()
            .when(bold, |d| d.text_sm().font_weight(FontWeight::SEMIBOLD))
            .when(!bold, |d| d.text_xs().text_color(muted))
            .child(text)
    };
    v_flex()
        .flex_1()
        .min_w_0()
        .items_start()
        .child(line(top, bold_top))
        .child(line(bottom, !bold_top))
}

/// A big toolbar button, sized in plain pixels and with no padding of its own.
///
/// The dropdown wrapper copies the trigger's style onto itself and then lays the trigger
/// out inside it. Percentage sizes (`h_full`) therefore resolve against a wrapper with no
/// size of its own, and padding gets applied at both levels: that is what left the branch
/// button smaller and off-centre next to the plain sync one. A pixel size means the same
/// thing at both levels, so both buttons come out exactly slot-sized; the content pads.
fn big_button(id: &'static str, width: f32) -> Button {
    crate::ui::button(id)
        .ghost()
        // The slot minus its 1px divider, the toolbar minus its 1px bottom border.
        .w(px(width - 1.))
        .h(px(TOOLBAR_HEIGHT - 1.))
        .rounded_none()
}

/// One fixed-width cell of the toolbar.
fn slot(
    selector: &'static str,
    width: f32,
    border: Hsla,
    button: impl IntoElement,
) -> impl IntoElement {
    div()
        .debug_selector(move || selector.into())
        .flex_none()
        .h_full()
        .w(px(width))
        .border_r_1()
        .border_color(border)
        .child(button)
}

/// Icon / two-line label / trailing bits, laid out identically in both buttons.
fn content(selector: &'static str) -> gpui_kit::Div {
    h_flex()
        .debug_selector(move || selector.into())
        .size_full()
        .px_2()
        .gap_2p5()
        .items_center()
}

impl Workspace {
    pub fn render_toolbar(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let repo = self.active_repo()?;
        let muted = cx.theme().muted_foreground;
        let border = cx.theme().border;
        let current = repo.current_branch();
        let branch_name = repo
            .branch_label()
            .map(SharedString::from)
            .unwrap_or_else(|| tr!("repo.branch.detached"));
        let branches: Vec<Branch> = repo
            .branches
            .iter()
            .filter(|b| !b.is_remote)
            .cloned()
            .collect();
        let workspace = cx.entity();
        let open = self.branch_popover_open;
        let on_github = repo
            .origin
            .as_deref()
            .is_some_and(anano_core::external::is_github);
        let filter = self.branch_filter.clone();

        // Which connected GitHub account this repository syncs as (GitHub remotes only).
        let logins = self.account_logins();
        let account_menu = repo
            .origin
            .as_deref()
            .filter(|url| !logins.is_empty() && anano_core::external::is_github(url))
            .map(|url| {
                // Show the account this repository actually syncs as, pinned or not: its
                // owner's account for your own repositories, otherwise the default. Picking
                // another one here pins the repository to it.
                let pinned = anano_core::external::accounts_for(url, &self.accounts)
                    .first()
                    .map(|a| a.login.clone());
                let menu_ws = cx.entity();
                let label = pinned
                    .clone()
                    .map(SharedString::from)
                    .unwrap_or_else(|| tr!("repo.sync.account"));
                // A proper button, not a small icon: the account's picture (cached on disk
                // after the first download) and its name.
                let picture = pinned
                    .as_ref()
                    .and_then(|login| self.account_avatars.get(login))
                    .and_then(|p| p.as_ref());
                let face = crate::ui::avatar(picture, &label, px(26.), cx);
                crate::ui::button("toolbar-account")
                    .ghost()
                    .w(px(ACCOUNT_WIDTH - 16.))
                    .h(px(38.))
                    .tooltip(tr!("repo.sync.account_tooltip"))
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(face)
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_sm()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(label),
                            )
                            .child(Icon::new(IconName::ChevronDown).size_4()),
                    )
                    .dropdown_menu(move |menu, _, _| {
                        // Which account this repository uses (and the views below browse).
                        let menu = menu.item(PopupMenuItem::label(tr!("repo.account.switch")));
                        let menu = logins.iter().fold(menu, |menu, login| {
                            let menu_ws = menu_ws.clone();
                            let checked = pinned.as_deref() == Some(login.as_str());
                            let login = login.clone();
                            menu.item(PopupMenuItem::new(login.clone()).checked(checked).on_click(
                                move |_, _, cx| {
                                    menu_ws
                                        .update(cx, |ws, cx| ws.switch_account(login.clone(), cx));
                                },
                            ))
                        });
                        // What the account has on GitHub, cloned or not.
                        let browse = |label: SharedString, page: AccountPage| {
                            let (ws, login) = (menu_ws.clone(), pinned.clone());
                            PopupMenuItem::new(label).on_click(move |_, window, cx| {
                                let (ws, login, page) = (ws.clone(), login.clone(), page.clone());
                                // Next frame: the menu has closed by then.
                                window.defer(cx, move |_, cx| {
                                    if let Some(login) = login {
                                        ws.update(cx, |ws, cx| {
                                            ws.open_account_view(login, page, cx)
                                        });
                                    }
                                });
                            })
                        };
                        menu.separator()
                            .item(browse(tr!("repo.account.view_repos"), AccountPage::Repos))
                            .item(browse(tr!("repo.account.view_orgs"), AccountPage::Orgs))
                    })
            });

        let (kind, title) = next_sync(current);
        let syncing = repo.syncing.is_some();
        let counts = current
            .and_then(|b| b.ahead_behind)
            .filter(|(ahead, behind)| ahead + behind > 0)
            .map(|(ahead, behind)| {
                tr!(
                    "repo.branch.ahead_behind",
                    ahead = ahead.to_string(),
                    behind = behind.to_string()
                )
            });
        let publishing = current.is_some_and(|b| b.upstream.is_none());
        let subtitle = if syncing {
            tr!("repo.toolbar.working")
        } else if publishing {
            tr!("repo.toolbar.publish_hint")
        } else {
            last_fetched_label(repo.last_fetched)
        };
        let sync_icon = if publishing {
            ICON_UPLOAD_CLOUD
        } else {
            ICON_REFRESH
        };

        Some(
            h_flex()
                .flex_none()
                .w_full()
                .h(px(TOOLBAR_HEIGHT))
                .border_b_1()
                .border_color(border)
                .child(slot(
                    "toolbar-branch-slot",
                    BRANCH_WIDTH,
                    border,
                    Popover::new("toolbar-branch-popover")
                        .open(open)
                        .on_open_change(cx.listener(|this, open: &bool, _, cx| {
                            this.branch_popover_open = *open;
                            cx.notify();
                        }))
                        .trigger_style(gpui_kit::StyleRefinement::default())
                        .trigger(
                            big_button("toolbar-branch", BRANCH_WIDTH).child(
                                content("toolbar-branch-content")
                                    .child(Icon::empty().path(ICON_GIT_BRANCH).size_5())
                                    .child(big_label(
                                        tr!("repo.toolbar.current_branch"),
                                        branch_name,
                                        false,
                                        muted,
                                    ))
                                    .child(
                                        Icon::new(if open {
                                            IconName::ChevronUp
                                        } else {
                                            IconName::ChevronDown
                                        })
                                        .size_4(),
                                    ),
                            ),
                        )
                        .content(move |_, _, cx| branch_panel(&branches, &filter, &workspace, cx)),
                ))
                .child(slot(
                    "toolbar-sync-slot",
                    SYNC_WIDTH,
                    border,
                    big_button("toolbar-sync", SYNC_WIDTH)
                        .loading(syncing)
                        .child(
                            content("toolbar-sync-content")
                                .when(!syncing, |h| {
                                    h.child(Icon::empty().path(sync_icon).size_5())
                                })
                                .child(
                                    v_flex()
                                        .flex_1()
                                        .min_w_0()
                                        .items_start()
                                        // The ↑/↓ counts ride on the title line: that keeps
                                        // the button as narrow as its subtitle.
                                        .child(
                                            h_flex()
                                                .w_full()
                                                .min_w_0()
                                                .gap_1p5()
                                                .items_baseline()
                                                .child(
                                                    div()
                                                        .truncate()
                                                        .text_sm()
                                                        .font_weight(FontWeight::SEMIBOLD)
                                                        .child(title),
                                                )
                                                .children(counts.map(|counts| {
                                                    div()
                                                        .flex_none()
                                                        .text_xs()
                                                        .text_color(muted)
                                                        .child(counts)
                                                })),
                                        )
                                        .child(
                                            div()
                                                .truncate()
                                                .text_xs()
                                                .text_color(muted)
                                                .child(subtitle),
                                        ),
                                ),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| match kind {
                            SyncKind::Fetch => this.fetch(cx),
                            SyncKind::Pull => this.pull(cx),
                            SyncKind::Push => this.push(cx),
                        })),
                ))
                // Only for repositories whose origin is on GitHub.
                .when(on_github, |toolbar| {
                    toolbar.child(slot(
                        "toolbar-pulls-slot",
                        PULLS_WIDTH,
                        border,
                        big_button("toolbar-pulls", PULLS_WIDTH)
                            .child(
                                content("toolbar-pulls-content")
                                    .child(Icon::empty().path(ICON_PULL_REQUEST).size_5())
                                    .child(big_label(
                                        tr!("repo.pulls.title"),
                                        tr!("repo.pulls.button_hint"),
                                        true,
                                        muted,
                                    )),
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.open_pull_requests(cx))),
                    ))
                })
                // Right of Pull Requests: GitHub Actions, for repositories on GitHub.
                .when(on_github, |toolbar| {
                    toolbar.child(slot(
                        "toolbar-workflows-slot",
                        WORKFLOWS_WIDTH,
                        border,
                        big_button("toolbar-workflows", WORKFLOWS_WIDTH)
                            .child(
                                content("toolbar-workflows-content")
                                    .child(Icon::empty().path(ICON_WORKFLOW).size_5())
                                    .child(big_label(
                                        tr!("repo.workflows.title"),
                                        tr!("repo.workflows.button_hint"),
                                        true,
                                        muted,
                                    )),
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.open_workflows(cx))),
                    ))
                })
                .child(div().flex_1())
                .children(account_menu.map(|menu| {
                    div()
                        .debug_selector(|| "toolbar-account-slot".into())
                        .flex_none()
                        .w(px(ACCOUNT_WIDTH))
                        .px_2()
                        .child(menu)
                })),
        )
    }
}

impl Workspace {
    /// What used to sit under the sidebar's sync buttons: why the last branch or sync
    /// action failed, and the "git-lfs isn't installed" warning. A strip under the toolbar,
    /// only there when there is something to say.
    pub fn render_repo_notice(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let repo = self.active_repo()?;
        let error = repo.action_error.clone();
        let lfs_missing = repo.lfs_status == Some(anano_core::git::LfsSync::NotInstalled);
        if error.is_none() && !lfs_missing {
            return None;
        }
        let theme = cx.theme();
        let (danger, warning, border) = (theme.danger, theme.warning, theme.border);
        Some(
            v_flex()
                .flex_none()
                .w_full()
                .border_b_1()
                .border_color(border)
                .children(error.map(|error| {
                    h_flex()
                        .px_3()
                        .py_1p5()
                        .gap_2()
                        .items_center()
                        .bg(danger.opacity(0.1))
                        .child(div().flex_1().min_w_0().text_sm().text_color(danger).child(
                            crate::ui::selectable(
                                "notice-error",
                                tr!("repo.sync.error", error = error),
                            ),
                        ))
                        .child(
                            crate::ui::button("notice-dismiss")
                                .ghost()
                                .xsmall()
                                .icon(IconName::Close)
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.dismiss_action_error(cx)),
                                ),
                        )
                }))
                .when(lfs_missing, |strip| {
                    strip.child(
                        div()
                            .px_3()
                            .py_1p5()
                            .text_sm()
                            .text_color(warning)
                            .child(tr!("repo.sync.lfs_not_installed")),
                    )
                }),
        )
    }

    /// Deleting a branch throws away any commits only it has, so ask first.
    pub fn confirm_delete_branch(
        &mut self,
        name: String,
        window: &mut gpui_kit::Window,
        cx: &mut Context<Self>,
    ) {
        use gpui_kit::component::WindowExt as _;
        self.close_branch_popover(cx);
        if window.has_active_dialog(cx) {
            return;
        }
        let workspace = cx.entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let (workspace, name) = (workspace.clone(), name.clone());
            dialog
                .title(tr!("repo.branch.delete", name = name.clone()))
                .w(px(420.))
                .content(|content, _, _| {
                    content.child(div().text_sm().child(tr!("repo.branch.delete_warning")))
                })
                .footer(
                    gpui_kit::component::dialog::DialogFooter::new()
                        .child(
                            crate::ui::button("delete-branch-cancel")
                                .outline()
                                .label(tr!("common.cancel"))
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            crate::ui::button("delete-branch-confirm")
                                .danger()
                                .label(tr!("repo.branch.delete_action"))
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    let name = name.clone();
                                    workspace.update(cx, |ws, cx| ws.delete_branch(name, cx));
                                }),
                        ),
                )
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn branch(upstream: bool, ahead_behind: Option<(usize, usize)>) -> Branch {
        Branch {
            name: "main".into(),
            is_head: true,
            is_remote: false,
            upstream: upstream.then(|| "origin/main".to_string()),
            ahead_behind,
            last_commit_time: None,
        }
    }

    #[test]
    fn ago_reads_as_minutes_then_hours_then_days() {
        let _locale = crate::i18n::locale_test_lock();
        let read = |secs| ago(secs).to_string();
        assert_eq!(read(60), "1m ago");
        assert_eq!(read(15 * 60 + 59), "15m ago");
        assert_eq!(read(3600), "1hr ago");
        assert_eq!(read(10 * 3600), "10hr ago");
        assert_eq!(read(24 * 3600), "24hr ago");
        assert_eq!(read(4 * 86_400), "4d ago");
    }

    #[test]
    fn sync_button_picks_the_next_step_like_github_desktop() {
        let kind = |b: Option<&Branch>| next_sync(b).0;
        assert_eq!(kind(None), SyncKind::Fetch);
        assert_eq!(kind(Some(&branch(false, None))), SyncKind::Push); // publish
        assert_eq!(kind(Some(&branch(true, Some((0, 0))))), SyncKind::Fetch);
        assert_eq!(kind(Some(&branch(true, Some((2, 0))))), SyncKind::Push);
        // Behind wins over ahead: you can't push until you've pulled.
        assert_eq!(kind(Some(&branch(true, Some((2, 1))))), SyncKind::Pull);
    }

    /// The two big buttons must be the same size and sit the same way in their slots: the
    /// branch one goes through the dropdown wrapper, which once shrank and off-centred it.
    #[gpui_kit::test]
    fn both_toolbar_buttons_fill_their_slots_identically(cx: &mut gpui_kit::TestAppContext) {
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
        workspace.update(cx, |ws, cx| {
            ws.open_repo_path("/tmp/anano-toolbar-test".into(), cx)
        });
        cx.run_until_parked();

        let bounds = |cx: &mut gpui_kit::VisualTestContext, name: &'static str| {
            cx.debug_bounds(name)
                .unwrap_or_else(|| panic!("{name} not on screen"))
        };
        let (branch_slot, sync_slot) = (
            bounds(cx, "toolbar-branch-slot"),
            bounds(cx, "toolbar-sync-slot"),
        );
        let (branch, sync) = (
            bounds(cx, "toolbar-branch-content"),
            bounds(cx, "toolbar-sync-content"),
        );
        // Each is as wide as it was asked to be, and both are the full toolbar height.
        assert_eq!(branch_slot.size.width, px(BRANCH_WIDTH));
        assert_eq!(sync_slot.size.width, px(SYNC_WIDTH));
        assert_eq!(branch_slot.size.height, sync_slot.size.height);
        assert_eq!(
            branch.size.height, sync.size.height,
            "the two buttons' contents differ in height"
        );
        // Same inset from its slot on every side, i.e. each fills its slot the same way.
        assert_eq!(
            branch.origin - branch_slot.origin,
            sync.origin - sync_slot.origin
        );
        // And the content really spans the slot, not a shrunk box: full height (minus the
        // 1px divider), full width minus the button's own side padding.
        assert!(branch.size.width >= branch_slot.size.width - px(24.));
        assert!(
            branch.size.height >= branch_slot.size.height - px(2.),
            "content {:?} in slot {:?}",
            branch,
            branch_slot
        );
    }

    /// Current Branch → New Branch → the "Create a Branch" dialog; and with a real
    /// repository on screen, selecting a commit draws its header, file list and diff.
    #[gpui_kit::test]
    fn branch_panel_and_commit_view_work_on_a_real_repository(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::AppContext as _;
        use gpui_kit::component::WindowExt as _;
        cx.executor().allow_parking();
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::bridge::init(cx);
        });
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            assert!(
                std::process::Command::new("git")
                    .args(args)
                    .current_dir(dir.path())
                    .status()
                    .unwrap()
                    .success()
            );
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", ""]);
        git(&["config", "user.name", "test"]);
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "init"]);

        let mut workspace = None;
        let window = cx.add_window(|window, cx| {
            let ws = cx.new(|cx| Workspace::new(window, cx));
            workspace = Some(ws.clone());
            gpui_kit::component::Root::new(ws, window, cx)
        });
        let workspace = workspace.unwrap();
        let cx = &mut gpui_kit::VisualTestContext::from_window(window.into(), cx);
        let repo = dir.path().canonicalize().unwrap();
        workspace.update(cx, |ws, cx| ws.open_repo_path(repo, cx));
        // git work runs on tokio's pool; give the three refreshes time to land.
        let loaded = |cx: &mut gpui_kit::VisualTestContext| {
            workspace.read_with(cx, |ws, _| {
                ws.active_repo()
                    .is_some_and(|r| !r.commits.is_empty() && !r.branches.is_empty())
            })
        };
        for _ in 0..200 {
            cx.run_until_parked();
            if loaded(cx) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(loaded(cx), "repository never finished loading");

        // The commit view: header + changed files + diff, drawn without incident.
        let head = workspace.read_with(cx, |ws, _| ws.active_repo().unwrap().commits[0].id.clone());
        workspace.update(cx, |ws, cx| {
            ws.select(crate::state::workspace::Selected::Commit(head), cx)
        });
        for _ in 0..200 {
            cx.run_until_parked();
            if workspace.read_with(cx, |ws, _| ws.diff.is_some()) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let files = workspace.read_with(cx, |ws, _| match &ws.diff {
            Some(Ok(view)) => view
                .lines
                .iter()
                .filter(|l| l.kind == anano_core::git::LineKind::File)
                .count(),
            other => panic!(
                "no diff: {:?}",
                other.as_ref().map(|r| r.as_ref().map(|_| ()))
            ),
        });
        assert_eq!(files, 1);
        cx.run_until_parked();

        // The branch panel.
        let button = cx.debug_bounds("toolbar-branch-content").unwrap();
        cx.simulate_click(button.center(), gpui_kit::Modifiers::none());
        cx.run_until_parked();
        let new_branch = cx
            .debug_bounds("branch-new")
            .expect("branch panel didn't open");
        cx.simulate_click(new_branch.center(), gpui_kit::Modifiers::none());
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert!(window.has_active_dialog(cx), "New Branch opened no dialog");
            assert!(!workspace.read(cx).branch_popover_open, "panel stayed open");
            window.close_dialog(cx);
        });
        cx.run_until_parked();
    }

    /// Opening (or switching to) a repository with an origin fetches it without being asked.
    #[gpui_kit::test]
    fn switching_to_a_repository_fetches_its_origin(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::AppContext as _;
        cx.executor().allow_parking();
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::bridge::init(cx);
        });
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str], cwd: &std::path::Path| {
            assert!(
                std::process::Command::new("git")
                    .args(args)
                    .current_dir(cwd)
                    .status()
                    .unwrap()
                    .success()
            );
        };
        let (remote, local) = (dir.path().join("remote.git"), dir.path().join("local"));
        git(
            &[
                "init",
                "-q",
                "--bare",
                "-b",
                "main",
                remote.to_str().unwrap(),
            ],
            dir.path(),
        );
        git(
            &[
                "clone",
                "-q",
                remote.to_str().unwrap(),
                local.to_str().unwrap(),
            ],
            dir.path(),
        );
        // A fresh clone has never been fetched: no FETCH_HEAD yet.
        let fetch_head = local.join(".git/FETCH_HEAD");
        assert!(!fetch_head.exists());

        let mut workspace = None;
        let window = cx.add_window(|window, cx| {
            let ws = cx.new(|cx| Workspace::new(window, cx));
            workspace = Some(ws.clone());
            gpui_kit::component::Root::new(ws, window, cx)
        });
        let workspace = workspace.unwrap();
        let cx = &mut gpui_kit::VisualTestContext::from_window(window.into(), cx);
        workspace.update(cx, |ws, cx| {
            ws.open_repo_path(local.canonicalize().unwrap(), cx)
        });
        for _ in 0..300 {
            cx.run_until_parked();
            if fetch_head.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(fetch_head.exists(), "the repository was never fetched");
    }

    /// Two repositories restored at launch: switching to the one that wasn't active must
    /// load it (its branch name, its history), not leave it on the loading placeholder.
    #[gpui_kit::test]
    fn switching_to_a_restored_repository_loads_it(cx: &mut gpui_kit::TestAppContext) {
        use anano_core::git::RepoEntry;
        use gpui_kit::AppContext as _;
        cx.executor().allow_parking();
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::bridge::init(cx);
        });
        let dir = tempfile::tempdir().unwrap();
        let make = |name: &str, branch: &str| {
            let path = dir.path().join(name);
            std::fs::create_dir_all(&path).unwrap();
            let git = |args: &[&str]| {
                assert!(
                    std::process::Command::new("git")
                        .args(args)
                        .current_dir(&path)
                        .status()
                        .unwrap()
                        .success()
                );
            };
            git(&["init", "-q", "-b", branch]);
            git(&["config", "user.email", ""]);
            git(&["config", "user.name", "test"]);
            std::fs::write(path.join("a.txt"), "one\n").unwrap();
            git(&["add", "."]);
            git(&["commit", "-q", "-m", "init"]);
            RepoEntry::new(path.canonicalize().unwrap())
        };
        let loaded = anano_core::store::Loaded {
            repos: vec![make("first", "main"), make("second", "develop")],
            ..Default::default()
        };
        let mut workspace = None;
        let window = cx.add_window(|window, cx| {
            let ws = cx.new(|cx| Workspace::restore(loaded, window, cx));
            workspace = Some(ws.clone());
            gpui_kit::component::Root::new(ws, window, cx)
        });
        let workspace = workspace.unwrap();
        let cx = &mut gpui_kit::VisualTestContext::from_window(window.into(), cx);

        workspace.update(cx, |ws, cx| ws.activate(1, cx));
        let ready = |cx: &mut gpui_kit::VisualTestContext| {
            workspace.read_with(cx, |ws, _| {
                let repo = ws.active_repo().unwrap();
                repo.branch_label().as_deref() == Some("develop") && !repo.commits.is_empty()
            })
        };
        for _ in 0..300 {
            cx.run_until_parked();
            if ready(cx) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(ready(cx), "the repository switched to was never loaded");
    }

    /// A file row keeps its height however short the window gets: with more files than
    /// fit, the list scrolls instead of squeezing its rows.
    #[gpui_kit::test]
    fn file_rows_keep_their_height_in_a_short_window(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::AppContext as _;
        cx.executor().allow_parking();
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::bridge::init(cx);
        });
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().canonicalize().unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "-q", "-b", "main"])
                .current_dir(&repo)
                .status()
                .unwrap()
                .success()
        );
        for n in 0..40 {
            std::fs::write(repo.join(format!("file-{n:02}.txt")), "x\n").unwrap();
        }
        let mut workspace = None;
        let window = cx.add_window(|window, cx| {
            let ws = cx.new(|cx| Workspace::new(window, cx));
            workspace = Some(ws.clone());
            gpui_kit::component::Root::new(ws, window, cx)
        });
        let workspace = workspace.unwrap();
        let cx = &mut gpui_kit::VisualTestContext::from_window(window.into(), cx);
        workspace.update(cx, |ws, cx| ws.open_repo_path(repo, cx));
        for _ in 0..300 {
            cx.run_until_parked();
            if workspace.read_with(cx, |ws, _| {
                ws.active_repo().unwrap().changed_files().len() == 40
            }) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let row_height = |cx: &mut gpui_kit::VisualTestContext| {
            cx.run_until_parked();
            cx.debug_bounds("first-file-row")
                .expect("no file row drawn")
                .size
                .height
        };
        let tall = row_height(cx);
        cx.simulate_resize(gpui_kit::size(px(1120.), px(420.)));
        let short = row_height(cx);
        assert_eq!(tall, px(36.));
        assert_eq!(short, tall, "rows were squeezed when the window got short");
    }

    /// At the smallest window the app allows, every toolbar button is fully on screen,
    /// the account button at the far right included.
    #[gpui_kit::test]
    fn at_the_minimum_window_size_the_whole_toolbar_fits(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::AppContext as _;
        cx.executor().allow_parking();
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::bridge::init(cx);
        });
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().canonicalize().unwrap();
        let git = |args: &[&str]| {
            assert!(
                std::process::Command::new("git")
                    .args(args)
                    .current_dir(&repo)
                    .status()
                    .unwrap()
                    .success()
            );
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["remote", "add", "origin", "https://github.com/o/r.git"]);

        let mut workspace = None;
        let window = cx.add_window(|window, cx| {
            let ws = cx.new(|cx| Workspace::new(window, cx));
            workspace = Some(ws.clone());
            gpui_kit::component::Root::new(ws, window, cx)
        });
        let workspace = workspace.unwrap();
        let cx = &mut gpui_kit::VisualTestContext::from_window(window.into(), cx);
        workspace.update(cx, |ws, cx| {
            ws.accounts = vec![anano_core::external::Account {
                login: "a-fairly-long-login".into(),
                active: true,
            }];
            ws.open_repo_path(repo, cx);
        });
        for _ in 0..300 {
            cx.run_until_parked();
            if workspace.read_with(cx, |ws, _| {
                ws.active_repo().is_some_and(|r| r.origin.is_some())
            }) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        cx.simulate_resize(gpui_kit::size(px(crate::MIN_WIDTH), px(crate::MIN_HEIGHT)));
        cx.run_until_parked();

        for slot in [
            "toolbar-branch-slot",
            "toolbar-sync-slot",
            "toolbar-pulls-slot",
            "toolbar-workflows-slot",
            "toolbar-account-slot",
        ] {
            let bounds = cx
                .debug_bounds(slot)
                .unwrap_or_else(|| panic!("{slot} isn't on screen"));
            assert!(
                bounds.right() <= px(crate::MIN_WIDTH),
                "{slot} runs off the window: right edge {:?} > {}",
                bounds.right(),
                crate::MIN_WIDTH
            );
        }
    }
}
