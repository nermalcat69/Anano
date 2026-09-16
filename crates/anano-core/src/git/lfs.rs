//! Git LFS：libgit2 不跑 smudge/clean 过滤器，没有稳定的 Rust 绑定，标准做法是直接
//! 调系统装的 `git-lfs` 二进制。这里只管两件事：检测（仓库是否启用 LFS、本机装没装
//! `git-lfs`）与同步（在 git2 的 fetch/pull/push 成功之后，在仓库工作目录里跑对应的
//! `git-lfs fetch/pull/push`）。checkout/commit 阶段的 smudge/clean 过滤不归这里管——
//! 那部分仍然是系统 `git`/`git-lfs` 钩子的职责，这个模块不拦截 git2 的 checkout/commit。

use std::path::Path;
use std::process::Command;

use crate::git::error::GitError;

/// 仓库是否启用了 LFS：`.gitattributes` 里出现过 `filter=lfs` 就算。纯文本扫描，
/// 不用为了检测就先跑一次 `git-lfs`。
pub fn is_lfs_enabled(repo_path: &Path) -> bool {
    std::fs::read_to_string(repo_path.join(".gitattributes"))
        .map(|text| text.contains("filter=lfs"))
        .unwrap_or(false)
}

/// 本机是否装了 `git-lfs`。
pub fn is_lfs_installed() -> bool {
    Command::new("git-lfs")
        .arg("version")
        .output()
        .is_ok_and(|out| out.status.success())
}

/// 一次 `git-lfs` 同步之后的结果，供 UI 区分「不需要」「成功了」「该装但没装」三种状态，
/// 而不是把「没装 git-lfs」悄悄当成同步失败或悄悄跳过。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LfsSync {
    /// 仓库没启用 LFS。
    NotApplicable,
    Synced,
    /// 仓库启用了 LFS，但本机没装 `git-lfs`。
    NotInstalled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LfsOp {
    Fetch,
    Pull,
    Push,
}

impl LfsOp {
    fn arg(self) -> &'static str {
        match self {
            LfsOp::Fetch => "fetch",
            LfsOp::Pull => "pull",
            LfsOp::Push => "push",
        }
    }
}

/// 检测 + 同步一步到位：不是 LFS 仓库就直接说不需要；是但没装 `git-lfs` 就说清楚；
/// 装了才真的跑 `git-lfs <op>`，命令失败时把 stderr 带回去。
pub fn sync(repo_path: &Path, op: LfsOp) -> Result<LfsSync, GitError> {
    if !is_lfs_enabled(repo_path) {
        return Ok(LfsSync::NotApplicable);
    }
    if !is_lfs_installed() {
        return Ok(LfsSync::NotInstalled);
    }
    let output = Command::new("git-lfs")
        .arg(op.arg())
        .current_dir(repo_path)
        .output()
        .map_err(|e| GitError::Lfs(e.to_string()))?;
    if output.status.success() {
        Ok(LfsSync::Synced)
    } else {
        Err(GitError::Lfs(
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ))
    }
}

/// LFS 指针 blob 的识别：内容以这个 spec header 开头（真实文件内容早被 smudge 过滤器
/// 换掉了，工作树里能看到的原始 blob 只有走了 `git show`/`git2` 直接读 blob 内容时才是
/// 指针文本本身）。Phase 2 的 diff 视图还没做，这里先把识别逻辑落地，供之后接线。
pub fn is_lfs_pointer(blob: &[u8]) -> bool {
    blob.starts_with(b"version https://git-lfs.github.com/spec")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_lfs_attribute_in_gitattributes() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!is_lfs_enabled(dir.path()));

        std::fs::write(
            dir.path().join(".gitattributes"),
            "*.psd filter=lfs diff=lfs merge=lfs",
        )
        .unwrap();
        assert!(is_lfs_enabled(dir.path()));
    }

    #[test]
    fn plain_gitattributes_without_lfs_is_not_enabled() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".gitattributes"), "* text=auto").unwrap();
        assert!(!is_lfs_enabled(dir.path()));
    }

    #[test]
    fn recognizes_lfs_pointer_blobs() {
        let pointer = b"version https://git-lfs.github.com/spec/v1\noid sha256:abc\nsize 123\n";
        assert!(is_lfs_pointer(pointer));
        assert!(!is_lfs_pointer(b"just a normal text file"));
    }

    #[test]
    fn sync_is_not_applicable_when_repo_has_no_lfs_attributes() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            sync(dir.path(), LfsOp::Fetch).unwrap(),
            LfsSync::NotApplicable
        );
    }
}
