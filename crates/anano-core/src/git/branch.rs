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

        let last_commit_time = branch
            .get()
            .peel_to_commit()
            .ok()
            .map(|c| c.time().seconds());
        out.push(Branch {
            name: name.to_string(),
            is_head,
            is_remote,
            upstream,
            ahead_behind,
            last_commit_time,
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

/// The branch `HEAD` names, read from the symbolic ref itself rather than by resolving it.
///
/// That distinction matters for a repository with no commits yet: `HEAD` already says
/// `refs/heads/main`, but there is no commit for it to resolve to, so [`current_branch_name`]
/// errors and the branch list is empty. Only a genuinely detached `HEAD` gives `None` here.
pub fn head_branch(repo_path: &Path) -> Option<String> {
    let repo = open(repo_path).ok()?;
    let head = repo.find_reference("HEAD").ok()?;
    head.symbolic_target()?
        .strip_prefix("refs/heads/")
        .map(str::to_string)
}

/// 新建分支；`from` 是提交号 / 分支名等 revspec，`None` 表示从当前 `HEAD` 开始。
pub fn create_branch(repo_path: &Path, name: &str, from: Option<&str>) -> Result<(), GitError> {
    let repo = open(repo_path)?;
    let target = from.unwrap_or("HEAD");
    let commit = repo
        .revparse_single(target)
        .and_then(|obj| obj.peel_to_commit())
        .map_err(GitError::Branch)?;
    repo.branch(name, &commit, false)
        .map_err(GitError::Branch)?;
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
            repo.set_head_detached(object.id())
                .map_err(GitError::Branch)?;
        }
    }
    Ok(())
}

/// The stash message that marks "work left behind on `branch`" (see [`switch_branch`]).
fn stash_marker(branch: &str) -> String {
    format!("anano: changes left on {branch}")
}

/// Switch branches the way GitHub Desktop's "Switch Branch" dialog offers.
///
/// - `leave_changes == false`: plain safe checkout; uncommitted work comes along (and the
///   switch is refused, untouched, if it would be overwritten).
/// - `leave_changes == true`: stash the uncommitted work under the branch being left, then
///   switch with a clean tree.
///
/// Either way, arriving on a branch that has work stashed this way brings that work back,
/// so "leave my changes on main" means they are there again the next time you're on main.
pub fn switch_branch(repo_path: &Path, name: &str, leave_changes: bool) -> Result<(), GitError> {
    let mut repo = open(repo_path)?;
    if leave_changes && let Some(current) = current_branch_name(repo_path)? {
        let signature = repo.signature().map_err(GitError::Branch)?;
        let flags = git2::StashFlags::INCLUDE_UNTRACKED;
        match repo.stash_save(&signature, &stash_marker(&current), Some(flags)) {
            Ok(_) => {}
            // Nothing to stash is not a failure, just nothing to leave behind.
            Err(e) if e.code() == git2::ErrorCode::NotFound => {}
            Err(e) => return Err(GitError::Branch(e)),
        }
    }
    checkout_branch(repo_path, name)?;

    let marker = stash_marker(name);
    let mut left_here = None;
    repo.stash_foreach(|index, message, _| {
        if message.ends_with(&marker) {
            left_here = Some(index);
        }
        left_here.is_none()
    })
    .map_err(GitError::Branch)?;
    if let Some(index) = left_here {
        repo.stash_pop(index, None).map_err(GitError::Branch)?;
    }
    Ok(())
}

/// Rename a local branch (`git branch -m old new`): its upstream setting follows it, and
/// `HEAD` follows too when it is the checked-out one. Refuses a name that is taken.
pub fn rename_branch(repo_path: &Path, old: &str, new: &str) -> Result<(), GitError> {
    let repo = open(repo_path)?;
    let mut branch = repo
        .find_branch(old, BranchType::Local)
        .map_err(GitError::Branch)?;
    branch.rename(new, false).map_err(GitError::Branch)?;
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

    #[test]
    fn leaving_changes_stashes_them_and_coming_back_restores_them() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        let git = |args: &[&str]| {
            assert!(
                std::process::Command::new("git")
                    .args(args)
                    .current_dir(p)
                    .status()
                    .unwrap()
                    .success()
            );
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "a@b.c"]);
        git(&["config", "user.name", "test"]);
        std::fs::write(p.join("a.txt"), "one\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "init"]);
        create_branch(p, "feature", None).unwrap();

        // Work in progress on main: an edit and a brand new file.
        std::fs::write(p.join("a.txt"), "edited\n").unwrap();
        std::fs::write(p.join("new.txt"), "wip\n").unwrap();

        switch_branch(p, "feature", true).unwrap();
        assert_eq!(current_branch_name(p).unwrap().as_deref(), Some("feature"));
        assert_eq!(std::fs::read_to_string(p.join("a.txt")).unwrap(), "one\n");
        assert!(!p.join("new.txt").exists(), "left-behind work followed us");

        switch_branch(p, "main", false).unwrap();
        assert_eq!(
            std::fs::read_to_string(p.join("a.txt")).unwrap(),
            "edited\n"
        );
        assert!(
            p.join("new.txt").exists(),
            "left-behind work wasn't restored"
        );

        // "Bring my changes": they come along and nothing is stashed.
        switch_branch(p, "feature", false).unwrap();
        assert_eq!(
            std::fs::read_to_string(p.join("a.txt")).unwrap(),
            "edited\n"
        );
    }

    #[test]
    fn head_branch_is_known_before_the_first_commit_and_none_when_detached() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        let git = |args: &[&str]| {
            assert!(
                std::process::Command::new("git")
                    .args(args)
                    .current_dir(p)
                    .status()
                    .unwrap()
                    .success()
            );
        };
        git(&["init", "-q", "-b", "main"]);
        // No commits yet: nothing to resolve, but HEAD still names the branch.
        assert!(current_branch_name(p).is_err());
        assert_eq!(head_branch(p).as_deref(), Some("main"));

        git(&["config", "user.email", "a@b.c"]);
        git(&["config", "user.name", "test"]);
        std::fs::write(p.join("a.txt"), "one\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "init"]);
        assert_eq!(head_branch(p).as_deref(), Some("main"));

        git(&["checkout", "-q", "--detach"]);
        assert_eq!(head_branch(p), None);
    }

    #[test]
    fn renaming_a_branch_moves_head_and_refuses_a_taken_name() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        let git = |args: &[&str]| {
            assert!(
                std::process::Command::new("git")
                    .args(args)
                    .current_dir(p)
                    .status()
                    .unwrap()
                    .success()
            );
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "a@b.c"]);
        git(&["config", "user.name", "test"]);
        std::fs::write(p.join("a.txt"), "one\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "init"]);
        create_branch(p, "side", None).unwrap();

        // The checked-out branch: HEAD follows the new name.
        rename_branch(p, "main", "trunk").unwrap();
        assert_eq!(current_branch_name(p).unwrap().as_deref(), Some("trunk"));
        let names: Vec<_> = list_branches(p)
            .unwrap()
            .into_iter()
            .map(|b| b.name)
            .collect();
        assert!(names.contains(&"trunk".to_string()) && !names.contains(&"main".to_string()));

        // A name that exists is refused and nothing changes.
        assert!(rename_branch(p, "side", "trunk").is_err());
        assert!(rename_branch(p, "side", "").is_err());
        assert!(rename_branch(p, "nope", "other").is_err());
        let names: Vec<_> = list_branches(p)
            .unwrap()
            .into_iter()
            .map(|b| b.name)
            .collect();
        assert!(names.contains(&"side".to_string()));
    }
}
