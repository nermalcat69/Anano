//! The permanent toolbar above the repository tabs, after GitHub Desktop's: one big
//! "Current Branch" button (click to switch) and one big sync button that names the next
//! sensible step — Fetch, Pull when the branch is behind, Push when it is ahead, Publish
//! when it has no upstream yet — with when the repository was last fetched underneath.

use std::time::SystemTime;

use anano_core::git::Branch;
use gpui_kit::component::{
    ActiveTheme, Icon, IconName,
    button::{Button, ButtonVariants},
    h_flex,
    input::{Input, InputState},
    menu::{PopupMenu, PopupMenuItem},
    popover::Popover,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, Context, Entity, FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, div, px,
};

use crate::assets::{
    ICON_GIT_BRANCH, ICON_PULL_REQUEST, ICON_REFRESH, ICON_TRASH, ICON_UPLOAD_CLOUD,
};
use crate::i18n::tr;
use crate::state::repos::SyncKind;
use crate::state::workspace::Workspace;

const TOOLBAR_HEIGHT: f32 = 52.;
const BUTTON_WIDTH: f32 = 260.;
/// The branch panel is wider than its button: names plus "2 months ago" need the room.
const PANEL_WIDTH: f32 = 380.;

/// Branch entries shared by the toolbar button and the sidebar's branch row: one item per
/// local branch (the checked one is current), each other branch followed by its delete.
pub(crate) fn branch_menu(
    menu: PopupMenu,
    branches: &[Branch],
    workspace: &Entity<Workspace>,
) -> PopupMenu {
    branches.iter().fold(menu, |menu, branch| {
        let is_head = branch.is_head;
        let (switch_ws, delete_ws) = (workspace.clone(), workspace.clone());
        let (switch_to, delete) = (branch.name.clone(), branch.name.clone());
        let menu = menu.item(
            PopupMenuItem::new(branch.name.clone())
                .checked(is_head)
                .on_click(move |_, window, cx| {
                    if !is_head {
                        switch_ws.update(cx, |ws, cx| {
                            ws.request_switch_branch(switch_to.clone(), window, cx)
                        });
                    }
                }),
        );
        if is_head {
            return menu;
        }
        menu.item(
            PopupMenuItem::new(tr!("repo.branch.delete", name = branch.name.clone()))
                .icon(Icon::empty().path(ICON_TRASH))
                .on_click(move |_, _, cx| {
                    delete_ws.update(cx, |ws, cx| ws.delete_branch(delete.clone(), cx));
                }),
        )
    })
}

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
        h_flex()
            .id(("branch-row", ix))
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
fn next_sync(branch: Option<&Branch>) -> (SyncKind, SharedString) {
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
fn big_button(id: &'static str) -> Button {
    crate::ui::button(id)
        .ghost()
        // The slot minus its 1px divider, the toolbar minus its 1px bottom border.
        .w(px(BUTTON_WIDTH - 1.))
        .h(px(TOOLBAR_HEIGHT - 1.))
        .rounded_none()
}

/// One fixed-width cell of the toolbar.
fn slot(selector: &'static str, border: Hsla, button: impl IntoElement) -> impl IntoElement {
    div()
        .debug_selector(move || selector.into())
        .flex_none()
        .h_full()
        .w(px(BUTTON_WIDTH))
        .border_r_1()
        .border_color(border)
        .child(button)
}

/// Icon / two-line label / trailing bits, laid out identically in both buttons.
fn content(selector: &'static str) -> gpui_kit::Div {
    h_flex()
        .debug_selector(move || selector.into())
        .size_full()
        .px_3()
        .gap_3()
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
                    border,
                    Popover::new("toolbar-branch-popover")
                        .open(open)
                        .on_open_change(cx.listener(|this, open: &bool, _, cx| {
                            this.branch_popover_open = *open;
                            cx.notify();
                        }))
                        .trigger_style(gpui_kit::StyleRefinement::default())
                        .trigger(
                            big_button("toolbar-branch").child(
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
                    border,
                    big_button("toolbar-sync")
                        .loading(syncing)
                        .child(
                            content("toolbar-sync-content")
                                .when(!syncing, |h| {
                                    h.child(Icon::empty().path(sync_icon).size_5())
                                })
                                .child(big_label(title, subtitle, true, muted))
                                .children(counts.map(|counts| {
                                    div().flex_none().text_xs().text_color(muted).child(counts)
                                })),
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
                        border,
                        big_button("toolbar-pulls")
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
                }),
        )
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
        assert_eq!(branch_slot.size, sync_slot.size);
        assert_eq!(
            branch.size, sync.size,
            "the two buttons' contents differ in size"
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
}
