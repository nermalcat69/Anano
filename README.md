<div align="right">
<b>English</b> · <a href="README.ja.md">日本語</a>
</div>

<div align="center">

<img src="crates/anano-app/assets/logo/anano.png" width="128" alt="Anano">

# Anano

**A native, cross-platform Git client built with Rust + [GPUI](https://gpui.rs)**

No GitKraken, No Sourcetree, Just Anano!

GPU rendering · Low resource usage · No account required · All data stays local · No Electron, No Tauri, No WebView

[![License](https://img.shields.io/badge/License-Apache%202.0-007EC6?style=flat-square)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-1.97%2B-CE422B?style=flat-square&logo=rust&logoColor=white)](https://www.rust-lang.org)
[![GPUI](https://img.shields.io/badge/UI-GPUI-8B5CF6?style=flat-square)](https://gpui.rs)
[![macOS](https://img.shields.io/badge/macOS-000000?style=flat-square&logo=apple&logoColor=white)](https://github.com/finch-xu/Anano/releases)
[![Linux](https://img.shields.io/badge/Linux-FCC624?style=flat-square&logo=linux&logoColor=black)](https://github.com/finch-xu/Anano/releases)
[![Windows](https://img.shields.io/badge/Windows-0078D6?style=flat-square&logo=windows&logoColor=white)](https://github.com/finch-xu/Anano/releases)

[DeepWiki docs](https://deepwiki.com/finch-xu/Anano) · [Official site](https://anano.io/)

<img src="assets/screenshot.png" width="900" alt="Anano main window: repositories and branches on the left, commit history and diff on the right">

</div>

## Highlights

- **Native and fast**: a GPU-rendered native window, not Electron / Tauri / WebView; the same UI on macOS, Linux, and Windows.
- **Core workflow, done right**: open a local repository, view working-tree status, stage / unstage changes (by hunk or by line), write a commit message and commit, with a syntax-highlighted diff view that makes changes easy to read.
- **Branches and history**: a visual commit graph, create / switch / delete branches, merge and rebase (with conflicted files highlighted), tag and stash management, and line-by-line blame.
- **Remotes**: fetch / pull / push, with SSH agent and HTTPS credential authentication, and live progress.
- **Git LFS support**: LFS-enabled repositories are detected automatically. Large files tracked by LFS show up in the diff view as an "LFS object" placeholder (with size) instead of a binary diff, and large file content is synced automatically alongside fetch / push — no extra steps required.
- **Your data stays yours**: no extra copies of history, nothing uploaded — Anano operates directly on your local `.git`. The app's own settings and list of opened repositories are stored as pretty-printed JSON files you can edit by hand.
- **Theme and language follow the system**, or can be pinned to Light / Dark and English / 中文 / 日本語; a custom-drawn titlebar keeps the same look on all three platforms.
- **Accessible**: every control has an accessible name and works with screen readers.

## Installation

Download the package for your platform from [GitHub Releases](https://github.com/finch-xu/Anano/releases).

<table>
  <thead>
    <tr><th>Platform</th><th>File</th><th>Download</th><th>Notes</th></tr>
  </thead>
  <tbody>
    <tr><td>macOS (Apple Silicon)</td><td><code>Anano-macos-arm64.dmg</code></td><td><a href="https://github.com/finch-xu/Anano/releases/latest/download/Anano-macos-arm64.dmg">Global</a> · <a href="https://d.mirror.catonthe.top/Anano/Anano-macos-arm64.dmg">China mirror</a></td><td rowspan="2">Signed and notarized; drag into Applications</td></tr>
    <tr><td>macOS (Intel)</td><td><code>Anano-macos-x64.dmg</code></td><td><a href="https://github.com/finch-xu/Anano/releases/latest/download/Anano-macos-x64.dmg">Global</a> · <a href="https://d.mirror.catonthe.top/Anano/Anano-macos-x64.dmg">China mirror</a></td></tr>
    <tr><td>Linux (x64)</td><td><code>Anano-linux-x64.tar.gz</code></td><td><a href="https://github.com/finch-xu/Anano/releases/latest/download/Anano-linux-x64.tar.gz">Global</a> · <a href="https://d.mirror.catonthe.top/Anano/Anano-linux-x64.tar.gz">China mirror</a></td><td rowspan="2">Extracts to <code>anano</code>; see system requirements below</td></tr>
    <tr><td>Linux (arm64)</td><td><code>Anano-linux-arm64.tar.gz</code></td><td><a href="https://github.com/finch-xu/Anano/releases/latest/download/Anano-linux-arm64.tar.gz">Global</a> · <a href="https://d.mirror.catonthe.top/Anano/Anano-linux-arm64.tar.gz">China mirror</a></td></tr>
    <tr><td>Windows (portable, x64) <strong>recommended</strong></td><td><code>Anano-windows-x64.exe</code></td><td><a href="https://github.com/finch-xu/Anano/releases/latest/download/Anano-windows-x64.exe">Global</a> · <a href="https://d.mirror.catonthe.top/Anano/Anano-windows-x64.exe">China mirror</a></td><td rowspan="2">Single file, runs from anywhere; see system requirements below</td></tr>
    <tr><td>Windows (portable, arm64) <strong>recommended</strong></td><td><code>Anano-windows-arm64.exe</code></td><td><a href="https://github.com/finch-xu/Anano/releases/latest/download/Anano-windows-arm64.exe">Global</a> · <a href="https://d.mirror.catonthe.top/Anano/Anano-windows-arm64.exe">China mirror</a></td></tr>
    <tr><td>Windows (installer, x64)</td><td><code>Anano-windows-x64.msi</code></td><td><a href="https://github.com/finch-xu/Anano/releases/latest/download/Anano-windows-x64.msi">Global</a> · <a href="https://d.mirror.catonthe.top/Anano/Anano-windows-x64.msi">China mirror</a></td><td rowspan="2">Installs per-user, no admin rights needed; launchable from the Start menu</td></tr>
    <tr><td>Windows (installer, arm64)</td><td><code>Anano-windows-arm64.msi</code></td><td><a href="https://github.com/finch-xu/Anano/releases/latest/download/Anano-windows-arm64.msi">Global</a> · <a href="https://d.mirror.catonthe.top/Anano/Anano-windows-arm64.msi">China mirror</a></td></tr>
  </tbody>
</table>

<details>
<summary>Supported Linux versions</summary>

Mainstream desktop distributions from 2022 onward are supported: **Ubuntu 22.04+**, **Debian 12+**, **Fedora 36+**, **Linux Mint 21+**, **openSUSE Leap 15.6+**, as well as rolling releases like Arch and openSUSE Tumbleweed. Graphics drivers on these systems work out of the box; nothing extra to install.

Older distributions won't work: Ubuntu 20.04, Debian 11, and RHEL / Rocky / AlmaLinux 9 — the floor is glibc 2.35, and they're all below it.

After extracting, just run `./anano`. To make it show up in your app list (Ubuntu's "Show Applications") and Dock, open **Settings → General → Add to application menu**: Anano writes a launcher entry and icon under `~/.local/share`, after which you can launch it by searching "Anano" (press Super), and right-click to "Add to Favorites" to pin it to the Dock; turning the switch off removes it. On Wayland, the window and Dock icon also come from this launcher entry, so the taskbar shows a generic icon until the switch is turned on.

The launcher entry points at the current executable, so put `anano` in a stable location before turning the switch on, e.g.:

```bash
tar -xzf Anano-linux-x64.tar.gz
install -Dm755 anano ~/.local/bin/anano
~/.local/bin/anano
```

If you move the file afterward, toggle the switch off and back on to refresh the path.

</details>

<details>
<summary>Black screen on Linux launch, or a Vulkan / "no GPU found" error</summary>

The UI is rendered on the GPU via Vulkan. Desktop distributions usually ship drivers already; check first:

```bash
vulkaninfo --summary
```

If there's no output, or it reports no device found, install a driver for your GPU:

| Environment | Command |
|---|---|
| Ubuntu / Debian + Intel or AMD GPU | `sudo apt install mesa-vulkan-drivers` |
| Fedora + Intel or AMD GPU | `sudo dnf install mesa-vulkan-drivers` |
| Arch + Intel or AMD GPU | `sudo pacman -S vulkan-intel` or `vulkan-radeon` |
| NVIDIA GPU | Install the vendor's proprietary driver (e.g. `nvidia-driver-550`); the open-source nouveau driver doesn't provide Vulkan |
| VM / no dedicated GPU | Install `mesa-vulkan-drivers`; this falls back to lavapipe software rendering — usable but slow |

</details>

<details>
<summary>Supported Windows versions</summary>

Requires **Windows 10 1803 (April 2018 Update) or later**, or Windows 11. The UI renders via Direct3D 11, so a GPU from around 2010 is enough (feature level 10.1+); DirectX 12 is not required.

Both packages work; the portable build is recommended (on ARM devices, such as Snapdragon laptops, pick the `-arm64` build):

- **`Anano-windows-<arch>.exe` (portable, recommended)**: a single file — run it from a USB drive or any directory, no registry writes.
- **`Anano-windows-<arch>.msi` (installer)**: installs to `%LOCALAPPDATA%\Programs\Anano`, no admin rights needed, appears in the Start menu, and can be removed from "Apps & features".

In-app auto-update supports both: MSI installs pull a new MSI and upgrade silently, while portable installs simply replace the exe.

Neither is code-signed yet, so SmartScreen will flag the first run: for the portable exe, click "More info" → "Run anyway"; for the MSI installer, SmartScreen is more insistent but the same "More info" path lets it through.

</details>

## Usage

1. **Open a repository**: use "Open Repository" in the sidebar to pick a local Git directory, or open one from the recent list.
2. **Working-tree status**: the middle panel lists unstaged / staged changes; click a file to see its diff, and stage / unstage by hunk or by line.
3. **Commit**: write a commit message and press **⌘ Enter** (Ctrl Enter on Windows / Linux) to commit.
4. **History and branches**: switch branches and view the commit graph from the sidebar; right-click a commit for checkout / cherry-pick / revert.
5. **Sync**: fetch / pull / push from the toolbar. If a repository has Git LFS enabled, large files are pulled / pushed automatically as part of the sync.

| Action | macOS | Windows / Linux |
|---|---|---|
| Commit | ⌘ Enter | Ctrl Enter |
| Toggle sidebar | ⌘ B | Ctrl B |
| Refresh status | ⌘ R | Ctrl R |
| Search within diff | ⌘ F | Ctrl F |
| Settings | ⌘ , | Ctrl , |

Settings let you change the UI language (follow system / English / 中文 / 日本語), the diff editor's font size, and whether to check for updates on startup.

### Data directory

Anano doesn't copy or cache your repository data — all Git objects, refs, and LFS caches stay inside each repository's own `.git/`. The app itself only records which repositories you've opened and your UI preferences:

| Platform | Directory |
|---|---|
| macOS | `~/Library/Application Support/Anano/` |
| Linux | `$XDG_DATA_HOME/anano/` (defaults to `~/.local/share/anano/`) |
| Windows | `%APPDATA%\Anano\data\` |

```
repos.json              # list of opened repositories (path, display name, last-opened time)
settings.json           # app settings
```

Writes are atomic (temp file → rename), so a crash never leaves a half-written file; files that fail to parse are renamed to `.corrupt-<timestamp>` and skipped.

### Git LFS

If an opened repository has a `.gitattributes` declaring `filter=lfs`, Anano automatically detects it as an LFS repository: large files behind LFS pointers show up in the diff view as a placeholder (file type + size) instead of being diffed as text, and fetch / pull / push also invoke the system-installed `git-lfs` to sync the actual large-file content. You'll need [Git LFS](https://git-lfs.com/) installed locally and `git lfs install` run once, same as with any other Git client.

v## Development

### Architecture

```
crates/
├─ anano-core   # UI-free core: git2 wrapper (status / diff / commit / branch / remote), LFS detection & sync, JSON file storage
└─ anano-app    # GPUI interface: repository list, status & diff panels, commit history, settings dialog, in-app updates
```

- The UI framework is Zed's [gpui](https://github.com/zed-industries/zed/tree/main/crates/gpui) plus [GPUI Kit](https://github.com/longbridge/gpui-kit) (the gpui-component library). Following Kit 0.6's official shape, the app depends only on the single crates.io package `gpui-kit`, which pins the matching gpui version.
- Git operations are built on [libgit2](https://libgit2.org/) (the `git2` crate) and run on tokio's blocking thread pool, with results sent back to the GPUI main thread over a channel. libgit2 itself doesn't know about LFS smudge/clean filters, so syncing LFS large-file content shells out to the system-installed `git-lfs` executable instead.
- Persistence has no database: `anano-core/src/store` only records the repository list and app settings; writes go through a dedicated thread with coalescing.

### Build & debug

- Rust ≥ 1.97 (edition 2024). macOS needs no extra toolchain; Linux needs Vulkan plus Wayland / X11 / fontconfig headers (see `.github/workflows/ci.yml` for the full list); Windows needs the MSVC toolchain — Direct3D 11 is already included in the Windows SDK.
- App logo: `crates/anano-app/assets/logo/cat.png` is the background-removed original art; `scripts/gen-logo.py` composites it into three artifacts — the app-embedded `anano.png`, the macOS icon source `resources/macos/anano-1024.png`, and the Windows exe icon `resources/windows/anano.ico`. After changing the logo, re-run the script manually and commit the artifacts (CI doesn't generate them; requires `pip install pillow numpy`).
- The Windows exe icon and version info are embedded by `crates/anano-app/build.rs`, which only takes effect when compiling natively on Windows (an exe cross-compiled from macOS has no icon). The installer is defined in `crates/anano-app/resources/windows/Anano.wxs` and needs WiX v6: `dotnet tool install --global wix --version 6.*`.

```bash
cargo run -p anano-app                         # run
cargo test --workspace                          # unit + gpui TestAppContext tests
RUST_LOG=debug cargo run -p anano-app          # adjust log level
cargo run -p anano-app --features inspector    # element inspector: ⌘⌥I / Ctrl+Shift+I to view id / role
ANANO_UPDATE_CHECK=1 cargo run -p anano-app   # check for updates on startup even in a dev build (check only, no install)
```

Manually verifying LFS behavior requires [git-lfs](https://git-lfs.com/) installed locally, and a test repository with `git lfs track` set up.

Before committing: `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`. CI builds and tests on all three platforms, and uses cargo-deny to block copyleft dependencies.

## License

[Apache-2.0](LICENSE). See [THIRD-PARTY.md](THIRD-PARTY.md) for the third-party dependency list.


Ps: this project was named anano after arjun's ex from 2022 ~ "Anano Adeishvili"
