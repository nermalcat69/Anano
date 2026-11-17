//! tokio ⇄ gpui 桥接。git2 是阻塞调用，扔进 tokio 的阻塞线程池上跑
//! （`tokio::task::spawn_blocking`），结果通过 `Tokio::spawn_result` 带回 gpui 的 `Task`。

use std::path::PathBuf;

use anano_core::external::{self, Account, PendingLogin, Repo, Templates};
use anano_core::git::lfs::sync as lfs_sync;
use anano_core::git::{
    Branch, ChangeKind, CommitInfo, DiffView, FileStatus, GitError, LfsOp, LfsSync, clone_repo,
    create_branch, delete_branch, discard_change, discover_root, fetch, ignore_extension,
    ignore_path, list_branches, pull, push, recent_commits, repo_status,
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

/// Commit exactly the ticked files (`include`), leaving the unticked ones (`exclude`) out.
pub fn commit_selected(
    cx: &App,
    repo: PathBuf,
    message: String,
    include: Vec<String>,
    exclude: Vec<String>,
) -> Task<anyhow::Result<Result<String, GitError>>> {
    Tokio::spawn_result(cx, async move {
        Ok(tokio::task::spawn_blocking(move || {
            anano_core::git::commit_paths(&repo, &message, &include, &exclude)
        })
        .await?)
    })
}

/// Profile pictures of the connected accounts themselves, by login (cached on disk).
pub fn account_avatars(cx: &App, logins: Vec<String>) -> Task<anyhow::Result<Vec<Avatar>>> {
    Tokio::spawn_result(cx, async move {
        Ok(tokio::task::spawn_blocking(move || {
            logins
                .into_iter()
                .map(|login| {
                    let path = external::account_avatar(&login);
                    (login, path)
                })
                .collect()
        })
        .await?)
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

/// Merge or rebase the upstream's new commits into the current branch.
pub fn integrate_upstream(
    cx: &App,
    repo: PathBuf,
    rebase: bool,
) -> Task<anyhow::Result<Result<(), GitError>>> {
    Tokio::spawn_result(cx, async move {
        Ok(
            tokio::task::spawn_blocking(move || anano_core::git::integrate_upstream(&repo, rebase))
                .await?,
        )
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
            Selected::File { path } => anano_core::git::diff_file_worktree(&repo, &path),
            Selected::Commit(id) => anano_core::git::diff_commit(&repo, &id),
        })
        .await?)
    })
}

/// The before/after pictures of the selected change, if it is an image (or a commit that
/// changed some).
pub fn images(
    cx: &App,
    repo: PathBuf,
    selected: crate::state::workspace::Selected,
) -> Task<anyhow::Result<Vec<anano_core::git::ImagePair>>> {
    use crate::state::workspace::Selected;
    Tokio::spawn_result(cx, async move {
        Ok(tokio::task::spawn_blocking(move || match selected {
            Selected::File { path } => anano_core::git::image_pair_worktree(&repo, &path)
                .ok()
                .flatten()
                .into_iter()
                .collect(),
            Selected::Commit(id) => {
                anano_core::git::image_pairs_commit(&repo, &id).unwrap_or_default()
            }
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

/// A pull request list and how many pull requests match in all (more than the list holds
/// until every page is loaded).
pub type PullList = (Vec<external::PullRequest>, usize);

/// The plain list for a state as last saved to disk (instant; may be stale).
pub fn cached_pull_requests(
    cx: &App,
    origin: String,
    search: external::PullSearch,
) -> Task<anyhow::Result<Option<PullList>>> {
    Tokio::spawn_result(cx, async move {
        Ok(
            tokio::task::spawn_blocking(move || external::cached_pull_requests(&origin, &search))
                .await?,
        )
    })
}

/// A job's log as last saved to disk (instant; finished jobs' logs never change).
pub fn cached_job_log(
    cx: &App,
    origin: String,
    job: u64,
) -> Task<anyhow::Result<Option<Vec<anano_core::workflows::LogLine>>>> {
    Tokio::spawn_result(cx, async move {
        Ok(
            tokio::task::spawn_blocking(move || {
                anano_core::workflows::cached_job_log(&origin, job)
            })
            .await?,
        )
    })
}

/// Re-read the first `pages` pages of a search from GitHub (and cache them).
pub fn refresh_pull_requests(
    cx: &App,
    origin: String,
    search: external::PullSearch,
    pages: usize,
) -> Task<anyhow::Result<PullList>> {
    Tokio::spawn_result(cx, async move {
        tokio::task::spawn_blocking(move || {
            external::refresh_pull_requests(&origin, &search, pages)
        })
        .await?
        .map_err(anyhow::Error::msg)
    })
}

/// Load the next page of a search and add it to `current` (and to the cache if plain).
pub fn more_pull_requests(
    cx: &App,
    origin: String,
    search: external::PullSearch,
    page: usize,
    current: Vec<external::PullRequest>,
) -> Task<anyhow::Result<PullList>> {
    Tokio::spawn_result(cx, async move {
        tokio::task::spawn_blocking(move || {
            external::more_pull_requests(&origin, &search, page, current)
        })
        .await?
        .map_err(anyhow::Error::msg)
    })
}

/// Open / merged / closed totals for an author and text.
pub fn pull_counts(
    cx: &App,
    origin: String,
    author: Option<String>,
    text: String,
) -> Task<anyhow::Result<external::PullCounts>> {
    Tokio::spawn_result(cx, async move {
        tokio::task::spawn_blocking(move || {
            external::pull_counts(&origin, author.as_deref(), &text)
        })
        .await?
        .map_err(anyhow::Error::msg)
    })
}

/// The people the author filter offers (saved list, instantly; may be stale).
pub fn cached_pull_authors(cx: &App, origin: String) -> Task<anyhow::Result<Option<Vec<String>>>> {
    Tokio::spawn_result(cx, async move {
        Ok(tokio::task::spawn_blocking(move || external::cached_pull_authors(&origin)).await?)
    })
}

/// The people the author filter offers, fresh from GitHub (and saved).
pub fn pull_authors(cx: &App, origin: String) -> Task<anyhow::Result<Vec<String>>> {
    Tokio::spawn_result(cx, async move {
        tokio::task::spawn_blocking(move || external::pull_authors(&origin))
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

/// Close a pull request without merging it.
pub fn close_pull_request(cx: &App, origin: String, number: u64) -> Task<anyhow::Result<()>> {
    Tokio::spawn_result(cx, async move {
        tokio::task::spawn_blocking(move || external::close_pull_request(&origin, number))
            .await?
            .map_err(anyhow::Error::msg)
    })
}

/// Every file a pull request changes, with its lines numbered on both sides.
pub fn pull_request_files(
    cx: &App,
    origin: String,
    number: u64,
) -> Task<anyhow::Result<Vec<anano_core::git::PatchFile>>> {
    Tokio::spawn_result(cx, async move {
        tokio::task::spawn_blocking(move || external::pull_request_files(&origin, number))
            .await?
            .map_err(anyhow::Error::msg)
    })
}

/// Submit a review (comment / approval / request for changes) of a pull request.
pub fn submit_review(
    cx: &App,
    origin: String,
    number: u64,
    verdict: external::ReviewVerdict,
    body: String,
) -> Task<anyhow::Result<()>> {
    Tokio::spawn_result(cx, async move {
        tokio::task::spawn_blocking(move || {
            external::submit_review(&origin, number, verdict, &body)
        })
        .await?
        .map_err(anyhow::Error::msg)
    })
}

/// Rename a local branch.
pub fn rename_branch(
    cx: &App,
    repo: PathBuf,
    old: String,
    new: String,
) -> Task<anyhow::Result<Result<(), GitError>>> {
    Tokio::spawn_result(cx, async move {
        Ok(
            tokio::task::spawn_blocking(move || anano_core::git::rename_branch(&repo, &old, &new))
                .await?,
        )
    })
}

/// Every workflow of the repository.
pub fn list_workflows(
    cx: &App,
    origin: String,
) -> Task<anyhow::Result<Vec<anano_core::workflows::Workflow>>> {
    Tokio::spawn_result(cx, async move {
        tokio::task::spawn_blocking(move || anano_core::workflows::list_workflows(&origin))
            .await?
            .map_err(anyhow::Error::msg)
    })
}

/// One page of workflow runs, and how many there are in all.
pub fn list_runs(
    cx: &App,
    origin: String,
    workflow: Option<u64>,
    status: anano_core::workflows::RunStatus,
    page: usize,
) -> Task<anyhow::Result<(Vec<anano_core::workflows::WorkflowRun>, usize)>> {
    Tokio::spawn_result(cx, async move {
        tokio::task::spawn_blocking(move || {
            anano_core::workflows::list_runs(&origin, workflow, status, page)
                .map(|page| (page.items, page.total))
        })
        .await?
        .map_err(anyhow::Error::msg)
    })
}

/// The jobs of a run, with their steps.
pub fn run_jobs(
    cx: &App,
    origin: String,
    run: u64,
) -> Task<anyhow::Result<Vec<anano_core::workflows::RunJob>>> {
    Tokio::spawn_result(cx, async move {
        tokio::task::spawn_blocking(move || anano_core::workflows::run_jobs(&origin, run))
            .await?
            .map_err(anyhow::Error::msg)
    })
}

/// One job's log, parsed into classified lines.
pub fn job_log(
    cx: &App,
    origin: String,
    job: u64,
) -> Task<anyhow::Result<Vec<anano_core::workflows::LogLine>>> {
    Tokio::spawn_result(cx, async move {
        tokio::task::spawn_blocking(move || anano_core::workflows::job_log(&origin, job))
            .await?
            .map_err(anyhow::Error::msg)
    })
}

/// One page of the repositories an account owns.
pub fn account_repos_page(
    cx: &App,
    login: String,
    page: usize,
) -> Task<anyhow::Result<Vec<anano_core::browse::RemoteRepo>>> {
    Tokio::spawn_result(cx, async move {
        tokio::task::spawn_blocking(move || anano_core::browse::account_repos(&login, page))
            .await?
            .map_err(anyhow::Error::msg)
    })
}

/// The organizations an account belongs to.
pub fn account_orgs(cx: &App, login: String) -> Task<anyhow::Result<Vec<anano_core::browse::Org>>> {
    Tokio::spawn_result(cx, async move {
        tokio::task::spawn_blocking(move || anano_core::browse::orgs(&login))
            .await?
            .map_err(anyhow::Error::msg)
    })
}

/// One page of an organization's repositories, as `login` sees them.
pub fn org_repos_page(
    cx: &App,
    login: String,
    org: String,
    page: usize,
) -> Task<anyhow::Result<Vec<anano_core::browse::RemoteRepo>>> {
    Tokio::spawn_result(cx, async move {
        tokio::task::spawn_blocking(move || anano_core::browse::org_repos(&login, &org, page))
            .await?
            .map_err(anyhow::Error::msg)
    })
}

/// One page of a repository's commit history.
pub fn commits_page(
    cx: &App,
    login: String,
    slug: String,
    page: usize,
) -> Task<anyhow::Result<Vec<anano_core::browse::RemoteCommit>>> {
    Tokio::spawn_result(cx, async move {
        tokio::task::spawn_blocking(move || anano_core::browse::commits(&login, &slug, page))
            .await?
            .map_err(anyhow::Error::msg)
    })
}

/// What one commit of a repository changed.
pub fn remote_commit_files(
    cx: &App,
    login: String,
    slug: String,
    sha: String,
) -> Task<anyhow::Result<Vec<anano_core::git::PatchFile>>> {
    Tokio::spawn_result(cx, async move {
        tokio::task::spawn_blocking(move || anano_core::browse::commit_files(&login, &slug, &sha))
            .await?
            .map_err(anyhow::Error::msg)
    })
}
