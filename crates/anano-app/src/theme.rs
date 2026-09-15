//! Anano 配色：把 BucketCat 的主题变量装进 gpui-component 的 Theme。
//!
//! 色值取自 BucketCat 的 `src/index.css`（`:root` 与 `.dark` 两组变量），
//! 只有 `muted.foreground` 是相对它的调整：原值 #8794a1 在白底上只有
//! 2.99:1，而它承载了侧栏副标题、参数描述、耗时与大小等大量次要文字。
//!
//! 主题不是每次切换后打补丁，而是替换 [`Theme`] 的 `light_theme` /
//! `dark_theme` 两个配置：此后 `Theme::change` 与
//! `Theme::sync_system_appearance` 自动用这两套，跟随系统外观也不会退回
//! gpui-component 的默认灰。

use gpui_kit::App;
use gpui_kit::component::{Theme, ThemeRegistry, scroll::ScrollbarMode};

const THEME_JSON: &str = include_str!("theme.json");
const LIGHT: &str = "Anano Light";
const DARK: &str = "Anano Dark";

/// 把 Anano 的浅色 / 深色配色装成当前主题。
///
/// 必须在 `gpui_kit::component::init` 之后调用。任何一步失败都只记日志：
/// 配色装不上时界面退回 gpui-component 的默认主题，功能不受影响。
pub fn install(cx: &mut App) {
    if let Err(err) = ThemeRegistry::global_mut(cx).load_themes_from_str(THEME_JSON) {
        tracing::error!("加载 Anano 主题失败，沿用默认配色：{err}");
        return;
    }

    let registry = ThemeRegistry::global(cx);
    let (Some(light), Some(dark)) = (
        registry.themes().get(LIGHT).cloned(),
        registry.themes().get(DARK).cloned(),
    ) else {
        tracing::error!("Anano 主题未出现在注册表里，沿用默认配色");
        return;
    };

    let theme = Theme::global_mut(cx);
    theme.light_theme = light;
    theme.dark_theme = dark;
    let mode = theme.mode;
    // 重新套用当前模式，让刚换上的配置立刻生效（并同步 Base 层）。
    Theme::change(mode, None, cx);

    // 滚动条常驻。上游默认是 `Scrolling`——只在滚动时画、静止后淡出，于是「这块内容
    // 还能往右拉」这件事根本没有可见线索：响应体里一行超宽 JSON 看上去就只是被切掉了。
    // 接口调试要盯的恰恰是长行，所以这里换成 `Always`。
    //
    // 必须放在 `Theme::change` **之后**：`change` 会用 `scrollbar_mode` 重建 Base 层投影，
    // 反过来写就被它按旧值覆盖了。此后 `change` / `sync_system_appearance` 都从
    // `Theme::scrollbar_mode` 读，切明暗不会退回默认（有测试钉住）。
    Theme::set_scrollbar_mode(ScrollbarMode::Always, cx);
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::component::{ActiveTheme, ThemeMode};
    use gpui_kit::{Hsla, TestAppContext};

    fn hex(color: Hsla) -> u32 {
        let rgba = gpui_kit::Rgba::from(color);
        let to8 = |v: f32| (v * 255.0).round() as u32;
        (to8(rgba.r) << 16) | (to8(rgba.g) << 8) | to8(rgba.b)
    }

    /// 滚动条常驻，且切换明暗后不退回上游默认的「滚动时才画」。
    #[gpui_kit::test]
    fn install_pins_the_scrollbar_to_always_visible(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            install(cx);
            assert_eq!(Theme::global(cx).scrollbar_mode, ScrollbarMode::Always);

            for mode in [ThemeMode::Light, ThemeMode::Dark, ThemeMode::Light] {
                Theme::change(mode, None, cx);
                assert_eq!(
                    Theme::global(cx).scrollbar_mode,
                    ScrollbarMode::Always,
                    "切到 {mode:?} 后滚动条退回了默认模式"
                );
            }
        });
    }

    /// 配色取自 BucketCat 的 src/index.css；muted_foreground 是唯一一处
    /// 相对它的调整（原值 #8794a1 在白底上只有 2.99:1）。
    #[gpui_kit::test]
    fn install_replaces_the_default_palette(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            install(cx);

            Theme::change(ThemeMode::Light, None, cx);
            assert_eq!(hex(cx.theme().background), 0xffffff);
            assert_eq!(hex(cx.theme().foreground), 0x1c2329);
            assert_eq!(hex(cx.theme().primary), 0x3f87bd);
            assert_eq!(hex(cx.theme().sidebar), 0xf4f6f8);
            assert_eq!(hex(cx.theme().title_bar), 0xf4f6f8);
            assert_eq!(hex(cx.theme().border), 0xe3e8ed);
            assert_eq!(hex(cx.theme().muted_foreground), 0x5e6a77);

            Theme::change(ThemeMode::Dark, None, cx);
            assert_eq!(hex(cx.theme().background), 0x16191c);
            assert_eq!(hex(cx.theme().foreground), 0xeef1f4);
            assert_eq!(hex(cx.theme().primary), 0x6fa8d4);
            assert_eq!(hex(cx.theme().sidebar), 0x14171a);
            assert_eq!(hex(cx.theme().title_bar), 0x14171a);
            assert_eq!(hex(cx.theme().border), 0x2a3037);
            assert_eq!(hex(cx.theme().muted_foreground), 0x8f9aa3);
        });
    }
}
