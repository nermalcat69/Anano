//! Git 领域模型：一个已打开的仓库条目，与工作树里一个文件的状态。

use serde::{Deserialize, Serialize};

use crate::model::{Ulid, now_ms};
use std::path::{Path, PathBuf};

/// 侧栏里的一个仓库；跨会话持久化（`repos.json`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoEntry {
    pub id: Ulid,
    pub path: PathBuf,
    pub display_name: String,
    /// Unix 毫秒；目前只用于展示/排序，phase 1 不做「最近」列表。
    pub last_opened: i64,
}

impl RepoEntry {
    /// 用文件夹路径新建一条：显示名取路径末段，取不到（如根目录）就用完整路径。
    pub fn new(path: PathBuf) -> Self {
        let display_name = display_name_for(&path);
        Self {
            id: Ulid::generate(),
            path,
            display_name,
            last_opened: now_ms(),
        }
    }
}

fn display_name_for(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// 一个文件的改动种类；由 `git2::Status` 的位标志归并而来（见 [`crate::git::status`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    New,
    Modified,
    Deleted,
    Renamed,
    TypeChange,
    Conflicted,
}

/// 工作树里一个文件的一条状态记录。同一路径在索引与工作树两边都有改动时，
/// 会各产出一条（`staged` 区分是哪一边），与 `git status` 的展示口径一致。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStatus {
    pub path: String,
    pub kind: ChangeKind,
    pub staged: bool,
}

/// 一个分支（本地或远程跟踪）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branch {
    /// 本地分支是短名（`main`）；远程跟踪分支带远程前缀（`origin/main`）。
    pub name: String,
    /// 是不是当前 `HEAD` 指向的那个（只有本地分支可能是）。
    pub is_head: bool,
    pub is_remote: bool,
    /// 配置的上游（如 `origin/main`）；本地分支没配上游、或本身就是远程跟踪分支时为 `None`。
    pub upstream: Option<String>,
    /// 相对上游的 (ahead, behind)；只有配了上游的本地分支才算，算起来不贵就顺手带上。
    pub ahead_behind: Option<(usize, usize)>,
}

/// 一条提交记录（`git log` 里的一行），供历史列表展示。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitInfo {
    /// 完整十六进制 hash；UI 只显示前 7 位。
    pub id: String,
    /// 提交信息的第一行。
    pub summary: String,
    pub author: String,
    /// 作者时间，Unix 秒。
    pub time: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_name_takes_the_last_path_segment() {
        let entry = RepoEntry::new(PathBuf::from("/home/user/projects/anano"));
        assert_eq!(entry.display_name, "anano");
    }

    #[test]
    fn display_name_falls_back_to_full_path_when_no_file_name() {
        let entry = RepoEntry::new(PathBuf::from("/"));
        assert_eq!(entry.display_name, "/");
    }
}
