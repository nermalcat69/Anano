//! 分支：列出（本地 + 远程跟踪）、新建、切换（checkout）、删除。全部阻塞调用，
//! 调用方（app 层的 `bridge.rs`）负责扔进 tokio 阻塞线程池。

use std::path::Path;

use git2::{BranchType, Repository, build::CheckoutBuilder};

use crate::git::error::GitError;
use crate::git::model::Branch;

fn open(path: &Path) -> Result<Repository, GitError> {
    Repository::discover(path).map_err(GitError::Open)
}

/// 列出全部分支（本地在前，远程跟踪在后）；本地分支若配了上游，顺带算一次 ahead/behind。
pub fn list_branches(repo_path: &Path) -> Result<Vec<Branch>, GitError> {
    let repo = open(repo_path)?;
    let head_name = repo
        .head()
        .ok()
        .filter(|h| h.is_branch())
        .and_then(|h| h.shorthand().map(str::to_string));

    let mut out = Vec::new();
    for item in repo.branches(None).map_err(GitError::Branch)? {
        let (branch, kind) = item.map_err(GitError::Branch)?;
        let Some(name) = branch.name().map_err(GitError::Branch)? else {
            continue; // 名字不是合法 UTF-8：跳过，界面反正也显示不了
        };
        let is_remote = kind == BranchType::Remote;
        let is_head = !is_remote && head_name.as_deref() == Some(name);
        let upstream = branch
            .upstream()
            .ok()
            .and_then(|u| u.name().ok().flatten().map(str::to_string));

        let ahead_behind = if is_remote {
            None
        } else if let (Some(local_oid), Some(upstream_name)) =
            (branch.get().target(), upstream.as_deref())
        {
            repo.find_branch(upstream_name, BranchType::Remote)
                .ok()
                .and_then(|u| u.get().target())
                .and_then(|upstream_oid| repo.graph_ahead_behind(local_oid, upstream_oid).ok())
        } else {
            None
        };

        out.push(Branch {
            name: name.to_string(),
            is_head,
            is_remote,
            upstream,
            ahead_behind,
        });
    }
    Ok(out)
}

/// 当前 `HEAD` 指向的本地分支短名；`HEAD` 分离（detached）时为 `None`。
pub fn current_branch_name(repo_path: &Path) -> Result<Option<String>, GitError> {
    let repo = open(repo_path)?;
    let head = repo.head().map_err(GitError::Branch)?;
    Ok(head
        .is_branch()
        .then(|| head.shorthand().map(str::to_string))
        .flatten())
}

/// 新建分支；`from` 是提交号 / 分支名等 revspec，`None` 表示从当前 `HEAD` 开始。
pub fn create_branch(repo_path: &Path, name: &str, from: Option<&str>) -> Result<(), GitError> {
    let repo = open(repo_path)?;
    let target = from.unwrap_or("HEAD");
    let commit = repo
        .revparse_single(target)
        .and_then(|obj| obj.peel_to_commit())
        .map_err(GitError::Branch)?;
    repo.branch(name, &commit, false).map_err(GitError::Branch)?;
    Ok(())
}

/// 切到一个分支（或任意 revspec，落到分离 HEAD）。libgit2 的默认 checkout 策略是
/// 「safe」：工作树里有未提交改动、切换会覆盖它们时直接报错而不是强推——这正是
/// 「显式报错而不是强制 checkout」要的行为，不用我们自己再判一遍。
pub fn checkout_branch(repo_path: &Path, name: &str) -> Result<(), GitError> {
    let repo = open(repo_path)?;
    let (object, reference) = repo.revparse_ext(name).map_err(GitError::Branch)?;
    let mut checkout = CheckoutBuilder::new();
    checkout.safe();
    repo.checkout_tree(&object, Some(&mut checkout))
        .map_err(GitError::Branch)?;
    match reference {
        Some(r) if r.is_branch() => {
            let branch_ref = r.name().ok_or_else(|| {
                GitError::Branch(git2::Error::from_str("branch ref name isn't valid UTF-8"))
            })?;
            repo.set_head(branch_ref).map_err(GitError::Branch)?;
        }
        _ => {
            repo.set_head_detached(object.id()).map_err(GitError::Branch)?;
        }
    }
    Ok(())
}

/// 删除一个本地分支。删除当前签出的分支时 libgit2 本身就会拒绝
/// （`Cannot delete branch '<name>' checked out at ...`），这里不需要重复判断。
pub fn delete_branch(repo_path: &Path, name: &str) -> Result<(), GitError> {
    let repo = open(repo_path)?;
    let mut branch = repo
        .find_branch(name, BranchType::Local)
        .map_err(GitError::Branch)?;
    branch.delete().map_err(GitError::Branch)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;

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
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "user.email", "a@b.c"]);
        run(&["config", "user.name", "test"]);
        fs::write(dir.path().join("a.txt"), "hi").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "init"]);
        dir
    }

    #[test]
    fn lists_the_only_branch_as_head() {
        let dir = init_repo();
        let branches = list_branches(dir.path()).unwrap();
        assert_eq!(branches.len(), 1);
        assert_eq!(branches[0].name, "main");
        assert!(branches[0].is_head);
        assert!(!branches[0].is_remote);
    }

    #[test]
    fn create_switch_and_delete_round_trip() {
        let dir = init_repo();
        create_branch(dir.path(), "feature", None).unwrap();
        let branches = list_branches(dir.path()).unwrap();
        assert_eq!(branches.len(), 2);
        assert!(branches.iter().any(|b| b.name == "feature" && !b.is_head));

        checkout_branch(dir.path(), "feature").unwrap();
        assert_eq!(
            current_branch_name(dir.path()).unwrap().as_deref(),
            Some("feature")
        );

        // 不能删掉当前签出的分支
        assert!(delete_branch(dir.path(), "feature").is_err());

        checkout_branch(dir.path(), "main").unwrap();
        delete_branch(dir.path(), "feature").unwrap();
        let branches = list_branches(dir.path()).unwrap();
        assert_eq!(branches.len(), 1);
        assert_eq!(branches[0].name, "main");
    }

    #[test]
    fn checkout_refuses_to_clobber_uncommitted_changes() {
        let dir = init_repo();
        let run = |args: &[&str]| {
            let status = Command::new("git")
                .args(args)
                .current_dir(dir.path())
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?} failed");
        };

        // feature 分支上把 a.txt 改成和 main 不同的内容并提交，这样两个分支的
        // a.txt 才是真的不同——同一个提交下切分支永远是安全的，测不出「会覆盖」。
        create_branch(dir.path(), "feature", None).unwrap();
        checkout_branch(dir.path(), "feature").unwrap();
        fs::write(dir.path().join("a.txt"), "feature content").unwrap();
        run(&["commit", "-aqm", "feature change"]);
        checkout_branch(dir.path(), "main").unwrap();

        // 工作树里有未提交的改动，且三方（HEAD / feature / 工作树）内容互不相同
        fs::write(dir.path().join("a.txt"), "conflicting local edit").unwrap();
        assert!(checkout_branch(dir.path(), "feature").is_err());
        // 没有被强推：文件内容还是本地改动，HEAD 也没变
        assert_eq!(
            fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "conflicting local edit"
        );
        assert_eq!(
            current_branch_name(dir.path()).unwrap().as_deref(),
            Some("main")
        );
    }
}
