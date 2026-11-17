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

/// Everything that would be committed for one file: HEAD → what is on disk now, staged or
/// not. This is the diff for a row of the changes list, which no longer splits a file into
/// a staged and an unstaged half.
pub fn diff_file_worktree(repo_path: &Path, file: &str) -> Result<DiffView, GitError> {
    let repo = open(repo_path)?;
    let mut opts = DiffOptions::new();
    opts.pathspec(file)
        .disable_pathspec_match(true)
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .show_untracked_content(true);
    let head = repo.head().ok().and_then(|h| h.peel_to_tree().ok());
    let diff = repo
        .diff_tree_to_workdir_with_index(head.as_ref(), Some(&mut opts))
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

/// What a pull request did to one file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchStatus {
    Added,
    Deleted,
    Modified,
    Renamed,
}

/// One file of a unified diff (what `git diff` / `gh pr diff` print), with its lines
/// already numbered on both sides: the same rows the commit view draws.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchFile {
    pub path: String,
    /// Where a renamed file came from.
    pub old_path: Option<String>,
    pub status: PatchStatus,
    pub additions: usize,
    pub deletions: usize,
    pub binary: bool,
    /// Hunk headers, then context / added / deleted lines.
    pub lines: Vec<DiffLine>,
}

/// `@@ -12,6 +14,8 @@ fn x` → (12, 14).
fn hunk_start(header: &str) -> (u32, u32) {
    let number = |after: char| -> u32 {
        header
            .split_once(after)
            .map(|(_, rest)| {
                rest.chars()
                    .take_while(char::is_ascii_digit)
                    .collect::<String>()
            })
            .and_then(|digits| digits.parse().ok())
            .unwrap_or(0)
    };
    (number('-'), number('+'))
}

/// Split the text of a unified diff into its files. Anything that isn't a recognisable
/// header or hunk line is skipped, so a diff cut short by the server still yields the
/// files before the cut.
pub fn parse_patch(text: &str) -> Vec<PatchFile> {
    let mut files: Vec<PatchFile> = Vec::new();
    let (mut old_no, mut new_no) = (0u32, 0u32);
    let mut in_hunk = false;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            // `a/old b/new`; a path with a space makes the split ambiguous, so take the
            // last ` b/` (the new path is what the rest of the header agrees with).
            let (old, new) = rest.rsplit_once(" b/").unwrap_or((rest, rest));
            let old = old.strip_prefix("a/").unwrap_or(old);
            files.push(PatchFile {
                path: new.to_string(),
                old_path: (old != new).then(|| old.to_string()),
                status: PatchStatus::Modified,
                additions: 0,
                deletions: 0,
                binary: false,
                lines: Vec::new(),
            });
            in_hunk = false;
            continue;
        }
        let Some(file) = files.last_mut() else {
            continue;
        };
        let mut push = |kind, old: Option<u32>, new: Option<u32>, text: &str| {
            file.lines.push(DiffLine {
                kind,
                old_no: old,
                new_no: new,
                text: text.to_string(),
            });
        };
        if line.starts_with("@@") {
            (old_no, new_no) = hunk_start(line);
            in_hunk = true;
            push(LineKind::Hunk, None, None, line);
            continue;
        }
        if !in_hunk {
            if line.starts_with("new file mode") {
                file.status = PatchStatus::Added;
            } else if line.starts_with("deleted file mode") {
                file.status = PatchStatus::Deleted;
            } else if let Some(from) = line.strip_prefix("rename from ") {
                file.status = PatchStatus::Renamed;
                file.old_path = Some(from.to_string());
            } else if line.starts_with("Binary files") || line.starts_with("GIT binary patch") {
                file.binary = true;
                push(LineKind::Hunk, None, None, "Binary file");
            }
            continue;
        }
        match line.chars().next() {
            Some('+') => {
                file.additions += 1;
                push(LineKind::Add, None, Some(new_no), &line[1..]);
                new_no += 1;
            }
            Some('-') => {
                file.deletions += 1;
                push(LineKind::Del, Some(old_no), None, &line[1..]);
                old_no += 1;
            }
            // "\ No newline at end of file" belongs to the line above and has no number.
            Some('\\') => push(LineKind::Context, None, None, line),
            // A context line starts with a space; some tools strip it from empty ones.
            _ => {
                push(
                    LineKind::Context,
                    Some(old_no),
                    Some(new_no),
                    line.strip_prefix(' ').unwrap_or(line),
                );
                old_no += 1;
                new_no += 1;
            }
        }
    }
    files
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

        // The combined view sees the edit whether or not it is staged.
        assert!(kinds(&diff_file_worktree(p, "a.txt").unwrap()).contains(&(LineKind::Add, "TWO")));

        // Untracked files show their content instead of an empty diff.
        fs::write(p.join("new.txt"), "fresh\n").unwrap();
        assert!(
            kinds(&diff_file(p, "new.txt", false).unwrap()).contains(&(LineKind::Add, "fresh"))
        );
    }

    #[test]
    fn a_unified_diff_splits_into_numbered_files() {
        let patch = "\
diff --git a/src/a.rs b/src/a.rs
index 111..222 100644
--- a/src/a.rs
+++ b/src/a.rs
@@ -1,3 +1,4 @@ fn main() {
 keep
-old line
+new line
+extra
 tail
\\ No newline at end of file
diff --git a/new.txt b/new.txt
new file mode 100644
--- /dev/null
+++ b/new.txt
@@ -0,0 +1,2 @@
+one
+-- two
diff --git a/gone.txt b/gone.txt
deleted file mode 100644
--- a/gone.txt
+++ /dev/null
@@ -1 +0,0 @@
-bye
diff --git a/old.md b/renamed.md
similarity index 100%
rename from old.md
rename to renamed.md
diff --git a/img.png b/img.png
Binary files a/img.png and b/img.png differ
";
        let files = parse_patch(patch);
        let summary: Vec<_> = files
            .iter()
            .map(|f| {
                (
                    f.path.as_str(),
                    f.status,
                    f.additions,
                    f.deletions,
                    f.binary,
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                ("src/a.rs", PatchStatus::Modified, 2, 1, false),
                ("new.txt", PatchStatus::Added, 2, 0, false),
                ("gone.txt", PatchStatus::Deleted, 0, 1, false),
                ("renamed.md", PatchStatus::Renamed, 0, 0, false),
                ("img.png", PatchStatus::Modified, 0, 0, true),
            ]
        );
        assert_eq!(files[3].old_path.as_deref(), Some("old.md"));

        // Numbers follow both sides: the replaced line is 2 before and 2 after, and the
        // extra line only exists after.
        let rows: Vec<_> = files[0]
            .lines
            .iter()
            .map(|l| (l.kind, l.old_no, l.new_no, l.text.as_str()))
            .collect();
        assert_eq!(rows[1], (LineKind::Context, Some(1), Some(1), "keep"));
        assert_eq!(rows[2], (LineKind::Del, Some(2), None, "old line"));
        assert_eq!(rows[3], (LineKind::Add, None, Some(2), "new line"));
        assert_eq!(rows[4], (LineKind::Add, None, Some(3), "extra"));
        assert_eq!(rows[5], (LineKind::Context, Some(3), Some(4), "tail"));
        // A removed line that happens to start with dashes is still a removal.
        assert_eq!(files[1].lines[2].text, "-- two");
        assert_eq!(files[1].lines[2].kind, LineKind::Add);
    }
}
