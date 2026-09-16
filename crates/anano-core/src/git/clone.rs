//! 「Add Repository」里的 Clone / Create New：不接 GitHub OAuth，只用 git URL / 本地路径。
//! 认证复用 `remote::auth_callbacks`（SSH agent + HTTPS credential helper），不重复实现。

use std::path::{Path, PathBuf};

use git2::Repository;
use git2::build::RepoBuilder;

use crate::git::error::GitError;
use crate::git::remote::auth_callbacks;

/// `git clone <url> <dest>`。`dest` 必须还不存在（或是空目录），跟 `git clone` 本身一致。
pub fn clone_repo(url: &str, dest: &Path) -> Result<PathBuf, GitError> {
    let mut fetch_opts = git2::FetchOptions::new();
    fetch_opts.remote_callbacks(auth_callbacks());
    let mut builder = RepoBuilder::new();
    builder.fetch_options(fetch_opts);
    builder
        .clone(url, dest)
        .map_err(|e| GitError::Clone(e.to_string()))?;
    Ok(dest.to_path_buf())
}

/// 在 `parent/name` 新建一个空仓库（`git init`）。
pub fn init_repo(parent: &Path, name: &str) -> Result<PathBuf, GitError> {
    let dest = parent.join(name);
    std::fs::create_dir_all(&dest).map_err(|e| GitError::Init(e.to_string()))?;
    Repository::init(&dest).map_err(|e| GitError::Init(e.to_string()))?;
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn init_repo_creates_a_git_directory() {
        let parent = tempfile::tempdir().unwrap();
        let dest = init_repo(parent.path(), "my-repo").unwrap();
        assert!(dest.join(".git").is_dir());
    }

    #[test]
    fn clone_repo_checks_out_the_remote_files() {
        // 本地裸仓库当「远程」，跟 `remote.rs` 的测试手法一致，不需要真实网络。
        let remote_dir = tempfile::tempdir().unwrap();
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
        let seed = tempfile::tempdir().unwrap();
        run(&["init", "-q", "-b", "main"], seed.path());
        run(&["config", "user.email", "a@b.c"], seed.path());
        run(&["config", "user.name", "test"], seed.path());
        std::fs::write(seed.path().join("a.txt"), "hi").unwrap();
        run(&["add", "."], seed.path());
        run(&["commit", "-q", "-m", "init"], seed.path());
        run(
            &[
                "clone",
                "-q",
                "--bare",
                seed.path().to_str().unwrap(),
                remote_dir.path().to_str().unwrap(),
            ],
            seed.path(),
        );

        let parent = tempfile::tempdir().unwrap();
        let dest = parent.path().join("cloned");
        clone_repo(remote_dir.path().to_str().unwrap(), &dest).unwrap();
        assert!(dest.join("a.txt").exists());
    }
}
