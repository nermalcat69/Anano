//! 顶层工作区：已打开仓库列表、侧栏、主题、全局动作；负责从磁盘恢复，并把改动写回。
//!
//! Phase 1 skeleton swap：这里原本是 HTTP 请求 Tab 的宿主（标签栏 = 请求 Tab，内容区 =
//! 请求 / 响应分栏）。现在标签栏的每个标签是一个「打开的仓库」，内容区是它的
//! `git status`（见 [`crate::ui::status_pane`]）。窗口外壳（标题栏 / 图标栏 / 可拖宽
//! 分栏 / 标签栏骨架）原样保留，换的只是标签与内容区渲染的是什么。

// 显式导入而非 `use gpui_kit::*`：本文件含 `#[cfg(test)] mod tests`，通配符会引入
// gpui 重导出的 `#[proc_macro_attribute] test`，与标准库 `#[test]` 同名冲突。
use std::path::PathBuf;

use anano_core::git::{GitError, LfsOp, RepoEntry};
use anano_core::model::{ThemePref, Ulid, WorkspaceState};
use anano_core::store::Loaded;
use gpui_kit::component::{
    ActiveTheme, IconName, Sizable, Theme, ThemeMode, TitleBar,
    alert::Alert,
    button::{Button, ButtonVariants},
    h_flex,
    input::InputState,
    resizable::{ResizableState, h_resizable, resizable_panel},
    status_bar::StatusBar,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext, Context, Entity, FocusHandle, FontWeight, InteractiveElement, IntoElement,
    ParentElement, PathPromptOptions, Render, Role, ScrollHandle, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Window, div, px,
};

use gpui_updater::UpdateStatus;

use crate::brand::APP_NAME;
use crate::bridge;
use crate::i18n::tr;
use crate::state::repos::{OpenRepo, SyncKind};
use crate::state::store::{banner, store};
use crate::state::update;
use crate::ui::settings_dialog::{SettingsPage, open_settings, open_settings_page};
use crate::{CloseTab, OpenRepoAction, OpenSettings, ToggleSidebar};

/// 侧栏默认宽度。比 HTTP 版本的窄栏宽一些：仓库路径普遍比请求标题长，
/// 底部还常驻着提交信息输入框，太窄会把两者都挤成省略号。
pub const SIDEBAR_DEFAULT_WIDTH: f32 = 340.;
pub(crate) const SIDEBAR_MIN_WIDTH: f32 = 260.;
pub(crate) const SIDEBAR_MAX_WIDTH: f32 = 600.;
/// 历史列表拉多少条；够看最近在干嘛，不做分页/加载更多（那是后续阶段的事）。
const RECENT_COMMITS_LIMIT: usize = 30;

pub struct Workspace {
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
    /// 「新建分支」的内联输入框；点「New Branch…」才展开，跟提交框一样常驻一份、
    /// 复用同一个 `InputState`（不是每次展开都新建，输入法组合状态不会被打断）。
    pub(crate) new_branch_input: Entity<InputState>,
    pub(crate) new_branch_open: bool,
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
        let state = state.unwrap_or_default();
        let mut ws = Self {
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
            new_branch_input: cx.new(|cx| {
                InputState::new(window, cx).placeholder(tr!("repo.branch.new_placeholder"))
            }),
            new_branch_open: false,
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
        self.repos.push(OpenRepo::new(entry));
        self.active = self.repos.len() - 1;
        self.persist_repos(cx);
        self.refresh_active_status(cx);
        self.refresh_active_log(cx);
        self.refresh_active_branches(cx);
        cx.notify();
    }

    pub fn activate(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.repos.len() && ix != self.active {
            self.active = ix;
            self.persist_repos(cx);
            cx.notify();
            if self.repos[ix].statuses.is_empty() && !self.repos[ix].loading {
                self.refresh_active_status(cx);
            }
            if self.repos[ix].commits.is_empty() && !self.repos[ix].log_loading {
                self.refresh_active_log(cx);
            }
            if self.repos[ix].branches.is_empty() && !self.repos[ix].branches_loading {
                self.refresh_active_branches(cx);
            }
        }
    }

    pub fn close_repo(&mut self, ix: usize, _window: &mut Window, cx: &mut Context<Self>) {
        if ix >= self.repos.len() {
            return;
        }
        self.repos.remove(ix);
        if self.active >= self.repos.len() {
            self.active = self.repos.len().saturating_sub(1);
        } else if ix < self.active {
            self.active -= 1;
        }
        self.persist_repos(cx);
        cx.notify();
    }

    /// 重新读一次当前激活仓库的 `git status`（后台线程，见 `bridge::refresh_status`）。
    pub fn refresh_active_status(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.repos.get_mut(self.active) else {
            return;
        };
        repo.loading = true;
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
                    }
                    Ok(Err(err)) => repo.error = Some(err.to_string()),
                    Err(err) => repo.error = Some(err.to_string()),
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
        repo.log_loading = true;
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

    /// 展开 / 收起侧栏里「新建分支」的内联输入框。
    pub fn toggle_new_branch(&mut self, cx: &mut Context<Self>) {
        self.new_branch_open = !self.new_branch_open;
        cx.notify();
    }

    /// 内联输入框的「创建」：取输入框里的名字新建分支，成功与否都收起输入框
    /// （名字为空直接忽略，按钮在渲染层已经据此置灰，这里兜底）。
    pub fn submit_new_branch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.new_branch_input.read(cx).value().trim().to_string();
        if name.is_empty() {
            return;
        }
        self.new_branch_input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.new_branch_open = false;
        self.create_branch(name, cx);
    }

    /// 从 `HEAD` 新建一个分支（不自动切过去）。
    pub fn create_branch(&mut self, name: String, cx: &mut Context<Self>) {
        let Some(repo) = self.active_repo() else {
            return;
        };
        if name.trim().is_empty() {
            return;
        }
        let path = repo.entry.path.clone();
        let id = repo.entry.id;
        let task = bridge::new_branch(cx, path, name);
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

    /// 切换到一个分支；失败（多半是工作树有会被覆盖的未提交改动）时把错误显示在
    /// 分支区域，不动当前状态。
    pub fn switch_branch(&mut self, name: String, cx: &mut Context<Self>) {
        let Some(repo) = self.active_repo() else {
            return;
        };
        let path = repo.entry.path.clone();
        let id = repo.entry.id;
        let task = bridge::switch_branch(cx, path, name);
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

    fn run_sync(&mut self, kind: SyncKind, cx: &mut Context<Self>) {
        let Some(repo) = self.active_repo() else {
            return;
        };
        if repo.syncing.is_some() {
            return; // 上一次还没跑完，别叠加一次
        }
        let path = repo.entry.path.clone();
        let id = repo.entry.id;
        let branch_name = repo.current_branch().map(|b| b.name.clone());

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

    /// 状态列表里点一下「暂存」/「取消暂存」；成功后刷新一次 status。
    pub fn toggle_stage(&mut self, file: String, currently_staged: bool, cx: &mut Context<Self>) {
        let Some(repo) = self.active_repo() else {
            return;
        };
        let path = repo.entry.path.clone();
        let task = bridge::stage(cx, path, file, currently_staged);
        cx.spawn(async move |ws, cx| {
            let result = task.await;
            let _ = ws.update(cx, |ws, cx| {
                if !matches!(result, Ok(Ok(()))) {
                    tracing::warn!("git stage/unstage failed: {result:?}");
                }
                ws.refresh_active_status(cx);
            });
        })
        .detach();
    }

    /// 侧栏底部的「提交」：把 `commit_input` 里的文字当提交信息，提交当前仓库。
    ///
    /// 索引里如果还没有任何暂存内容，先把当前显示的全部改动暂存一遍再提交——
    /// 覆盖最常见的「改完一批文件，一次性提交」场景，不强制用户先手动点某个文件的
    /// 暂存按钮。已经手动挑过 stage/unstage 的文件视为用户的显式选择，原样尊重，
    /// 不会被这里悄悄覆盖。空白信息或没有任何改动都直接忽略（按钮在渲染层已经据此
    /// 置灰，这里兜底）。
    pub fn commit_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.active_repo() else {
            return;
        };
        let message = self.commit_input.read(cx).value().trim().to_string();
        if message.is_empty() || repo.statuses.is_empty() {
            return;
        }
        let path = repo.entry.path.clone();
        let need_stage_all = !repo.has_staged_changes();
        self.commit_input
            .update(cx, |input, cx| input.set_value("", window, cx));

        cx.spawn(async move |ws, cx| {
            if need_stage_all {
                let stage_task = ws.update(cx, |_, cx| bridge::stage_all_changes(cx, path.clone()));
                if let Ok(task) = stage_task {
                    let _ = task.await;
                }
            }
            let commit_task = ws.update(cx, |_, cx| {
                bridge::commit(cx, path.clone(), message.clone())
            });
            let Ok(commit_task) = commit_task else {
                return;
            };
            let result = commit_task.await;
            let _ = ws.update(cx, |ws, cx| {
                match result {
                    Ok(Ok(_id)) => {}
                    Ok(Err(err)) => tracing::warn!("git commit failed: {err}"),
                    Err(err) => tracing::warn!("git commit failed: {err}"),
                }
                ws.refresh_active_status(cx);
                ws.refresh_active_log(cx);
            });
        })
        .detach();
    }

    /// 当前布局的快照；`tab_order` / `active` 复用为仓库 id 的顺序 / 当前激活项。
    pub(crate) fn workspace_state(&self) -> WorkspaceState {
        WorkspaceState {
            tab_order: self.repos.iter().map(|r| r.entry.id).collect(),
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

    pub fn set_theme(&mut self, pref: ThemePref, window: &mut Window, cx: &mut Context<Self>) {
        self.set_theme_with(pref, Some(window), cx);
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

    pub fn cycle_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_theme(self.theme.next(), window, cx);
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
            .right(h_flex().items_center().gap_0p5().when_some(
                update_hint,
                |bar, (version, staged)| {
                    bar.child(if staged {
                        Button::new("update-restart")
                            .ghost()
                            .xsmall()
                            .icon(IconName::ArrowUp)
                            .label(tr!("status.update_restart", version = version))
                            .tooltip(tr!("status.update_restart_tooltip"))
                            .on_click(|_, _, cx| update::restart(cx))
                    } else {
                        Button::new("update-available")
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

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.panels_normalize_pending && !self.sidebar_state.read(cx).sizes().is_empty() {
            self.panels_normalize_pending = false;
            self.reset_workspace_panels(cx);
        }
        let sidebar_width = px(self.sidebar_width.unwrap_or(SIDEBAR_DEFAULT_WIDTH));
        let dialog_layer = gpui_kit::component::Root::render_dialog_layer(window, cx);
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
            .on_action(cx.listener(|this, _: &CloseTab, window, cx| {
                let ix = this.active;
                this.close_repo(ix, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| this.toggle_sidebar(cx)))
            .on_action(
                cx.listener(|this, _: &OpenSettings, window, cx| this.open_settings(window, cx)),
            )
            .child(self.render_title_bar(cx))
            .when_some(banner(cx), |d, text| d.child(render_banner(text)))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .items_stretch()
                    .child(self.render_sidebar_rail(cx))
                    .child(
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
