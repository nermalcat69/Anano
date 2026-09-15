//! 一个已打开的仓库：路径、显示名，与最近一次读到的 `git status` / 提交历史。
//!
//! 不是 `Entity<T>`——不像 `RequestTab` 那样持有子实体（`InputState` / `KvTable`），
//! 纯数据放在 [`crate::state::workspace::Workspace`] 的 `Vec` 里就够了。

use anano_core::git::{Branch, CommitInfo, FileStatus, LfsSync, RepoEntry};

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
        }
    }

    /// 当前签出的本地分支，找不到（比如还没刷新过 / 分离 HEAD）时为 `None`。
    pub fn current_branch(&self) -> Option<&Branch> {
        self.branches.iter().find(|b| b.is_head)
    }

    pub fn staged(&self) -> impl Iterator<Item = &FileStatus> {
        self.statuses.iter().filter(|s| s.staged)
    }

    pub fn unstaged(&self) -> impl Iterator<Item = &FileStatus> {
        self.statuses.iter().filter(|s| !s.staged)
    }

    pub fn has_staged_changes(&self) -> bool {
        self.statuses.iter().any(|s| s.staged)
    }
}
