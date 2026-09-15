//! Git 集成：打开仓库、`git status`、分支、远程 fetch/pull/push、Git LFS。
//! 基于 libgit2 绑定 `git2`；LFS 没有稳定的 Rust 绑定，`lfs.rs` 里的同步部分直接
//! 调系统装的 `git-lfs` 二进制。diff / stash / tag / blame 还没做——那些是后续阶段。

pub mod branch;
pub mod error;
pub mod lfs;
pub mod model;
pub mod remote;
pub mod repo;
pub mod status;

pub use branch::{checkout_branch, create_branch, current_branch_name, delete_branch, list_branches};
pub use error::GitError;
pub use lfs::{LfsOp, LfsSync, is_lfs_enabled, is_lfs_installed, is_lfs_pointer};
pub use model::{Branch, ChangeKind, CommitInfo, FileStatus, RepoEntry};
pub use remote::{fetch, pull, push};
pub use repo::{
    commit_staged, discover_root, recent_commits, repo_status, stage_all, stage_path, unstage_path,
};
