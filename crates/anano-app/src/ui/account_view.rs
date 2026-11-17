//! The account view: what a connected GitHub account has, without anything being cloned.
//! Its repositories, the organizations it belongs to and each organization's repositories,
//! each repository with buttons to clone it (or open it, if it is already here), read its
//! pull requests, and browse its commit history with the changes of every commit.
//! Reached from the account button at the right of the toolbar. Read-only.

use std::collections::HashMap;
use std::rc::Rc;

use anano_core::browse::{Org, PAGE, RemoteCommit, RemoteRepo};
use anano_core::git::PatchFile;
use gpui_kit::component::{
    ActiveTheme, Disableable, Icon, IconName, Sizable, button::ButtonVariants, h_flex,
    input::Input, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, ClipboardItem, Context, FontWeight, Hsla, InteractiveElement, IntoElement,
    ParentElement, PathPromptOptions, ScrollStrategy, SharedString, StatefulInteractiveElement,
    Styled, div, px, uniform_list,
};

use crate::assets::ICON_REFRESH;
use crate::i18n::tr;
use crate::state::workspace::Workspace;
use crate::ui::status_pane::diff_row;

const COMMITS_WIDTH: f32 = 340.;
const FILES_WIDTH: f32 = 230.;

/// Which page of the account view is showing.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum AccountPage {
    /// The account's own repositories.
    #[default]
    Repos,
    /// The organizations it belongs to.
    Orgs,
    /// One organization's repositories.
    OrgRepos(String),
}

/// One list of repositories as it loads. `None` in `items` means "loading".
#[derive(Default)]
pub struct RepoList {
    pub items: Option<Result<Vec<RemoteRepo>, String>>,
    /// A page came back short: there is nothing after what is loaded.
    pub exhausted: bool,
    pub loading: bool,
    pub loading_more: bool,
    pub more_error: Option<String>,
}

/// A repository's commit history, and the commit being read.
pub struct CommitBrowser {
    /// `owner/name`
    pub repo: String,
    pub commits: Option<Result<Vec<RemoteCommit>, String>>,
    pub exhausted: bool,
    pub loading_more: bool,
    pub open: Option<String>,
    pub files: Option<Result<Rc<Vec<PatchFile>>, String>>,
    pub file: usize,
}

#[derive(Default)]
pub struct AccountState {
    /// The connected account everything is read as.
    pub login: String,
    pub page: AccountPage,
    /// "" is the account's own repositories; otherwise an organization's login.
    pub lists: HashMap<String, RepoList>,
    pub orgs: Option<Result<Vec<Org>, String>>,
    pub commits: Option<CommitBrowser>,
}

impl AccountState {
    fn scope(&self) -> String {
        match &self.page {
            AccountPage::OrgRepos(org) => org.clone(),
            _ => String::new(),
        }
    }
}

/// Does a repository match what was typed in the filter box (name or description)?
pub(crate) fn matches_filter(repo: &RemoteRepo, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    query.is_empty()
        || repo.full_name.to_lowercase().contains(&query)
        || repo.description.to_lowercase().contains(&query)
}

fn note(text: SharedString, color: Hsla) -> AnyElement {
    div()
        .p_4()
        .text_sm()
        .text_color(color)
        .child(text)
        .into_any_element()
}

fn pill(text: SharedString, color: Hsla) -> impl IntoElement {
    div()
        .flex_none()
        .px_1p5()
        .py_0p5()
        .rounded(px(999.))
        .border_1()
        .border_color(color.opacity(0.5))
        .text_xs()
        .text_color(color)
        .child(text)
}

impl Workspace {
    // ------------------------------------------------------------------ opening

    /// The account button's "View repositories" / "View organizations".
    pub fn open_account_view(&mut self, login: String, page: AccountPage, cx: &mut Context<Self>) {
        self.pulls = None;
        self.workflows = None;
        if self.account.as_ref().is_none_or(|a| a.login != login) {
            self.account = Some(AccountState {
                login: login.clone(),
                ..Default::default()
            });
        }
        self.load_logins(vec![login], cx);
        self.show_account_page(page, cx);
    }

    pub fn close_account_view(&mut self, cx: &mut Context<Self>) {
        self.account = None;
        cx.notify();
    }

    /// "Switch account" in the account menu: this repository now uses that account, and an
    /// open account view follows it.
    pub fn switch_account(&mut self, login: String, cx: &mut Context<Self>) {
        self.set_repo_account(login.clone(), cx);
        if let Some(account) = &mut self.account
            && account.login != login
        {
            let page = match account.page {
                AccountPage::OrgRepos(_) => AccountPage::Orgs,
                ref other => other.clone(),
            };
            *account = AccountState {
                login: login.clone(),
                ..Default::default()
            };
            self.load_logins(vec![login], cx);
            self.show_account_page(page, cx);
        }
    }

    pub fn show_account_page(&mut self, page: AccountPage, cx: &mut Context<Self>) {
        let Some(account) = &mut self.account else {
            return;
        };
        account.page = page.clone();
        account.commits = None;
        match page {
            AccountPage::Repos => self.load_repo_list(String::new(), cx),
            AccountPage::Orgs => self.load_orgs(cx),
            AccountPage::OrgRepos(org) => self.load_repo_list(org, cx),
        }
        cx.notify();
    }

    /// The header's refresh: forget the page showing and read it again.
    pub fn refresh_account_view(&mut self, cx: &mut Context<Self>) {
        let Some(account) = &mut self.account else {
            return;
        };
        match account.page.clone() {
            AccountPage::Orgs => account.orgs = None,
            _ => {
                let scope = account.scope();
                account.lists.remove(&scope);
            }
        }
        let page = account.page.clone();
        self.show_account_page(page, cx);
    }

    // ------------------------------------------------------------------ repositories

    fn load_repo_list(&mut self, scope: String, cx: &mut Context<Self>) {
        let Some(account) = &mut self.account else {
            return;
        };
        let list = account.lists.entry(scope.clone()).or_default();
        if list.loading || matches!(list.items, Some(Ok(_))) {
            return;
        }
        list.loading = true;
        let login = account.login.clone();
        let task = if scope.is_empty() {
            crate::bridge::account_repos_page(cx, login.clone(), 1)
        } else {
            crate::bridge::org_repos_page(cx, login.clone(), scope.clone(), 1)
        };
        cx.spawn(async move |ws, cx| {
            let result = task.await.map_err(|e| e.to_string());
            let _ = ws.update(cx, |ws, cx| {
                let Some(account) = &mut ws.account else {
                    return;
                };
                if account.login != login {
                    return;
                }
                let list = account.lists.entry(scope).or_default();
                list.loading = false;
                let mut owners: Vec<String> = Vec::new();
                if let Ok(items) = &result {
                    list.exhausted = items.len() < PAGE;
                    owners = items.iter().map(|r| r.owner.clone()).collect();
                }
                list.items = Some(result);
                ws.load_logins(owners, cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// "Load more" at the end of a repository list.
    pub fn load_more_repos(&mut self, cx: &mut Context<Self>) {
        let Some(account) = &mut self.account else {
            return;
        };
        let scope = account.scope();
        let login = account.login.clone();
        let Some(list) = account.lists.get_mut(&scope) else {
            return;
        };
        let Some(Ok(items)) = &list.items else {
            return;
        };
        if list.loading_more || list.exhausted {
            return;
        }
        let page = items.len().div_ceil(PAGE) + 1;
        list.loading_more = true;
        list.more_error = None;
        let task = if scope.is_empty() {
            crate::bridge::account_repos_page(cx, login.clone(), page)
        } else {
            crate::bridge::org_repos_page(cx, login.clone(), scope.clone(), page)
        };
        cx.notify();
        cx.spawn(async move |ws, cx| {
            let result = task.await.map_err(|e| e.to_string());
            let _ = ws.update(cx, |ws, cx| {
                let Some(account) = &mut ws.account else {
                    return;
                };
                if account.login != login {
                    return;
                }
                let list = account.lists.entry(scope).or_default();
                list.loading_more = false;
                match result {
                    Ok(more) => {
                        list.exhausted = more.len() < PAGE;
                        if let Some(Ok(items)) = &mut list.items {
                            for repo in more {
                                if !items.iter().any(|r| r.full_name == repo.full_name) {
                                    items.push(repo);
                                }
                            }
                        }
                    }
                    Err(err) => list.more_error = Some(err),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn load_orgs(&mut self, cx: &mut Context<Self>) {
        let Some(account) = &self.account else {
            return;
        };
        if account.orgs.is_some() {
            return;
        }
        let login = account.login.clone();
        let task = crate::bridge::account_orgs(cx, login.clone());
        cx.spawn(async move |ws, cx| {
            let result = task.await.map_err(|e| e.to_string());
            let _ = ws.update(cx, |ws, cx| {
                let Some(account) = &mut ws.account else {
                    return;
                };
                if account.login != login {
                    return;
                }
                let logins = match &result {
                    Ok(orgs) => orgs.iter().map(|o| o.login.clone()).collect(),
                    Err(_) => Vec::new(),
                };
                account.orgs = Some(result);
                ws.load_logins(logins, cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Where a repository of the list is already on this computer, if it is: the index of
    /// the open repository whose origin is it.
    fn local_repo(&self, full_name: &str) -> Option<usize> {
        self.find_repo_by_slug(full_name)
    }

    /// The URL a repository of the list is cloned from or read through: pinned to the
    /// account the list was read as, so the right token is used.
    fn remote_url(&self, full_name: &str) -> Option<String> {
        let account = self.account.as_ref()?;
        Some(anano_core::external::with_account(
            &format!("https://github.com/{full_name}.git"),
            &account.login,
        ))
    }

    /// "Clone": pick a folder, then clone there as the account.
    pub fn clone_remote_repo(&mut self, full_name: String, cx: &mut Context<Self>) {
        let Some(url) = self.remote_url(&full_name) else {
            return;
        };
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(tr!("repo.clone.destination")),
        });
        cx.spawn(async move |ws, cx| {
            let Ok(Ok(Some(mut paths))) = receiver.await else {
                return;
            };
            let Some(parent) = paths.pop() else { return };
            let _ = ws.update(cx, |ws, cx| {
                // Back to what is being cloned: the view gives way to its progress.
                ws.account = None;
                ws.clone_into(url, parent, cx)
            });
        })
        .detach();
    }

    /// "Open": the repository is already here; switch to it.
    pub fn open_cloned_repo(&mut self, full_name: &str, cx: &mut Context<Self>) {
        if let Some(ix) = self.local_repo(full_name) {
            self.activate(ix, cx);
            self.account = None;
            cx.notify();
        }
    }

    /// "Pull requests": the Pull Requests view for that repository.
    pub fn view_remote_pull_requests(&mut self, full_name: &str, cx: &mut Context<Self>) {
        if let Some(url) = self.remote_url(full_name) {
            self.open_pull_requests_for(url, cx);
        }
    }

    // ------------------------------------------------------------------ commit history

    /// "Commits": that repository's history, newest first; the newest commit opens.
    pub fn open_repo_commits(&mut self, full_name: String, cx: &mut Context<Self>) {
        let Some(account) = &mut self.account else {
            return;
        };
        let login = account.login.clone();
        account.commits = Some(CommitBrowser {
            repo: full_name.clone(),
            commits: None,
            exhausted: false,
            loading_more: false,
            open: None,
            files: None,
            file: 0,
        });
        let task = crate::bridge::commits_page(cx, login.clone(), full_name.clone(), 1);
        cx.notify();
        cx.spawn(async move |ws, cx| {
            let result = task.await.map_err(|e| e.to_string());
            let _ = ws.update(cx, |ws, cx| {
                let Some(account) = &mut ws.account else {
                    return;
                };
                let Some(browser) = &mut account.commits else {
                    return;
                };
                if account.login != login || browser.repo != full_name {
                    return;
                }
                let first = match &result {
                    Ok(list) => {
                        browser.exhausted = list.len() < PAGE;
                        list.first().map(|c| c.sha.clone())
                    }
                    Err(_) => None,
                };
                let logins: Vec<String> = match &result {
                    Ok(list) => list.iter().map(|c| c.login.clone()).collect(),
                    Err(_) => Vec::new(),
                };
                browser.commits = Some(result);
                ws.load_logins(logins, cx);
                if let Some(sha) = first {
                    ws.open_remote_commit(sha, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Back from a repository's history to the list it came from.
    pub fn close_repo_commits(&mut self, cx: &mut Context<Self>) {
        if let Some(account) = &mut self.account {
            account.commits = None;
        }
        cx.notify();
    }

    pub fn load_more_commits(&mut self, cx: &mut Context<Self>) {
        let Some(account) = &mut self.account else {
            return;
        };
        let login = account.login.clone();
        let Some(browser) = &mut account.commits else {
            return;
        };
        let Some(Ok(list)) = &browser.commits else {
            return;
        };
        if browser.loading_more || browser.exhausted {
            return;
        }
        let repo = browser.repo.clone();
        let page = list.len().div_ceil(PAGE) + 1;
        browser.loading_more = true;
        let task = crate::bridge::commits_page(cx, login.clone(), repo.clone(), page);
        cx.notify();
        cx.spawn(async move |ws, cx| {
            let result = task.await.map_err(|e| e.to_string());
            let _ = ws.update(cx, |ws, cx| {
                let Some(account) = &mut ws.account else {
                    return;
                };
                let Some(browser) = &mut account.commits else {
                    return;
                };
                if account.login != login || browser.repo != repo {
                    return;
                }
                browser.loading_more = false;
                let mut logins = Vec::new();
                if let Ok(more) = result {
                    browser.exhausted = more.len() < PAGE;
                    if let Some(Ok(list)) = &mut browser.commits {
                        for commit in more {
                            if !list.iter().any(|c| c.sha == commit.sha) {
                                logins.push(commit.login.clone());
                                list.push(commit);
                            }
                        }
                    }
                }
                ws.load_logins(logins, cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Read one commit: its message, and the changes it made.
    pub fn open_remote_commit(&mut self, sha: String, cx: &mut Context<Self>) {
        let Some(account) = &mut self.account else {
            return;
        };
        let login = account.login.clone();
        let Some(browser) = &mut account.commits else {
            return;
        };
        browser.open = Some(sha.clone());
        browser.files = None;
        browser.file = 0;
        self.commit_scroll.scroll_to_item(0, ScrollStrategy::Top);
        let repo = browser.repo.clone();
        let task = crate::bridge::remote_commit_files(cx, login.clone(), repo.clone(), sha.clone());
        cx.notify();
        cx.spawn(async move |ws, cx| {
            let result = task.await.map(Rc::new).map_err(|e| e.to_string());
            let _ = ws.update(cx, |ws, cx| {
                let Some(account) = &mut ws.account else {
                    return;
                };
                let Some(browser) = &mut account.commits else {
                    return;
                };
                // Another commit may have been clicked while this loaded.
                if account.login == login
                    && browser.repo == repo
                    && browser.open.as_deref() == Some(sha.as_str())
                {
                    browser.files = Some(result);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub fn select_commit_file(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(browser) = self.account.as_mut().and_then(|a| a.commits.as_mut()) {
            browser.file = ix;
        }
        self.commit_scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    // ------------------------------------------------------------------ drawing

    fn picture(&self, login: &str) -> Option<&std::path::PathBuf> {
        self.account_avatars.get(login).and_then(|p| p.as_ref())
    }

    fn repo_row(&self, ix: usize, repo: &RemoteRepo, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, border) = (theme.muted_foreground, theme.border);
        let local = self.local_repo(&repo.full_name).is_some();
        let (clone_name, open_name, prs_name, commits_name) = (
            repo.full_name.clone(),
            repo.full_name.clone(),
            repo.full_name.clone(),
            repo.full_name.clone(),
        );
        let meta = {
            let mut parts: Vec<String> = Vec::new();
            if !repo.language.is_empty() {
                parts.push(repo.language.clone());
            }
            if repo.stars > 0 {
                parts.push(format!("★ {}", repo.stars));
            }
            if repo.pushed_at > 0 {
                parts.push(
                    tr!(
                        "repo.account.updated",
                        when = crate::ui::time_ago(repo.pushed_at)
                    )
                    .to_string(),
                );
            }
            parts.join(" • ")
        };
        h_flex()
            .id(("account-repo", ix))
            .flex_none()
            .gap_4()
            .px_4()
            .py_3()
            .items_center()
            .border_b_1()
            .border_color(border)
            // The owner's logo (an organization's, or the account's own).
            .child(crate::ui::avatar(
                self.picture(&repo.owner),
                &repo.owner,
                px(28.),
                cx,
            ))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(repo.full_name.clone()),
                            )
                            .child(pill(
                                if repo.private {
                                    tr!("repo.account.private")
                                } else {
                                    tr!("repo.account.public")
                                },
                                muted,
                            ))
                            .when(repo.fork, |h| {
                                h.child(pill(tr!("repo.account.fork"), muted))
                            })
                            .when(repo.archived, |h| {
                                h.child(pill(tr!("repo.account.archived"), theme.warning))
                            }),
                    )
                    .when(!repo.description.is_empty(), |v| {
                        v.child(
                            div()
                                .truncate()
                                .text_sm()
                                .text_color(muted)
                                .child(repo.description.clone()),
                        )
                    })
                    .child(div().truncate().text_xs().text_color(muted).child(meta)),
            )
            .child(
                h_flex()
                    .flex_none()
                    .gap_2()
                    .child(
                        crate::ui::button(("repo-commits", ix))
                            .outline()
                            .small()
                            .label(tr!("repo.account.commits"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.open_repo_commits(commits_name.clone(), cx)
                            })),
                    )
                    .child(
                        crate::ui::button(("repo-prs", ix))
                            .outline()
                            .small()
                            .label(tr!("repo.account.prs"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.view_remote_pull_requests(&prs_name, cx)
                            })),
                    )
                    .child(if local {
                        crate::ui::button(("repo-open", ix))
                            .outline()
                            .small()
                            .label(tr!("repo.account.open"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.open_cloned_repo(&open_name, cx)
                            }))
                    } else {
                        crate::ui::button(("repo-clone", ix))
                            .primary()
                            .small()
                            .label(tr!("repo.account.clone"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.clone_remote_repo(clone_name.clone(), cx)
                            }))
                    }),
            )
            .into_any_element()
    }

    fn render_repo_list(&self, account: &AccountState, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, danger) = (theme.muted_foreground, theme.danger);
        let scope = account.scope();
        let Some(list) = account.lists.get(&scope) else {
            return crate::ui::skeleton_list(8);
        };
        let query = self.account_search_input.read(cx).value().to_string();
        match &list.items {
            None => crate::ui::skeleton_list(8),
            Some(Err(err)) => note(err.clone().into(), danger),
            Some(Ok(items)) => {
                let shown: Vec<&RemoteRepo> =
                    items.iter().filter(|r| matches_filter(r, &query)).collect();
                let footer = (!list.exhausted).then(|| {
                    v_flex()
                        .flex_none()
                        .p_3()
                        .gap_2()
                        .children(
                            list.more_error
                                .clone()
                                .map(|e| div().text_xs().text_color(danger).child(e)),
                        )
                        .child(
                            crate::ui::button("repos-load-more")
                                .outline()
                                .small()
                                .w_full()
                                .loading(list.loading_more)
                                .disabled(list.loading_more)
                                .label(tr!("repo.pulls.load_more"))
                                .on_click(cx.listener(|this, _, _, cx| this.load_more_repos(cx))),
                        )
                });
                v_flex()
                    .id("account-repos")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .when(shown.is_empty(), |v| {
                        v.child(note(
                            if query.trim().is_empty() {
                                tr!("repo.account.no_repos")
                            } else {
                                tr!("repo.account.no_match")
                            },
                            muted,
                        ))
                    })
                    .children(
                        shown
                            .into_iter()
                            .enumerate()
                            .map(|(ix, repo)| self.repo_row(ix, repo, cx)),
                    )
                    .children(footer)
                    .into_any_element()
            }
        }
    }

    fn render_org_list(&self, account: &AccountState, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, danger, border, hover) = (
            theme.muted_foreground,
            theme.danger,
            theme.border,
            theme.list_hover,
        );
        match &account.orgs {
            None => crate::ui::skeleton_list(8),
            Some(Err(err)) => note(err.clone().into(), danger),
            Some(Ok(orgs)) if orgs.is_empty() => note(tr!("repo.account.no_orgs"), muted),
            Some(Ok(orgs)) => v_flex()
                .id("account-orgs")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .children(orgs.iter().enumerate().map(|(ix, org)| {
                    let login = org.login.clone();
                    h_flex()
                        .id(("account-org", ix))
                        .flex_none()
                        .gap_3()
                        .px_4()
                        .py_3()
                        .items_center()
                        .border_b_1()
                        .border_color(border)
                        .cursor_pointer()
                        .hover(move |style| style.bg(hover))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.show_account_page(AccountPage::OrgRepos(login.clone()), cx)
                        }))
                        .child(crate::ui::avatar(
                            self.picture(&org.login),
                            &org.login,
                            px(32.),
                            cx,
                        ))
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .gap_0p5()
                                .child(
                                    div()
                                        .truncate()
                                        .text_sm()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(org.login.clone()),
                                )
                                .when(!org.description.is_empty(), |v| {
                                    v.child(
                                        div()
                                            .truncate()
                                            .text_sm()
                                            .text_color(muted)
                                            .child(org.description.clone()),
                                    )
                                }),
                        )
                        .child(Icon::new(IconName::ChevronRight).size_4().text_color(muted))
                }))
                .into_any_element(),
        }
    }

    fn render_commit_browser(
        &self,
        account: &AccountState,
        browser: &CommitBrowser,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let (muted, danger, border, hover, active) = (
            theme.muted_foreground,
            theme.danger,
            theme.border,
            theme.list_hover,
            theme.list_active,
        );
        let _ = account;
        // ---- the history
        let history = match &browser.commits {
            None => crate::ui::skeleton_list(8),
            Some(Err(err)) => note(err.clone().into(), danger),
            Some(Ok(list)) if list.is_empty() => note(tr!("repo.account.no_commits"), muted),
            Some(Ok(list)) => {
                let footer = (!browser.exhausted).then(|| {
                    div().flex_none().p_3().child(
                        crate::ui::button("commits-load-more")
                            .outline()
                            .small()
                            .w_full()
                            .loading(browser.loading_more)
                            .disabled(browser.loading_more)
                            .label(tr!("repo.pulls.load_more"))
                            .on_click(cx.listener(|this, _, _, cx| this.load_more_commits(cx))),
                    )
                });
                v_flex()
                    .id("remote-commits")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(list.iter().map(|commit| {
                        let sha = commit.sha.clone();
                        let selected = browser.open.as_deref() == Some(commit.sha.as_str());
                        let subject = commit.message.lines().next().unwrap_or("").to_string();
                        v_flex()
                            .id(SharedString::from(format!("rc-{}", commit.sha)))
                            .flex_none()
                            .px_3()
                            .py_2()
                            .gap_1()
                            .border_b_1()
                            .border_color(border)
                            .cursor_pointer()
                            .when(selected, |row| row.bg(active))
                            .hover(move |style| style.bg(hover))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.open_remote_commit(sha.clone(), cx)
                            }))
                            .child(
                                div()
                                    .truncate()
                                    .text_sm()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(subject),
                            )
                            .child(
                                h_flex()
                                    .gap_1p5()
                                    .items_center()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(crate::ui::avatar(
                                        self.picture(&commit.login),
                                        &commit.author,
                                        px(14.),
                                        cx,
                                    ))
                                    .child(div().min_w_0().truncate().child(format!(
                                        "{} • {}",
                                        commit.author,
                                        crate::ui::time_ago(commit.at)
                                    ))),
                            )
                    }))
                    .children(footer)
                    .into_any_element()
            }
        };

        // ---- the commit being read
        let open = match (&browser.open, &browser.commits) {
            (Some(sha), Some(Ok(list))) => list.iter().find(|c| &c.sha == sha),
            _ => None,
        };
        let detail: AnyElement = match open {
            None => note(tr!("repo.account.pick_commit"), muted),
            Some(commit) => {
                let (subject, body) = match commit.message.split_once('\n') {
                    Some((s, b)) => (s.to_string(), b.trim().to_string()),
                    None => (commit.message.clone(), String::new()),
                };
                let sha = commit.sha.clone();
                let short: String = commit.sha.chars().take(7).collect();
                let header = v_flex()
                    .flex_none()
                    .gap_1p5()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(border)
                    .child(
                        div()
                            .text_base()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(crate::ui::selectable("rc-subject", subject)),
                    )
                    .when(!body.is_empty(), |v| {
                        v.child(
                            div()
                                .id("rc-body")
                                .max_h(px(96.))
                                .overflow_y_scroll()
                                .text_sm()
                                .text_color(muted)
                                .child(crate::ui::selectable("rc-body-text", body)),
                        )
                    })
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .text_xs()
                            .text_color(muted)
                            .child(crate::ui::avatar(
                                self.picture(&commit.login),
                                &commit.author,
                                px(18.),
                                cx,
                            ))
                            .child(commit.author.clone())
                            .child(crate::ui::time_ago(commit.at))
                            .child(
                                div()
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .child(short),
                            )
                            .child(
                                crate::ui::button("rc-copy-sha")
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::Copy)
                                    .tooltip(tr!("repo.history.copy_sha"))
                                    .on_click(move |_, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            sha.clone(),
                                        ))
                                    }),
                            ),
                    );
                let changes: AnyElement = match &browser.files {
                    None => crate::ui::skeleton_text(12),
                    Some(Err(err)) => note(err.clone().into(), danger),
                    Some(Ok(files)) if files.is_empty() => {
                        note(tr!("repo.pulls.review_no_files"), muted)
                    }
                    Some(Ok(files)) => {
                        let selected = browser.file.min(files.len() - 1);
                        let list =
                            v_flex()
                                .id("rc-files")
                                .flex_none()
                                .w(px(FILES_WIDTH))
                                .h_full()
                                .border_r_1()
                                .border_color(border)
                                .overflow_y_scroll()
                                .child(div().px_3().py_2().text_sm().text_color(muted).child(tr!(
                                    "repo.history.changed_files",
                                    count = files.len().to_string()
                                )))
                                .children(files.iter().enumerate().map(|(ix, file)| {
                                    h_flex()
                                        .id(("rc-file", ix))
                                        .flex_none()
                                        .gap_2()
                                        .px_3()
                                        .py_1p5()
                                        .items_center()
                                        .cursor_pointer()
                                        .when(ix == selected, |r| r.bg(active))
                                        .hover(move |style| style.bg(hover))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.select_commit_file(ix, cx)
                                        }))
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .truncate()
                                                .text_sm()
                                                .child(file.path.clone()),
                                        )
                                        .child(div().flex_none().text_xs().text_color(muted).child(
                                            format!("+{} −{}", file.additions, file.deletions),
                                        ))
                                }));
                        let file = &files[selected];
                        let diff: AnyElement = if file.lines.is_empty() {
                            note(tr!("repo.pulls.review_file_empty"), muted)
                        } else {
                            let files = files.clone();
                            uniform_list("commit-diff", file.lines.len(), move |range, _, cx| {
                                range
                                    .map(|i| diff_row(i, &files[selected].lines[i], cx))
                                    .collect()
                            })
                            .track_scroll(&self.commit_scroll)
                            .flex_1()
                            .min_h_0()
                            .into_any_element()
                        };
                        h_flex()
                            .flex_1()
                            .min_h_0()
                            .items_start()
                            .child(list)
                            .child(v_flex().flex_1().min_w_0().h_full().child(diff))
                            .into_any_element()
                    }
                };
                v_flex()
                    .size_full()
                    .child(header)
                    .child(changes)
                    .into_any_element()
            }
        };

        v_flex()
            .size_full()
            .child(
                h_flex()
                    .flex_none()
                    .h_10()
                    .px_3()
                    .gap_2()
                    .items_center()
                    .border_b_1()
                    .border_color(border)
                    .child(
                        crate::ui::button("commits-back")
                            .ghost()
                            .small()
                            .icon(IconName::ChevronLeft)
                            .label(tr!("repo.account.back"))
                            .on_click(cx.listener(|this, _, _, cx| this.close_repo_commits(cx))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(browser.repo.clone()),
                    ),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_start()
                    .child(
                        div()
                            .flex_none()
                            .w(px(COMMITS_WIDTH))
                            .h_full()
                            .border_r_1()
                            .border_color(border)
                            .child(history),
                    )
                    .child(div().flex_1().min_w_0().h_full().child(detail)),
            )
            .into_any_element()
    }

    pub fn render_account(
        &self,
        account: &AccountState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, border, active) = (theme.muted_foreground, theme.border, theme.list_active);
        // A repository's history takes over the whole view.
        if let Some(browser) = &account.commits {
            return v_flex()
                .id("account-view")
                .size_full()
                .child(self.render_commit_browser(account, browser, cx));
        }
        let owner = match &account.page {
            AccountPage::OrgRepos(org) => org.clone(),
            _ => account.login.clone(),
        };
        let on_orgs = matches!(account.page, AccountPage::Orgs | AccountPage::OrgRepos(_));
        let repos_page = !matches!(account.page, AccountPage::Orgs);
        let title = match &account.page {
            AccountPage::OrgRepos(org) => format!("{} › {org}", tr!("repo.account.orgs_tab")),
            _ => owner.clone(),
        };
        let body = match &account.page {
            AccountPage::Orgs => self.render_org_list(account, cx),
            _ => self.render_repo_list(account, cx),
        };
        let tab = |id: &'static str, label: SharedString, on: bool, to: AccountPage| {
            crate::ui::button(id)
                .ghost()
                .small()
                .label(label)
                .when(on, |b| b.bg(active))
                .on_click(cx.listener(move |this, _, _, cx| this.show_account_page(to.clone(), cx)))
        };
        v_flex()
            .id("account-view")
            .size_full()
            .child(
                h_flex()
                    .flex_none()
                    .h_12()
                    .px_3()
                    .gap_3()
                    .items_center()
                    .border_b_1()
                    .border_color(border)
                    .child(crate::ui::avatar(self.picture(&owner), &owner, px(26.), cx))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .child(tab(
                                "account-tab-repos",
                                tr!("repo.account.repos_tab"),
                                !on_orgs,
                                AccountPage::Repos,
                            ))
                            .child(tab(
                                "account-tab-orgs",
                                tr!("repo.account.orgs_tab"),
                                on_orgs,
                                AccountPage::Orgs,
                            )),
                    )
                    .child(div().flex_1())
                    .when(repos_page, |h| {
                        h.child(
                            div()
                                .flex_none()
                                .w(px(260.))
                                .rounded(px(6.))
                                .border_1()
                                .border_color(border)
                                .child(Input::new(&self.account_search_input).prefix(
                                    Icon::new(IconName::Search).size_4().text_color(muted),
                                )),
                        )
                    })
                    .child(
                        crate::ui::button("account-refresh")
                            .ghost()
                            .xsmall()
                            .icon(Icon::empty().path(ICON_REFRESH))
                            .tooltip(tr!("repo.clone.refresh"))
                            .on_click(cx.listener(|this, _, _, cx| this.refresh_account_view(cx))),
                    )
                    .child(
                        crate::ui::button("account-close")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Close)
                            .tooltip(tr!("repo.close"))
                            .on_click(cx.listener(|this, _, _, cx| this.close_account_view(cx))),
                    ),
            )
            .child(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(name: &str, description: &str) -> RemoteRepo {
        RemoteRepo {
            full_name: format!("me/{name}"),
            name: name.into(),
            owner: "me".into(),
            description: description.into(),
            private: false,
            fork: false,
            archived: false,
            language: "Rust".into(),
            stars: 1,
            pushed_at: 1_789_427_775,
        }
    }

    #[test]
    fn the_filter_box_matches_names_and_descriptions() {
        let r = repo("Anano", "A native Git client");
        assert!(matches_filter(&r, ""));
        assert!(matches_filter(&r, "  "));
        assert!(matches_filter(&r, "anan"));
        assert!(matches_filter(&r, "NATIVE"));
        assert!(matches_filter(&r, "me/anano"));
        assert!(!matches_filter(&r, "zzz"));
    }

    /// Every page of the account view draws: repositories, organizations, an organization's
    /// repositories, and a repository's history with the changes of a commit.
    #[gpui_kit::test]
    fn the_account_view_draws_every_page(cx: &mut gpui_kit::TestAppContext) {
        use anano_core::git::{DiffLine, LineKind, PatchStatus};
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
        let draw = |cx: &mut gpui_kit::VisualTestContext| {
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
        };
        let cx = &mut gpui_kit::VisualTestContext::from_window(window.into(), cx);
        let list = |names: &[&str]| RepoList {
            items: Some(Ok(names.iter().map(|n| repo(n, "A thing")).collect())),
            exhausted: false,
            ..Default::default()
        };
        let file = PatchFile {
            path: "src/a.rs".into(),
            old_path: None,
            status: PatchStatus::Modified,
            additions: 1,
            deletions: 1,
            binary: false,
            lines: vec![
                DiffLine {
                    kind: LineKind::Hunk,
                    old_no: None,
                    new_no: None,
                    text: "@@ -1 +1 @@".into(),
                },
                DiffLine {
                    kind: LineKind::Del,
                    old_no: Some(1),
                    new_no: None,
                    text: "old".into(),
                },
                DiffLine {
                    kind: LineKind::Add,
                    old_no: None,
                    new_no: Some(1),
                    text: "new".into(),
                },
            ],
        };
        workspace.update(cx, |ws, cx| {
            ws.account = Some(AccountState {
                login: "me".into(),
                page: AccountPage::Repos,
                lists: HashMap::from([
                    (String::new(), list(&["alpha", "beta"])),
                    ("acme".to_string(), list(&["gamma"])),
                ]),
                orgs: Some(Ok(vec![Org {
                    login: "acme".into(),
                    description: "Things".into(),
                }])),
                commits: None,
            });
            cx.notify();
        });
        draw(cx); // repositories
        workspace.update(cx, |ws, cx| {
            ws.account.as_mut().unwrap().page = AccountPage::Orgs;
            cx.notify();
        });
        draw(cx); // organizations
        workspace.update(cx, |ws, cx| {
            ws.account.as_mut().unwrap().page = AccountPage::OrgRepos("acme".into());
            cx.notify();
        });
        draw(cx); // one organization's repositories
        workspace.update(cx, |ws, cx| {
            ws.account.as_mut().unwrap().commits = Some(CommitBrowser {
                repo: "acme/gamma".into(),
                commits: Some(Ok(vec![RemoteCommit {
                    sha: "abc123".into(),
                    message: "Fix it\n\nWith a longer explanation.".into(),
                    author: "Me".into(),
                    login: "me".into(),
                    at: 1_789_427_775,
                }])),
                exhausted: true,
                loading_more: false,
                open: Some("abc123".into()),
                files: Some(Ok(Rc::new(vec![file]))),
                file: 0,
            });
            cx.notify();
        });
        draw(cx); // a repository's history and a commit's changes
        workspace.update(cx, |ws, cx| ws.close_repo_commits(cx));
        assert!(workspace.read_with(cx, |ws, _| ws.account.as_ref().unwrap().commits.is_none()));
    }

    /// "Pull requests" on a repository of the list aims the Pull Requests view at that
    /// repository, as the account the list was read as, and not at the open one.
    #[gpui_kit::test]
    fn pull_requests_of_a_listed_repository_use_its_own_address(cx: &mut gpui_kit::TestAppContext) {
        use crate::ui::pull_requests::PullRequests;
        cx.executor().allow_parking();
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::bridge::init(cx);
        });
        let (workspace, cx) = cx.add_window_view(Workspace::new);
        workspace.update(cx, |ws, _| {
            ws.account = Some(AccountState {
                login: "me".into(),
                ..Default::default()
            });
            let url = ws.remote_url("acme/gamma").unwrap();
            assert_eq!(url, "https://me@github.com/acme/gamma.git");
            ws.pulls = Some(PullRequests {
                remote: Some(url.clone()),
                ..Default::default()
            });
            assert_eq!(ws.pulls_origin(), Some(url));
            // Without it, the view is about the open repository (none here).
            ws.pulls = Some(PullRequests::default());
            assert_eq!(ws.pulls_origin(), None);
        });
    }
}
