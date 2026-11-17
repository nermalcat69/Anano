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

/// The hover background of the Review and Commit & Push buttons.
const DEFAULT_HOVER: &str = "#1a1d21";

/// Which of the two accent buttons.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AccentButton {
    Commit,
    Review,
}

/// `#RRGGBB` (or `RRGGBB`) as a colour; `None` for anything else, including empty.
pub fn parse_hex(text: &str) -> Option<gpui_kit::Hsla> {
    let hex = text.trim().trim_start_matches('#');
    (hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| u32::from_str_radix(hex, 16).ok())
        .flatten()
        .map(|rgb| gpui_kit::Hsla::from(gpui_kit::rgb(rgb)))
}

/// The colours of the Commit & Push / Review button: (fill, border, text). What the person
/// chose in Settings, else the defaults: a dark fill like the Fetch origin button's, with
/// the theme's neutral gray border. The text goes white on a dark fill, black on a light one.
pub fn accent_colors(
    cx: &gpui_kit::App,
    which: AccentButton,
) -> (gpui_kit::Hsla, gpui_kit::Hsla, gpui_kit::Hsla) {
    use gpui_kit::component::ActiveTheme as _;
    let settings = crate::state::settings::settings(cx);
    let (bg, border) = match which {
        AccentButton::Commit => (settings.commit_button_bg, settings.commit_button_border),
        AccentButton::Review => (settings.review_button_bg, settings.review_button_border),
    };
    let theme = cx.theme();
    let fill = parse_hex(&bg).unwrap_or(theme.secondary);
    let border = parse_hex(&border).unwrap_or(theme.border);
    let text = if parse_hex(&bg).is_some() {
        if fill.l > 0.6 {
            gpui_kit::black()
        } else {
            gpui_kit::white()
        }
    } else {
        theme.foreground
    };
    (fill, border, text)
}

/// The button style that goes with [`accent_colors`]: a touch lighter on hover.
pub fn accent_button(
    cx: &gpui_kit::App,
    which: AccentButton,
) -> gpui_kit::component::button::ButtonCustomVariant {
    let (fill, _, text) = accent_colors(cx, which);
    let settings = crate::state::settings::settings(cx);
    let chosen = parse_hex(match which {
        AccentButton::Commit => &settings.commit_button_hover,
        AccentButton::Review => &settings.review_button_hover,
    });
    // Hover is #1a1d21 for both buttons, unless a colour was chosen in Settings; pressed is
    // that colour a little darker.
    let hover = chosen.or_else(|| parse_hex(DEFAULT_HOVER)).unwrap_or(fill);
    let active = gpui_kit::Hsla {
        l: (hover.l - 0.03).clamp(0., 1.),
        ..hover
    };
    gpui_kit::component::button::ButtonCustomVariant::new(cx)
        .color(fill)
        .hover(hover)
        .active(active)
        .foreground(text)
}

const SKELETON_WIDTHS: [f32; 6] = [0.72, 0.55, 0.84, 0.63, 0.78, 0.5];

/// What a list shows while it loads: rows of a picture and two lines, pulsing, instead of the
/// word "Loading…". The shapes match the rows that replace them, so nothing jumps.
pub fn skeleton_list(rows: usize) -> gpui_kit::AnyElement {
    use gpui_kit::component::{h_flex, skeleton::Skeleton, v_flex};
    use gpui_kit::{IntoElement as _, ParentElement as _, Styled as _, px, relative};
    v_flex()
        .w_full()
        .children((0..rows).map(|i| {
            let (a, b) = (
                SKELETON_WIDTHS[i % SKELETON_WIDTHS.len()],
                SKELETON_WIDTHS[(i + 3) % SKELETON_WIDTHS.len()] * 0.8,
            );
            h_flex()
                .gap_3()
                .px_4()
                .py_3()
                .items_center()
                .child(Skeleton::new().size(px(30.)).rounded_full().flex_none())
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_2()
                        .child(Skeleton::new().h(px(12.)).w(relative(a)))
                        .child(Skeleton::new().h(px(10.)).w(relative(b))),
                )
        }))
        .into_any_element()
}

/// The same for a page of text (a diff, a log, a description): bars of varying length.
pub fn skeleton_text(lines: usize) -> gpui_kit::AnyElement {
    use gpui_kit::component::{skeleton::Skeleton, v_flex};
    use gpui_kit::{IntoElement as _, ParentElement as _, Styled as _, px, relative};
    v_flex()
        .w_full()
        .gap_3()
        .p_4()
        .children((0..lines).map(|i| {
            Skeleton::new().h(px(12.)).w(relative(
                SKELETON_WIDTHS[(i * 5 + i / 3) % SKELETON_WIDTHS.len()],
            ))
        }))
        .into_any_element()
}

/// Where a dialog of about `height` px opens: vertically centred in the window, so the
/// buttons are never a long way from where the eye is. (The stock position is a tenth of the
/// way down, which puts a short dialog's buttons near the top edge.)
pub fn centered(
    dialog: gpui_kit::component::dialog::Dialog,
    window: &gpui_kit::Window,
    height: f32,
) -> gpui_kit::component::dialog::Dialog {
    let free = window.viewport_size().height - gpui_kit::px(height);
    dialog.margin_top((free / 2.).max(gpui_kit::px(12.)))
}

/// Tell the person something finished while they were looking elsewhere: a toast in the
/// window, and (on macOS) a system notification in case Anano isn't in front.
pub fn notify(window: &mut gpui_kit::Window, cx: &mut gpui_kit::App, text: gpui_kit::SharedString) {
    use gpui_kit::component::WindowExt as _;
    window.push_notification(
        gpui_kit::component::notification::Notification::success(text.clone()),
        cx,
    );
    #[cfg(target_os = "macos")]
    {
        let escaped = text.replace('\\', "\\\\").replace('"', "\\\"");
        let _ = std::process::Command::new("osascript")
            .args([
                "-e",
                &format!("display notification \"{escaped}\" with title \"Anano\""),
            ])
            .spawn();
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn button_colours_take_six_digit_hex_only() {
        use crate::ui::parse_hex;
        assert!(parse_hex("#26272B").is_some() && parse_hex("26272b").is_some());
        assert!(parse_hex(" #FFFFFF ").is_some());
        for bad in ["", "#", "#12345", "#1234567", "#GGGGGG", "red"] {
            assert!(parse_hex(bad).is_none(), "{bad:?}");
        }
    }

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
