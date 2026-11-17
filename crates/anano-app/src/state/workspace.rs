//! 顶层工作区：已打开仓库列表、侧栏、主题、全局动作；负责从磁盘恢复，并把改动写回。
//!
//! Phase 1 skeleton swap：这里原本是 HTTP 请求 Tab 的宿主（标签栏 = 请求 Tab，内容区 =
//! 请求 / 响应分栏）。现在标签栏的每个标签是一个「打开的仓库」，内容区是它的
//! `git status`（见 [`crate::ui::status_pane`]）。窗口外壳（标题栏 / 图标栏 / 可拖宽
//! 分栏 / 标签栏骨架）原样保留，换的只是标签与内容区渲染的是什么。

// 显式导入而非 `use gpui_kit::*`：本文件含 `#[cfg(test)] mod tests`，通配符会引入
// gpui 重导出的 `#[proc_macro_attribute] test`，与标准库 `#[test]` 同名冲突。
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use anano_core::external::{self, Account};
use anano_core::git::{ChangeKind, DiffView, GitError, LfsOp, RepoEntry};
use anano_core::model::{ThemePref, Ulid, WorkspaceState};
use anano_core::store::Loaded;
use gpui_kit::component::{
    ActiveTheme, IconName, Sizable, Theme, ThemeMode, TitleBar, WindowExt,
    alert::Alert,
    button::ButtonVariants,
    h_flex,
    input::{InputState, TextareaState},
    resizable::{ResizableState, h_resizable, resizable_panel},
    status_bar::StatusBar,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext, ClipboardItem, Context, Entity, FocusHandle, FontWeight, InteractiveElement,
    IntoElement, ParentElement, PathPromptOptions, Render, Role, ScrollHandle, ScrollStrategy,
    SharedString, StatefulInteractiveElement, Styled, Subscription, UniformListScrollHandle,
    Window, div, px,
};

use gpui_updater::UpdateStatus;

use crate::brand::APP_NAME;
use crate::bridge;
use crate::i18n::tr;
use crate::state::repos::{OpenRepo, SyncKind};
use crate::state::store::{banner, store};
use crate::state::update;
use crate::ui::settings_dialog::{SettingsPage, open_settings, open_settings_page};
use crate::{
    CloseTab, OpenRepoAction, OpenRepositoryOnGitHub, OpenSettings, RevealRepository, SyncNow,
    ToggleSidebar,
};

/// 侧栏默认宽度。比 HTTP 版本的窄栏宽一些：仓库路径普遍比请求标题长，
/// 底部还常驻着提交信息输入框，太窄会把两者都挤成省略号。
pub const SIDEBAR_DEFAULT_WIDTH: f32 = 340.;
pub(crate) const SIDEBAR_MIN_WIDTH: f32 = 260.;
pub(crate) const SIDEBAR_MAX_WIDTH: f32 = 600.;
/// 历史列表拉多少条；够看最近在干嘛，不做分页/加载更多（那是后续阶段的事）。
const RECENT_COMMITS_LIMIT: usize = 30;

/// 侧栏下半部分的两个标签：改动 / 历史。GitHub Desktop 同款布局——分支与同步是
/// 常驻的上下文，标签只切换「改了什么」和「都提交过什么」这两块内容。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SidebarTab {
    #[default]
    Changes,
    History,
}

/// Where "Add account" (Settings → Accounts) currently stands.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum LoginState {
    #[default]
    Idle,
    Starting,
    /// Waiting for the user to type this one-time code into github.com/login/device.
    Waiting(String),
    Failed(String),
}

/// What the main pane is showing the diff of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selected {
    /// A row of the Changes tab: everything changed in that file since HEAD.
    File { path: String },
    /// A row of the History tab, by full commit id.
    Commit(String),
}

pub struct Workspace {
    /// The Claude Code run (or the prompt waiting to be run) behind the dialog.
    /// Every Claude Code agent started here (running, finished, or the prompt being written).
    pub(crate) claude: Vec<crate::ui::claude_dialog::ClaudeSession>,
    pub(crate) claude_active: usize,
    /// One watcher per open repository: what changes it on disk (a terminal, an editor,
    /// another tool) reloads the changes, history and branch views by itself.
    watchers: HashMap<PathBuf, anano_core::git::RepoWatcher>,
    /// Off in tests, where a wake-up from the watcher's thread would trip the test
    /// scheduler's determinism check; the one test of watching turns it on.
    pub(crate) watch_enabled: bool,
    /// Claude reviews of pull requests, running or waiting for you (origin, number).
    pub(crate) pr_claude: HashMap<(String, u64), crate::ui::review_dialog::PrClaude>,
    pub(crate) claude_input: Entity<TextareaState>,
    pub(crate) claude_scroll: ScrollHandle,
    /// The account view (repositories, organizations, commit history of an account's
    /// repositories), when it is what the main pane shows.
    pub(crate) account: Option<crate::ui::account_view::AccountState>,
    pub(crate) account_search_input: Entity<InputState>,
    pub(crate) commit_scroll: UniformListScrollHandle,
    /// The Workflows view (GitHub Actions), when it is what the main pane shows.
    pub(crate) workflows: Option<crate::ui::workflows::WorkflowsState>,
    pub(crate) log_scroll: UniformListScrollHandle,
    /// The filter box inside the workflow picker.
    pub(crate) workflow_search_input: Entity<InputState>,
    /// The message of the review being written, and the diff's scroll position.
    pub(crate) review_input: Entity<TextareaState>,
    pub(crate) review_scroll: UniformListScrollHandle,
    /// The Pull Requests view's search box (`author:` words and free text).
    pub(crate) pr_search_input: Entity<InputState>,
    /// The Pull Requests view, when it is what the main pane shows.
    pub(crate) pulls: Option<crate::ui::pull_requests::PullRequests>,
    /// Commit-author profile pictures by email. `None` = asked for, not (or never) found;
    /// the row shows the author's initial meanwhile.
    pub(crate) avatars: HashMap<String, Option<PathBuf>>,
    /// The toolbar's "Current Branch" panel.
    pub(crate) branch_popover_open: bool,
    pub(crate) branch_filter: Entity<InputState>,
    /// Accounts whose repository list is being (re)fetched from GitHub right now.
    pub(crate) repos_loading: std::collections::HashSet<String>,
    /// Accounts already revalidated against GitHub this session.
    repos_fresh: std::collections::HashSet<String>,
    /// A clone / create in flight ("Cloning foo…"); the main pane shows it with a progress
    /// bar so the wait doesn't look like nothing happened.
    pub(crate) busy: Option<SharedString>,
    /// Set after creating a repository with initial files: once its history loads, show
    /// that first commit so the files it added are on screen.
    show_head_when_loaded: bool,
    /// The create dialog's controls, while (and after) it is open.
    pub(crate) create_form: Option<crate::ui::repo_dialogs::CreateForm>,
    pub(crate) switcher_open: bool,
    selected: Option<Selected>,
    /// `None` while the diff for `selected` is loading.
    pub(crate) diff: Option<Result<Rc<DiffView>, String>>,
    pub(crate) diff_scroll: UniformListScrollHandle,
    /// Before/after pictures of the selected change when it touches images.
    pub(crate) images: Vec<anano_core::git::ImagePair>,
    /// Repositories each connected account can clone; filled on demand by the clone dialog.
    pub(crate) account_repos: HashMap<String, Result<Vec<external::Repo>, String>>,
    /// Git Ignore / License choices for the create dialog; fetched the first time it opens.
    pub(crate) templates: Option<external::Templates>,
    /// GitHub accounts `gh` is signed into; refreshed at startup and after each login.
    pub(crate) accounts: Vec<Account>,
    pub(crate) login: LoginState,
    /// Whether the `gh` / `nermal` CLIs exist on this machine (checked once at startup).
    pub(crate) gh_available: bool,
    pub(crate) nermal_available: bool,
    repos: Vec<OpenRepo>,
    active: usize,
    sidebar_collapsed: bool,
    sidebar_width: Option<f32>,
    sidebar_state: Entity<ResizableState>,
    /// 见 HTTP 版本同名字段的注释（首帧后归一化一次比例重分配保护）。
    panels_normalize_pending: bool,
    theme: ThemePref,
    tab_scroll: ScrollHandle,
    focus_handle: FocusHandle,
    update_status: UpdateStatus,
    /// 侧栏底部常驻的提交信息输入框；一个窗口一份，跟着 `active` 指向哪个仓库走。
    pub(crate) commit_input: Entity<InputState>,
    /// The optional longer description under the summary (the commit message body).
    pub(crate) commit_description: Entity<TextareaState>,
    /// Profile pictures of the connected accounts, by login.
    pub(crate) account_avatars: HashMap<String, Option<PathBuf>>,
    /// 侧栏下半部分当前显示哪个标签（改动 / 历史）。
    pub(crate) sidebar_tab: SidebarTab,
    /// 仓库切换器里的过滤输入框。
    pub(crate) repo_search_input: Entity<InputState>,
    _subs: Vec<Subscription>,
}

impl Workspace {
    #[cfg(test)]
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::restore(Loaded::default(), window, cx)
    }

    /// 从启动读取的结果重建：按 `repos.json` 里的顺序恢复打开的仓库标签。
    pub fn restore(loaded: Loaded, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let Loaded {
            workspace: state,
            settings: _,
            drafts: _,
            requests: _,
            repos,
            errors: _,
        } = loaded;
        // Whether a layout was ever saved: only then does "not in the tab order" mean the
        // tab was closed (rather than "nothing recorded yet, give everything a tab").
        let had_layout = state.is_some();
        let state = state.unwrap_or_default();
        let pr_search_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(tr!("repo.pulls.search_placeholder"))
        });
        let review_input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(3, 6)
                .placeholder(tr!("repo.pulls.review_placeholder"))
        });
        let workflow_search_input = cx
            .new(|cx| InputState::new(window, cx).placeholder(tr!("repo.workflows.picker_search")));
        let account_search_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(tr!("repo.account.filter_placeholder"))
        });
        let claude_input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(5, 12)
                .placeholder(tr!("repo.claude.prompt_placeholder"))
        });
        let mut ws = Self {
            claude: Self::load_claude_sessions(),
            claude_active: 0,
            watchers: HashMap::new(),
            watch_enabled: !cfg!(test),
            pr_claude: HashMap::new(),
            claude_input: claude_input.clone(),
            claude_scroll: ScrollHandle::new(),
            account: None,
            account_search_input: account_search_input.clone(),
            commit_scroll: UniformListScrollHandle::new(),
            workflows: None,
            log_scroll: UniformListScrollHandle::new(),
            workflow_search_input: workflow_search_input.clone(),
            review_input: review_input.clone(),
            review_scroll: UniformListScrollHandle::new(),
            pr_search_input: pr_search_input.clone(),
            pulls: None,
            avatars: HashMap::new(),
            branch_popover_open: false,
            branch_filter: cx
                .new(|cx| InputState::new(window, cx).placeholder(tr!("repo.branch.filter"))),
            repos_loading: Default::default(),
            repos_fresh: Default::default(),
            busy: None,
            show_head_when_loaded: false,
            create_form: None,
            switcher_open: false,
            selected: None,
            diff: None,
            diff_scroll: UniformListScrollHandle::new(),
            images: Vec::new(),
            account_repos: HashMap::new(),
            templates: None,
            accounts: Vec::new(),
            login: LoginState::default(),
            gh_available: external::gh_available(),
            nermal_available: external::nermal_available(),
            repos: repos.into_iter().map(OpenRepo::new).collect(),
            active: 0,
            sidebar_collapsed: state.sidebar_collapsed,
            sidebar_width: state.sidebar_width,
            sidebar_state: cx.new(|_| ResizableState::default()),
            panels_normalize_pending: true,
            theme: state.theme,
            tab_scroll: ScrollHandle::new(),
            focus_handle: cx.focus_handle(),
            update_status: update::status(cx),
            commit_input: cx
                .new(|cx| InputState::new(window, cx).placeholder(tr!("repo.commit.placeholder"))),
            commit_description: cx.new(|cx| {
                TextareaState::new(window, cx)
                    .auto_grow(3, 8)
                    .placeholder(tr!("repo.commit.description"))
            }),
            account_avatars: HashMap::new(),
            sidebar_tab: SidebarTab::default(),
            repo_search_input: cx.new(|cx| {
                InputState::new(window, cx).placeholder(tr!("repo.switcher.search_placeholder"))
            }),
            _subs: Vec::new(),
        };

        if let Some(updater) = update::updater(cx) {
            ws._subs.push(cx.observe(&updater, |this, updater, cx| {
                this.update_status = updater.read(cx).status().clone();
                cx.notify();
            }));
        }

        // `tab_order`/`active` 里的 id 是仓库的 `RepoEntry::id`（复用既有的 WorkspaceState
        // 字段，Phase 1 没必要为同一件事再加一套字段）。
        ws.reorder_repos(&state.tab_order);
        // Repositories not in the saved tab order had their tab closed; they are still in
        // the app. That holds even when every tab was closed (an empty order).
        if had_layout {
            for repo in &mut ws.repos {
                repo.in_tabs = state.tab_order.contains(&repo.entry.id);
            }
        }
        if let Some(active_id) = state.active
            && let Some(ix) = ws.repos.iter().position(|r| r.entry.id == active_id)
        {
            ws.active = ix;
        }

        apply_theme(ws.theme, Some(window), cx);
        let weak = cx.entity().downgrade();
        ws._subs
            .push(window.observe_window_appearance(move |window, cx| {
                if let Some(ws) = weak.upgrade()
                    && ws.read(cx).theme == ThemePref::System
                {
                    Theme::sync_system_appearance(Some(window), cx);
                }
            }));

        ws._subs.push(cx.subscribe_in(
            &workflow_search_input,
            window,
            |this, _, event: &gpui_kit::component::input::InputEvent, _, cx| {
                this.on_workflow_search_event(event, cx)
            },
        ));
        ws._subs.push(cx.subscribe_in(
            &account_search_input,
            window,
            |_, _, event: &gpui_kit::component::input::InputEvent, _, cx| {
                if matches!(event, gpui_kit::component::input::InputEvent::Change) {
                    cx.notify();
                }
            },
        ));
        ws._subs.push(cx.subscribe_in(
            &claude_input,
            window,
            |this, _, event: &gpui_kit::component::input::InputEvent, _, cx| {
                this.on_claude_input_event(event, cx)
            },
        ));
        ws._subs.push(cx.subscribe_in(
            &review_input,
            window,
            |this, _, event: &gpui_kit::component::input::InputEvent, _, cx| {
                this.on_review_input_event(event, cx)
            },
        ));
        ws._subs.push(cx.subscribe_in(
            &pr_search_input,
            window,
            |this, _, event: &gpui_kit::component::input::InputEvent, _, cx| {
                this.on_pr_search_event(event, cx)
            },
        ));
        ws.refresh_accounts(cx);
        ws.learn_owners(cx);
        // Coming back from the editor / terminal: pick up whatever changed on disk meanwhile.
        ws._subs
            .push(cx.observe_window_activation(window, |ws, window, cx| {
                if window.is_window_active() && !ws.repos.is_empty() {
                    ws.refresh_active_status(cx);
                    ws.refresh_active_log(cx);
                    ws.refresh_active_branches(cx);
                }
            }));
        // 恢复出来的仓库还没有状态：把当前激活的那个刷一次。
        if !ws.repos.is_empty() {
            ws.refresh_active_status(cx);
            ws.refresh_active_log(cx);
            ws.refresh_active_branches(cx);
        }
        ws
    }

    fn reorder_repos(&mut self, order: &[Ulid]) {
        reorder(&mut self.repos, order);
    }

    pub fn repo_count(&self) -> usize {
        self.repos.len()
    }

    pub fn active_index(&self) -> usize {
        self.active
    }

    pub fn active_repo(&self) -> Option<&OpenRepo> {
        self.repos.get(self.active)
    }

    /// 渲染标签 / 侧栏列表用：每个打开仓库的（显示名, 完整路径）。
    pub(crate) fn repos_meta(&self, _cx: &App) -> Vec<(SharedString, SharedString)> {
        self.repos
            .iter()
            .map(|r| {
                (
                    SharedString::from(r.entry.display_name.clone()),
                    SharedString::from(r.entry.path.display().to_string()),
                )
            })
            .collect()
    }

    pub(crate) fn tab_scroll(&self) -> &ScrollHandle {
        &self.tab_scroll
    }

    pub fn theme(&self) -> ThemePref {
        self.theme
    }

    /// 图标栏 / 标签栏的「打开仓库」：原生文件夹选择器（`cx.prompt_for_paths`）。
    ///
    /// 选中的文件夹先经过 `git::discover_root`（在后台线程跑，见 `bridge::discover`）
    /// 找到仓库工作树的真正根：选中仓库内任意子目录也能打开，不会出现「选错一层就
    /// 读不出 status」的情况。找不到仓库（选的目录压根没有 `.git`）就原样按选中的
    /// 路径打开——`repo_status` 会在第一次刷新时把「不是仓库」的错误显示出来。
    pub fn open_repo_dialog(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(tr!("repo.open")),
        });
        cx.spawn(async move |ws, cx| {
            let Ok(Ok(Some(mut paths))) = receiver.await else {
                return;
            };
            let Some(path) = paths.pop() else { return };
            let discover = ws.update(cx, |_, cx| bridge::discover(cx, path.clone()));
            let Ok(discover) = discover else { return };
            let root = match discover.await {
                Ok(Ok(root)) => root,
                _ => path,
            };
            let _ = ws.update(cx, |ws, cx| ws.open_repo_path(root, cx));
        })
        .detach();
    }

    /// 打开（或激活已打开的）仓库路径。
    pub fn open_repo_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if let Some(ix) = self.repos.iter().position(|r| r.entry.path == path) {
            self.activate(ix, cx);
            return;
        }
        let entry = RepoEntry::new(path);
        self.clear_selection();
        self.repos.push(OpenRepo::new(entry));
        self.active = self.repos.len() - 1;
        self.persist_repos(cx);
        self.refresh_active_status(cx);
        self.refresh_active_log(cx);
        self.refresh_active_branches(cx);
        cx.notify();
    }

    pub fn activate(&mut self, ix: usize, cx: &mut Context<Self>) {
        // Picking a repository whose tab was closed brings its tab back.
        if let Some(repo) = self.repos.get_mut(ix) {
            repo.entry.last_opened = anano_core::model::now_ms();
            if !repo.in_tabs {
                repo.in_tabs = true;
                self.persist_repos(cx);
                cx.notify();
            }
        }
        if ix < self.repos.len() && ix != self.active {
            self.active = ix;
            self.repos[ix].needs_fetch = true;
            self.clear_selection();
            self.persist_repos(cx);
            cx.notify();
            self.refresh_active(cx);
        }
    }

    /// Read the active repository's status, history and branches from git.
    ///
    /// Always, on every switch. This used to be skipped unless the lists were empty *and*
    /// not marked loading, but a repository restored at launch starts out marked loading
    /// without anything having been started for it, so switching to it loaded nothing, ever:
    /// no changes, no history, and a branch name stuck on its placeholder.
    fn refresh_active(&mut self, cx: &mut Context<Self>) {
        self.refresh_active_status(cx);
        self.refresh_active_log(cx);
        self.refresh_active_branches(cx);
        self.maybe_auto_fetch(cx);
    }

    /// The repository list groups by GitHub owner, which comes from each repository's
    /// origin. For repositories that have never been on screen (so nothing is cached for
    /// them yet), read it once now, a couple of file reads each, and remember it.
    fn learn_owners(&mut self, cx: &mut Context<Self>) {
        let unknown: Vec<_> = self
            .repos
            .iter()
            .filter(|r| r.entry.last_origin.is_none())
            .map(|r| (r.entry.id, bridge::repo_facts(cx, r.entry.path.clone())))
            .collect();
        if unknown.is_empty() {
            return;
        }
        cx.spawn(async move |ws, cx| {
            let mut learned = Vec::new();
            for (id, task) in unknown {
                if let Ok(facts) = task.await {
                    learned.push((id, facts));
                }
            }
            let _ = ws.update(cx, |ws, cx| {
                for (id, facts) in learned {
                    if let Some(repo) = ws.repos.iter_mut().find(|r| r.entry.id == id) {
                        repo.entry.last_origin = facts.origin.or(repo.entry.last_origin.take());
                        repo.entry.last_branch =
                            facts.head_branch.or(repo.entry.last_branch.take());
                    }
                }
                ws.persist_repos(cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn is_tab(&self, ix: usize) -> bool {
        self.repos.get(ix).is_some_and(|r| r.in_tabs)
    }

    /// The tab's × (and ⌘W): close the tab, keep the repository. It stays in the
    /// repository list on the left and picking it there opens its tab again. If it was the
    /// one on screen, the nearest remaining tab takes over.
    pub fn close_tab(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(repo) = self.repos.get_mut(ix) else {
            return;
        };
        repo.in_tabs = false;
        if ix == self.active {
            let after = (ix + 1..self.repos.len()).find(|&i| self.repos[i].in_tabs);
            let before = (0..ix).rev().find(|&i| self.repos[i].in_tabs);
            if let Some(next) = after.or(before) {
                self.activate(next, cx);
            }
        }
        self.persist_repos(cx);
        cx.notify();
    }

    /// The index of the repository here whose origin is `owner/name` (any case).
    pub(crate) fn find_repo_by_slug(&self, full_name: &str) -> Option<usize> {
        self.repos.iter().position(|repo| {
            repo.origin
                .as_deref()
                .or(repo.entry.last_origin.as_deref())
                .and_then(external::repo_slug)
                .is_some_and(|slug| slug.eq_ignore_ascii_case(full_name))
        })
    }

    /// A repository's id, name and folder, for dialogs that talk about it.
    pub(crate) fn repo_summary(&self, ix: usize) -> Option<(Ulid, String, String)> {
        self.repos.get(ix).map(|r| {
            (
                r.entry.id,
                r.entry.display_name.clone(),
                r.entry.path.display().to_string(),
            )
        })
    }

    /// Remove a repository by id (positions can shift while a dialog is open).
    pub(crate) fn remove_repo_id(&mut self, id: Ulid, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.repos.iter().position(|r| r.entry.id == id) {
            self.close_repo(ix, window, cx);
        }
    }

    /// "Close Other Tabs": every tab but this one is closed (the repositories stay).
    pub fn close_other_tabs(&mut self, keep: usize, cx: &mut Context<Self>) {
        for (ix, repo) in self.repos.iter_mut().enumerate() {
            repo.in_tabs = ix == keep;
        }
        if self.active != keep {
            self.activate(keep, cx);
        }
        self.persist_repos(cx);
        cx.notify();
    }

    /// Everything the repository list shows per repository: its index, name, path, the
    /// owner to group it under (from its GitHub origin, when it has one) and when it was
    /// last opened.
    pub(crate) fn switcher_entries(&self) -> Vec<(usize, SharedString, SharedString, String, i64)> {
        self.repos
            .iter()
            .enumerate()
            .map(|(ix, r)| {
                let origin = r.origin.as_deref().or(r.entry.last_origin.as_deref());
                let owner = origin
                    .and_then(external::repo_slug)
                    .and_then(|slug| slug.split('/').next().map(str::to_string))
                    .unwrap_or_default();
                (
                    ix,
                    r.entry.display_name.clone().into(),
                    r.entry.path.display().to_string().into(),
                    owner,
                    r.entry.last_opened,
                )
            })
            .collect()
    }

    /// Remove a repository from the app altogether (the repository list's ×). Nothing on
    /// disk is touched.
    pub fn close_repo(&mut self, ix: usize, _window: &mut Window, cx: &mut Context<Self>) {
        if ix >= self.repos.len() {
            return;
        }
        self.repos.remove(ix);
        self.clear_selection();
        if self.active >= self.repos.len() {
            self.active = self.repos.len().saturating_sub(1);
        } else if ix < self.active {
            self.active -= 1;
        }
        self.persist_repos(cx);
        cx.notify();
        if !self.repos.is_empty() {
            self.refresh_active(cx);
        }
    }

    pub fn sidebar_tab(&self) -> SidebarTab {
        self.sidebar_tab
    }

    pub fn set_sidebar_tab(&mut self, tab: SidebarTab, cx: &mut Context<Self>) {
        self.sidebar_tab = tab;
        // Coming back to Changes: show one of its files, not whatever commit was open.
        if tab == SidebarTab::Changes {
            self.ensure_file_selected(cx);
        }
        cx.notify();
    }

    /// With the Changes tab showing and the repository having changes, the main pane shows
    /// a file's diff instead of an empty "select a file" placeholder: the file already open
    /// if it is still changed, otherwise the first one in the list. Leaves the Pull
    /// Requests view, the History tab and a commit being read alone. Returns whether it
    /// selected something (which also loads that diff).
    pub(crate) fn ensure_file_selected(&mut self, cx: &mut Context<Self>) -> bool {
        if self.pulls.is_some()
            || self.workflows.is_some()
            || self.account.is_some()
            || self.sidebar_tab != SidebarTab::Changes
        {
            return false;
        }
        let Some(repo) = self.active_repo() else {
            return false;
        };
        let files = repo.changed_files();
        let still_changed = match &self.selected {
            Some(Selected::File { path }) => files.iter().any(|(file, _)| file == path),
            _ => false,
        };
        if still_changed {
            return false;
        }
        match files.into_iter().next() {
            Some((path, _)) => {
                self.select(Selected::File { path }, cx);
                true
            }
            // Nothing left to show (everything was committed): drop the stale diff.
            None if matches!(self.selected, Some(Selected::File { .. })) => {
                self.clear_selection();
                false
            }
            None => false,
        }
    }

    /// Close the repository switcher popover. Its open state lives here (not inside the
    /// `Popover`) precisely so that picking a repository or one of the "Add" entries can
    /// close it: left to itself it stayed open on top of whatever was opened next.
    pub(crate) fn close_repo_switcher(&mut self, cx: &mut Context<Self>) {
        self.switcher_open = false;
        cx.notify();
    }

    /// 切换器里点一个「最近」条目：切过去。
    pub fn activate_from_switcher(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.activate(ix, cx);
        self.close_repo_switcher(cx);
    }

    /// Open the repository containing `path` (any folder inside it will do).
    pub fn open_repo_at(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let discover = bridge::discover(cx, path.clone());
        cx.spawn(async move |ws, cx| {
            let root = match discover.await {
                Ok(Ok(root)) => root,
                _ => path,
            };
            let _ = ws.update(cx, |ws, cx| ws.open_repo_path(root, cx));
        })
        .detach();
    }

    /// Clone `url` into `parent/<name from url>` and open it.
    pub(crate) fn clone_into(&mut self, url: String, parent: PathBuf, cx: &mut Context<Self>) {
        let name = repo_name_from_url(&url);
        let dest = parent.join(&name);
        self.busy = Some(tr!("repo.busy.cloning", name = name));
        cx.notify();
        let task = bridge::clone(cx, url, dest.clone());
        cx.spawn(async move |ws, cx| {
            let result = task.await;
            let _ = ws.update(cx, |ws, cx| {
                ws.busy = None;
                cx.notify();
            });
            let _ = ws.update(cx, |ws, cx| match result {
                Ok(Ok(path)) => ws.open_repo_path(path, cx),
                Ok(Err(err)) => {
                    ws.notify_add_repo_error(tr!("repo.clone.error", error = err.to_string()), cx)
                }
                Err(err) => {
                    ws.notify_add_repo_error(tr!("repo.clone.error", error = err.to_string()), cx)
                }
            });
        })
        .detach();
    }

    /// Run the "Create a New Repository" dialog's answers and open the result.
    pub(crate) fn create_repo_from(
        &mut self,
        options: bridge::CreateOptions,
        cx: &mut Context<Self>,
    ) {
        let has_first_commit =
            options.readme || options.gitignore.is_some() || options.license.is_some();
        self.busy = Some(tr!("repo.busy.creating", name = options.name.clone()));
        cx.notify();
        let task = bridge::create_repo(cx, options);
        cx.spawn(async move |ws, cx| {
            let result = task.await;
            let _ = ws.update(cx, |ws, cx| {
                ws.busy = None;
                cx.notify();
            });
            let _ = ws.update(cx, |ws, cx| match result {
                Ok(Ok((path, problems))) => {
                    ws.show_head_when_loaded = has_first_commit;
                    ws.open_repo_path(path, cx);
                    if let Some(err) = problems {
                        ws.notify_add_repo_error(tr!("repo.create.partial", error = err), cx);
                    }
                }
                Ok(Err(err)) => {
                    ws.notify_add_repo_error(tr!("repo.create.error", error = err.to_string()), cx)
                }
                Err(err) => {
                    ws.notify_add_repo_error(tr!("repo.create.error", error = err.to_string()), cx)
                }
            });
        })
        .detach();
    }

    pub(crate) fn account_logins(&self) -> Vec<String> {
        self.accounts.iter().map(|a| a.login.clone()).collect()
    }

    /// The repositories an account can clone, stale-while-revalidate:
    ///
    /// - the list saved on disk by the last successful fetch is shown immediately;
    /// - GitHub is asked for a fresh one behind it, once per session, or again when
    ///   `force` (the clone dialog's refresh button);
    /// - the list on screen is only replaced when the fresh one arrives, never blanked.
    pub fn load_account_repos(&mut self, login: String, force: bool, cx: &mut Context<Self>) {
        if self.repos_loading.contains(&login) || (!force && self.repos_fresh.contains(&login)) {
            return;
        }
        self.repos_loading.insert(login.clone());
        cx.notify();
        let cached = (!self.account_repos.contains_key(&login))
            .then(|| bridge::cached_account_repos(cx, login.clone()));
        let fresh = bridge::account_repos(cx, login.clone());
        cx.spawn(async move |ws, cx| {
            if let Some(cached) = cached
                && let Ok(Some(repos)) = cached.await
            {
                let login = login.clone();
                let _ = ws.update(cx, |ws, cx| {
                    ws.account_repos.entry(login).or_insert(Ok(repos));
                    cx.notify();
                });
            }
            let fresh = fresh.await.map_err(|e| e.to_string());
            let _ = ws.update(cx, |ws, cx| {
                ws.repos_loading.remove(&login);
                // A failed refresh keeps whatever list is already showing.
                if fresh.is_ok() || !matches!(ws.account_repos.get(&login), Some(Ok(_))) {
                    ws.repos_fresh.insert(login.clone());
                    ws.account_repos.insert(login, fresh);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Show a changed file's diff or a commit's diff in the main pane.
    pub fn select(&mut self, selected: Selected, cx: &mut Context<Self>) {
        self.pulls = None;
        self.workflows = None;
        self.account = None;
        if self.selected.as_ref() != Some(&selected) {
            self.diff = None;
            self.images.clear();
            self.diff_scroll.scroll_to_item(0, ScrollStrategy::Top);
        }
        self.selected = Some(selected);
        self.load_diff(cx);
        cx.notify();
    }

    pub(crate) fn selected(&self) -> Option<&Selected> {
        self.selected.as_ref()
    }

    fn clear_selection(&mut self) {
        self.pulls = None;
        self.workflows = None;
        self.account = None;
        self.selected = None;
        self.diff = None;
    }

    /// (Re)load the diff for the current selection; the old one stays on screen meanwhile.
    fn load_diff(&mut self, cx: &mut Context<Self>) {
        let (Some(repo), Some(selected)) = (self.active_repo(), self.selected.clone()) else {
            return;
        };
        let task = bridge::diff(cx, repo.entry.path.clone(), selected.clone());
        let images = bridge::images(cx, repo.entry.path.clone(), selected.clone());
        cx.spawn(async move |ws, cx| {
            let result = task.await;
            let images = images.await.unwrap_or_default();
            let _ = ws.update(cx, |ws, cx| {
                // The user may have clicked something else while this was loading.
                if ws.selected.as_ref() != Some(&selected) {
                    return;
                }
                ws.images = images;
                ws.diff = Some(match result {
                    Ok(Ok(view)) => Ok(Rc::new(view)),
                    Ok(Err(err)) => Err(err.to_string()),
                    Err(err) => Err(err.to_string()),
                });
                cx.notify();
            });
        })
        .detach();
    }

    /// Re-read which GitHub accounts `gh` is signed into.
    pub fn refresh_accounts(&mut self, cx: &mut Context<Self>) {
        let task = bridge::accounts(cx);
        cx.spawn(async move |ws, cx| {
            let accounts = task.await.unwrap_or_default();
            // Their pictures too (downloaded once, then read from the cache on disk).
            let logins: Vec<String> = accounts.iter().map(|a| a.login.clone()).collect();
            let avatars = ws.update(cx, |ws, cx| {
                ws.accounts = accounts;
                cx.notify();
                bridge::account_avatars(cx, logins)
            });
            if let Ok(avatars) = avatars
                && let Ok(avatars) = avatars.await
            {
                let _ = ws.update(cx, |ws, cx| {
                    ws.account_avatars.extend(avatars);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Settings → Accounts → "Add GitHub account": `gh`'s device flow. We copy the one-time
    /// code, open the GitHub page, and wait for `gh` to report the outcome.
    pub fn add_account(&mut self, cx: &mut Context<Self>) {
        if matches!(self.login, LoginState::Starting | LoginState::Waiting(_)) {
            return;
        }
        self.login = LoginState::Starting;
        cx.notify();
        let start = bridge::start_login(cx);
        cx.spawn(async move |ws, cx| {
            let result = match start.await {
                Err(err) => Err(err),
                Ok(pending) => {
                    let finish = ws.update(cx, |ws, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(pending.code.clone()));
                        cx.open_url(external::GITHUB_DEVICE_URL);
                        ws.login = LoginState::Waiting(pending.code.clone());
                        cx.notify();
                        bridge::finish_login(cx, pending)
                    });
                    match finish {
                        Ok(finish) => finish.await,
                        Err(err) => Err(err),
                    }
                }
            };
            let _ = ws.update(cx, |ws, cx| {
                ws.login = match result {
                    Ok(()) => LoginState::Idle,
                    Err(err) => LoginState::Failed(err.to_string()),
                };
                ws.refresh_accounts(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// "Open in Nermal": open the active repository's directory in the Nermal IDE.
    pub fn open_in_nermal(&mut self, cx: &mut Context<Self>) {
        self.open_repo_in_nermal(self.active, cx);
    }

    /// Open one repository's folder in a new Nermal window (any tab, not just the active).
    pub fn open_repo_in_nermal(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(repo) = self.repos.get(ix) else {
            return;
        };
        let task = bridge::open_in_nermal(cx, repo.entry.path.clone());
        cx.spawn(async move |ws, cx| {
            if let Err(err) = task.await {
                let _ = ws.update(cx, |ws, cx| {
                    ws.notify_add_repo_error(
                        tr!("repo.open_in_nermal_error", error = err.to_string()),
                        cx,
                    )
                });
            }
        })
        .detach();
    }

    /// Clone / Create 在打开任何仓库之前就可能失败，没有 `OpenRepo::action_error` 可以
    /// 挂——用一次性通知代替，跟 sync 那套「挂在仓库上」的错误展示分开也说得通：
    /// 这压根不是某个已打开仓库的操作。
    pub(crate) fn notify_add_repo_error(&self, message: SharedString, cx: &mut App) {
        tracing::warn!("{message}");
        cx.defer(move |cx| {
            if let Some(window) = cx.active_window() {
                let _ = window.update(cx, |_, window, cx| {
                    window.push_notification(
                        gpui_kit::component::notification::Notification::error(message),
                        cx,
                    );
                });
            }
        });
    }

    /// 右键菜单「Discard Changes」。
    pub fn discard_file_change(&mut self, file: String, kind: ChangeKind, cx: &mut Context<Self>) {
        let Some(repo) = self.active_repo() else {
            return;
        };
        let path = repo.entry.path.clone();
        let id = repo.entry.id;
        let task = bridge::discard(cx, path, file, kind);
        cx.spawn(async move |ws, cx| {
            let result = task.await;
            let _ = ws.update(cx, |ws, cx| {
                if let Some(repo) = ws.repos.iter_mut().find(|r| r.entry.id == id) {
                    repo.action_error = action_error(result);
                }
                ws.refresh_active_status(cx);
            });
        })
        .detach();
    }

    /// 右键菜单「Ignore File」。
    pub fn ignore_file(&mut self, file: String, cx: &mut Context<Self>) {
        let Some(repo) = self.active_repo() else {
            return;
        };
        let path = repo.entry.path.clone();
        let id = repo.entry.id;
        let task = bridge::ignore_file(cx, path, file);
        cx.spawn(async move |ws, cx| {
            let result = task.await;
            let _ = ws.update(cx, |ws, cx| {
                if let Some(repo) = ws.repos.iter_mut().find(|r| r.entry.id == id) {
                    repo.action_error = action_error(result);
                }
                ws.refresh_active_status(cx);
            });
        })
        .detach();
    }

    /// 右键菜单「Ignore All *.ext Files」。
    pub fn ignore_file_extension(&mut self, file: String, cx: &mut Context<Self>) {
        let Some(repo) = self.active_repo() else {
            return;
        };
        let path = repo.entry.path.clone();
        let id = repo.entry.id;
        let task = bridge::ignore_file_extension(cx, path, file);
        cx.spawn(async move |ws, cx| {
            let result = task.await;
            let _ = ws.update(cx, |ws, cx| {
                if let Some(repo) = ws.repos.iter_mut().find(|r| r.entry.id == id) {
                    repo.action_error = action_error(result);
                }
                ws.refresh_active_status(cx);
            });
        })
        .detach();
    }

    /// 右键菜单「Copy File Path」：绝对路径。
    pub fn copy_file_path(&self, file: &str, cx: &mut App) {
        let Some(repo) = self.active_repo() else {
            return;
        };
        let abs = repo.entry.path.join(file);
        cx.write_to_clipboard(ClipboardItem::new_string(abs.display().to_string()));
    }

    /// 右键菜单「Copy Relative File Path」：仓库相对路径，就是 `FileStatus.path` 本身。
    pub fn copy_relative_file_path(&self, file: &str, cx: &mut App) {
        cx.write_to_clipboard(ClipboardItem::new_string(file.to_string()));
    }

    /// 重新读一次当前激活仓库的 `git status`（后台线程，见 `bridge::refresh_status`）。
    /// Watch every open repository (and stop watching closed ones). Cheap to call again:
    /// it only acts on repositories that gained or lost a watcher.
    pub(crate) fn sync_watchers(&mut self, cx: &mut Context<Self>) {
        if !self.watch_enabled {
            return;
        }
        let open: Vec<PathBuf> = self.repos.iter().map(|r| r.entry.path.clone()).collect();
        self.watchers.retain(|path, _| open.contains(path));
        for path in open {
            if self.watchers.contains_key(&path) {
                continue;
            }
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<()>();
            let Ok(watcher) = anano_core::git::watch(&path, move || {
                let _ = tx.send(());
            }) else {
                continue;
            };
            self.watchers.insert(path.clone(), watcher);
            cx.spawn(async move |ws, cx| {
                // Ends when the watcher is dropped (its sender goes with it).
                while rx.recv().await.is_some() {
                    let alive = ws.update(cx, |ws, cx| ws.on_repo_changed(&path, cx));
                    if alive.is_err() {
                        break;
                    }
                }
            })
            .detach();
        }
    }

    /// The sidebar's Reload button: read everything about the open repository again.
    pub fn reload_active_repo(&mut self, cx: &mut Context<Self>) {
        self.refresh_active_status(cx);
        self.refresh_active_log(cx);
        self.refresh_active_branches(cx);
    }

    /// `path` changed on disk. If it is the repository on screen, re-read everything that
    /// can have moved: the changes list, history, and branches with ahead / behind and last
    /// fetched (so the Fetch / Pull / Push button follows a push or fetch made elsewhere).
    /// Other repositories are read when they are switched to.
    fn on_repo_changed(&mut self, path: &std::path::Path, cx: &mut Context<Self>) {
        tracing::debug!(target: "anano::watch", "repo changed: {}", path.display());
        if self.active_repo().is_some_and(|r| r.entry.path == path) {
            self.refresh_active_status(cx);
            self.refresh_active_log(cx);
            self.refresh_active_branches(cx);
        }
    }

    pub fn refresh_active_status(&mut self, cx: &mut Context<Self>) {
        // New repositories are noticed here: every way of adding one reads its status.
        self.sync_watchers(cx);
        let Some(repo) = self.repos.get_mut(self.active) else {
            return;
        };
        // `loading` is only true until the first read: later refreshes (this runs whenever
        // the repository changes on disk) swap the data in without blanking the view.
        let path = repo.entry.path.clone();
        let id = repo.entry.id;
        cx.notify();

        let task = bridge::refresh_status(cx, path);
        cx.spawn(async move |ws, cx| {
            let result = task.await;
            let _ = ws.update(cx, |ws, cx| {
                let Some(repo) = ws.repos.iter_mut().find(|r| r.entry.id == id) else {
                    return;
                };
                repo.loading = false;
                match result {
                    Ok(Ok(statuses)) => {
                        repo.statuses = statuses;
                        repo.error = None;
                        let changed: std::collections::HashSet<&str> =
                            repo.statuses.iter().map(|s| s.path.as_str()).collect();
                        repo.unchecked
                            .retain(|path| changed.contains(path.as_str()));
                    }
                    Ok(Err(err)) => repo.error = Some(err.to_string()),
                    Err(err) => repo.error = Some(err.to_string()),
                }
                // Keep a file selected whenever there are changes (see `ensure_file_selected`);
                // otherwise the file on screen may just have changed, so re-read its diff.
                let is_active = ws.active_repo().is_some_and(|r| r.entry.id == id);
                if !(is_active && ws.ensure_file_selected(cx))
                    && matches!(ws.selected, Some(Selected::File { .. }))
                {
                    ws.load_diff(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 重新读一次当前激活仓库的最近提交历史。
    pub fn refresh_active_log(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.repos.get_mut(self.active) else {
            return;
        };
        let path = repo.entry.path.clone();
        let id = repo.entry.id;
        cx.notify();

        let task = bridge::refresh_log(cx, path, RECENT_COMMITS_LIMIT);
        cx.spawn(async move |ws, cx| {
            let result = task.await;
            let _ = ws.update(cx, |ws, cx| {
                let Some(repo) = ws.repos.iter_mut().find(|r| r.entry.id == id) else {
                    return;
                };
                repo.log_loading = false;
                if let Ok(Ok(commits)) = result {
                    repo.commits = commits;
                }
                let head = repo.commits.first().map(|c| c.id.clone());
                let mut emails: Vec<String> =
                    repo.commits.iter().map(|c| c.email.clone()).collect();
                emails.sort();
                emails.dedup();
                ws.load_avatars(emails, cx);
                if std::mem::take(&mut ws.show_head_when_loaded)
                    && let Some(head) = head
                {
                    ws.sidebar_tab = SidebarTab::History;
                    ws.select(Selected::Commit(head), cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 重新读一次当前激活仓库的分支列表（含 ahead/behind）。
    pub fn refresh_active_branches(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.repos.get_mut(self.active) else {
            return;
        };
        repo.branches_loading = true;
        let path = repo.entry.path.clone();
        let id = repo.entry.id;
        cx.notify();

        // The quick facts (which branch HEAD names, origin, last fetch) are a couple of
        // file reads; listing branches with their ahead/behind counts can take a while on a
        // big repository. Apply the quick ones as soon as they arrive instead of making the
        // toolbar wait for the slow one.
        let facts_task = bridge::repo_facts(cx, path.clone());
        cx.spawn(async move |ws, cx| {
            let Ok(facts) = facts_task.await else { return };
            let _ = ws.update(cx, |ws, cx| {
                let Some(repo) = ws.repos.iter_mut().find(|r| r.entry.id == id) else {
                    return;
                };
                repo.origin = facts.origin.clone();
                repo.last_fetched = facts.last_fetched;
                repo.facts_loaded = true;
                // Remember the branch for next time (see `OpenRepo::branch_label`).
                let branch_changed = repo.entry.last_branch != facts.head_branch
                    || (facts.origin.is_some() && repo.entry.last_origin != facts.origin);
                if facts.head_branch.is_some() {
                    repo.entry.last_branch = facts.head_branch.clone();
                }
                if facts.origin.is_some() {
                    repo.entry.last_origin = facts.origin.clone();
                }
                repo.head_branch = facts.head_branch;
                if branch_changed {
                    ws.persist_repos(cx);
                }
                if ws.active_repo().is_some_and(|r| r.entry.id == id) {
                    ws.maybe_auto_fetch(cx);
                }
                cx.notify();
            });
        })
        .detach();

        let task = bridge::branches(cx, path);
        cx.spawn(async move |ws, cx| {
            let result = task.await;
            let _ = ws.update(cx, |ws, cx| {
                let Some(repo) = ws.repos.iter_mut().find(|r| r.entry.id == id) else {
                    return;
                };
                repo.branches_loading = false;
                if let Ok(Ok(branches)) = result {
                    repo.branches = branches;
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Dismiss the error strip under the toolbar.
    pub fn dismiss_action_error(&mut self, cx: &mut Context<Self>) {
        if let Some(repo) = self.repos.get_mut(self.active) {
            repo.action_error = None;
            cx.notify();
        }
    }

    /// Fetch profile pictures for authors we haven't asked about yet.
    fn load_avatars(&mut self, emails: Vec<String>, cx: &mut Context<Self>) {
        let mut wanted: Vec<String> = emails
            .into_iter()
            .filter(|e| !e.is_empty() && !self.avatars.contains_key(e))
            .collect();
        // Pictures already on disk show at once.
        wanted.retain(|email| match anano_core::external::cached_avatar(email) {
            Some(path) => {
                self.avatars.insert(email.clone(), Some(path));
                false
            }
            None => true,
        });
        if wanted.is_empty() {
            cx.notify();
            return;
        }
        for email in &wanted {
            self.avatars.insert(email.clone(), None);
        }
        let task = bridge::avatars(cx, wanted);
        cx.spawn(async move |ws, cx| {
            let Ok(found) = task.await else { return };
            let _ = ws.update(cx, |ws, cx| {
                ws.avatars.extend(found);
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn close_branch_popover(&mut self, cx: &mut Context<Self>) {
        self.branch_popover_open = false;
        cx.notify();
    }

    /// Create a branch from `from` (a commit id, or `HEAD` when `None`) and switch to it.
    pub fn create_branch(&mut self, name: String, from: Option<String>, cx: &mut Context<Self>) {
        let Some(repo) = self.active_repo() else {
            return;
        };
        if name.trim().is_empty() {
            return;
        }
        let path = repo.entry.path.clone();
        let id = repo.entry.id;
        let task = bridge::new_branch(cx, path, name, from);
        cx.spawn(async move |ws, cx| {
            let result = task.await;
            let _ = ws.update(cx, |ws, cx| {
                if let Some(repo) = ws.repos.iter_mut().find(|r| r.entry.id == id) {
                    repo.action_error = action_error(result);
                }
                ws.refresh_active_status(cx);
                ws.refresh_active_log(cx);
                ws.refresh_active_branches(cx);
            });
        })
        .detach();
    }

    /// Switch to `name`. With uncommitted changes in the tree, ask first what should
    /// happen to them (GitHub Desktop's "Switch Branch" dialog); otherwise just switch.
    pub fn request_switch_branch(
        &mut self,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(repo) = self.active_repo() else {
            return;
        };
        if repo.statuses.is_empty() {
            self.switch_branch(name, false, cx);
        } else {
            self.open_switch_branch_dialog(name, window, cx);
        }
    }

    /// Switch branches; `leave_changes` stashes uncommitted work on the branch being left
    /// (it comes back when you return to that branch), otherwise it follows you.
    pub fn switch_branch(&mut self, name: String, leave_changes: bool, cx: &mut Context<Self>) {
        let Some(repo) = self.active_repo() else {
            return;
        };
        let path = repo.entry.path.clone();
        let id = repo.entry.id;
        self.clear_selection();
        let task = bridge::switch_branch(cx, path, name, leave_changes);
        cx.spawn(async move |ws, cx| {
            let result = task.await;
            let _ = ws.update(cx, |ws, cx| {
                if let Some(repo) = ws.repos.iter_mut().find(|r| r.entry.id == id) {
                    repo.action_error = action_error(result);
                }
                ws.refresh_active_status(cx);
                ws.refresh_active_log(cx);
                ws.refresh_active_branches(cx);
            });
        })
        .detach();
    }

    /// Rename a local branch (the branch panel's right-click menu).
    pub fn rename_branch(&mut self, old: String, new: String, cx: &mut Context<Self>) {
        let Some(repo) = self.active_repo() else {
            return;
        };
        if new.trim().is_empty() || new == old {
            return;
        }
        let id = repo.entry.id;
        let task = bridge::rename_branch(cx, repo.entry.path.clone(), old, new);
        cx.spawn(async move |ws, cx| {
            let result = task.await;
            let _ = ws.update(cx, |ws, cx| {
                if let Some(repo) = ws.repos.iter_mut().find(|r| r.entry.id == id) {
                    repo.action_error = action_error(result);
                }
                ws.refresh_active_branches(cx);
            });
        })
        .detach();
    }

    /// 删除一个本地分支；删掉当前签出的那个会失败（libgit2 自己拒绝），错误照样冒出来。
    pub fn delete_branch(&mut self, name: String, cx: &mut Context<Self>) {
        let Some(repo) = self.active_repo() else {
            return;
        };
        let path = repo.entry.path.clone();
        let id = repo.entry.id;
        let task = bridge::remove_branch(cx, path, name);
        cx.spawn(async move |ws, cx| {
            let result = task.await;
            let _ = ws.update(cx, |ws, cx| {
                if let Some(repo) = ws.repos.iter_mut().find(|r| r.entry.id == id) {
                    repo.action_error = action_error(result);
                }
                ws.refresh_active_branches(cx);
            });
        })
        .detach();
    }

    /// Make the active repository fetch / pull / push as one connected GitHub account.
    pub fn set_repo_account(&mut self, login: String, cx: &mut Context<Self>) {
        let Some(repo) = self.active_repo() else {
            return;
        };
        let id = repo.entry.id;
        let task = bridge::set_origin_account(cx, repo.entry.path.clone(), login);
        cx.spawn(async move |ws, cx| {
            let result = task.await;
            let _ = ws.update(cx, |ws, cx| {
                if let Some(repo) = ws.repos.iter_mut().find(|r| r.entry.id == id) {
                    repo.action_error = action_error(result);
                }
                ws.refresh_active_branches(cx);
            });
        })
        .detach();
    }

    /// Fetch the repository that was just opened or switched to, so its ahead/behind and
    /// history are current without pressing "Fetch origin". Once per switch; skipped if it
    /// was fetched within the last half minute (flipping between tabs shouldn't hammer the
    /// remote) or has no origin; and silent on failure, since the user didn't ask for it.
    pub(crate) fn maybe_auto_fetch(&mut self, cx: &mut Context<Self>) {
        const FRESH: std::time::Duration = std::time::Duration::from_secs(30);
        let Some(repo) = self.repos.get_mut(self.active) else {
            return;
        };
        // No origin known yet: the branch refresh in flight calls back here once it is.
        if !repo.needs_fetch || repo.origin.is_none() || repo.syncing.is_some() {
            return;
        }
        repo.needs_fetch = false;
        let fresh = repo
            .last_fetched
            .and_then(|at| at.elapsed().ok())
            .is_some_and(|age| age < FRESH);
        if !fresh {
            repo.quiet_sync = true;
            self.fetch(cx);
        }
    }

    /// `git fetch`：只更新远程跟踪分支，不碰工作树。Fetch / Pull / Push 是三个独立
    /// 按钮——commit 只管本地提交，跟远程同步完全是另一回事，不该混在一起。
    pub fn fetch(&mut self, cx: &mut Context<Self>) {
        self.run_sync(SyncKind::Fetch, cx);
    }

    /// `git pull`：只做快进；分叉了会报错，不自动造合并提交（那需要用户介入）。
    pub fn pull(&mut self, cx: &mut Context<Self>) {
        self.run_sync(SyncKind::Pull, cx);
    }

    /// `git push`：推当前分支到同名远程分支。
    pub fn push(&mut self, cx: &mut Context<Self>) {
        self.run_sync(SyncKind::Push, cx);
    }

    /// The branch and its upstream have each moved on: merge (or rebase onto) the upstream.
    /// Conflicts, if any, are left for the Changes list.
    pub fn integrate_upstream(&mut self, rebase: bool, cx: &mut Context<Self>) {
        let Some(repo) = self.active_repo() else {
            return;
        };
        if repo.syncing.is_some() {
            return;
        }
        let (path, id) = (repo.entry.path.clone(), repo.entry.id);
        if let Some(repo) = self.repos.iter_mut().find(|r| r.entry.id == id) {
            repo.syncing = Some(SyncKind::Pull);
            repo.action_error = None;
        }
        let task = bridge::integrate_upstream(cx, path, rebase);
        cx.notify();
        cx.spawn(async move |ws, cx| {
            let result = task.await;
            let _ = ws.update(cx, |ws, cx| {
                if let Some(repo) = ws.repos.iter_mut().find(|r| r.entry.id == id) {
                    repo.syncing = None;
                    repo.action_error = action_error(result);
                }
                ws.refresh_active_status(cx);
                ws.refresh_active_log(cx);
                ws.refresh_active_branches(cx);
            });
        })
        .detach();
    }

    fn run_sync(&mut self, kind: SyncKind, cx: &mut Context<Self>) {
        let Some(repo) = self.active_repo() else {
            return;
        };
        // Prevention: pushing a branch the remote has moved past is refused by the server
        // anyway, and is how a branch ends up diverged. Bring the remote's commits in first
        // (a fast-forward when possible; otherwise the Merge / Rebase choice appears).
        let kind = match (kind, repo.current_branch().and_then(|b| b.ahead_behind)) {
            (SyncKind::Push, Some((_, behind))) if behind > 0 => SyncKind::Pull,
            _ => kind,
        };
        if repo.syncing.is_some() {
            return; // 上一次还没跑完，别叠加一次
        }
        let path = repo.entry.path.clone();
        let id = repo.entry.id;
        let branch_name = repo
            .current_branch()
            .map(|b| b.name.clone())
            .or_else(|| repo.head_branch.clone());

        if let Some(repo) = self.repos.iter_mut().find(|r| r.entry.id == id) {
            repo.syncing = Some(kind);
            repo.action_error = None;
        }
        cx.notify();

        let git_task = match kind {
            SyncKind::Fetch => bridge::fetch_remote(cx, path.clone()),
            SyncKind::Pull => bridge::pull_remote(cx, path.clone()),
            SyncKind::Push => {
                let Some(branch_name) = branch_name else {
                    if let Some(repo) = self.repos.iter_mut().find(|r| r.entry.id == id) {
                        repo.syncing = None;
                        repo.action_error =
                            Some("no branch checked out (or branches haven't loaded yet)".into());
                    }
                    cx.notify();
                    return;
                };
                bridge::push_remote(cx, path.clone(), branch_name)
            }
        };
        let lfs_op = match kind {
            SyncKind::Fetch => LfsOp::Fetch,
            SyncKind::Pull => LfsOp::Pull,
            SyncKind::Push => LfsOp::Push,
        };

        cx.spawn(async move |ws, cx| {
            let git_result = git_task.await;
            let git_ok = matches!(git_result, Ok(Ok(())));
            let lfs_result = if git_ok {
                let lfs_task = ws.update(cx, |_, cx| bridge::sync_lfs(cx, path.clone(), lfs_op));
                match lfs_task {
                    Ok(task) => Some(task.await),
                    Err(_) => None,
                }
            } else {
                None
            };

            let _ = ws.update(cx, |ws, cx| {
                let Some(repo) = ws.repos.iter_mut().find(|r| r.entry.id == id) else {
                    return;
                };
                repo.syncing = None;
                repo.action_error = action_error(git_result);
                if std::mem::take(&mut repo.quiet_sync) {
                    repo.action_error = None;
                }
                if let Some(lfs_result) = lfs_result {
                    match lfs_result {
                        Ok(Ok(status)) => repo.lfs_status = Some(status),
                        Ok(Err(err)) => repo.action_error = Some(err.to_string()),
                        Err(err) => repo.action_error = Some(err.to_string()),
                    }
                }
                ws.refresh_active_status(cx);
                ws.refresh_active_log(cx);
                ws.refresh_active_branches(cx);
            });
        })
        .detach();
    }

    /// Tick or untick one changed file for the next commit.
    pub fn toggle_file(&mut self, file: String, cx: &mut Context<Self>) {
        if let Some(repo) = self.repos.get_mut(self.active) {
            if !repo.unchecked.remove(&file) {
                repo.unchecked.insert(file);
            }
            cx.notify();
        }
    }

    /// The header checkbox: tick everything, or untick everything if it all already is.
    pub fn toggle_all_files(&mut self, cx: &mut Context<Self>) {
        if let Some(repo) = self.repos.get_mut(self.active) {
            if repo.unchecked.is_empty() {
                repo.unchecked = repo.changed_files().into_iter().map(|(p, _)| p).collect();
            } else {
                repo.unchecked.clear();
            }
            cx.notify();
        }
    }

    /// The Commit button: commit the ticked files with the summary as the subject line
    /// and the description, if any, as the body. Unticked files are left out and left alone.
    /// Nothing ticked or no summary: nothing happens (the button is disabled for both).
    pub fn commit_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.active_repo() else {
            return;
        };
        let summary = self.commit_input.read(cx).value().trim().to_string();
        let description = self.commit_description.read(cx).value().trim().to_string();
        let include = repo.checked_files();
        if summary.is_empty() || include.is_empty() {
            return;
        }
        let message = if description.is_empty() {
            summary
        } else {
            format!("{summary}\n\n{description}")
        };
        let exclude: Vec<String> = repo.unchecked.iter().cloned().collect();
        // Commit & push: the push half needs somewhere to push to.
        let has_origin = repo.origin.is_some();
        let task = bridge::commit_selected(cx, repo.entry.path.clone(), message, include, exclude);
        self.commit_input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.commit_description
            .update(cx, |input, cx| input.set_value("", window, cx));

        cx.spawn(async move |ws, cx| {
            let result = task.await;
            let committed = matches!(result, Ok(Ok(_)));
            let _ = ws.update(cx, |ws, cx| {
                if let Some(repo) = ws.repos.get_mut(ws.active) {
                    repo.action_error = action_error(result);
                }
                ws.refresh_active_status(cx);
                ws.refresh_active_log(cx);
                // The first commit is what brings the branch into existence, and every
                // commit changes ahead/behind.
                ws.refresh_active_branches(cx);
                if committed && has_origin {
                    ws.push(cx);
                }
            });
        })
        .detach();
    }

    /// 当前布局的快照；`tab_order` / `active` 复用为仓库 id 的顺序 / 当前激活项。
    pub(crate) fn workspace_state(&self) -> WorkspaceState {
        WorkspaceState {
            tab_order: self
                .repos
                .iter()
                .filter(|r| r.in_tabs)
                .map(|r| r.entry.id)
                .collect(),
            active: self.repos.get(self.active).map(|r| r.entry.id),
            sidebar_width: self.sidebar_width,
            sidebar_collapsed: self.sidebar_collapsed,
            theme: self.theme,
            ..Default::default()
        }
    }

    pub(crate) fn persist_workspace(&self, cx: &App) {
        if let Some(store) = store(cx) {
            store.write_workspace(self.workspace_state());
        }
    }

    fn persist_repos(&self, cx: &App) {
        self.persist_workspace(cx);
        if let Some(store) = store(cx) {
            let list = self.repos.iter().map(|r| r.entry.clone()).collect();
            store.write_repo_list(list);
        }
    }

    pub fn sidebar_collapsed(&self) -> bool {
        self.sidebar_collapsed
    }

    pub fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.sidebar_collapsed = !self.sidebar_collapsed;
        if !self.sidebar_collapsed {
            self.reset_workspace_panels(cx);
        }
        self.persist_workspace(cx);
        cx.notify();
    }

    fn reset_workspace_panels(&self, cx: &mut App) {
        let count = self.sidebar_state.read(cx).sizes().len();
        self.sidebar_state.update(cx, |state, cx| {
            for ix in 0..count {
                state.reset_panel(ix, cx);
            }
        });
    }

    fn on_sidebar_resized(&mut self, state: &Entity<ResizableState>, cx: &mut Context<Self>) {
        let Some(width) = state.read(cx).sizes().first().copied().map(f32::from) else {
            return;
        };
        if self.sidebar_width != Some(width) {
            self.sidebar_width = Some(width);
            self.persist_workspace(cx);
        }
        self.reset_workspace_panels(cx);
    }

    pub fn set_theme_global(&mut self, pref: ThemePref, cx: &mut Context<Self>) {
        self.set_theme_with(pref, None, cx);
    }

    fn set_theme_with(
        &mut self,
        pref: ThemePref,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        if self.theme == pref {
            return;
        }
        self.theme = pref;
        apply_theme(pref, window, cx);
        self.persist_workspace(cx);
        cx.notify();
    }

    pub fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        open_settings(cx.entity(), window, cx);
    }

    pub fn open_settings_page(
        &mut self,
        page: SettingsPage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        open_settings_page(cx.entity(), page, window, cx);
    }

    /// 标题栏副标题：当前激活仓库的名字，没有仓库打开时不显示。
    fn title_bar_subtitle(&self, _cx: &App) -> Option<SharedString> {
        self.active_repo()
            .map(|r| SharedString::from(r.entry.display_name.clone()))
    }

    fn render_title_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        TitleBar::new().child(
            h_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_1p5()
                .when(cfg!(target_os = "macos"), |h| h.pr(px(80.)))
                .text_sm()
                .min_w_0()
                .child(
                    div()
                        .flex_none()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(cx.theme().foreground)
                        .child(APP_NAME),
                )
                .when_some(self.title_bar_subtitle(cx), |d, subtitle| {
                    d.child(
                        div()
                            .flex_none()
                            .text_color(cx.theme().muted_foreground)
                            .child("·"),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(cx.theme().muted_foreground)
                            .truncate()
                            .child(subtitle),
                    )
                }),
        )
    }

    fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let update_hint = update::hint_version(&self.update_status)
            .map(|(version, staged)| (version.clone(), staged));
        StatusBar::new()
            .py_0p5()
            .right(h_flex().items_center().gap_1().when_some(
                update_hint,
                |bar, (version, staged)| {
                    bar.child(if staged {
                        crate::ui::button("update-restart")
                            .ghost()
                            .xsmall()
                            .icon(IconName::ArrowUp)
                            .label(tr!("status.update_restart", version = version))
                            .tooltip(tr!("status.update_restart_tooltip"))
                            .on_click(|_, _, cx| update::restart(cx))
                    } else {
                        crate::ui::button("update-available")
                            .ghost()
                            .xsmall()
                            .icon(IconName::ArrowUp)
                            .label(tr!("status.update_available", version = version))
                            .tooltip(tr!("status.update_available_tooltip"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_settings_page(SettingsPage::Updates, window, cx)
                            }))
                    })
                },
            ))
    }
}

/// `bridge` 里那些 `Task<anyhow::Result<Result<T, GitError>>>` 的通用错误提取：
/// 成功是 `None`，两层里任何一层出错都取它的说明文字——调用方只关心「要不要显示」。
fn action_error<T>(result: anyhow::Result<Result<T, GitError>>) -> Option<String> {
    match result {
        Ok(Ok(_)) => None,
        Ok(Err(err)) => Some(err.to_string()),
        Err(err) => Some(err.to_string()),
    }
}

/// 从 git URL 推断仓库名：取最后一段，去掉常见的 `.git` 后缀与末尾斜杠。
/// 推不出来（比如空字符串）就退回一个占位名，`init`/`clone` 到磁盘上的目录名
/// 总得是个合法路径分量。
fn repo_name_from_url(url: &str) -> String {
    let trimmed = url.trim().trim_end_matches('/');
    let last = trimmed
        .rsplit(['/', ':'])
        .next()
        .unwrap_or(trimmed)
        .trim_end_matches(".git");
    if last.is_empty() {
        "repository".to_string()
    } else {
        last.to_string()
    }
}

/// 按 `order` 排列仓库：顺序里没有的 id 跳过；没被提到的仓库按原顺序追加在末尾。
fn reorder(repos: &mut Vec<OpenRepo>, order: &[Ulid]) {
    let mut ordered = Vec::with_capacity(repos.len());
    for id in order {
        if let Some(pos) = repos.iter().position(|r| r.entry.id == *id) {
            ordered.push(repos.remove(pos));
        }
    }
    ordered.append(repos);
    *repos = ordered;
}

pub(crate) fn apply_theme(pref: ThemePref, window: Option<&mut Window>, cx: &mut App) {
    match pref {
        ThemePref::System => Theme::sync_system_appearance(window, cx),
        ThemePref::Light => Theme::change(ThemeMode::Light, window, cx),
        ThemePref::Dark => Theme::change(ThemeMode::Dark, window, cx),
    }
}

fn render_banner(text: String) -> impl IntoElement {
    Alert::error("store-banner", text).banner().xsmall()
}

/// Frame statistics for the log (`RUST_LOG=anano=debug`): how often the window rebuilds
/// its element tree and how long that takes, so "it lags" can be told apart into "too many
/// frames" and "frames that are too slow" without guessing.
struct FrameStats {
    since: std::time::Instant,
    frames: u32,
    total: std::time::Duration,
    worst: std::time::Duration,
}

thread_local! {
    static FRAMES: std::cell::RefCell<Option<FrameStats>> = const { std::cell::RefCell::new(None) };
}

fn record_frame(took: std::time::Duration) {
    // A single slow build is worth a line of its own.
    if took > std::time::Duration::from_millis(8) {
        tracing::warn!(target: "anano::frames", "slow render: {:.1} ms", took.as_secs_f64() * 1000.0);
    }
    FRAMES.with(|cell| {
        let mut slot = cell.borrow_mut();
        let stats = slot.get_or_insert_with(|| FrameStats {
            since: std::time::Instant::now(),
            frames: 0,
            total: Default::default(),
            worst: Default::default(),
        });
        stats.frames += 1;
        stats.total += took;
        stats.worst = stats.worst.max(took);
        if stats.since.elapsed() >= std::time::Duration::from_secs(2) {
            tracing::info!(
                target: "anano::frames",
                "{} renders in {:.1}s, avg {:.2} ms, worst {:.1} ms",
                stats.frames,
                stats.since.elapsed().as_secs_f64(),
                stats.total.as_secs_f64() * 1000.0 / stats.frames as f64,
                stats.worst.as_secs_f64() * 1000.0
            );
            *slot = None;
        }
    });
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let started = std::time::Instant::now();
        let tree = self.render_tree(window, cx);
        record_frame(started.elapsed());
        tree
    }
}

impl Workspace {
    fn render_tree(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.panels_normalize_pending && !self.sidebar_state.read(cx).sizes().is_empty() {
            self.panels_normalize_pending = false;
            self.reset_workspace_panels(cx);
        }
        let sidebar_width = px(self.sidebar_width.unwrap_or(SIDEBAR_DEFAULT_WIDTH));
        let dialog_layer = gpui_kit::component::Root::render_dialog_layer(window, cx);
        // Without this layer `push_notification` has nowhere to draw, and every clone /
        // create / "open in Nermal" error was being reported to nobody.
        let notification_layer = gpui_kit::component::Root::render_notification_layer(window, cx);
        let sheet_layer = gpui_kit::component::Root::render_sheet_layer(window, cx);
        div()
            .id("workspace")
            .role(Role::Group)
            .aria_label(tr!("app.workspace_aria"))
            .key_context("Workspace")
            .track_focus(&self.focus_handle)
            .size_full()
            .flex()
            .flex_col()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .on_action(
                cx.listener(|this, _: &OpenRepoAction, window, cx| {
                    this.open_repo_dialog(window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &CloseTab, _, cx| {
                let ix = this.active;
                this.close_tab(ix, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| this.toggle_sidebar(cx)))
            .on_action(cx.listener(|this, _: &RevealRepository, _, cx| this.reveal_active_repo(cx)))
            .on_action(
                cx.listener(|this, _: &OpenRepositoryOnGitHub, _, cx| {
                    this.open_active_on_github(cx)
                }),
            )
            .on_action(cx.listener(|this, _: &SyncNow, _, cx| this.sync_now(cx)))
            .on_action(
                cx.listener(|this, _: &OpenSettings, window, cx| this.open_settings(window, cx)),
            )
            .child(self.render_title_bar(cx))
            .when_some(banner(cx), |d, text| d.child(render_banner(text)))
            .child(
                h_flex().flex_1().min_h_0().w_full().items_stretch().child(
                    div().flex_1().min_w_0().h_full().child(
                        h_resizable("workspace")
                            .with_state(&self.sidebar_state)
                            .on_resize(cx.listener(
                                |this, state: &Entity<ResizableState>, _, cx| {
                                    this.on_sidebar_resized(state, cx)
                                },
                            ))
                            .child(
                                resizable_panel()
                                    .size(sidebar_width)
                                    .size_range(px(SIDEBAR_MIN_WIDTH)..px(SIDEBAR_MAX_WIDTH))
                                    .flex_none()
                                    .visible(!self.sidebar_collapsed)
                                    .child(self.render_sidebar(window, cx)),
                            )
                            .child(
                                resizable_panel().child(
                                    v_flex()
                                        .size_full()
                                        .min_w_0()
                                        .children(self.render_toolbar(cx))
                                        .children(self.render_repo_notice(cx))
                                        .child(self.render_tab_strip(cx))
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_h_0()
                                                .child(self.render_status_pane(cx)),
                                        ),
                                ),
                            ),
                    ),
                ),
            )
            .child(self.render_status_bar(cx))
            .children(sheet_layer)
            .children(dialog_layer)
            .children(notification_layer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_name_from_url_strips_dot_git_and_trailing_slash() {
        assert_eq!(
            repo_name_from_url("https://github.com/user/anano.git"),
            "anano"
        );
        assert_eq!(repo_name_from_url("git@github.com:user/anano.git"), "anano");
        assert_eq!(repo_name_from_url("https://example.com/repo/"), "repo");
        assert_eq!(repo_name_from_url(""), "repository");
    }

    #[test]
    fn reorder_follows_the_given_order_and_appends_unlisted_by_position() {
        let a = RepoEntry::new(PathBuf::from("/a"));
        let b = RepoEntry::new(PathBuf::from("/b"));
        let c = RepoEntry::new(PathBuf::from("/c"));
        let mut repos = vec![
            OpenRepo::new(a.clone()),
            OpenRepo::new(b.clone()),
            OpenRepo::new(c.clone()),
        ];
        reorder(&mut repos, &[c.id, a.id]);
        let ids: Vec<Ulid> = repos.iter().map(|r| r.entry.id).collect();
        assert_eq!(ids, vec![c.id, a.id, b.id]);
    }

    #[test]
    fn reorder_ignores_ids_not_present() {
        let a = RepoEntry::new(PathBuf::from("/a"));
        let missing = Ulid::generate();
        let mut repos = vec![OpenRepo::new(a.clone())];
        reorder(&mut repos, &[missing, a.id]);
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].entry.id, a.id);
    }

    /// 端到端钉住 [`Workspace`] 本体的状态转移：打开、切换、关闭都要维护好 `active`
    /// 下标——这是 phase 1 skeleton 里唯一非纯函数的业务逻辑，值得一个真实的 gpui 测试。
    #[gpui_kit::test]
    fn opening_switching_and_closing_repos_keeps_active_consistent(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        // The repositories' git reads finish on tokio's threads, outside the test scheduler.
        cx.executor().allow_parking();
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::bridge::init(cx);
        });
        let (workspace, cx) = cx.add_window_view(Workspace::new);
        cx.update(|window, cx| {
            workspace.update(cx, |ws, cx| {
                assert_eq!(ws.repo_count(), 0);
                assert!(ws.active_repo().is_none());

                ws.open_repo_path(PathBuf::from("/tmp/anano-test-a"), cx);
                assert_eq!(ws.repo_count(), 1);
                assert_eq!(ws.active_index(), 0);

                ws.open_repo_path(PathBuf::from("/tmp/anano-test-b"), cx);
                assert_eq!(ws.repo_count(), 2);
                // 新打开的仓库立即成为激活项
                assert_eq!(ws.active_index(), 1);

                ws.activate(0, cx);
                assert_eq!(ws.active_index(), 0);

                // 关掉激活项左边的一个不该移动激活下标；这里关的正是激活项本身（下标 0），
                // 关闭后只剩一个仓库，激活下标必须夹回 0（不能越界指向已不存在的第二项）
                ws.close_repo(0, window, cx);
                assert_eq!(ws.repo_count(), 1);
                assert_eq!(ws.active_index(), 0);
                assert_eq!(
                    ws.active_repo().unwrap().entry.path,
                    PathBuf::from("/tmp/anano-test-b")
                );
            });
        });
    }

    /// The tab's × closes the tab and nothing else: the repository is still in the app
    /// (and so in the repository list), and picking it again brings its tab back.
    #[gpui_kit::test]
    fn closing_a_tab_keeps_the_repository(cx: &mut gpui_kit::TestAppContext) {
        cx.executor().allow_parking();
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::bridge::init(cx);
        });
        let (workspace, cx) = cx.add_window_view(Workspace::new);
        cx.update(|_, cx| {
            workspace.update(cx, |ws, cx| {
                ws.open_repo_path(PathBuf::from("/tmp/anano-tab-a"), cx);
                ws.open_repo_path(PathBuf::from("/tmp/anano-tab-b"), cx);
                assert_eq!(ws.active_index(), 1);

                ws.close_tab(1, cx);
                assert_eq!(ws.repo_count(), 2, "closing a tab removed the repository");
                assert!(!ws.is_tab(1) && ws.is_tab(0));
                assert_eq!(ws.active_index(), 0, "the remaining tab should take over");
                assert_eq!(
                    ws.switcher_entries().len(),
                    2,
                    "it left the repository list"
                );
                // Only tabs are saved as tabs; the repository itself is still saved.
                assert_eq!(ws.workspace_state().tab_order.len(), 1);

                ws.activate(1, cx);
                assert!(ws.is_tab(1), "picking it again didn't reopen its tab");
                assert_eq!(ws.active_index(), 1);
            });
        });
    }

    /// The Changes tab end to end on a real repository: one row per file, untick one,
    /// commit with a summary and a description, and only the ticked file is committed.
    #[gpui_kit::test]
    fn commit_takes_the_ticked_files_with_summary_and_description(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.executor().allow_parking();
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::bridge::init(cx);
        });
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().canonicalize().unwrap();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .args(args)
                .current_dir(&repo)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}");
            String::from_utf8(out.stdout).unwrap()
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", ""]);
        git(&["config", "user.name", "test"]);
        std::fs::write(repo.join("keep.txt"), "one\n").unwrap();
        std::fs::write(repo.join("skip.txt"), "one\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "init"]);
        std::fs::write(repo.join("keep.txt"), "two\n").unwrap();
        std::fs::write(repo.join("skip.txt"), "two\n").unwrap();

        let (workspace, cx) = cx.add_window_view(Workspace::new);
        workspace.update(cx, |ws, cx| ws.open_repo_path(repo.clone(), cx));
        let wait = |cx: &mut gpui_kit::VisualTestContext, done: &dyn Fn(&Workspace) -> bool| {
            for _ in 0..300 {
                cx.run_until_parked();
                if workspace.read_with(cx, |ws, _| done(ws)) {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            panic!("timed out waiting for the repository");
        };
        wait(cx, &|ws| {
            ws.active_repo().unwrap().changed_files().len() == 2
        });

        cx.update(|window, cx| {
            workspace.update(cx, |ws, cx| {
                // Everything starts ticked; untick one.
                assert_eq!(ws.active_repo().unwrap().checked_files().len(), 2);
                ws.toggle_file("skip.txt".into(), cx);
                assert_eq!(ws.active_repo().unwrap().checked_files(), ["keep.txt"]);
                ws.commit_input
                    .update(cx, |i, cx| i.set_value("Summary line", window, cx));
                ws.commit_description
                    .update(cx, |i, cx| i.set_value("Longer explanation.", window, cx));
                ws.commit_active(window, cx);
            });
        });
        wait(cx, &|ws| {
            ws.active_repo().unwrap().changed_files().len() == 1
        });

        assert_eq!(
            git(&["log", "-1", "--format=%B"]).trim(),
            "Summary line\n\nLonger explanation."
        );
        assert_eq!(
            git(&["show", "--name-only", "--format=", "HEAD"]).trim(),
            "keep.txt"
        );
        workspace.read_with(cx, |ws, cx| {
            let repo = ws.active_repo().unwrap();
            assert_eq!(
                repo.changed_files()[0].0,
                "skip.txt",
                "the unticked file was committed"
            );
            assert!(
                ws.commit_input.read(cx).value().is_empty(),
                "summary wasn't cleared"
            );
        });
    }

    /// Opening a repository that has changes shows the first changed file's diff straight
    /// away, switching files works, and once everything is committed the stale diff goes.
    #[gpui_kit::test]
    fn the_first_changed_file_is_selected_by_default(cx: &mut gpui_kit::TestAppContext) {
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
        git(&["config", "user.email", ""]);
        git(&["config", "user.name", "test"]);
        std::fs::write(repo.join("a.txt"), "one\n").unwrap();
        std::fs::write(repo.join("b.txt"), "one\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "init"]);
        std::fs::write(repo.join("a.txt"), "two\n").unwrap();
        std::fs::write(repo.join("b.txt"), "two\n").unwrap();

        let (workspace, cx) = cx.add_window_view(Workspace::new);
        let wait = |cx: &mut gpui_kit::VisualTestContext, done: &dyn Fn(&Workspace) -> bool| {
            for _ in 0..300 {
                cx.run_until_parked();
                if workspace.read_with(cx, |ws, _| done(ws)) {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            panic!("timed out");
        };
        workspace.update(cx, |ws, cx| ws.open_repo_path(repo.clone(), cx));
        // No click: the first file is simply open, with its diff loaded.
        wait(cx, &|ws| ws.diff.is_some());
        workspace.read_with(cx, |ws, _| {
            assert_eq!(
                ws.selected(),
                Some(&Selected::File {
                    path: "a.txt".into()
                })
            );
        });

        // Switching files still works.
        workspace.update(cx, |ws, cx| {
            ws.select(
                Selected::File {
                    path: "b.txt".into(),
                },
                cx,
            )
        });
        wait(cx, &|ws| ws.diff.is_some());
        workspace.read_with(cx, |ws, _| {
            assert_eq!(
                ws.selected(),
                Some(&Selected::File {
                    path: "b.txt".into()
                })
            );
        });

        // Commit everything: nothing is left to show, so the stale diff goes away.
        git(&["commit", "-qam", "all"]);
        workspace.update(cx, |ws, cx| ws.refresh_active_status(cx));
        wait(cx, &|ws| {
            ws.active_repo().unwrap().changed_files().is_empty()
        });
        workspace.read_with(cx, |ws, _| assert_eq!(ws.selected(), None));
    }

    /// The right-click actions that replaced the cross buttons: closing other tabs keeps
    /// the repositories, and removing a repository asks first (rename opens its dialog).
    #[gpui_kit::test]
    fn right_click_actions_close_other_tabs_and_ask_before_removing(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use gpui_kit::component::WindowExt as _;
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
        cx.update(|window, cx| {
            workspace.update(cx, |ws, cx| {
                for name in ["a", "b", "c"] {
                    ws.open_repo_path(PathBuf::from(format!("/tmp/anano-menu-{name}")), cx);
                }
                // Keep only "b": the others lose their tab but stay in the app.
                ws.close_other_tabs(1, cx);
                assert_eq!(ws.repo_count(), 3);
                assert_eq!(ws.active_index(), 1);
                assert!(ws.is_tab(1) && !ws.is_tab(0) && !ws.is_tab(2));
                assert_eq!(ws.switcher_entries().len(), 3);

                // Removing is a question first, and nothing is removed by asking.
                ws.confirm_remove_repo(0, window, cx);
                assert!(window.has_active_dialog(cx));
                assert_eq!(ws.repo_count(), 3);
            });
            let _ = window.draw(cx);
            window.close_dialog(cx);
            workspace.update(cx, |ws, cx| {
                ws.open_rename_branch_dialog("main".into(), window, cx);
                assert!(window.has_active_dialog(cx));
            });
            let _ = window.draw(cx);
            window.close_dialog(cx);
            workspace.update(cx, |ws, cx| {
                // Confirming removes by id, wherever that repository has moved to.
                let (id, _, _) = ws.repo_summary(2).unwrap();
                ws.remove_repo_id(id, window, cx);
                assert_eq!(ws.repo_count(), 2);
            });
        });
    }
}
