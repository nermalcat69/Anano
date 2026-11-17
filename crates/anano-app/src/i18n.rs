//! UI text. English only: strings live in `crates/anano-app/locales/app.yml` (rust-i18n v2
//! format, one `en:` entry per key) and `rust_i18n::i18n!` registers them in `main.rs`.

/// `t!` as a [`gpui_kit::SharedString`]: what most gpui-component APIs accept.
macro_rules! tr {
    ($($args:tt)*) => {
        ::gpui_kit::SharedString::from(::rust_i18n::t!($($args)*).into_owned())
    };
}
pub(crate) use tr;

/// Tests that assert on translated text take this first: rust-i18n's locale is
/// process-global, so parallel tests could otherwise see each other's changes.
#[cfg(test)]
pub(crate) fn locale_test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}
