//! core 类型的界面文案：core 的 `Display` 是英文技术文案，界面上按变体翻译。

use anano_core::model::{ThemePref, UpdateSourcePref};
use gpui_kit::SharedString;

use crate::i18n::tr;

pub fn theme_label(pref: ThemePref) -> SharedString {
    match pref {
        ThemePref::System => tr!("theme.system"),
        ThemePref::Light => tr!("theme.light"),
        ThemePref::Dark => tr!("theme.dark"),
    }
}

pub fn update_source_label(pref: UpdateSourcePref) -> SharedString {
    match pref {
        UpdateSourcePref::Auto => tr!("update_source.auto"),
        UpdateSourcePref::Global => tr!("update_source.global"),
        UpdateSourcePref::ChinaMirror => tr!("update_source.china_mirror"),
    }
}
