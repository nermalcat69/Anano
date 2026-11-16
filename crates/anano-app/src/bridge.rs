//! tokio ⇄ gpui 桥接。git2 是阻塞调用，扔进 tokio 的阻塞线程池上跑
//! （`tokio::task::spawn_blocking`），结果通过 `Tokio::spawn_result` 带回 gpui 的 `Task`。

use std::path::PathBuf;

use anano_core::external::{self, Account, PendingLogin};
use anano_core::git::lfs::sync as lfs_sync;
use anano_core::git::{
    Branch, ChangeKind, CommitInfo, DiffView, FileStatus, GitError, LfsOp, LfsSync,
    checkout_branch, clone_repo, commit_staged, create_branch, delete_branch, discard_change,
    discover_root, fetch, ignore_extension, ignore_path, init_repo, list_branches, pull, push,
    recent_commits, repo_status, stage_all, stage_path, unstage_path,
};
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

/// `git clone <url> <parent>/<name>`（Add Repository → Clone Repository）。
pub fn clone(
    cx: &App,
    url: String,
    dest: PathBuf,
) -> Task<anyhow::Result<Result<PathBuf, GitError>>> {
    Tokio::spawn_result(cx, async move {
        Ok(tokio::task::spawn_blocking(move || clone_repo(&url, &dest)).await?)
    })
}

/// 丢弃一个文件的改动（右键菜单「Discard Changes」）。
pub fn discard(
    cx: &App,
    repo: PathBuf,
    file: String,
    kind: ChangeKind,
) -> Task<anyhow::Result<Result<(), GitError>>> {
    Tokio::spawn_result(cx, async move {
        Ok(tokio::task::spawn_blocking(move || discard_change(&repo, &file, kind)).await?)
    })
}

/// 把一个文件精确路径加进 `.gitignore`（右键菜单「Ignore File」）。
pub fn ignore_file(
    cx: &App,
    repo: PathBuf,
    file: String,
) -> Task<anyhow::Result<Result<(), GitError>>> {
    Tokio::spawn_result(cx, async move {
        Ok(tokio::task::spawn_blocking(move || ignore_path(&repo, &file)).await?)
    })
}

/// 把一个文件的扩展名 glob 加进 `.gitignore`（右键菜单「Ignore All *.ext Files」）。
pub fn ignore_file_extension(
    cx: &App,
    repo: PathBuf,
    file: String,
) -> Task<anyhow::Result<Result<(), GitError>>> {
    Tokio::spawn_result(cx, async move {
        Ok(tokio::task::spawn_blocking(move || ignore_extension(&repo, &file)).await?)
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

/// GitHub accounts `gh` is signed into (Settings → Accounts, clone dialog).
pub fn accounts(cx: &App) -> Task<anyhow::Result<Vec<Account>>> {
    Tokio::spawn_result(cx, async move {
        Ok(tokio::task::spawn_blocking(external::list_accounts).await?)
    })
}

/// First half of "Add account": resolves once `gh` has a one-time code to show.
pub fn start_login(cx: &App) -> Task<anyhow::Result<PendingLogin>> {
    Tokio::spawn_result(cx, async move {
        tokio::task::spawn_blocking(external::start_login)
            .await?
            .map_err(anyhow::Error::msg)
    })
}

/// Second half: resolves when the user finished (or abandoned) the browser step.
pub fn finish_login(cx: &App, pending: PendingLogin) -> Task<anyhow::Result<()>> {
    Tokio::spawn_result(cx, async move {
        tokio::task::spawn_blocking(move || pending.finish())
            .await?
            .map_err(anyhow::Error::msg)
    })
}

/// Open a repository directory in the Nermal IDE.
pub fn open_in_nermal(cx: &App, path: PathBuf) -> Task<anyhow::Result<()>> {
    Tokio::spawn_result(cx, async move {
        tokio::task::spawn_blocking(move || external::open_in_nermal(&path))
            .await?
            .map_err(anyhow::Error::msg)
    })
}

/// Repositories a connected account can clone (`owner/name`).
pub fn account_repos(cx: &App, login: String) -> Task<anyhow::Result<Vec<String>>> {
    Tokio::spawn_result(cx, async move {
        tokio::task::spawn_blocking(move || external::list_repos(&login))
            .await?
            .map_err(anyhow::Error::msg)
    })
}

/// Where the new repository is, plus why the GitHub half failed (if it did).
pub type CreatedRepo = (PathBuf, Option<String>);

/// `git init` in `parent/name`; with `github = (login, private)` also create the repository
/// on GitHub under that account and wire it up as `origin`. The local repository survives a
/// GitHub failure; that failure comes back as the second tuple field.
pub fn create_repo(
    cx: &App,
    parent: PathBuf,
    name: String,
    github: Option<(String, bool)>,
) -> Task<anyhow::Result<Result<CreatedRepo, GitError>>> {
    Tokio::spawn_result(cx, async move {
        Ok(tokio::task::spawn_blocking(move || {
            let path = init_repo(&parent, &name)?;
            let Some((login, private)) = github else {
                return Ok((path, None));
            };
            let remote = external::create_repo(&login, &name, private).and_then(|url| {
                anano_core::git::set_origin(&path, &external::with_account(&url, &login))
                    .map_err(|e| e.to_string())
            });
            Ok((path, remote.err()))
        })
        .await?)
    })
}

/// Diff for the main pane: one changed file, or a whole commit.
pub fn diff(
    cx: &App,
    repo: PathBuf,
    selected: crate::state::workspace::Selected,
) -> Task<anyhow::Result<Result<DiffView, GitError>>> {
    use crate::state::workspace::Selected;
    Tokio::spawn_result(cx, async move {
        Ok(tokio::task::spawn_blocking(move || match selected {
            Selected::File { path, staged } => anano_core::git::diff_file(&repo, &path, staged),
            Selected::Commit(id) => anano_core::git::diff_commit(&repo, &id),
        })
        .await?)
    })
}

/// `origin`'s URL (to show / change which GitHub account a repository uses).
pub fn origin(cx: &App, repo: PathBuf) -> Task<anyhow::Result<Option<String>>> {
    Tokio::spawn_result(cx, async move {
        Ok(tokio::task::spawn_blocking(move || anano_core::git::origin_url(&repo)).await?)
    })
}

/// Re-point `origin` so the repository fetches and pushes as `login`.
pub fn set_origin_account(
    cx: &App,
    repo: PathBuf,
    login: String,
) -> Task<anyhow::Result<Result<(), GitError>>> {
    Tokio::spawn_result(cx, async move {
        Ok(
            tokio::task::spawn_blocking(move || match anano_core::git::origin_url(&repo) {
                Some(url) => {
                    anano_core::git::set_origin(&repo, &external::with_account(&url, &login))
                }
                None => Ok(()),
            })
            .await?,
        )
    })
}
