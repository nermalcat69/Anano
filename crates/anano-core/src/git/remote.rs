//! 远程：fetch / pull（快进专属）/ push，三个独立操作，不揉进 commit。
//!
//! 认证走 `git2::RemoteCallbacks`：`ssh://` / `git@` 先试 SSH agent，`https://` 退回系统
//! 的 git credential helper——覆盖常见场景，不需要现在就做一个凭据输入 UI。
//!
//! ponytail: 没有接实时进度（`transfer_progress`/`push_transfer_progress`）回 UI，
//! 只返回最终成功/失败；界面用一个 loading 转圈代替。要接实时进度条时，在这里的
//! `FetchOptions`/`PushOptions` 上挂回调，通过一个 channel 把字节数发回调用方即可。

use std::cell::RefCell;
use std::path::Path;

use git2::build::CheckoutBuilder;
use git2::{BranchType, Cred, CredentialType, FetchOptions, PushOptions, RemoteCallbacks};

use crate::git::error::GitError;

fn open(path: &Path) -> Result<git2::Repository, GitError> {
    git2::Repository::discover(path).map_err(GitError::Open)
}

/// 建立在 `pub(crate)` 上：`clone.rs` 里的克隆也要用同一套认证回调，不重复实现。
pub(crate) fn auth_callbacks() -> RemoteCallbacks<'static> {
    auth_callbacks_skipping(0)
}

/// GitHub answers a valid token without access to the repository with 403 (not 401), and
/// libgit2 treats that as final instead of asking for other credentials. So the caller
/// retries the whole operation with the next connected account: `skip` accounts are passed
/// over before the first one is offered.
fn is_forbidden(err: &git2::Error) -> bool {
    err.class() == git2::ErrorClass::Http && err.message().contains("403")
}

/// Run `op` with each candidate account in turn while the server keeps answering 403.
fn with_account_fallback<T>(
    remote_url: Option<&str>,
    mut op: impl FnMut(usize) -> Result<T, git2::Error>,
) -> Result<T, git2::Error> {
    let tries = remote_url
        .filter(|u| crate::external::is_github_https(u))
        .map_or(1, |u| {
            crate::external::accounts_for(u, &crate::external::list_accounts())
                .len()
                .max(1)
        });
    let mut result = op(0);
    for skip in 1..tries {
        match &result {
            Err(e) if is_forbidden(e) => result = op(skip),
            _ => break,
        }
    }
    result
}

fn auth_callbacks_skipping(skip: usize) -> RemoteCallbacks<'static> {
    let mut cb = RemoteCallbacks::new();
    let mut attempts = 0;
    cb.credentials(move |url, username_from_url, allowed| {
        // libgit2 asks again every time the server rejects what we returned; without a cap
        // a revoked token or a wrong key means an endless loop instead of an error.
        attempts += 1;
        if attempts > 4 {
            return Err(git2::Error::from_str("the server rejected the credentials"));
        }
        // `https://<login>@github.com/...` pins the repository to one connected GitHub
        // account (see `external::with_account`); use that account's token.
        if allowed.contains(CredentialType::USER_PASS_PLAINTEXT)
            && crate::external::is_github_https(url)
        {
            // Which account: the one the URL is pinned to, else the repository's owner if
            // that account is connected, else the terminal's default, then the rest (see
            // `external::accounts_for`). Each rejection brings us back for the next one.
            // This is also what makes a plain `https://github.com/...` remote work at all
            // when git has no credential helper libgit2 can see.
            // (The pin is read from `username_from_url`: libgit2 may hand us `url` with the
            // user part already stripped.)
            if let Some(login) = username_from_url {
                if let Some(token) = crate::external::token(login) {
                    return Cred::userpass_plaintext(login, &token);
                }
            } else {
                let accounts =
                    crate::external::accounts_for(url, &crate::external::list_accounts());
                if let Some(account) = accounts.get(attempts - 1 + skip)
                    && let Some(token) = crate::external::token(&account.login)
                {
                    return Cred::userpass_plaintext(&account.login, &token);
                }
            }
        }
        if allowed.contains(CredentialType::SSH_KEY)
            && let Some(user) = username_from_url
            && let Ok(cred) = Cred::ssh_key_from_agent(user)
        {
            return Ok(cred);
        }
        if allowed.contains(CredentialType::USER_PASS_PLAINTEXT)
            && let Ok(cfg) = git2::Config::open_default()
            && let Ok(cred) = Cred::credential_helper(&cfg, url, username_from_url)
        {
            return Ok(cred);
        }
        Err(git2::Error::from_str(
            "no usable git credentials found (tried the SSH agent and the git credential helper)",
        ))
    });
    cb
}

/// `git fetch <remote>`：更新远程跟踪分支，不碰工作树或本地分支。
pub fn fetch(repo_path: &Path, remote_name: &str) -> Result<(), GitError> {
    let repo = open(repo_path)?;
    let mut remote = repo.find_remote(remote_name).map_err(GitError::Remote)?;
    let url = remote.url().map(str::to_string);
    with_account_fallback(url.as_deref(), |skip| {
        let mut opts = FetchOptions::new();
        opts.remote_callbacks(auth_callbacks_skipping(skip));
        remote.fetch(&[] as &[&str], Some(&mut opts), None)
    })
    .map_err(GitError::Remote)?;
    Ok(())
}

/// Bring the commits the upstream has into the current branch when both sides have new
/// ones (what `pull` refuses to do for you): merge them in, or put your commits on top of
/// them (`rebase`). Local only: the upstream was fetched already. If it stops on conflicts
/// the repository is left mid-merge / mid-rebase with the conflicted files marked, which is
/// how the Changes list shows them; the error says so.
pub fn integrate_upstream(repo_path: &Path, rebase: bool) -> Result<(), GitError> {
    let args: &[&str] = if rebase {
        &["rebase", "@{u}"]
    } else {
        &["merge", "--no-edit", "@{u}"]
    };
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(repo_path)
        .env("GIT_EDITOR", "true")
        .output()
        .map_err(|e| GitError::Remote(git2::Error::from_str(&e.to_string())))?;
    if out.status.success() {
        return Ok(());
    }
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let message = if text.contains("CONFLICT") {
        "The merge stopped on conflicts. Resolve them in the Changes list (Claude Code can do it)."
            .to_string()
    } else {
        text.trim()
            .lines()
            .last()
            .unwrap_or("git failed")
            .to_string()
    };
    Err(GitError::Remote(git2::Error::from_str(&message)))
}

/// `git pull` 的快进子集：先 fetch，再检查当前分支相对上游能不能快进。
/// 分叉了（双方各有新提交）就报 [`GitError::Diverged`]，不自动造合并提交——
/// 那是需要用户决定怎么处理冲突的事，不是这一版要做的。
pub fn pull(repo_path: &Path, remote_name: &str) -> Result<(), GitError> {
    fetch(repo_path, remote_name)?;

    let repo = open(repo_path)?;
    let head = repo.head().map_err(GitError::Remote)?;
    if !head.is_branch() {
        return Err(GitError::Remote(git2::Error::from_str(
            "HEAD is detached; nothing to pull into",
        )));
    }
    let branch_name = head
        .shorthand()
        .ok_or_else(|| GitError::Remote(git2::Error::from_str("branch name isn't valid UTF-8")))?
        .to_string();
    let refname = head
        .name()
        .ok_or_else(|| GitError::Remote(git2::Error::from_str("ref name isn't valid UTF-8")))?
        .to_string();

    let local_branch = repo
        .find_branch(&branch_name, BranchType::Local)
        .map_err(GitError::Remote)?;
    let upstream = local_branch.upstream().map_err(|_| GitError::NoUpstream)?;
    let upstream_oid = upstream
        .get()
        .target()
        .ok_or_else(|| GitError::Remote(git2::Error::from_str("upstream ref has no target")))?;

    let upstream_commit = repo
        .find_annotated_commit(upstream_oid)
        .map_err(GitError::Remote)?;
    let (analysis, _) = repo
        .merge_analysis(&[&upstream_commit])
        .map_err(GitError::Remote)?;
    if analysis.is_up_to_date() {
        return Ok(());
    }
    if !analysis.is_fast_forward() {
        return Err(GitError::Diverged);
    }

    // 先 checkout（safe 模式，工作树有冲突改动就在这里报错、不动任何引用），
    // 确认没问题了再挪 ref 和 HEAD，保证失败时仓库还停在改动前的一致状态。
    let target_obj = repo
        .find_object(upstream_oid, None)
        .map_err(GitError::Remote)?;
    let mut checkout = CheckoutBuilder::new();
    checkout.safe();
    repo.checkout_tree(&target_obj, Some(&mut checkout))
        .map_err(GitError::Remote)?;
    repo.reference(&refname, upstream_oid, true, "fast-forward pull")
        .map_err(GitError::Remote)?;
    repo.set_head(&refname).map_err(GitError::Remote)?;
    Ok(())
}

/// `git push <remote> <branch>`：推当前分支到同名远程分支。libgit2 的 `push()` 在被
/// 远端拒绝（比如非快进）时不会返回 `Err`，得靠 `push_update_reference` 回调抓状态消息。
pub fn push(repo_path: &Path, remote_name: &str, branch_name: &str) -> Result<(), GitError> {
    let repo = open(repo_path)?;
    let mut remote = repo.find_remote(remote_name).map_err(GitError::Remote)?;

    let rejected: RefCell<Option<String>> = RefCell::new(None);
    let refspec = format!("refs/heads/{branch_name}:refs/heads/{branch_name}");
    let url = remote.url().map(str::to_string);
    with_account_fallback(url.as_deref(), |skip| {
        let mut cb = auth_callbacks_skipping(skip);
        cb.push_update_reference(|refname, status| {
            if let Some(msg) = status {
                *rejected.borrow_mut() = Some(format!("{refname}: {msg}"));
            }
            Ok(())
        });
        let mut opts = PushOptions::new();
        opts.remote_callbacks(cb);
        remote.push(&[refspec.as_str()], Some(&mut opts))
    })
    .map_err(GitError::Remote)?;

    if let Some(msg) = rejected.into_inner() {
        return Err(GitError::Remote(git2::Error::from_str(&msg)));
    }
    // First push of a branch (e.g. a repository just created on GitHub): track the remote
    // branch, otherwise pull and the ahead/behind count have nothing to compare against.
    if let Ok(mut branch) = repo.find_branch(branch_name, BranchType::Local)
        && branch.upstream().is_err()
    {
        let _ = branch.set_upstream(Some(&format!("{remote_name}/{branch_name}")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;

    /// 本地裸仓库当「远程」：不用真的网络就能测 fetch/pull/push，标准做法。
    fn init_repo_pair() -> (tempfile::TempDir, tempfile::TempDir) {
        let remote_dir = tempfile::tempdir().unwrap();
        git2::Repository::init_bare(remote_dir.path()).unwrap();
        // 裸仓库默认 HEAD 常指向 `refs/heads/master`（不管我们打算用什么分支名），
        // 之后 clone 这个裸仓库时找不到 master 就不会检出任何文件（只会打印一句
        // warning，其余命令继续在一个空/游离的工作树上跑，测试会莫名其妙地失败）。
        // 显式把 HEAD 指到我们实际会推送的 `main`，clone 出来的工作树才是对的。
        assert!(
            Command::new("git")
                .args(["symbolic-ref", "HEAD", "refs/heads/main"])
                .current_dir(remote_dir.path())
                .status()
                .unwrap()
                .success()
        );

        let local_dir = tempfile::tempdir().unwrap();
        let run = |args: &[&str], cwd: &Path| {
            let status = Command::new("git")
                .args(args)
                .current_dir(cwd)
                .status()
                .expect("git installed");
            assert!(status.success(), "git {args:?} failed");
        };
        run(&["init", "-q", "-b", "main"], local_dir.path());
        run(&["config", "user.email", "a@b.c"], local_dir.path());
        run(&["config", "user.name", "test"], local_dir.path());
        run(
            &[
                "remote",
                "add",
                "origin",
                remote_dir.path().to_str().unwrap(),
            ],
            local_dir.path(),
        );
        fs::write(local_dir.path().join("a.txt"), "hi").unwrap();
        run(&["add", "."], local_dir.path());
        run(&["commit", "-q", "-m", "init"], local_dir.path());
        run(&["push", "-q", "-u", "origin", "main"], local_dir.path());
        (remote_dir, local_dir)
    }

    #[test]
    fn push_lands_the_commit_on_the_bare_remote() {
        let (remote_dir, local_dir) = init_repo_pair();
        fs::write(local_dir.path().join("b.txt"), "more").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(local_dir.path())
            .status()
            .unwrap();
        Command::new("git")
            .args(["commit", "-q", "-m", "second"])
            .current_dir(local_dir.path())
            .status()
            .unwrap();

        push(local_dir.path(), "origin", "main").unwrap();

        let remote_repo = git2::Repository::open_bare(remote_dir.path()).unwrap();
        let head = remote_repo.find_branch("main", BranchType::Local).unwrap();
        let commit = head.get().peel_to_commit().unwrap();
        assert_eq!(commit.summary(), Some("second"));
    }

    #[test]
    fn fetch_updates_the_remote_tracking_branch() {
        let (remote_dir, local_dir) = init_repo_pair();

        // 从另一个 clone 推一个新提交到远程，本地此时还不知道
        let other_dir = tempfile::tempdir().unwrap();
        let run = |args: &[&str], cwd: &Path| {
            assert!(
                Command::new("git")
                    .args(args)
                    .current_dir(cwd)
                    .status()
                    .unwrap()
                    .success()
            );
        };
        run(
            &[
                "clone",
                "-q",
                remote_dir.path().to_str().unwrap(),
                other_dir.path().to_str().unwrap(),
            ],
            local_dir.path(),
        );
        run(&["config", "user.email", "a@b.c"], other_dir.path());
        run(&["config", "user.name", "test"], other_dir.path());
        fs::write(other_dir.path().join("c.txt"), "from elsewhere").unwrap();
        run(&["add", "."], other_dir.path());
        run(
            &["commit", "-q", "-m", "from other clone"],
            other_dir.path(),
        );
        run(&["push", "-q"], other_dir.path());

        fetch(local_dir.path(), "origin").unwrap();

        let repo = git2::Repository::open(local_dir.path()).unwrap();
        let tracking = repo.find_branch("origin/main", BranchType::Remote).unwrap();
        let commit = tracking.get().peel_to_commit().unwrap();
        assert_eq!(commit.summary(), Some("from other clone"));
    }

    #[test]
    fn pull_fast_forwards_when_upstream_has_new_commits() {
        let (remote_dir, local_dir) = init_repo_pair();

        let other_dir = tempfile::tempdir().unwrap();
        let run = |args: &[&str], cwd: &Path| {
            assert!(
                Command::new("git")
                    .args(args)
                    .current_dir(cwd)
                    .status()
                    .unwrap()
                    .success()
            );
        };
        run(
            &[
                "clone",
                "-q",
                remote_dir.path().to_str().unwrap(),
                other_dir.path().to_str().unwrap(),
            ],
            local_dir.path(),
        );
        run(&["config", "user.email", "a@b.c"], other_dir.path());
        run(&["config", "user.name", "test"], other_dir.path());
        fs::write(other_dir.path().join("c.txt"), "from elsewhere").unwrap();
        run(&["add", "."], other_dir.path());
        run(
            &["commit", "-q", "-m", "from other clone"],
            other_dir.path(),
        );
        run(&["push", "-q"], other_dir.path());

        pull(local_dir.path(), "origin").unwrap();
        assert!(local_dir.path().join("c.txt").exists());
    }

    #[test]
    fn pull_reports_diverged_when_both_sides_have_new_commits() {
        let (remote_dir, local_dir) = init_repo_pair();

        let other_dir = tempfile::tempdir().unwrap();
        let run = |args: &[&str], cwd: &Path| {
            assert!(
                Command::new("git")
                    .args(args)
                    .current_dir(cwd)
                    .status()
                    .unwrap()
                    .success()
            );
        };
        run(
            &[
                "clone",
                "-q",
                remote_dir.path().to_str().unwrap(),
                other_dir.path().to_str().unwrap(),
            ],
            local_dir.path(),
        );
        run(&["config", "user.email", "a@b.c"], other_dir.path());
        run(&["config", "user.name", "test"], other_dir.path());
        fs::write(other_dir.path().join("remote-side.txt"), "x").unwrap();
        run(&["add", "."], other_dir.path());
        run(&["commit", "-q", "-m", "remote side"], other_dir.path());
        run(&["push", "-q"], other_dir.path());

        // 本地也有一个还没推的新提交
        fs::write(local_dir.path().join("local-side.txt"), "y").unwrap();
        run(&["add", "."], local_dir.path());
        run(&["commit", "-q", "-m", "local side"], local_dir.path());

        assert!(matches!(
            pull(local_dir.path(), "origin"),
            Err(GitError::Diverged)
        ));
    }

    /// Both sides have a commit: merge brings them together; a clash reports conflicts and
    /// leaves the repository mid-merge for the Changes list.
    #[test]
    fn integrating_a_diverged_upstream_merges_or_reports_conflicts() {
        let root = tempfile::tempdir().unwrap();
        let git = |args: &[&str], cwd: &Path| {
            let ok = Command::new("git")
                .args(args)
                .current_dir(cwd)
                .output()
                .unwrap();
            assert!(
                ok.status.success(),
                "{args:?}: {}",
                String::from_utf8_lossy(&ok.stderr)
            );
        };
        let commit = |dir: &Path, file: &str, text: &str| {
            fs::write(dir.join(file), text).unwrap();
            git(&["add", "."], dir);
            git(
                &[
                    "-c",
                    "user.email=t@t",
                    "-c",
                    "user.name=t",
                    "commit",
                    "-qm",
                    file,
                ],
                dir,
            );
        };
        let bare = root.path().join("bare.git");
        git(
            &["init", "-q", "--bare", "-b", "main", bare.to_str().unwrap()],
            root.path(),
        );
        let (a, b) = (root.path().join("a"), root.path().join("b"));
        git(
            &["clone", "-q", bare.to_str().unwrap(), a.to_str().unwrap()],
            root.path(),
        );
        commit(&a, "base.txt", "base\n");
        git(&["push", "-q", "origin", "HEAD:main"], &a);
        git(
            &["clone", "-q", bare.to_str().unwrap(), b.to_str().unwrap()],
            root.path(),
        );
        // Distinct files on each side: merges cleanly.
        commit(&a, "from-a.txt", "a\n");
        git(&["push", "-q", "origin", "HEAD:main"], &a);
        commit(&b, "from-b.txt", "b\n");
        git(&["fetch", "-q"], &b);
        assert!(matches!(pull(&b, "origin"), Err(GitError::Diverged)));
        integrate_upstream(&b, false).unwrap();
        assert!(b.join("from-a.txt").is_file() && b.join("from-b.txt").is_file());
        // Same file edited on both sides: conflicts, reported and left in place.
        commit(&a, "same.txt", "one\n");
        git(&["push", "-q", "origin", "HEAD:main"], &a);
        git(&["pull", "-q", "--no-rebase", "--no-edit"], &b);
        commit(&a, "same.txt", "from a\n");
        git(&["push", "-q", "origin", "HEAD:main"], &a);
        commit(&b, "same.txt", "from b\n");
        git(&["fetch", "-q"], &b);
        let err = integrate_upstream(&b, false).unwrap_err().to_string();
        assert!(err.contains("conflicts"), "{err}");
        assert!(
            fs::read_to_string(b.join("same.txt"))
                .unwrap()
                .contains("<<<<<<<")
        );
    }
}
