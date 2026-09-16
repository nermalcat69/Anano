//! 应用资源：Anano 自己的 logo 与补充图标叠在 gpui-component 的图标集之上。
//!
//! gpui 的 `img()` / `svg()` 都经由全局 `AssetSource` 取字节；gpui-component-assets
//! 只嵌入了它自带的那份 `icons/**`，应用私有资源需要自己的来源。这里不引入 rust-embed，
//! 只用 `include_bytes!` 逐个嵌入，其余路径原样交给上游。

use std::borrow::Cow;

use gpui_kit::assets::Assets;
use gpui_kit::component::ActiveTheme;
use gpui_kit::{App, AssetSource, Result, SharedString};

/// App 图标的资源路径（`img(LOGO_PATH)`）：Linux 窗口图标 / .desktop 图标用的位图，
/// 不随主题变化。应用 UI 里显示的 logo改走 [`logo_path`]（多色、明暗各一份）。
///
/// 这份 PNG 由 `scripts/gen-logo.py` 从 `assets/logo/icon-source.png`（设计师导出的
/// App Store 1024×1024 成品图标）缩放而来，改 logo 要重跑脚本。
pub const LOGO_PATH: &str = "logo/anano.png";

/// Logo 的原始 PNG 字节（512 px 见方）。除了作为 [`LOGO_PATH`] 资源，Linux 上还直接用它：
/// 解码后作 X11 的窗口图标，原样写进 hicolor 主题目录给 .desktop 用。
pub const LOGO_PNG: &[u8] = include_bytes!("../assets/logo/anano.png");

/// 应用内展示用的 logo（侧栏、「关于」页）：多色矢量图，明暗各一份。
/// 走 `img()` 而不是 `svg()`——`svg()` 是单色蒙版，会被 `text_color` 整张染色，
/// 这两份 SVG 有自己的多色路径填充，`img()` 内置的 resvg 渲染器按原样栅格化。
pub const LOGO_LIGHT_PATH: &str = "logo/logo.svg";
pub const LOGO_DARK_PATH: &str = "logo/logo-dark.svg";

/// 按当前主题挑一份 logo；深色模式下用白色轮廓的那份，否则用黑色轮廓的那份。
pub fn logo_path(cx: &App) -> &'static str {
    if cx.theme().is_dark() {
        LOGO_DARK_PATH
    } else {
        LOGO_LIGHT_PATH
    }
}

/// 图标栏「打开仓库」按钮的图标。
pub const ICON_FOLDER_GIT: &str = "icons/folder-git.svg";
/// 分支区：当前分支名旁边的图标。
pub const ICON_GIT_BRANCH: &str = "icons/git-branch.svg";
/// 同步工具栏：fetch / pull / push，分别对应下载、云下载、云上传。
pub const ICON_DOWNLOAD: &str = "icons/download.svg";
pub const ICON_DOWNLOAD_CLOUD: &str = "icons/download-cloud.svg";
pub const ICON_UPLOAD_CLOUD: &str = "icons/upload-cloud.svg";
/// 分支下拉菜单里的「删除分支」项。
pub const ICON_TRASH: &str = "icons/trash-2.svg";
/// 侧栏「History」标签页的图标；默认图标集里没有钟表图形。
pub const ICON_HISTORY: &str = "icons/history.svg";

/// 应用自带的资源表：上游 `gpui-component-assets` 只嵌入了一小份「默认」图标
/// （见其 `default-icons.txt`），这里用到的 git/同步相关图形大多不在其中，
/// 补在这里；取自 Feather Icons（MIT），与上游图标集同为 24px 描边风格。
const ASSETS: &[(&str, &[u8])] = &[
    (LOGO_PATH, LOGO_PNG),
    (LOGO_LIGHT_PATH, include_bytes!("../assets/logo/logo.svg")),
    (
        LOGO_DARK_PATH,
        include_bytes!("../assets/logo/logo-dark.svg"),
    ),
    (
        ICON_FOLDER_GIT,
        include_bytes!("../assets/icons/folder-git.svg"),
    ),
    (
        ICON_GIT_BRANCH,
        include_bytes!("../assets/icons/git-branch.svg"),
    ),
    (
        ICON_DOWNLOAD,
        include_bytes!("../assets/icons/download.svg"),
    ),
    (
        ICON_DOWNLOAD_CLOUD,
        include_bytes!("../assets/icons/download-cloud.svg"),
    ),
    (
        ICON_UPLOAD_CLOUD,
        include_bytes!("../assets/icons/upload-cloud.svg"),
    ),
    (ICON_TRASH, include_bytes!("../assets/icons/trash-2.svg")),
    (ICON_HISTORY, include_bytes!("../assets/icons/history.svg")),
];

pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some((_, bytes)) = ASSETS.iter().find(|(name, _)| *name == path) {
            return Ok(Some(Cow::Borrowed(bytes)));
        }
        Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut entries = Assets.list(path)?;
        entries.extend(
            ASSETS
                .iter()
                .filter(|(name, _)| name.starts_with(path))
                .map(|(name, _)| SharedString::from(*name)),
        );
        Ok(entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logo_is_served_from_the_embedded_bytes() {
        let bytes = AppAssets.load(LOGO_PATH).unwrap().expect("logo present");
        assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
    }

    /// `logo_path` 按当前主题模式二选一。
    #[gpui_kit::test]
    fn logo_path_follows_theme_mode(cx: &mut gpui_kit::TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::install(cx);
            gpui_kit::component::Theme::change(gpui_kit::component::ThemeMode::Light, None, cx);
            assert_eq!(logo_path(cx), LOGO_LIGHT_PATH);
            gpui_kit::component::Theme::change(gpui_kit::component::ThemeMode::Dark, None, cx);
            assert_eq!(logo_path(cx), LOGO_DARK_PATH);
        });
    }

    /// 应用内 logo 是多色 SVG（不靠 `currentColor` 染色），且深色 / 浅色各挑对应那份。
    #[test]
    fn in_app_logo_svgs_are_multicolor_and_theme_switched() {
        for path in [LOGO_LIGHT_PATH, LOGO_DARK_PATH] {
            let bytes = AppAssets
                .load(path)
                .unwrap()
                .unwrap_or_else(|| panic!("{path} missing"));
            let text = std::str::from_utf8(&bytes).expect("SVG 是 UTF-8");
            assert!(text.starts_with("<svg"), "{path} 不是 SVG");
            // 多色路径填充，不是给 text_color 染色的单色蒙版
            assert!(text.contains("fill=\"#"), "{path} 应该带具体的多色填充");
        }
        assert_ne!(
            AppAssets.load(LOGO_LIGHT_PATH).unwrap(),
            AppAssets.load(LOGO_DARK_PATH).unwrap(),
            "明暗两份 logo 应该是不同的文件"
        );
    }

    /// 补的图标要真的能取到，而且是 gpui 的 `svg()` 认得的 SVG。
    #[test]
    fn app_icons_are_served_as_svg() {
        for path in [
            ICON_FOLDER_GIT,
            ICON_GIT_BRANCH,
            ICON_DOWNLOAD,
            ICON_DOWNLOAD_CLOUD,
            ICON_UPLOAD_CLOUD,
            ICON_TRASH,
            ICON_HISTORY,
        ] {
            let bytes = AppAssets
                .load(path)
                .unwrap()
                .unwrap_or_else(|| panic!("{path} missing"));
            let text = std::str::from_utf8(&bytes).expect("SVG 是 UTF-8");
            assert!(text.starts_with("<svg"), "{path} 不是 SVG");
            // 图标靠 text_color 染色，写死 stroke 会让它在深色主题里消失
            assert!(
                text.contains(r#"stroke="currentColor""#),
                "{path} 没有用 currentColor，换主题时会瞎"
            );
        }
    }

    #[test]
    fn icon_paths_still_come_from_gpui_component_assets() {
        let bytes = AppAssets
            .load("icons/moon.svg")
            .unwrap()
            .expect("upstream icon present");
        assert!(bytes.starts_with(b"<svg"));
        assert!(AppAssets.load("icons/does-not-exist.svg").is_err());
    }

    #[test]
    fn listing_includes_both_app_assets_and_upstream_icons() {
        let all = AppAssets.list("").unwrap();
        assert!(all.iter().any(|p| p.as_ref() == LOGO_PATH));
        assert!(all.iter().any(|p| p.as_ref() == ICON_FOLDER_GIT));
        assert!(all.iter().any(|p| p.as_ref() == "icons/moon.svg"));
        let logo_entries = AppAssets.list("logo").unwrap();
        for path in [LOGO_PATH, LOGO_LIGHT_PATH, LOGO_DARK_PATH] {
            assert!(
                logo_entries.iter().any(|p| p.as_ref() == path),
                "{path} missing from logo listing: {logo_entries:?}"
            );
        }
        // 前缀过滤要把应用图标和上游图标一起列出来
        let icons = AppAssets.list("icons").unwrap();
        assert!(icons.iter().any(|p| p.as_ref() == ICON_GIT_BRANCH));
        assert!(icons.iter().any(|p| p.as_ref() == "icons/moon.svg"));
    }
}
