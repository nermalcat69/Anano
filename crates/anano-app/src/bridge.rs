//! tokio ⇄ gpui 桥接。git2 是阻塞调用，扔进 tokio 的阻塞线程池上跑
//! （`tokio::task::spawn_blocking`），结果通过 `Tokio::spawn_result` 带回 gpui 的 `Task`。

use std::path::PathBuf;

use anano_core::git::{
    Branch, CommitInfo, FileStatus, GitError, LfsOp, LfsSync, checkout_branch, commit_staged,
    create_branch, delete_branch, discover_root, fetch, list_branches, pull, push, recent_commits,
    repo_status, stage_all, stage_path, unstage_path,
};
use anano_core::git::lfs::sync as lfs_sync;
use gpui_kit::{App, Task};
use gpui_tokio::Tokio;

pub fn init(cx: &mut App) {
    gpui_tokio::init(cx);
}

/// 从「打开仓库」选中的路径找到仓库工作树根（子目录也认，见 `git::discover_root`）。
pub fn discover(cx: &App, path: PathBuf) -> Task<anyhow::Result<Result<PathBuf, GitError>>> {
    Tokio::spawn_result(cx, async move {
        Ok(tokio::task::spawn_blocking(move || discover_root(&path)).await?)
    })
}

/// 读一次 `git status`；返回的 gpui Task 被 drop 时底层 tokio 任务自动 abort（结果作废，
/// 不影响下一次调用）。
pub fn refresh_status(
    cx: &App,
    path: PathBuf,
) -> Task<anyhow::Result<Result<Vec<FileStatus>, GitError>>> {
    Tokio::spawn_result(cx, async move {
        Ok(tokio::task::spawn_blocking(move || repo_status(&path)).await?)
    })
}

/// 最近的提交历史。
pub fn refresh_log(
    cx: &App,
    path: PathBuf,
    limit: usize,
) -> Task<anyhow::Result<Result<Vec<CommitInfo>, GitError>>> {
    Tokio::spawn_result(cx, async move {
        Ok(tokio::task::spawn_blocking(move || recent_commits(&path, limit)).await?)
    })
}

/// 暂存 / 取消暂存一个文件。
pub fn stage(
    cx: &App,
    repo: PathBuf,
    file: String,
    staged: bool,
) -> Task<anyhow::Result<Result<(), GitError>>> {
    Tokio::spawn_result(cx, async move {
        Ok(tokio::task::spawn_blocking(move || {
            if staged {
                unstage_path(&repo, &file)
            } else {
                stage_path(&repo, &file)
            }
        })
        .await?)
    })
}

/// `git add -A`。
pub fn stage_all_changes(cx: &App, repo: PathBuf) -> Task<anyhow::Result<Result<(), GitError>>> {
    Tokio::spawn_result(cx, async move {
        Ok(tokio::task::spawn_blocking(move || stage_all(&repo)).await?)
    })
}

/// 把当前索引提交为一个新 commit。
pub fn commit(
    cx: &App,
    repo: PathBuf,
    message: String,
) -> Task<anyhow::Result<Result<String, GitError>>> {
    Tokio::spawn_result(cx, async move {
        Ok(tokio::task::spawn_blocking(move || commit_staged(&repo, &message)).await?)
    })
}

/// 目前只支持单一远程；后续要支持多远程时把这个换成仓库设置里的一个字段即可。
pub const DEFAULT_REMOTE: &str = "origin";

/// 列出全部分支（本地 + 远程跟踪）。
pub fn branches(cx: &App, repo: PathBuf) -> Task<anyhow::Result<Result<Vec<Branch>, GitError>>> {
    Tokio::spawn_result(cx, async move {
        Ok(tokio::task::spawn_blocking(move || list_branches(&repo)).await?)
    })
}

/// 从 `HEAD` 新建一个分支（不切过去；切换是单独一步，对应真实 Git 的心智模型）。
pub fn new_branch(
    cx: &App,
    repo: PathBuf,
    name: String,
) -> Task<anyhow::Result<Result<(), GitError>>> {
    Tokio::spawn_result(cx, async move {
        Ok(tokio::task::spawn_blocking(move || create_branch(&repo, &name, None)).await?)
    })
}

/// 切换到一个分支；工作树有会被覆盖的未提交改动时返回错误，不强推。
pub fn switch_branch(
    cx: &App,
    repo: PathBuf,
    name: String,
) -> Task<anyhow::Result<Result<(), GitError>>> {
    Tokio::spawn_result(cx, async move {
        Ok(tokio::task::spawn_blocking(move || checkout_branch(&repo, &name)).await?)
    })
}

/// 删除一个本地分支；删除当前签出的分支会失败（libgit2 自己拒绝）。
pub fn remove_branch(
    cx: &App,
    repo: PathBuf,
    name: String,
) -> Task<anyhow::Result<Result<(), GitError>>> {
    Tokio::spawn_result(cx, async move {
        Ok(tokio::task::spawn_blocking(move || delete_branch(&repo, &name)).await?)
    })
}

/// `git fetch`。
pub fn fetch_remote(cx: &App, repo: PathBuf) -> Task<anyhow::Result<Result<(), GitError>>> {
    Tokio::spawn_result(cx, async move {
        Ok(tokio::task::spawn_blocking(move || fetch(&repo, DEFAULT_REMOTE)).await?)
    })
}

/// `git pull`（只做快进；分叉了会报 [`GitError::Diverged`]）。
pub fn pull_remote(cx: &App, repo: PathBuf) -> Task<anyhow::Result<Result<(), GitError>>> {
    Tokio::spawn_result(cx, async move {
        Ok(tokio::task::spawn_blocking(move || pull(&repo, DEFAULT_REMOTE)).await?)
    })
}

/// `git push`，推当前分支到同名远程分支。
pub fn push_remote(
    cx: &App,
    repo: PathBuf,
    branch_name: String,
) -> Task<anyhow::Result<Result<(), GitError>>> {
    Tokio::spawn_result(cx, async move {
        Ok(tokio::task::spawn_blocking(move || push(&repo, DEFAULT_REMOTE, &branch_name)).await?)
    })
}

/// LFS 检测 + 同步：不是 LFS 仓库直接说不需要，是但没装 `git-lfs` 就说清楚，
/// 装了就跑对应的 `git-lfs fetch/pull/push`。在 git2 那半成功之后调用。
pub fn sync_lfs(
    cx: &App,
    repo: PathBuf,
    op: LfsOp,
) -> Task<anyhow::Result<Result<LfsSync, GitError>>> {
    Tokio::spawn_result(cx, async move {
        Ok(tokio::task::spawn_blocking(move || lfs_sync(&repo, op)).await?)
    })
}
