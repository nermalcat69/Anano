//! tokio ⇄ gpui 桥接。git2 是阻塞调用，扔进 tokio 的阻塞线程池上跑
//! （`tokio::task::spawn_blocking`），结果通过 `Tokio::spawn_result` 带回 gpui 的 `Task`。

use std::path::PathBuf;

use anano_core::external::{self, Account, PendingLogin, Repo, Templates};
use anano_core::git::lfs::sync as lfs_sync;
use anano_core::git::{
    Branch, ChangeKind, CommitInfo, DiffView, FileStatus, GitError, LfsOp, LfsSync, clone_repo,
    commit_staged, create_branch, delete_branch, discard_change, discover_root, fetch,
    ignore_extension, ignore_path, list_branches, pull, push, recent_commits, repo_status,
    stage_all, stage_path, unstage_path,
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

/// Create a branch (from `from`: a commit id / branch name, or `HEAD`) and switch to it,
/// the way GitHub Desktop's "Create a Branch" does; uncommitted work comes along.
pub fn new_branch(
    cx: &App,
    repo: PathBuf,
    name: String,
    from: Option<String>,
) -> Task<anyhow::Result<Result<(), GitError>>> {
    Tokio::spawn_result(cx, async move {
        Ok(tokio::task::spawn_blocking(move || {
            create_branch(&repo, &name, from.as_deref())?;
            anano_core::git::switch_branch(&repo, &name, false)
        })
        .await?)
    })
}

/// Switch branches; `leave_changes` stashes uncommitted work on the branch being left
/// (see `anano_core::git::switch_branch`).
pub fn switch_branch(
    cx: &App,
    repo: PathBuf,
    name: String,
    leave_changes: bool,
) -> Task<anyhow::Result<Result<(), GitError>>> {
    Tokio::spawn_result(cx, async move {
        Ok(tokio::task::spawn_blocking(move || {
            anano_core::git::switch_branch(&repo, &name, leave_changes)
        })
        .await?)
    })
}

/// An author's email and the local file holding their picture (if one was found).
pub type Avatar = (String, Option<PathBuf>);

/// Profile pictures for commit authors, by email (downloaded once, then cached on disk).
pub fn avatars(cx: &App, emails: Vec<String>) -> Task<anyhow::Result<Vec<Avatar>>> {
    Tokio::spawn_result(cx, async move {
        Ok(tokio::task::spawn_blocking(move || {
            emails
                .into_iter()
                .map(|email| {
                    let path = external::avatar(&email);
                    (email, path)
                })
                .collect()
        })
        .await?)
    })
}

/// The account's repository list as last saved to disk (instant; may be stale).
pub fn cached_account_repos(cx: &App, login: String) -> Task<anyhow::Result<Option<Vec<Repo>>>> {
    Tokio::spawn_result(cx, async move {
        Ok(tokio::task::spawn_blocking(move || external::cached_repos(&login)).await?)
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
pub fn account_repos(cx: &App, login: String) -> Task<anyhow::Result<Vec<Repo>>> {
    Tokio::spawn_result(cx, async move {
        tokio::task::spawn_blocking(move || external::list_repos(&login))
            .await?
            .map_err(anyhow::Error::msg)
    })
}

/// Git Ignore / License choices for the create dialog.
pub fn templates(cx: &App) -> Task<anyhow::Result<Templates>> {
    Tokio::spawn_result(cx, async move {
        tokio::task::spawn_blocking(external::templates)
            .await?
            .map_err(anyhow::Error::msg)
    })
}

/// Everything the "Create a New Repository" dialog collects.
pub struct CreateOptions {
    pub parent: PathBuf,
    pub name: String,
    pub description: String,
    pub readme: bool,
    /// A GitHub gitignore template name (`Rust`, `Node`, …).
    pub gitignore: Option<String>,
    /// A GitHub license key (`mit`, `apache-2.0`, …).
    pub license: Option<String>,
    /// `(login, private)`: also create it on GitHub under that account.
    pub github: Option<(String, bool)>,
}

/// Where the new repository is, plus whatever went wrong on the way that didn't stop it
/// from existing locally (a template that couldn't be fetched, GitHub refusing the name).
pub type CreatedRepo = (PathBuf, Option<String>);

fn create_repo_blocking(o: CreateOptions) -> Result<CreatedRepo, GitError> {
    let mut problems = Vec::new();
    let mut files: Vec<(&str, String)> = Vec::new();
    if o.readme {
        let body = match o.description.is_empty() {
            true => format!("# {}\n", o.name),
            false => format!("# {}\n\n{}\n", o.name, o.description),
        };
        files.push(("README.md", body));
    }
    if let Some(template) = &o.gitignore {
        match external::gitignore_source(template) {
            Ok(source) => files.push((".gitignore", source)),
            Err(err) => problems.push(format!(".gitignore: {err}")),
        }
    }
    if let Some(key) = &o.license {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        let year = 1970 + secs / 31_556_952;
        let holder = anano_core::git::author_name().unwrap_or_default();
        match external::license_body(key, year, &holder) {
            Ok(body) => files.push(("LICENSE", body)),
            Err(err) => problems.push(format!("LICENSE: {err}")),
        }
    }
    let path = anano_core::git::init_repo_with(&o.parent, &o.name, &files)?;
    if let Some((login, private)) = &o.github {
        let published =
            external::create_repo(login, &o.name, &o.description, *private).and_then(|url| {
                anano_core::git::set_origin(&path, &external::with_account(&url, login))
                    .map_err(|e| e.to_string())
            });
        // Creating means it exists on GitHub with its files, not "exists once you find the
        // Push button": send the first commit straight away. (Nothing to push when no
        // README / gitignore / license was asked for; the repository is simply empty.)
        let published = published.and_then(|()| {
            if files.is_empty() {
                return Ok(());
            }
            let branch = anano_core::git::current_branch_name(&path)
                .map_err(|e| e.to_string())?
                .ok_or("no branch to push")?;
            push(&path, DEFAULT_REMOTE, &branch).map_err(|e| e.to_string())
        });
        if let Err(err) = published {
            problems.push(format!("GitHub: {err}"));
        }
    }
    Ok((path, (!problems.is_empty()).then(|| problems.join("; "))))
}

pub fn create_repo(
    cx: &App,
    options: CreateOptions,
) -> Task<anyhow::Result<Result<CreatedRepo, GitError>>> {
    Tokio::spawn_result(cx, async move {
        Ok(tokio::task::spawn_blocking(move || create_repo_blocking(options)).await?)
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

/// The cheap facts about a repository that ride along with every branch refresh.
#[derive(Default)]
pub struct RepoFacts {
    /// `origin`'s URL (which GitHub account the repository uses, pull requests, …).
    pub origin: Option<String>,
    /// When it was last fetched (toolbar).
    pub last_fetched: Option<std::time::SystemTime>,
    /// The branch `HEAD` names; `None` only when `HEAD` really is detached.
    pub head_branch: Option<String>,
}

pub fn repo_facts(cx: &App, repo: PathBuf) -> Task<anyhow::Result<RepoFacts>> {
    Tokio::spawn_result(cx, async move {
        Ok(tokio::task::spawn_blocking(move || RepoFacts {
            origin: anano_core::git::origin_url(&repo),
            last_fetched: anano_core::git::last_fetched(&repo),
            head_branch: anano_core::git::head_branch(&repo),
        })
        .await?)
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

/// The repository's pull requests (all states, newest first).
pub fn pull_requests(cx: &App, origin: String) -> Task<anyhow::Result<Vec<external::PullRequest>>> {
    Tokio::spawn_result(cx, async move {
        tokio::task::spawn_blocking(move || external::list_pull_requests(&origin))
            .await?
            .map_err(anyhow::Error::msg)
    })
}

/// One pull request with its description, history and merge verdict.
pub fn pull_request(
    cx: &App,
    origin: String,
    number: u64,
) -> Task<anyhow::Result<external::PullRequestDetail>> {
    Tokio::spawn_result(cx, async move {
        tokio::task::spawn_blocking(move || external::pull_request(&origin, number))
            .await?
            .map_err(anyhow::Error::msg)
    })
}

/// Merge a pull request on GitHub; a refusal comes back as the error, in GitHub's words.
pub fn merge_pull_request(cx: &App, origin: String, number: u64) -> Task<anyhow::Result<()>> {
    Tokio::spawn_result(cx, async move {
        tokio::task::spawn_blocking(move || external::merge_pull_request(&origin, number))
            .await?
            .map_err(anyhow::Error::msg)
    })
}
