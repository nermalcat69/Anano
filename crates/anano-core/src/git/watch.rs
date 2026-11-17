//! Tell the UI when a repository changed on disk: a commit, fetch, push or checkout done in
//! a terminal or another tool (they all touch `.git`), or a file edited in the working tree.
//!
//! Events are coalesced: a burst (a checkout rewrites hundreds of files) becomes one call,
//! 300 ms after it quietens. Noise is dropped: the object store, lock files, and working
//! tree files git ignores (build output, `node_modules`), so a build can't make the app
//! refresh in a loop.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use notify::{RecursiveMode, Watcher};

/// Keeps watching while it lives; dropping it stops the watch.
pub struct RepoWatcher {
    _watcher: notify::RecommendedWatcher,
}

/// Whether a changed path inside `.git` says something about the repository's state.
fn git_dir_path_matters(rel: &Path) -> bool {
    let first = rel.components().next().and_then(|c| c.as_os_str().to_str());
    let name = rel.file_name().and_then(|n| n.to_str()).unwrap_or("");
    // Packed objects, LFS caches and lock files change constantly and mean nothing alone;
    // the ref / HEAD / index write that follows is what counts.
    !matches!(first, Some("objects" | "lfs" | "hooks")) && !name.ends_with(".lock")
}

/// Call `on_change` (from a background thread) whenever `repo` changes on disk.
pub fn watch(repo: &Path, on_change: impl Fn() + Send + 'static) -> Result<RepoWatcher, String> {
    let root: PathBuf = repo.to_path_buf();
    let git_dir = root.join(".git");
    let (tx, rx) = mpsc::channel::<PathBuf>();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if let Ok(event) = event {
            // Reads (access) are not changes.
            if !matches!(event.kind, notify::EventKind::Access(_)) {
                for path in event.paths {
                    let _ = tx.send(path);
                }
            }
        }
    })
    .map_err(|e| e.to_string())?;
    watcher
        .watch(&root, RecursiveMode::Recursive)
        .map_err(|e| e.to_string())?;

    std::thread::spawn(move || {
        let ignored = git2::Repository::open(&root).ok();
        let relevant_path = |path: &Path| -> bool {
            if let Ok(rel) = path.strip_prefix(&git_dir) {
                return git_dir_path_matters(rel);
            }
            let Ok(rel) = path.strip_prefix(&root) else {
                return false;
            };
            // A working tree file: only if git would track or show it.
            !ignored
                .as_ref()
                .is_some_and(|r| r.is_path_ignored(rel).unwrap_or(false))
        };
        let relevant = |path: &Path| -> bool {
            let matters = relevant_path(path);
            if matters {
                tracing::debug!(target: "anano::watch", "change: {}", path.display());
            }
            matters
        };
        // Ends when the watcher is dropped (its sender goes with it).
        while let Ok(first) = rx.recv() {
            let mut matters = relevant(&first);
            // Coalesce the burst: wait until nothing arrives for 300 ms.
            while let Ok(path) = rx.recv_timeout(Duration::from_millis(300)) {
                matters |= relevant(&path);
            }
            if matters {
                on_change();
            }
        }
    });
    Ok(RepoWatcher { _watcher: watcher })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn git_internals_that_do_not_mean_anything_are_skipped() {
        for noise in [
            "objects/ab/cdef",
            "index.lock",
            "lfs/tmp/x",
            "refs/heads/x.lock",
        ] {
            assert!(!git_dir_path_matters(Path::new(noise)), "{noise}");
        }
        for signal in [
            "HEAD",
            "index",
            "FETCH_HEAD",
            "refs/remotes/origin/main",
            "logs/HEAD",
        ] {
            assert!(git_dir_path_matters(Path::new(signal)), "{signal}");
        }
    }

    /// A commit and an edit are noticed; ignored files, and the app's own reads of the
    /// repository, are not (otherwise refreshing would trigger refreshing).
    #[test]
    fn changes_are_noticed_and_reads_and_ignored_files_are_not() {
        let dir = tempfile::tempdir().unwrap();
        // The temp dir is under /var → /private/var on macOS; watch the real path.
        let root = dir.path().canonicalize().unwrap();
        let git = |args: &[&str]| {
            let ok = Command::new("git")
                .args(args)
                .current_dir(&root)
                .output()
                .unwrap();
            assert!(ok.status.success(), "{args:?}");
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
        std::fs::write(root.join("a.txt"), "1").unwrap();
        git(&["add", "."]);
        git(&["commit", "-qm", "one"]);
        std::thread::sleep(Duration::from_millis(600));

        let count = Arc::new(AtomicUsize::new(0));
        let seen = count.clone();
        let _watcher = watch(&root, move || {
            seen.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();
        let settle = || std::thread::sleep(Duration::from_millis(1200));
        settle();
        let base = count.load(Ordering::SeqCst);

        // The app reading: status and log must not wake the watcher.
        let _ = crate::git::repo_status(&root);
        let _ = crate::git::recent_commits(&root, 10);
        settle();
        assert_eq!(
            count.load(Ordering::SeqCst),
            base,
            "reads caused a change event"
        );

        // Ignored output (a build) is not a change.
        std::fs::create_dir_all(root.join("target")).unwrap();
        std::fs::write(root.join("target/out.o"), "x").unwrap();
        settle();
        assert_eq!(
            count.load(Ordering::SeqCst),
            base,
            "ignored files caused an event"
        );

        // A commit done elsewhere is.
        std::fs::write(root.join("a.txt"), "2").unwrap();
        git(&["commit", "-qam", "two"]);
        settle();
        assert!(
            count.load(Ordering::SeqCst) > base,
            "a commit went unnoticed"
        );
    }
}
