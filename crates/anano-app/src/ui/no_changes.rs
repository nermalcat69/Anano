//! What the main pane shows for a repository with nothing to commit: a short explanation and
//! the next sensible things to do, most pressing first (publish the branch, push, pull, open
//! a pull request), then a few always-useful shortcuts into the repository.

use anano_core::external;
use gpui_kit::component::{ActiveTheme, button::ButtonVariants, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, FontWeight, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, div, px,
};

use crate::i18n::tr;
use crate::state::repos::OpenRepo;
use crate::state::workspace::Workspace;

/// The one thing worth suggesting right now, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Suggestion {
    /// The branch exists only here.
    Publish { branch: String },
    /// Commits here that origin doesn't have.
    Push { branch: String, ahead: usize },
    /// Commits on origin that aren't here.
    Pull { branch: String, behind: usize },
    /// Published and in sync, on a branch that isn't the default: propose it.
    PullRequest { branch: String },
}

/// Work out the suggestion from the branch's state. Pulling comes before pushing (you can't
/// push what you haven't merged), and a pull request is only offered on GitHub for a
/// branch that isn't the default one.
pub(crate) fn suggestion(repo: &OpenRepo) -> Option<Suggestion> {
    let origin = repo.origin.as_deref()?;
    let current = repo.current_branch()?;
    let branch = current.name.clone();
    if current.upstream.is_none() {
        return Some(Suggestion::Publish { branch });
    }
    match current.ahead_behind {
        Some((_, behind)) if behind > 0 => Some(Suggestion::Pull { branch, behind }),
        Some((ahead, _)) if ahead > 0 => Some(Suggestion::Push { branch, ahead }),
        _ if external::is_github(origin) && !matches!(branch.as_str(), "main" | "master") => {
            Some(Suggestion::PullRequest { branch })
        }
        _ => None,
    }
}

/// A suggestion card's title, body, button id, button label and what the button does.
type Card = (
    SharedString,
    SharedString,
    &'static str,
    SharedString,
    fn(&mut Workspace, &mut Context<Workspace>),
);

/// The keys of a shortcut as little keycaps: ⌘ ⇧ F on a Mac, Ctrl Shift F elsewhere.
fn keycaps(letter: &str, shift: bool, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let mut keys: Vec<String> = vec![
        if cfg!(target_os = "macos") {
            "⌘"
        } else {
            "Ctrl"
        }
        .into(),
    ];
    if shift {
        keys.push(
            if cfg!(target_os = "macos") {
                "⇧"
            } else {
                "Shift"
            }
            .into(),
        );
    }
    keys.push(letter.to_uppercase());
    h_flex()
        .gap_1()
        .items_center()
        .children(keys.into_iter().map(|key| {
            div()
                .px_1p5()
                .py_0p5()
                .rounded(px(4.))
                .border_1()
                .border_color(theme.border)
                .bg(theme.secondary)
                .text_xs()
                .font_family(theme.mono_font_family.clone())
                .text_color(theme.foreground)
                .child(key)
        }))
        .into_any_element()
}

/// "Shortcut ⌘ ⇧ F" under a row's title.
fn shortcut_line(letter: &str, shift: bool, cx: &App) -> AnyElement {
    h_flex()
        .gap_2()
        .items_center()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(tr!("repo.nochanges.shortcut"))
        .child(keycaps(letter, shift, cx))
        .into_any_element()
}

impl Workspace {
    /// The panel, or `None` when there are changes (or the repository is still being read),
    /// in which case the caller shows its usual placeholder.
    pub(crate) fn render_no_changes(
        &self,
        repo: &OpenRepo,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if repo.loading || repo.error.is_some() || !repo.changed_files().is_empty() {
            return None;
        }
        let theme = cx.theme();
        let (border, muted, accent) = (theme.border, theme.muted_foreground, theme.primary);
        let slug = repo.origin.as_deref().and_then(external::repo_slug);
        let github = repo.origin.as_deref().is_some_and(external::is_github);

        // ---- the highlighted suggestion
        let card = suggestion(repo).map(|suggestion| {
            let (title, body, button_id, button, run): Card = match &suggestion {
                Suggestion::Publish { branch } => (
                    tr!("repo.nochanges.publish_title"),
                    tr!("repo.nochanges.publish_body", branch = branch.clone()),
                    "suggest-publish",
                    tr!("repo.toolbar.publish"),
                    |ws, cx| ws.push(cx),
                ),
                Suggestion::Push { branch, ahead } => (
                    tr!("repo.nochanges.push_title"),
                    tr!(
                        "repo.nochanges.push_body",
                        count = ahead.to_string(),
                        branch = branch.clone()
                    ),
                    "suggest-push",
                    tr!("repo.toolbar.push"),
                    |ws, cx| ws.push(cx),
                ),
                Suggestion::Pull { branch, behind } => (
                    tr!("repo.nochanges.pull_title"),
                    tr!(
                        "repo.nochanges.pull_body",
                        count = behind.to_string(),
                        branch = branch.clone()
                    ),
                    "suggest-pull",
                    tr!("repo.toolbar.pull"),
                    |ws, cx| ws.pull(cx),
                ),
                Suggestion::PullRequest { branch } => (
                    tr!("repo.nochanges.pr_title"),
                    tr!("repo.nochanges.pr_body", branch = branch.clone()),
                    "suggest-pr",
                    tr!("repo.nochanges.pr_button"),
                    |ws, cx| ws.open_new_pull_request_page(cx),
                ),
            };
            let in_toolbar = !matches!(suggestion, Suggestion::PullRequest { .. });
            h_flex()
                .gap_6()
                .p_6()
                .items_center()
                .rounded(px(8.))
                .border_1()
                .border_color(accent.opacity(0.6))
                .bg(accent.opacity(0.12))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_1()
                        .child(
                            div()
                                .text_base()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(title),
                        )
                        .child(div().text_sm().child(body))
                        // Push / pull / publish are the toolbar's sync button and ⌘P.
                        .when(in_toolbar, |v| {
                            v.child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .mt_1()
                                    .text_sm()
                                    .text_color(muted)
                                    .child(tr!("repo.nochanges.toolbar_or"))
                                    .child(keycaps("p", false, cx)),
                            )
                        }),
                )
                .child(
                    crate::ui::button(button_id)
                        .primary()
                        .label(button)
                        .on_click(cx.listener(move |this, _, _, cx| run(this, cx))),
                )
                .into_any_element()
        });

        // ---- always-useful ways into the repository
        let row = |id: &'static str,
                   title: SharedString,
                   hint: Option<AnyElement>,
                   label: SharedString,
                   run: fn(&mut Workspace, &mut Context<Workspace>),
                   cx: &mut Context<Self>| {
            h_flex()
                .gap_6()
                .px_6()
                .py_5()
                .items_center()
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_1()
                        .child(
                            div()
                                .text_base()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(title),
                        )
                        .children(hint),
                )
                .child(
                    crate::ui::button(id)
                        .outline()
                        .label(label)
                        .on_click(cx.listener(move |this, _, _, cx| run(this, cx))),
                )
                .into_any_element()
        };
        let mut rows: Vec<AnyElement> = vec![row(
            "nochanges-reveal",
            if cfg!(target_os = "macos") {
                tr!("repo.nochanges.reveal_finder")
            } else {
                tr!("repo.nochanges.reveal_other")
            },
            Some(shortcut_line("f", true, cx)),
            if cfg!(target_os = "macos") {
                tr!("repo.nochanges.reveal_finder_button")
            } else {
                tr!("repo.nochanges.reveal_other_button")
            },
            |ws, cx| ws.reveal_active_repo(cx),
            cx,
        )];
        if github && slug.is_some() {
            rows.push(row(
                "nochanges-github",
                tr!("repo.nochanges.github"),
                Some(shortcut_line("g", true, cx)),
                tr!("repo.history.view_on_github"),
                |ws, cx| ws.open_active_on_github(cx),
                cx,
            ));
        }
        if self.nermal_available {
            rows.push(row(
                "nochanges-nermal",
                tr!("repo.nochanges.nermal"),
                None,
                tr!("repo.open_in_nermal"),
                |ws, cx| ws.open_in_nermal(cx),
                cx,
            ));
        }
        let last = rows.len() - 1;
        let list = v_flex()
            .rounded(px(8.))
            .border_1()
            .border_color(border)
            .overflow_hidden()
            .children(rows.into_iter().enumerate().map(|(ix, row)| {
                div()
                    .when(ix != last, |d| d.border_b_1().border_color(border))
                    .child(row)
            }));

        Some(
            v_flex()
                .id("no-local-changes")
                .debug_selector(|| "no-local-changes".into())
                .size_full()
                .overflow_y_scroll()
                .child(
                    v_flex()
                        .w_full()
                        .max_w(px(900.))
                        .p_8()
                        .gap_6()
                        .child(
                            v_flex()
                                .gap_2()
                                .child(div().text_3xl().child(tr!("repo.nochanges.title")))
                                .child(
                                    div()
                                        .text_base()
                                        .text_color(muted)
                                        .child(tr!("repo.nochanges.body")),
                                ),
                        )
                        .children(card)
                        .child(list),
                )
                .into_any_element(),
        )
    }

    /// Show the repository's folder in Finder / the file manager.
    pub fn reveal_active_repo(&mut self, cx: &mut Context<Self>) {
        if let Some(repo) = self.active_repo() {
            cx.reveal_path(&repo.entry.path);
        }
    }

    /// Open the repository's page on GitHub in the browser.
    pub fn open_active_on_github(&mut self, cx: &mut Context<Self>) {
        let slug = self
            .active_repo()
            .and_then(|r| r.origin.as_deref())
            .filter(|origin| external::is_github(origin))
            .and_then(external::repo_slug);
        if let Some(slug) = slug {
            cx.open_url(&format!("https://github.com/{slug}"));
        }
    }

    /// GitHub's "open a pull request" page for the current branch.
    pub fn open_new_pull_request_page(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.active_repo() else {
            return;
        };
        let slug = repo.origin.as_deref().and_then(external::repo_slug);
        if let (Some(slug), Some(branch)) = (slug, repo.current_branch()) {
            cx.open_url(&format!(
                "https://github.com/{slug}/pull/new/{}",
                branch.name
            ));
        }
    }

    /// ⌘P: do whatever the toolbar's sync button says (fetch, pull, push or publish).
    pub fn sync_now(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.active_repo() else {
            return;
        };
        match crate::ui::toolbar::next_sync(repo.current_branch()).0 {
            crate::state::repos::SyncKind::Fetch => self.fetch(cx),
            crate::state::repos::SyncKind::Pull => self.pull(cx),
            crate::state::repos::SyncKind::Push => self.push(cx),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anano_core::git::{Branch, RepoEntry};

    fn repo(origin: Option<&str>, branch: Branch) -> OpenRepo {
        let mut repo = OpenRepo::new(RepoEntry::new("/tmp/anano-nochanges".into()));
        repo.origin = origin.map(str::to_string);
        repo.branches = vec![branch];
        repo
    }

    fn branch(name: &str, upstream: bool, ahead_behind: Option<(usize, usize)>) -> Branch {
        Branch {
            name: name.into(),
            is_head: true,
            is_remote: false,
            upstream: upstream.then(|| format!("origin/{name}")),
            ahead_behind,
            last_commit_time: None,
        }
    }

    #[test]
    fn the_most_pressing_suggestion_wins() {
        let gh = Some("https://github.com/o/r.git");
        let pick = |origin, b| suggestion(&repo(origin, b));
        // Never published.
        assert_eq!(
            pick(gh, branch("test", false, None)),
            Some(Suggestion::Publish {
                branch: "test".into()
            })
        );
        // Behind beats ahead: pull before you push.
        assert_eq!(
            pick(gh, branch("test", true, Some((2, 3)))),
            Some(Suggestion::Pull {
                branch: "test".into(),
                behind: 3
            })
        );
        assert_eq!(
            pick(gh, branch("test", true, Some((2, 0)))),
            Some(Suggestion::Push {
                branch: "test".into(),
                ahead: 2
            })
        );
        // In sync: a pull request for a topic branch, nothing for the default one.
        assert_eq!(
            pick(gh, branch("feature", true, Some((0, 0)))),
            Some(Suggestion::PullRequest {
                branch: "feature".into()
            })
        );
        assert_eq!(pick(gh, branch("main", true, Some((0, 0)))), None);
        // Not on GitHub there is no pull request page; with no remote, nothing at all.
        assert_eq!(
            pick(
                Some("https://gitlab.com/o/r.git"),
                branch("feature", true, Some((0, 0)))
            ),
            None
        );
        assert_eq!(pick(None, branch("test", false, None)), None);
    }

    /// A repository with nothing to commit shows the panel (and one with changes doesn't).
    #[gpui_kit::test]
    fn a_clean_repository_shows_the_no_local_changes_panel(cx: &mut gpui_kit::TestAppContext) {
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
        git(&["config", "user.email", "a@b.c"]);
        git(&["config", "user.name", "test"]);
        git(&["remote", "add", "origin", "https://github.com/o/r.git"]);
        std::fs::write(repo.join("a.txt"), "one\n").unwrap();
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
        workspace.update(cx, |ws, cx| ws.open_repo_path(repo.clone(), cx));
        let loaded = |cx: &mut gpui_kit::VisualTestContext| {
            workspace.read_with(cx, |ws, _| {
                ws.active_repo()
                    .is_some_and(|r| !r.loading && r.origin.is_some() && !r.branches.is_empty())
            })
        };
        for _ in 0..300 {
            cx.run_until_parked();
            if loaded(cx) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(loaded(cx), "repository never finished loading");
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(
            cx.debug_bounds("no-local-changes").is_some(),
            "no panel for a clean repository"
        );

        // An edit takes the panel away again: the file's diff is what shows.
        std::fs::write(repo.join("a.txt"), "two\n").unwrap();
        workspace.update(cx, |ws, cx| ws.refresh_active_status(cx));
        for _ in 0..300 {
            cx.run_until_parked();
            if workspace.read_with(cx, |ws, _| {
                ws.active_repo()
                    .is_some_and(|r| !r.changed_files().is_empty())
            }) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(
            cx.debug_bounds("no-local-changes").is_none(),
            "panel still showing over changes"
        );
    }
}
