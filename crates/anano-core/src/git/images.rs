//! Image diffs: for a changed image, the before and after as files the UI can show.
//!
//! Git stores the old version as a blob, not a file, so each side is written once into the
//! cache under its blob id (same content, same file: nothing is written twice, and the UI's
//! image cache can't show a stale copy of a file that changed under the same name).

use std::path::{Path, PathBuf};

use git2::{Delta, Oid, Repository};

use crate::git::error::GitError;

/// One image file's two sides. `old` is `None` for an added image, `new` for a deleted one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImagePair {
    pub path: String,
    pub old: Option<PathBuf>,
    pub new: Option<PathBuf>,
}

/// Whether the file name is an image the UI can draw.
pub fn is_image(path: &str) -> bool {
    path.rsplit_once('.').is_some_and(|(_, ext)| {
        matches!(
            ext.to_ascii_lowercase().as_str(),
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" | "ico"
        )
    })
}

fn open(path: &Path) -> Result<Repository, GitError> {
    Repository::discover(path).map_err(GitError::Open)
}

fn store_dir() -> Option<PathBuf> {
    Some(crate::external::cache_dir()?.join("images"))
}

/// Write blob `id` to the cache (once) with `path`'s extension; `None` for the null id.
fn blob_file(repo: &Repository, id: Oid, path: &str) -> Option<PathBuf> {
    if id.is_zero() {
        return None;
    }
    let ext = path.rsplit_once('.').map_or("png", |(_, e)| e);
    let file = store_dir()?.join(format!("{id}.{}", ext.to_ascii_lowercase()));
    if !file.is_file() {
        let blob = repo.find_blob(id).ok()?;
        std::fs::create_dir_all(file.parent()?).ok()?;
        std::fs::write(&file, blob.content()).ok()?;
    }
    Some(file)
}

/// A file on disk, copied into the cache under its content id.
fn disk_file(repo: &Repository, path: &Path, name: &str) -> Option<PathBuf> {
    let id = Oid::hash_file(git2::ObjectType::Blob, path).ok()?;
    let file = store_dir()?.join(format!(
        "{id}.{}",
        name.rsplit_once('.')
            .map_or("png", |(_, e)| e)
            .to_ascii_lowercase()
    ));
    let _ = repo;
    if !file.is_file() {
        std::fs::create_dir_all(file.parent()?).ok()?;
        std::fs::copy(path, &file).ok()?;
    }
    Some(file)
}

/// A changed file of the changes list: HEAD against what is on disk.
pub fn image_pair_worktree(repo_path: &Path, file: &str) -> Result<Option<ImagePair>, GitError> {
    if !is_image(file) {
        return Ok(None);
    }
    let repo = open(repo_path)?;
    let old = repo
        .head()
        .ok()
        .and_then(|h| h.peel_to_tree().ok())
        .and_then(|t| t.get_path(Path::new(file)).ok())
        .and_then(|entry| blob_file(&repo, entry.id(), file));
    let on_disk = repo.workdir().map(|w| w.join(file));
    let new = on_disk
        .filter(|p| p.is_file())
        .and_then(|p| disk_file(&repo, &p, file));
    Ok((old.is_some() || new.is_some()).then(|| ImagePair {
        path: file.to_string(),
        old,
        new,
    }))
}

/// Every image a commit changed, against its first parent.
pub fn image_pairs_commit(repo_path: &Path, id: &str) -> Result<Vec<ImagePair>, GitError> {
    let repo = open(repo_path)?;
    let commit = Oid::from_str(id)
        .and_then(|oid| repo.find_commit(oid))
        .map_err(GitError::Diff)?;
    let tree = commit.tree().map_err(GitError::Diff)?;
    let parent = commit.parent(0).ok().and_then(|p| p.tree().ok());
    let diff = repo
        .diff_tree_to_tree(parent.as_ref(), Some(&tree), None)
        .map_err(GitError::Diff)?;
    let mut pairs = Vec::new();
    for delta in diff.deltas() {
        let Some(path) = delta.new_file().path().and_then(|p| p.to_str()) else {
            continue;
        };
        if !is_image(path) || delta.status() == Delta::Unmodified {
            continue;
        }
        let old_path = delta
            .old_file()
            .path()
            .and_then(|p| p.to_str())
            .unwrap_or(path);
        pairs.push(ImagePair {
            path: path.to_string(),
            old: blob_file(&repo, delta.old_file().id(), old_path),
            new: blob_file(&repo, delta.new_file().id(), path),
        });
    }
    Ok(pairs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn only_image_extensions_count() {
        for yes in ["a.png", "dir/B.JPG", "x.svg", "logo.webp"] {
            assert!(is_image(yes), "{yes}");
        }
        for no in ["a.rs", "png", "a.png.txt", "Makefile"] {
            assert!(!is_image(no), "{no}");
        }
    }

    /// A committed image that is then replaced: the pair has both sides, with different
    /// files; a commit that adds one has only the new side.
    #[test]
    fn pairs_for_a_changed_and_an_added_image() {
        let dir = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            let ok = Command::new("git")
                .args(args)
                .current_dir(dir.path())
                .output()
                .unwrap();
            assert!(ok.status.success(), "{args:?}");
        };
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "user.email", "t@t"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(dir.path().join("a.png"), b"one-one-one").unwrap();
        run(&["add", "."]);
        run(&["commit", "-qm", "add"]);
        let head = String::from_utf8(
            Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(dir.path())
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();
        let added = image_pairs_commit(dir.path(), head.trim()).unwrap();
        assert_eq!(added.len(), 1);
        assert!(added[0].old.is_none() && added[0].new.is_some());

        std::fs::write(dir.path().join("a.png"), b"two-two-two").unwrap();
        let pair = image_pair_worktree(dir.path(), "a.png").unwrap().unwrap();
        let (old, new) = (pair.old.unwrap(), pair.new.unwrap());
        assert_ne!(old, new);
        assert_eq!(std::fs::read(old).unwrap(), b"one-one-one");
        assert_eq!(std::fs::read(new).unwrap(), b"two-two-two");
        assert!(image_pair_worktree(dir.path(), "a.rs").unwrap().is_none());
    }
}
