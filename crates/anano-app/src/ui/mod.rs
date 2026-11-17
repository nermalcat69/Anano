pub mod account_view;
pub mod claude_dialog;
pub mod no_changes;
pub mod pull_requests;
pub mod repo_dialogs;
pub mod review_dialog;
pub mod settings_dialog;
pub mod sidebar;
pub mod status_pane;
pub mod tab_strip;
pub mod text;
pub mod toolbar;
pub mod workflows;

/// Every button in the app is built through here so they all show the pointing-hand
/// cursor (the component's own default is the arrow).
pub fn button(id: impl Into<gpui_kit::ElementId>) -> gpui_kit::component::button::Button {
    use gpui_kit::Styled as _;
    gpui_kit::component::button::Button::new(id).cursor_pointer()
}

/// A round profile picture, or the author's initial on a tinted disc while (or if) there
/// is no picture for them.
pub fn avatar(
    picture: Option<&std::path::PathBuf>,
    name: &str,
    size: gpui_kit::Pixels,
    cx: &gpui_kit::App,
) -> gpui_kit::AnyElement {
    use gpui_kit::component::ActiveTheme as _;
    use gpui_kit::{IntoElement as _, ParentElement as _, Styled as _};
    match picture {
        Some(path) => gpui_kit::img(path.clone())
            .size(size)
            .flex_none()
            .rounded_full()
            .into_any_element(),
        None => gpui_kit::div()
            .size(size)
            .flex_none()
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(cx.theme().secondary)
            .text_color(cx.theme().muted_foreground)
            .text_size(size * 0.55)
            .child(
                name.chars()
                    .next()
                    .unwrap_or('?')
                    .to_uppercase()
                    .to_string(),
            )
            .into_any_element(),
    }
}

/// "2 months ago" for a Unix timestamp, worked out when drawn.
pub fn time_ago(unix_seconds: i64) -> gpui_kit::SharedString {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    long_ago((now - unix_seconds).max(0) as u64)
}

fn long_ago(secs: u64) -> gpui_kit::SharedString {
    const UNITS: [(u64, &str); 5] = [
        (365 * 86_400, "year"),
        (30 * 86_400, "month"),
        (86_400, "day"),
        (3600, "hour"),
        (60, "minute"),
    ];
    for (span, unit) in UNITS {
        let count = secs / span;
        if count >= 1 {
            let key = match count {
                1 => format!("time.{unit}_ago"),
                _ => format!("time.{unit}s_ago"),
            };
            return rust_i18n::t!(&key, count = count.to_string())
                .to_string()
                .into();
        }
    }
    crate::i18n::tr!("time.just_now")
}

/// Text you can select with the mouse (drag, double-click a word) and copy (⌘C / Ctrl C).
/// Selection works across every such text in the window, like a web page. Plain `div`
/// text is not selectable in gpui, so everything worth copying (diffs, logs, titles,
/// messages) goes through here. `id` must be stable and unique among siblings.
pub fn selectable(
    id: impl Into<gpui_kit::ElementId>,
    text: impl Into<gpui_kit::SharedString>,
) -> gpui_kit::base::SelectableText {
    gpui_kit::base::SelectableText::new(id, text)
}

#[cfg(test)]
mod tests {
    #[test]
    fn long_ago_picks_the_largest_unit_and_pluralises() {
        let _locale = crate::i18n::locale_test_lock();
        let read = |secs| super::long_ago(secs).to_string();
        assert_eq!(read(30), "just now");
        assert_eq!(read(60), "1 minute ago");
        assert_eq!(read(3 * 3600), "3 hours ago");
        assert_eq!(read(86_400), "1 day ago");
        assert_eq!(read(61 * 86_400), "2 months ago");
        assert_eq!(read(800 * 86_400), "2 years ago");
    }
}
