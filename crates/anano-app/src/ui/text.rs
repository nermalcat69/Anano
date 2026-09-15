//! core 类型的界面文案：core 的 `Display` 是英文技术文案，界面上按变体翻译。

use anano_core::model::{LanguagePref, ThemePref, UpdateSourcePref};
use gpui_kit::SharedString;

use crate::i18n::tr;

pub fn theme_label(pref: ThemePref) -> SharedString {
    match pref {
        ThemePref::System => tr!("theme.system"),
        ThemePref::Light => tr!("theme.light"),
        ThemePref::Dark => tr!("theme.dark"),
    }
}

pub fn language_label(pref: LanguagePref) -> SharedString {
    match pref {
        LanguagePref::System => tr!("language.system"),
        LanguagePref::English => tr!("language.english"),
        LanguagePref::Chinese => tr!("language.chinese"),
        LanguagePref::Japanese => tr!("language.japanese"),
    }
}

pub fn update_source_label(pref: UpdateSourcePref) -> SharedString {
    match pref {
        UpdateSourcePref::Auto => tr!("update_source.auto"),
        UpdateSourcePref::Global => tr!("update_source.global"),
        UpdateSourcePref::ChinaMirror => tr!("update_source.china_mirror"),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn chinese_locale_is_wired_up() {
        assert_eq!(
            rust_i18n::t!("theme.system", locale = "zh-CN").as_ref(),
            "跟随系统"
        );
        assert_eq!(
            rust_i18n::t!("theme.system", locale = "en").as_ref(),
            "System"
        );
    }

    #[test]
    fn japanese_locale_is_wired_up() {
        assert_eq!(
            rust_i18n::t!("theme.system", locale = "ja").as_ref(),
            "システムに従う"
        );
        for locale in ["en", "zh-CN", "ja"] {
            assert_eq!(
                rust_i18n::t!("language.japanese", locale = locale).as_ref(),
                "日本語",
                "{locale}"
            );
        }
    }
}
