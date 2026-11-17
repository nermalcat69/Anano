<div align="center">

<img src="crates/anano-app/assets/logo/anano.png" width="128" alt="Anano">

# Anano

**A native Git client built with Rust + [GPUI](https://gpui.rs)**

No GitKraken, No Sourcetree, Just Anano!

GPU rendering · Low resource usage · All data stays local · No Electron, No Tauri, No WebView

<img src="assets/screenshot.png" width="900" alt="Anano main window">

</div>

## Features

- **Repositories**: clone, create (README, `.gitignore`, license) or add a local one. Closing a tab keeps the repository in the list, grouped by GitHub owner.
- **Changes**: staged and unstaged files with a line-numbered diff; stage, unstage, discard, ignore.
- **Commit & push**: one button commits the ticked files and pushes them.
- **History**: commits with author, profile picture and age; changed files and diff per commit; create a branch from a commit, copy its SHA, open it on GitHub.
- **Branches**: switch, create and delete. Switching with uncommitted work asks whether to leave it on the old branch (stashed, restored when you return) or bring it along.
- **Sync**: one toolbar button that fetches, pulls, pushes or publishes depending on the branch state. Repositories are fetched automatically when you switch to them. Git LFS is synced alongside.
- **GitHub accounts**: connect several at once and choose one per repository. Clone from your repository list, or create a repository directly on GitHub.
- **Pull requests**: list, read the description and history, and merge when GitHub allows it.
- **Open in Nermal**: open the repository's folder in the Nermal IDE.
- Light / dark theme.

## Requirements

- [GitHub CLI](https://cli.github.com) (`gh`) for GitHub accounts, repository lists and pull requests. Sign-in and tokens are handled by `gh`; Anano stores no credentials.
- [git-lfs](https://git-lfs.com) only for repositories that use LFS.
- The Nermal IDE only for the "Open in Nermal" button.

## Run

Rust 1.97 or newer.

```bash
cargo run -p anano-app      # run
cargo test --workspace      # tests
```

| Action | macOS | Windows / Linux |
|---|---|---|
| Open repository | ⌘ O | Ctrl O |
| Close tab | ⌘ W | Ctrl W |
| Toggle sidebar | ⌘ B | Ctrl B |
| Settings | ⌘ , | Ctrl , |

Anano works directly on each repository's own `.git`. It only saves the list of repositories and your settings, as JSON you can edit by hand:

| Platform | Directory |
|---|---|
| macOS | `~/Library/Application Support/Anano/` |
| Linux | `~/.local/share/anano/` |
| Windows | `%APPDATA%\Anano\data\` |

## License

[MIT](LICENSE). Code inherited from GetCat stays under its original Apache-2.0 license ([LICENSE-APACHE](LICENSE-APACHE)). Third-party dependencies are listed in [THIRD-PARTY.md](THIRD-PARTY.md).

Anano is forked from [GetCat](https://github.com/finch-xu/GetCat) by [finch-xu](https://github.com/finch-xu).
