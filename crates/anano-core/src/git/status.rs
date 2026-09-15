//! `git2::Status` 位标志 → [`ChangeKind`] 的映射。一个文件可能同时在索引（已 stage）
//! 与工作树（未 stage）两边都有改动，各算一条，与 `git status` 的展示口径一致。

use git2::Status;

use crate::git::model::ChangeKind;

/// 把一个文件的 `Status` 位标志拆成（种类, 是否已 staged）的列表；忽略的文件返回空。
pub fn map_status(status: Status) -> Vec<(ChangeKind, bool)> {
    let mut out = Vec::with_capacity(2);
    if status.intersects(Status::CONFLICTED) {
        out.push((ChangeKind::Conflicted, false));
        return out;
    }
    let staged = [
        (Status::INDEX_NEW, ChangeKind::New),
        (Status::INDEX_MODIFIED, ChangeKind::Modified),
        (Status::INDEX_DELETED, ChangeKind::Deleted),
        (Status::INDEX_RENAMED, ChangeKind::Renamed),
        (Status::INDEX_TYPECHANGE, ChangeKind::TypeChange),
    ];
    for (bit, kind) in staged {
        if status.intersects(bit) {
            out.push((kind, true));
        }
    }
    let unstaged = [
        (Status::WT_NEW, ChangeKind::New),
        (Status::WT_MODIFIED, ChangeKind::Modified),
        (Status::WT_DELETED, ChangeKind::Deleted),
        (Status::WT_RENAMED, ChangeKind::Renamed),
        (Status::WT_TYPECHANGE, ChangeKind::TypeChange),
    ];
    for (bit, kind) in unstaged {
        if status.intersects(bit) {
            out.push((kind, false));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conflicted_wins_over_everything_else() {
        let s = Status::CONFLICTED | Status::WT_MODIFIED;
        assert_eq!(map_status(s), vec![(ChangeKind::Conflicted, false)]);
    }

    #[test]
    fn staged_and_unstaged_both_produce_entries() {
        let s = Status::INDEX_MODIFIED | Status::WT_MODIFIED;
        assert_eq!(
            map_status(s),
            vec![(ChangeKind::Modified, true), (ChangeKind::Modified, false)]
        );
    }

    #[test]
    fn untracked_file_is_new_and_unstaged() {
        assert_eq!(map_status(Status::WT_NEW), vec![(ChangeKind::New, false)]);
    }

    #[test]
    fn clean_file_produces_nothing() {
        assert!(map_status(Status::CURRENT).is_empty());
    }
}
