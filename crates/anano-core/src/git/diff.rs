//! Diffs for the main pane: one changed file (staged or unstaged side), or a whole commit.
//!
//! Shape borrowed from the Nermal IDE's `core/git/diff.rs`: a flat list of rows, each a file
//! header, a hunk header, or a context / added / deleted line carrying its old and new line
//! numbers, with a line budget so one giant diff can't stall the UI.

use std::path::Path;

use git2::{Diff, DiffFormat, DiffOptions, Oid, Repository};

use crate::git::error::GitError;

/// Rows kept per diff; the rest is dropped and [`DiffView::truncated`] is set.
const MAX_LINES: usize = 20_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    File,
    Hunk,
    Context,
    Add,
    Del,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: LineKind,
    pub old_no: Option<u32>,
    pub new_no: Option<u32>,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiffView {
    pub lines: Vec<DiffLine>,
    pub truncated: bool,
}

fn open(path: &Path) -> Result<Repository, GitError> {
    Repository::discover(path).map_err(GitError::Open)
}

fn collect(diff: &Diff<'_>) -> Result<DiffView, GitError> {
    let mut view = DiffView::default();
    let result = diff.print(DiffFormat::Patch, |delta, _hunk, line| {
        if view.lines.len() >= MAX_LINES {
            view.truncated = true;
            return false;
        }
        let text = String::from_utf8_lossy(line.content());
        let text = text.trim_end_matches(['\n', '\r']);
        let (kind, text) = match line.origin() {
            // libgit2's file header is the 4-line `diff --git` block; the path says it all.
            'F' => {
                let file = delta.new_file().path().or(delta.old_file().path());
                let path = file.map(|p| p.display().to_string()).unwrap_or_default();
                (LineKind::File, path)
            }
            'H' => (LineKind::Hunk, text.to_string()),
            '+' => (LineKind::Add, text.to_string()),
            '-' => (LineKind::Del, text.to_string()),
            'B' => (LineKind::Hunk, "Binary file".to_string()),
            // ' ' plus the "no newline at end of file" markers ('=', '>', '<').
            _ => (LineKind::Context, text.to_string()),
        };
        view.lines.push(DiffLine {
            kind,
            old_no: line.old_lineno(),
            new_no: line.new_lineno(),
            text,
        });
        true
    });
    // Our own `return false` (budget hit) surfaces as a "user" error; that one is expected.
    match result {
        Err(err) if !view.truncated => Err(GitError::Diff(err)),
        _ => Ok(view),
    }
}

/// The diff of one file: index→worktree when `staged` is false (untracked files show as
/// all-added), HEAD→index when true.
pub fn diff_file(repo_path: &Path, file: &str, staged: bool) -> Result<DiffView, GitError> {
    let repo = open(repo_path)?;
    let mut opts = DiffOptions::new();
    opts.pathspec(file)
        .disable_pathspec_match(true)
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .show_untracked_content(true);
    let diff = if staged {
        // No HEAD yet (fresh repo): everything staged is new, i.e. a diff against nothing.
        let head = repo.head().ok().and_then(|h| h.peel_to_tree().ok());
        repo.diff_tree_to_index(head.as_ref(), None, Some(&mut opts))
    } else {
        repo.diff_index_to_workdir(None, Some(&mut opts))
    }
    .map_err(GitError::Diff)?;
    collect(&diff)
}

/// Everything one commit changed, against its first parent (or against nothing for a root).
pub fn diff_commit(repo_path: &Path, id: &str) -> Result<DiffView, GitError> {
    let repo = open(repo_path)?;
    let commit = Oid::from_str(id)
        .and_then(|oid| repo.find_commit(oid))
        .map_err(GitError::Diff)?;
    let tree = commit.tree().map_err(GitError::Diff)?;
    let parent = commit.parent(0).ok().and_then(|p| p.tree().ok());
    let diff = repo
        .diff_tree_to_tree(parent.as_ref(), Some(&tree), None)
        .map_err(GitError::Diff)?;
    collect(&diff)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;

    fn git(args: &[&str], cwd: &Path) {
        assert!(
            Command::new("git")
                .args(args)
                .current_dir(cwd)
                .status()
                .unwrap()
                .success()
        );
    }

    fn kinds(view: &DiffView) -> Vec<(LineKind, &str)> {
        view.lines
            .iter()
            .map(|l| (l.kind, l.text.as_str()))
            .collect()
    }

    #[test]
    fn file_and_commit_diffs_carry_kinds_and_line_numbers() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        git(&["init", "-q", "-b", "main"], p);
        git(&["config", "user.email", "a@b.c"], p);
        git(&["config", "user.name", "test"], p);
        fs::write(p.join("a.txt"), "one\ntwo\n").unwrap();
        git(&["add", "."], p);
        git(&["commit", "-q", "-m", "init"], p);

        // Root commit: the whole file is added.
        let head = Repository::open(p)
            .unwrap()
            .head()
            .unwrap()
            .target()
            .unwrap();
        let commit = diff_commit(p, &head.to_string()).unwrap();
        assert_eq!(
            commit.lines[0],
            DiffLine {
                kind: LineKind::File,
                old_no: None,
                new_no: None,
                text: "a.txt".into(),
            }
        );
        assert!(kinds(&commit).contains(&(LineKind::Add, "two")));

        // Unstaged edit: one line replaced, numbered on the right sides.
        fs::write(p.join("a.txt"), "one\nTWO\n").unwrap();
        let unstaged = diff_file(p, "a.txt", false).unwrap();
        let del = unstaged
            .lines
            .iter()
            .find(|l| l.kind == LineKind::Del)
            .unwrap();
        let add = unstaged
            .lines
            .iter()
            .find(|l| l.kind == LineKind::Add)
            .unwrap();
        assert_eq!(
            (del.text.as_str(), del.old_no, del.new_no),
            ("two", Some(2), None)
        );
        assert_eq!(
            (add.text.as_str(), add.old_no, add.new_no),
            ("TWO", None, Some(2))
        );
        assert!(diff_file(p, "a.txt", true).unwrap().lines.is_empty());

        // Staged: the same edit moves to the HEAD→index side.
        git(&["add", "."], p);
        assert!(diff_file(p, "a.txt", false).unwrap().lines.is_empty());
        assert!(kinds(&diff_file(p, "a.txt", true).unwrap()).contains(&(LineKind::Add, "TWO")));

        // Untracked files show their content instead of an empty diff.
        fs::write(p.join("new.txt"), "fresh\n").unwrap();
        assert!(
            kinds(&diff_file(p, "new.txt", false).unwrap()).contains(&(LineKind::Add, "fresh"))
        );
    }
}
