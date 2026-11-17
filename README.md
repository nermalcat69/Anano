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

## Install

Download the build for your OS and CPU from [Releases](https://github.com/nermalcat69/Anano/releases):

| OS | File |
|---|---|
| macOS | `Anano-macos-arm64.dmg` / `Anano-macos-x64.dmg` |
| Windows | `Anano-windows-<arch>.msi` (installer) or `.exe` (portable) |
| Linux | `Anano-linux-<arch>.tar.gz` |

Anano checks GitHub Releases for updates a few seconds after launch and installs them in place. Every download is verified against `SHA256SUMS` and a [minisign](https://jedisct1.github.io/minisign/) signature (public key: [`minisign.pub`](minisign.pub)).

## Run from source

Rust 1.97 or newer. On Linux install the Wayland/X11, xkbcommon, fontconfig and Vulkan headers (see the list in [`ci.yml`](.github/workflows/ci.yml)).

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

## Releasing

CI (`ci.yml`) builds and tests on macOS, Windows and Linux for every PR. Releases are tag-driven:

```bash
scripts/release.sh 0.9.0          # bumps Cargo.toml + Cargo.lock, runs the checks
git commit -am "Release 0.9.0" && git tag v0.9.0 && git push --follow-tags
```

`release.yml` then builds all six targets, creates `SHA256SUMS` and `.minisig` files, and publishes the release. Tags containing `-` (e.g. `v0.9.0-rc.1`) become prereleases that the updater ignores.

One-time setup in **Settings → Environments → `prod`**: `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID` (macOS signing and notarization) and `MINISIGN_SECRET_KEY`. The checked-in `minisign.pub` must match that key: generate your own with `minisign -G -W` and replace `minisign.pub` before the first release. The optional Aliyun mirror job runs only if the repository variable `OSS_MIRROR` is `true`.

## License

[MIT](LICENSE). Code inherited from GetCat stays under its original Apache-2.0 license ([LICENSE-APACHE](LICENSE-APACHE)). Third-party dependencies are listed in [THIRD-PARTY.md](THIRD-PARTY.md).

Anano is forked from [GetCat](https://github.com/finch-xu/GetCat) by [finch-xu](https://github.com/finch-xu).
