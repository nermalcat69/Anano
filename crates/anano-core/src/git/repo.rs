//! 打开仓库、读取状态、暂存 / 取消暂存、提交、读历史。全部是阻塞调用（git2 本身不是
//! async）；调用方（app 层）负责扔到 tokio 的阻塞线程池上跑（见 `anano-app` 的 `bridge.rs`）。

use std::path::{Path, PathBuf};

use git2::build::CheckoutBuilder;
use git2::{IndexAddOption, Repository, Sort, StatusOptions};

use crate::git::error::GitError;
use crate::git::model::{ChangeKind, CommitInfo, FileStatus};
use crate::git::status::map_status;

/// 从 `path`（可以是仓库内任意子目录）找到仓库工作树的根。`Repository::open` 要求
/// 精确路径，选中仓库内的子文件夹会直接打不开；`discover` 像 `git` 命令本身一样向上找。
pub fn discover_root(path: &Path) -> Result<PathBuf, GitError> {
    let repo = Repository::discover(path).map_err(GitError::Open)?;
    Ok(repo
        .workdir()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| repo.path().to_path_buf()))
}

fn open(path: &Path) -> Result<Repository, GitError> {
    Repository::discover(path).map_err(GitError::Open)
}

/// 列出工作树状态（未 stage + 已 stage，含未跟踪文件）。
pub fn repo_status(path: &Path) -> Result<Vec<FileStatus>, GitError> {
    let repo = open(path)?;
    let mut opts = StatusOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_ignored(false);
    let statuses = repo.statuses(Some(&mut opts)).map_err(GitError::Status)?;

    let mut out = Vec::new();
    for entry in statuses.iter() {
        let Some(path) = entry.path() else { continue };
        let path = path.to_string();
        for (kind, staged) in map_status(entry.status()) {
            out.push(FileStatus {
                path: path.clone(),
                kind,
                staged,
            });
        }
    }
    Ok(out)
}

/// 暂存一个文件（`git add <file>`）：文件还在磁盘上就加进索引，已被删除就把索引里的删除
/// 记下来（`add_path` 在文件不存在时会报错，这里退回 `remove_path`）。
pub fn stage_path(repo_path: &Path, file: &str) -> Result<(), GitError> {
    let repo = open(repo_path)?;
    let mut index = repo.index().map_err(GitError::Status)?;
    let rel = Path::new(file);
    if repo_path.join(file).exists() {
        index.add_path(rel).map_err(GitError::Status)?;
    } else {
        index.remove_path(rel).map_err(GitError::Status)?;
    }
    index.write().map_err(GitError::Status)?;
    Ok(())
}

/// 取消暂存一个文件（`git reset HEAD -- <file>`）：把索引里这一项还原成 HEAD 的版本；
/// 仓库还没有任何提交（unborn HEAD）时，索引本来就没有 HEAD 可还原，直接从索引移除。
pub fn unstage_path(repo_path: &Path, file: &str) -> Result<(), GitError> {
    let repo = open(repo_path)?;
    match repo.head().and_then(|h| h.peel_to_commit()) {
        Ok(head_commit) => {
            repo.reset_default(Some(head_commit.as_object()), [file])
                .map_err(GitError::Status)?;
        }
        Err(_) => {
            let mut index = repo.index().map_err(GitError::Status)?;
            index
                .remove_path(Path::new(file))
                .map_err(GitError::Status)?;
            index.write().map_err(GitError::Status)?;
        }
    }
    Ok(())
}

/// 把当前索引（已 stage 的内容）提交为一个新 commit；`HEAD` 是首个提交时没有父提交。
/// 签名优先用仓库 / 全局 git config 里的 `user.name` / `user.email`，没配过就退回一个
/// 占位身份——本地提交允许这样，往后阶段有远程推送时再逼用户配置。
pub fn commit_staged(repo_path: &Path, message: &str) -> Result<String, GitError> {
    let repo = open(repo_path)?;
    let mut index = repo.index().map_err(GitError::Commit)?;
    let tree_id = index.write_tree().map_err(GitError::Commit)?;
    let tree = repo.find_tree(tree_id).map_err(GitError::Commit)?;
    let sig = repo
        .signature()
        .or_else(|_| git2::Signature::now("Anano", "anano@localhost"))
        .map_err(GitError::Commit)?;
    let parent = repo.head().and_then(|h| h.peel_to_commit()).ok();
    let parents: Vec<&git2::Commit> = parent.iter().collect();
    let oid = repo
        .commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)
        .map_err(GitError::Commit)?;
    Ok(oid.to_string())
}

/// 最近 `limit` 条提交，按时间倒序（最新在前）；还没有任何提交时返回空列表。
pub fn recent_commits(repo_path: &Path, limit: usize) -> Result<Vec<CommitInfo>, GitError> {
    let repo = open(repo_path)?;
    let mut revwalk = repo.revwalk().map_err(GitError::Status)?;
    if revwalk.push_head().is_err() {
        // unborn HEAD：还没有第一个提交
        return Ok(Vec::new());
    }
    revwalk.set_sorting(Sort::TIME).map_err(GitError::Status)?;

    let mut out = Vec::with_capacity(limit);
    for oid in revwalk.take(limit) {
        let oid = oid.map_err(GitError::Status)?;
        let commit = repo.find_commit(oid).map_err(GitError::Status)?;
        out.push(CommitInfo {
            id: oid.to_string(),
            summary: commit.summary().unwrap_or("").to_string(),
            author: commit.author().name().unwrap_or("").to_string(),
            time: commit.time().seconds(),
        });
    }
    Ok(out)
}

/// `git add -A`：一次性暂存全部改动（新增 / 修改 / 删除），侧栏「全部暂存」按钮用。
pub fn stage_all(repo_path: &Path) -> Result<(), GitError> {
    let repo = open(repo_path)?;
    let mut index = repo.index().map_err(GitError::Status)?;
    index
        .add_all(["*"], IndexAddOption::DEFAULT, None)
        .map_err(GitError::Status)?;
    index.update_all(["*"], None).map_err(GitError::Status)?;
    index.write().map_err(GitError::Status)?;
    Ok(())
}

/// 丢弃一个文件的改动：新增/未跟踪文件直接从磁盘删掉；其余（修改/删除/重命名/类型变更）
/// 用 `checkout_head` 强制把工作树那一条路径还原成 `HEAD` 里的版本。调用方（UI）已经从
/// `FileStatus.kind` 知道是哪一种，这里不用再重新查一遍状态。
pub fn discard_change(repo_path: &Path, file: &str, kind: ChangeKind) -> Result<(), GitError> {
    if kind == ChangeKind::New {
        let abs = repo_path.join(file);
        if abs.exists() {
            std::fs::remove_file(&abs).map_err(|e| GitError::Discard(e.to_string()))?;
        }
        return Ok(());
    }
    let repo = open(repo_path)?;
    let mut checkout = CheckoutBuilder::new();
    checkout.force();
    checkout.path(file);
    repo.checkout_head(Some(&mut checkout))
        .map_err(|e| GitError::Discard(e.to_string()))
}

/// 把一条 `.gitignore` 条目追加进去：文件不存在就新建，已有同样一行就不重复加。
fn append_gitignore_entry(repo_path: &Path, entry: &str) -> Result<(), GitError> {
    let gitignore = repo_path.join(".gitignore");
    let existing = std::fs::read_to_string(&gitignore).unwrap_or_default();
    if existing.lines().any(|line| line.trim() == entry) {
        return Ok(());
    }
    let mut content = existing;
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str(entry);
    content.push('\n');
    std::fs::write(&gitignore, content).map_err(|e| GitError::Ignore(e.to_string()))
}

/// 「Ignore File」：把仓库相对路径原样追加进 `.gitignore`。
pub fn ignore_path(repo_path: &Path, file: &str) -> Result<(), GitError> {
    append_gitignore_entry(repo_path, file)
}

/// 「Ignore All *.ext Files」：按文件扩展名生成一条 glob 规则；没有扩展名就退回精确路径
/// （没有更好的通配写法，跟 `ignore_path` 行为一致好过报错）。
pub fn ignore_extension(repo_path: &Path, file: &str) -> Result<(), GitError> {
    let pattern = match Path::new(file).extension().and_then(|e| e.to_str()) {
        Some(ext) => format!("*.{ext}"),
        None => return append_gitignore_entry(repo_path, file),
    };
    append_gitignore_entry(repo_path, &pattern)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;

    /// 用系统 git 建一个真实仓库比手搓 git2 的底层 API 稳：只要环境里有 `git` 就够跑。
    fn init_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            let status = Command::new("git")
                .args(args)
                .current_dir(dir.path())
                .status()
                .expect("git installed");
            assert!(status.success(), "git {args:?} failed");
        };
        run(&["init", "-q"]);
        run(&["config", "user.email", "a@b.c"]);
        run(&["config", "user.name", "test"]);
        dir
    }

    #[test]
    fn untracked_and_modified_files_are_reported() {
        let dir = init_repo();
        fs::write(dir.path().join("new.txt"), "hi").unwrap();
        let statuses = repo_status(dir.path()).unwrap();
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].path, "new.txt");
        assert_eq!(statuses[0].kind, crate::git::model::ChangeKind::New);
        assert!(!statuses[0].staged);
    }

    #[test]
    fn clean_repo_has_no_statuses() {
        let dir = init_repo();
        fs::write(dir.path().join("a.txt"), "hi").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(dir.path())
            .status()
            .unwrap();
        Command::new("git")
            .args(["commit", "-q", "-m", "init"])
            .current_dir(dir.path())
            .status()
            .unwrap();
        assert!(repo_status(dir.path()).unwrap().is_empty());
    }

    #[test]
    fn missing_repo_is_an_open_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(repo_status(dir.path()), Err(GitError::Open(_))));
    }

    /// `Repository::open` 要求精确路径；`discover` 能从仓库内的子目录找到根，
    /// 这是修掉「选中子文件夹时 status 读不出来」那个 bug 的关键行为。
    #[test]
    fn status_works_from_a_subdirectory() {
        let dir = init_repo();
        let sub = dir.path().join("src");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("main.rs"), "fn main() {}").unwrap();
        let statuses = repo_status(&sub).unwrap();
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].path, "src/main.rs");
    }

    #[test]
    fn discover_root_finds_the_worktree_root_from_a_subdirectory() {
        let dir = init_repo();
        let sub = dir.path().join("src");
        fs::create_dir(&sub).unwrap();
        let root = discover_root(&sub).unwrap();
        assert_eq!(
            fs::canonicalize(root).unwrap(),
            fs::canonicalize(dir.path()).unwrap()
        );
    }

    #[test]
    fn stage_and_unstage_round_trip() {
        let dir = init_repo();
        fs::write(dir.path().join("a.txt"), "hi").unwrap();
        stage_path(dir.path(), "a.txt").unwrap();
        let statuses = repo_status(dir.path()).unwrap();
        assert_eq!(
            statuses,
            vec![FileStatus {
                path: "a.txt".into(),
                kind: crate::git::model::ChangeKind::New,
                staged: true,
            }]
        );

        unstage_path(dir.path(), "a.txt").unwrap();
        let statuses = repo_status(dir.path()).unwrap();
        assert!(!statuses[0].staged);
    }

    #[test]
    fn commit_staged_creates_a_commit_and_clears_status() {
        let dir = init_repo();
        fs::write(dir.path().join("a.txt"), "hi").unwrap();
        stage_path(dir.path(), "a.txt").unwrap();
        let id = commit_staged(dir.path(), "first commit").unwrap();
        assert_eq!(id.len(), 40, "{id}");
        assert!(repo_status(dir.path()).unwrap().is_empty());

        let commits = recent_commits(dir.path(), 10).unwrap();
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].summary, "first commit");
        assert_eq!(commits[0].id, id);
    }

    #[test]
    fn recent_commits_is_empty_before_the_first_commit() {
        let dir = init_repo();
        assert!(recent_commits(dir.path(), 10).unwrap().is_empty());
    }

    #[test]
    fn stage_all_stages_every_pending_change() {
        let dir = init_repo();
        fs::write(dir.path().join("a.txt"), "a").unwrap();
        fs::write(dir.path().join("b.txt"), "b").unwrap();
        stage_all(dir.path()).unwrap();
        let statuses = repo_status(dir.path()).unwrap();
        assert_eq!(statuses.len(), 2);
        assert!(statuses.iter().all(|s| s.staged));
    }

    #[test]
    fn discard_change_deletes_a_new_untracked_file() {
        let dir = init_repo();
        let file = dir.path().join("new.txt");
        fs::write(&file, "hi").unwrap();
        discard_change(dir.path(), "new.txt", ChangeKind::New).unwrap();
        assert!(!file.exists());
    }

    #[test]
    fn discard_change_restores_a_modified_tracked_file() {
        let dir = init_repo();
        let file = dir.path().join("a.txt");
        fs::write(&file, "original").unwrap();
        stage_path(dir.path(), "a.txt").unwrap();
        commit_staged(dir.path(), "init").unwrap();

        fs::write(&file, "changed").unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "changed");

        discard_change(dir.path(), "a.txt", ChangeKind::Modified).unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "original");
    }

    #[test]
    fn ignore_path_appends_once_and_skips_duplicates() {
        let dir = init_repo();
        ignore_path(dir.path(), "build/output.log").unwrap();
        ignore_path(dir.path(), "build/output.log").unwrap();
        let content = fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert_eq!(content.matches("build/output.log").count(), 1);
    }

    #[test]
    fn ignore_extension_derives_a_glob_from_the_file_extension() {
        let dir = init_repo();
        ignore_extension(dir.path(), "notes/todo.log").unwrap();
        let content = fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert!(content.lines().any(|l| l == "*.log"));
    }
}
