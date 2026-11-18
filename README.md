<div align="center">

<img src="crates/anano-app/assets/logo/anano.png" width="128" alt="Anano">

# Anano

**A native Git client built with Rust + [GPUI](https://gpui.rs)**

Ps: this project was named anano after arjun's ex from 2022 ~ "Anano Adeishvili"

Anano is forked from [GetCat](https://github.com/finch-xu/GetCat) by [finch-xu](https://github.com/finch-xu).

</div>

## Install on macOS

Anano's macOS builds are ad-hoc signed and not notarized, so Gatekeeper shows
*"Apple could not verify “Anano-macos-arm64.dmg” is free of malware…"* the first time.
Clear the quarantine flag once:

```sh
xattr -cr ~/Downloads/Anano-macos-arm64.dmg   # or Anano-macos-x64.dmg on Intel
```

Open the dmg, drag Anano to Applications, then clear the flag on the installed app too:

```sh
xattr -cr /Applications/Anano.app
```

Alternatively, right-click the app → **Open**, or go to System Settings → Privacy & Security
→ **Open Anyway**.

This project is for arjun's personal usecase considering as of 2026, not everyone prefers using this.
