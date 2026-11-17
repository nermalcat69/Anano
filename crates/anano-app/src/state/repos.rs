//! 一个已打开的仓库：路径、显示名，与最近一次读到的 `git status` / 提交历史。
//!
//! 不是 `Entity<T>`——不像 `RequestTab` 那样持有子实体（`InputState` / `KvTable`），
//! 纯数据放在 [`crate::state::workspace::Workspace`] 的 `Vec` 里就够了。

use anano_core::git::{Branch, ChangeKind, CommitInfo, FileStatus, LfsSync, RepoEntry};

pub struct OpenRepo {
    pub entry: RepoEntry,
    pub statuses: Vec<FileStatus>,
    pub loading: bool,
    /// 上一次刷新失败的说明（打不开仓库 / 读状态失败）；成功一次就清空。
    pub error: Option<String>,
    pub commits: Vec<CommitInfo>,
    pub log_loading: bool,
    pub branches: Vec<Branch>,
    pub branches_loading: bool,
    /// 分支新建 / 切换 / 删除，或 fetch / pull / push 失败时的说明；跟 `error`
    /// （只管 status 刷新）分开放，两类操作互不遮盖对方的报错。
    pub action_error: Option<String>,
    /// fetch / pull / push 中的哪一个正在跑；`None` 表示都没在跑。
    pub syncing: Option<SyncKind>,
    /// 上一次 fetch / pull / push 之后，仓库 LFS 那部分的同步结果。
    pub lfs_status: Option<LfsSync>,
    /// `origin`'s URL; refreshed together with the branch list.
    pub origin: Option<String>,
    pub last_fetched: Option<std::time::SystemTime>,
    /// The branch `HEAD` names, known even before the first commit exists.
    pub head_branch: Option<String>,
    /// Set when the repository is opened or switched to; cleared by the automatic fetch
    /// that follows (see `Workspace::maybe_auto_fetch`).
    /// Whether `origin` / `head_branch` / `last_fetched` have been read from git yet.
    /// Whether it has a tab in the tab strip. Closing a tab clears this and nothing else:
    /// the repository stays in the app (and in the repository list) until it is removed.
    /// Changed files the user unticked: left out of the next commit. Everything else that
    /// is changed is ticked, so a new change is included by default.
    pub unchecked: std::collections::HashSet<String>,
    pub in_tabs: bool,
    pub facts_loaded: bool,
    pub needs_fetch: bool,
    /// The sync in flight was started by the app, not the user: if it fails, stay quiet.
    pub quiet_sync: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncKind {
    Fetch,
    Pull,
    Push,
}

impl OpenRepo {
    pub fn new(entry: RepoEntry) -> Self {
        Self {
            entry,
            statuses: Vec::new(),
            loading: true,
            error: None,
            commits: Vec::new(),
            log_loading: true,
            branches: Vec::new(),
            branches_loading: true,
            action_error: None,
            syncing: None,
            lfs_status: None,
            origin: None,
            last_fetched: None,
            head_branch: None,
            unchecked: Default::default(),
            in_tabs: true,
            facts_loaded: false,
            needs_fetch: true,
            quiet_sync: false,
        }
    }

    /// 当前签出的本地分支，找不到（比如还没刷新过 / 分离 HEAD）时为 `None`。
    pub fn current_branch(&self) -> Option<&Branch> {
        self.branches.iter().find(|b| b.is_head)
    }

    /// What to call the current branch in the UI. `None` means "say Detached HEAD".
    ///
    /// In order: the checked-out branch from the loaded branch list; the branch `HEAD`
    /// names (known even in a repository with no commits yet); and, until git has been
    /// read at all, the branch remembered from last time, so there is a name on screen
    /// immediately. Only a repository that has been read and whose `HEAD` points at a
    /// commit instead of a branch is called detached.
    pub fn branch_label(&self) -> Option<String> {
        self.current_branch()
            .map(|b| b.name.clone())
            .or_else(|| self.head_branch.clone())
            .or_else(|| {
                let unread = !self.facts_loaded;
                unread.then(|| self.entry.last_branch.clone().unwrap_or_else(|| "…".into()))
            })
    }

    /// The changes list: one row per changed file, however git splits it between the
    /// index and the working tree. When a path has two entries the more telling kind wins
    /// (a file that is new in the index and then edited again is still "new").
    pub fn changed_files(&self) -> Vec<(String, ChangeKind)> {
        fn weight(kind: ChangeKind) -> u8 {
            match kind {
                ChangeKind::Conflicted => 5,
                ChangeKind::New => 4,
                ChangeKind::Deleted => 3,
                ChangeKind::Renamed => 2,
                ChangeKind::TypeChange => 1,
                ChangeKind::Modified => 0,
            }
        }
        let mut files: Vec<(String, ChangeKind)> = Vec::new();
        for status in &self.statuses {
            match files.iter_mut().find(|(path, _)| *path == status.path) {
                Some((_, kind)) if weight(status.kind) > weight(*kind) => *kind = status.kind,
                Some(_) => {}
                None => files.push((status.path.clone(), status.kind)),
            }
        }
        files
    }

    /// The ticked changed files: what the Commit button will commit.
    pub fn checked_files(&self) -> Vec<String> {
        self.changed_files()
            .into_iter()
            .map(|(path, _)| path)
            .filter(|path| !self.unchecked.contains(path))
            .collect()
    }
}
