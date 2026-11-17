//! The Pull Requests view (toolbar → Pull Requests): the repository's pull requests down
//! the left; the open one on the right with its description, its history (commits,
//! comments, reviews, oldest first) and, at the end, GitHub's verdict on merging it, with
//! a Merge button when GitHub says it can be merged.

use std::collections::HashMap;
use std::path::PathBuf;

use anano_core::external::{
    PULLS_PER_PAGE, PrEvent, PrEventKind, PullCounts, PullRequest, PullRequestDetail, PullSearch,
    PullState, SEARCH_CAP,
};
use gpui_kit::component::{
    ActiveTheme, Disableable, Icon, IconName, Sizable, WindowExt,
    button::ButtonVariants,
    dialog::DialogFooter,
    h_flex,
    input::{Input, InputEvent},
    text::markdown,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};

use crate::assets::ICON_REFRESH;
use crate::i18n::tr;
use crate::state::workspace::Workspace;

const LIST_WIDTH: f32 = 340.;
/// The search box and the "Author" button beside it, and where the author picker hangs so
/// that it sits under the box (it is laid out in the body, which paints over the header).
const SEARCH_WIDTH: f32 = 260.;
const AUTHOR_BUTTON_WIDTH: f32 = 76.;
const PICKER_WIDTH: f32 = SEARCH_WIDTH;
const PICKER_RIGHT: f32 = 12. + 24. + 8. + 24. + 8. + AUTHOR_BUTTON_WIDTH + 8.;

/// Which pull requests the list shows. A draft counts as open; "closed" means closed
/// without being merged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PrFilter {
    #[default]
    Open,
    Merged,
    Closed,
    All,
}

impl PrFilter {
    const ALL: [PrFilter; 4] = [
        PrFilter::Open,
        PrFilter::Merged,
        PrFilter::Closed,
        PrFilter::All,
    ];

    fn state(self) -> PullState {
        match self {
            PrFilter::Open => PullState::Open,
            PrFilter::Merged => PullState::Merged,
            PrFilter::Closed => PullState::Closed,
            PrFilter::All => PullState::All,
        }
    }

    fn label(self) -> SharedString {
        match self {
            PrFilter::Open => tr!("repo.pulls.filter.open"),
            PrFilter::Merged => tr!("repo.pulls.filter.merged"),
            PrFilter::Closed => tr!("repo.pulls.filter.closed"),
            PrFilter::All => tr!("repo.pulls.filter.all"),
        }
    }

    fn empty_text(self) -> SharedString {
        match self {
            PrFilter::Open => tr!("repo.pulls.empty.open"),
            PrFilter::Merged => tr!("repo.pulls.empty.merged"),
            PrFilter::Closed => tr!("repo.pulls.empty.closed"),
            PrFilter::All => tr!("repo.pulls.none"),
        }
    }
}

/// What the Pull Requests view is showing. `None` in a slot means "loading".
#[derive(Default)]
pub struct PullRequests {
    /// A repository other than the open one (picked from an account's repository list).
    pub remote: Option<String>,
    pub filter: PrFilter,
    /// The applied search: one author (`author:login`) and free text. Set by Enter in the
    /// search box or by picking an author; what is typed but not applied changes nothing.
    pub author: Option<String>,
    pub text: String,
    /// How many pull requests match the filter and search in all. The list holds a page
    /// at a time, so it is usually fewer.
    pub total: usize,
    /// Open / merged / closed totals for the search: what the tabs show.
    pub counts: Option<PullCounts>,
    /// The people the author picker offers, most active first.
    pub authors: Vec<String>,
    pub authors_loading: bool,
    /// Bumped whenever the query changes, so answers to an older query are dropped.
    pub generation: u64,
    /// The Review dialog's state while it is open.
    pub review: Option<crate::ui::review_dialog::ReviewState>,
    /// The "Load more" request in flight.
    pub loading_more: bool,
    /// The list on screen is being re-checked against GitHub (it came from the cache, or
    /// the user pressed refresh).
    pub revalidating: bool,
    /// Why the last "Load more" failed.
    pub more_error: Option<String>,
    pub list: Option<Result<Vec<PullRequest>, String>>,
    pub open: Option<u64>,
    pub detail: Option<Result<PullRequestDetail, String>>,
    pub merging: bool,
    /// What GitHub said when the last merge attempt was refused.
    pub merge_error: Option<String>,
}

impl PullRequests {
    pub fn search(&self) -> PullSearch {
        PullSearch {
            state: self.filter.state(),
            author: self.author.clone(),
            text: self.text.clone(),
        }
    }

    /// GitHub has more matches than are loaded (its search stops at `SEARCH_CAP`).
    pub fn has_more(&self) -> bool {
        match &self.list {
            Some(Ok(list)) => list.len() < self.total.min(SEARCH_CAP),
            _ => false,
        }
    }

    /// The query changed: forget the list and drop answers to the old query.
    fn restart(&mut self) {
        self.generation += 1;
        self.list = None;
        self.total = 0;
        self.revalidating = false;
        self.loading_more = false;
        self.more_error = None;
    }
}

/// `author:name` words of the search box (case-insensitive prefix).
fn author_word(word: &str) -> Option<&str> {
    let prefix = word.get(..7)?;
    prefix.eq_ignore_ascii_case("author:").then(|| &word[7..])
}

/// The search box's text as the one author and the free text it asks for. The last
/// `author:` with a name wins; every other word is free text.
pub(crate) fn parse_pr_query(raw: &str) -> (Option<String>, String) {
    let mut author = None;
    let mut text = Vec::new();
    for word in raw.split_whitespace() {
        match author_word(word) {
            Some(name) if !name.is_empty() => author = Some(name.to_string()),
            Some(_) => {}
            None => text.push(word),
        }
    }
    (author, text.join(" "))
}

/// The partial name after `author:` when the word being typed (the last one, no space after
/// it yet) is an author filter: what the author picker is open for.
pub(crate) fn author_being_typed(raw: &str) -> Option<&str> {
    if raw.ends_with(char::is_whitespace) {
        return None;
    }
    raw.split_whitespace().last().and_then(author_word)
}

/// GitHub's merge verdict in words, and whether the Merge button should be live.
///
/// `mergeable` / `merge_state` are passed through from the API; this is the one place that
/// decides what they mean for the user.
pub(crate) fn merge_verdict(pr: &PullRequestDetail) -> (SharedString, bool) {
    match (
        pr.summary.state.as_str(),
        pr.mergeable.as_str(),
        pr.merge_state.as_str(),
    ) {
        ("MERGED", ..) => (tr!("repo.pulls.verdict.merged"), false),
        ("CLOSED", ..) => (tr!("repo.pulls.verdict.closed"), false),
        _ if pr.summary.draft => (tr!("repo.pulls.verdict.draft"), false),
        (_, "CONFLICTING", _) | (.., "DIRTY") => (tr!("repo.pulls.verdict.conflicts"), false),
        (.., "BLOCKED") => (tr!("repo.pulls.verdict.blocked"), false),
        (.., "BEHIND") => (tr!("repo.pulls.verdict.behind"), false),
        (_, "MERGEABLE", "UNSTABLE") => (tr!("repo.pulls.verdict.unstable"), true),
        (_, "MERGEABLE", _) => (tr!("repo.pulls.verdict.clean"), true),
        // GitHub computes mergeability lazily; right after opening it often says UNKNOWN.
        _ => (tr!("repo.pulls.verdict.unknown"), false),
    }
}

fn state_badge(pr: &PullRequest, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let (label, color) = match pr.state.as_str() {
        "MERGED" => (tr!("repo.pulls.state.merged"), theme.primary),
        "CLOSED" => (tr!("repo.pulls.state.closed"), theme.danger),
        _ if pr.draft => (tr!("repo.pulls.state.draft"), theme.muted_foreground),
        _ => (tr!("repo.pulls.state.open"), theme.success),
    };
    div()
        .flex_none()
        .px_1p5()
        .py_0p5()
        .rounded(px(999.))
        .text_xs()
        .text_color(color)
        .bg(color.opacity(0.15))
        .child(label)
}

fn note(text: SharedString, color: Hsla) -> AnyElement {
    div()
        .p_4()
        .text_sm()
        .text_color(color)
        .child(text)
        .into_any_element()
}

type Avatars = HashMap<String, Option<PathBuf>>;

fn picture<'a>(avatars: &'a Avatars, login: &str) -> Option<&'a PathBuf> {
    avatars.get(login).and_then(|p| p.as_ref())
}

/// A comment-shaped box: square at the top right, round everywhere else.
fn bubble(cx: &App) -> gpui_kit::Div {
    let theme = cx.theme();
    v_flex()
        .min_w_0()
        .p_3()
        .gap_2()
        .bg(theme.secondary)
        .border_1()
        .border_color(theme.border)
        .rounded_tl(px(12.))
        .rounded_bl(px(12.))
        .rounded_br(px(12.))
        .rounded_tr(px(0.))
}

/// "author  verb • 3 days ago", the line every bubble starts with.
fn byline(author: &str, what: SharedString, at: i64, cx: &App) -> gpui_kit::Div {
    h_flex()
        .gap_1p5()
        .items_center()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(cx.theme().foreground)
                .child(author.to_string()),
        )
        .child(what)
        .child("•")
        .child(crate::ui::time_ago(at))
}

/// "CHANGES_REQUESTED" → "Changes requested", coloured by what it means.
fn verdict_badge(verdict: &str, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let color = match verdict {
        "APPROVED" => theme.success,
        "CHANGES_REQUESTED" => theme.danger,
        _ => theme.muted_foreground,
    };
    let mut words = verdict.to_lowercase().replace('_', " ");
    if let Some(first) = words.get_mut(..1) {
        first.make_ascii_uppercase();
    }
    div()
        .flex_none()
        .px_1p5()
        .py_0p5()
        .rounded(px(999.))
        .text_xs()
        .text_color(color)
        .bg(color.opacity(0.15))
        .child(words)
}

/// One entry of the history: the person's picture, then what they did. Commits are
/// compact tinted cards with fully rounded corners; comments and reviews are speech
/// bubbles with a square top-right corner.
fn event_row(ix: usize, event: &PrEvent, avatars: &Avatars, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let face = crate::ui::avatar(picture(avatars, &event.author), &event.author, px(28.), cx);
    let content = match event.kind {
        PrEventKind::Commit => v_flex()
            .min_w_0()
            .gap_1()
            .px_3()
            .py_2()
            .rounded(px(10.))
            .border_1()
            .border_color(theme.primary.opacity(0.35))
            .bg(theme.primary.opacity(0.1))
            .child(byline(
                &event.author,
                tr!("repo.pulls.event.commit"),
                event.at,
                cx,
            ))
            .child(
                div()
                    .text_sm()
                    .font_family(theme.mono_font_family.clone())
                    .child(crate::ui::selectable(
                        ("pull-commit", ix),
                        event.text.clone(),
                    )),
            ),
        PrEventKind::Comment => bubble(cx)
            .child(byline(
                &event.author,
                tr!("repo.pulls.event.comment"),
                event.at,
                cx,
            ))
            .child(markdown(event.text.clone()).selectable(true)),
        PrEventKind::Review => {
            // A review's text is its verdict, then (optionally) what the reviewer wrote.
            let (verdict, body) = event.text.split_once('\n').unwrap_or((&event.text, ""));
            bubble(cx)
                .child(
                    byline(&event.author, tr!("repo.pulls.event.review"), event.at, cx)
                        .child(verdict_badge(verdict, cx)),
                )
                .when(!body.trim().is_empty(), |b| {
                    b.child(markdown(body.to_string()).selectable(true))
                })
        }
    };
    h_flex()
        .gap_3()
        .items_start()
        .child(face)
        .child(div().flex_1().min_w_0().child(content))
}

impl Workspace {
    /// Toolbar → Pull Requests: show the view and (re)load the list.
    pub fn open_pull_requests(&mut self, cx: &mut Context<Self>) {
        self.workflows = None;
        self.account = None;
        if self.pulls.is_none() {
            self.pulls = Some(PullRequests::default());
        }
        self.reload_pull_requests(cx);
        self.load_pr_authors(cx);
        cx.notify();
    }

    /// Download (once, then cached on disk) the pictures of everyone named in the Pull
    /// Requests view: the authors in the list, and the author and commenters of the open one.
    fn load_pr_pictures(&mut self, cx: &mut Context<Self>) {
        let Some(pulls) = &self.pulls else {
            return;
        };
        let mut logins: Vec<String> = Vec::new();
        if let Some(Ok(list)) = &pulls.list {
            logins.extend(list.iter().map(|pr| pr.author.clone()));
        }
        if let Some(Ok(detail)) = &pulls.detail {
            logins.extend(detail.events.iter().map(|e| e.author.clone()));
        }
        // The author picker shows pictures too; the long tail gets initials.
        logins.extend(pulls.authors.iter().take(30).cloned());
        self.load_logins(logins, cx);
    }

    /// Download (once, then cached on disk) the profile pictures of these logins that
    /// aren't known yet. Everything that shows people (pull requests, workflow runs)
    /// asks through here.
    pub(crate) fn load_logins(&mut self, mut logins: Vec<String>, cx: &mut Context<Self>) {
        logins.retain(|login| !login.is_empty() && !self.account_avatars.contains_key(login));
        logins.sort();
        logins.dedup();
        // What is already on disk shows at once, on the very next frame.
        logins.retain(
            |login| match anano_core::external::cached_account_avatar(login) {
                Some(path) => {
                    self.account_avatars.insert(login.clone(), Some(path));
                    false
                }
                None => true,
            },
        );
        if logins.is_empty() {
            cx.notify();
            return;
        }
        // Mark as asked so a list refresh doesn't ask again while this is in flight.
        for login in &logins {
            self.account_avatars.insert(login.clone(), None);
        }
        // The rest are downloaded a few at a time, several batches side by side, and each
        // logo appears as its batch lands instead of all of them waiting for the slowest.
        for batch in logins.chunks(4) {
            let task = crate::bridge::account_avatars(cx, batch.to_vec());
            cx.spawn(async move |ws, cx| {
                if let Ok(found) = task.await {
                    let _ = ws.update(cx, |ws, cx| {
                        ws.account_avatars.extend(found);
                        cx.notify();
                    });
                }
            })
            .detach();
        }
        cx.notify();
    }

    /// Open / Merged / Closed / All switch above the list.
    pub fn set_pr_filter(&mut self, filter: PrFilter, cx: &mut Context<Self>) {
        let Some(pulls) = &mut self.pulls else {
            return;
        };
        if pulls.filter == filter {
            return;
        }
        pulls.filter = filter;
        pulls.restart();
        self.reload_pull_requests(cx);
        cx.notify();
    }

    pub fn close_pull_requests(&mut self, cx: &mut Context<Self>) {
        self.pulls = None;
        cx.notify();
    }

    /// The repository the Pull Requests view is about: one picked from an account's
    /// repository list (not necessarily cloned), else the open repository.
    pub(crate) fn pulls_origin(&self) -> Option<String> {
        self.pulls
            .as_ref()
            .and_then(|p| p.remote.clone())
            .or_else(|| self.active_repo().and_then(|r| r.origin.clone()))
    }

    /// "View pull requests" on a row of an account's repository list: the Pull Requests
    /// view for that repository, as `login`. Closing it returns to the list.
    pub fn open_pull_requests_for(&mut self, url: String, cx: &mut Context<Self>) {
        self.workflows = None;
        self.pulls = Some(PullRequests {
            remote: Some(url),
            ..Default::default()
        });
        self.reload_pull_requests(cx);
        self.load_pr_authors(cx);
        cx.notify();
    }

    /// Load the pull request list for the current filter and search, stale-while-revalidate:
    /// the copy saved by the last visit is shown at once (plain lists only), then every
    /// page already on screen is re-read from GitHub and swapped in, so merged / closed
    /// pull requests update in place. A failed re-check (offline, rate limited) keeps what
    /// is showing. Also re-reads the tab totals.
    pub fn reload_pull_requests(&mut self, cx: &mut Context<Self>) {
        let Some(origin) = self.pulls_origin() else {
            if let Some(pulls) = &mut self.pulls {
                pulls.list = Some(Err(tr!("repo.pulls.no_origin").to_string()));
            }
            return;
        };
        let Some(pulls) = &mut self.pulls else {
            return;
        };
        if pulls.revalidating {
            return;
        }
        pulls.revalidating = true;
        let generation = pulls.generation;
        let search = pulls.search();
        let cached = (pulls.list.is_none() && search.is_plain())
            .then(|| crate::bridge::cached_pull_requests(cx, origin.clone(), search.state));
        cx.notify();
        self.load_pr_counts(cx);
        cx.spawn(async move |ws, cx| {
            // The view may have moved on: another repository, or another query.
            let current = |ws: &Workspace| {
                ws.pulls_origin().as_deref() == Some(origin.as_str())
                    && ws
                        .pulls
                        .as_ref()
                        .is_some_and(|p| p.generation == generation)
            };
            if let Some(cached) = cached
                && let Ok(Some((list, total))) = cached.await
            {
                let _ = ws.update(cx, |ws, cx| {
                    if current(ws)
                        && let Some(pulls) = &mut ws.pulls
                        && pulls.list.is_none()
                    {
                        pulls.list = Some(Ok(list));
                        pulls.total = total;
                        ws.load_pr_pictures(cx);
                        cx.notify();
                    }
                });
            }
            // Re-read as many pages as are showing.
            let pages = ws
                .update(cx, |ws, _| {
                    match ws.pulls.as_ref().and_then(|p| p.list.as_ref()) {
                        Some(Ok(list)) => list.len().div_ceil(PULLS_PER_PAGE).max(1),
                        _ => 1,
                    }
                })
                .unwrap_or(1);
            let fresh = ws.update(cx, |_, cx| {
                crate::bridge::refresh_pull_requests(cx, origin.clone(), search, pages)
            });
            let result = match fresh {
                Ok(task) => task.await.map_err(|e| e.to_string()),
                Err(err) => Err(err.to_string()),
            };
            let _ = ws.update(cx, |ws, cx| {
                if !current(ws) {
                    return;
                }
                if let Some(pulls) = &mut ws.pulls {
                    pulls.revalidating = false;
                    match result {
                        Ok((list, total)) => {
                            pulls.list = Some(Ok(list));
                            pulls.total = total;
                            pulls.more_error = None;
                        }
                        Err(err) if !matches!(pulls.list, Some(Ok(_))) => {
                            pulls.list = Some(Err(err));
                        }
                        Err(_) => {}
                    }
                    ws.load_pr_pictures(cx);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// "Load more": fetch the next page and add it to the list (and to the cache).
    pub fn load_more_pull_requests(&mut self, cx: &mut Context<Self>) {
        let Some(origin) = self.pulls_origin() else {
            return;
        };
        let Some(pulls) = &mut self.pulls else {
            return;
        };
        let Some(Ok(current)) = &pulls.list else {
            return;
        };
        if pulls.loading_more || !pulls.has_more() {
            return;
        }
        let current = current.clone();
        let page = current.len().div_ceil(PULLS_PER_PAGE) + 1;
        let generation = pulls.generation;
        pulls.loading_more = true;
        pulls.more_error = None;
        let task =
            crate::bridge::more_pull_requests(cx, origin.clone(), pulls.search(), page, current);
        cx.notify();
        cx.spawn(async move |ws, cx| {
            let result = task.await.map_err(|e| e.to_string());
            let _ = ws.update(cx, |ws, cx| {
                if ws.pulls_origin().as_deref() != Some(origin.as_str()) {
                    return;
                }
                if let Some(pulls) = &mut ws.pulls {
                    if pulls.generation != generation {
                        return;
                    }
                    pulls.loading_more = false;
                    match result {
                        Ok((list, total)) => {
                            pulls.list = Some(Ok(list));
                            pulls.total = total;
                        }
                        Err(err) => pulls.more_error = Some(err),
                    }
                    ws.load_pr_pictures(cx);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Re-read the Open / Merged / Closed totals for the applied author and text.
    fn load_pr_counts(&mut self, cx: &mut Context<Self>) {
        let (Some(origin), Some(pulls)) = (self.pulls_origin(), &self.pulls) else {
            return;
        };
        let (author, text) = (pulls.author.clone(), pulls.text.clone());
        let task = crate::bridge::pull_counts(cx, origin.clone(), author.clone(), text.clone());
        cx.spawn(async move |ws, cx| {
            let Ok(counts) = task.await else { return };
            let _ = ws.update(cx, |ws, cx| {
                if ws.pulls_origin().as_deref() == Some(origin.as_str())
                    && let Some(pulls) = &mut ws.pulls
                    && pulls.author == author
                    && pulls.text == text
                {
                    pulls.counts = Some(counts);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Load the people the author picker offers: the saved list at once, then fresh from
    /// GitHub. Once per visit to the view.
    fn load_pr_authors(&mut self, cx: &mut Context<Self>) {
        let (Some(origin), Some(pulls)) = (self.pulls_origin(), &mut self.pulls) else {
            return;
        };
        if pulls.authors_loading || !pulls.authors.is_empty() {
            return;
        }
        pulls.authors_loading = true;
        let cached = crate::bridge::cached_pull_authors(cx, origin.clone());
        let fresh = crate::bridge::pull_authors(cx, origin.clone());
        cx.spawn(async move |ws, cx| {
            let same = |ws: &Workspace| ws.pulls_origin().as_deref() == Some(origin.as_str());
            if let Ok(Some(authors)) = cached.await {
                let _ = ws.update(cx, |ws, cx| {
                    if same(ws)
                        && let Some(pulls) = &mut ws.pulls
                        && pulls.authors.is_empty()
                    {
                        pulls.authors = authors;
                        ws.load_pr_pictures(cx);
                        cx.notify();
                    }
                });
            }
            let fresh = fresh.await;
            let _ = ws.update(cx, |ws, cx| {
                if !same(ws) {
                    return;
                }
                if let Some(pulls) = &mut ws.pulls {
                    pulls.authors_loading = false;
                    if let Ok(authors) = fresh {
                        pulls.authors = authors;
                    }
                    ws.load_pr_pictures(cx);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Enter in the search box (or picking an author): run the search the box describes.
    pub fn apply_pr_search(&mut self, cx: &mut Context<Self>) {
        let raw = self.pr_search_input.read(cx).value().to_string();
        let (author, text) = parse_pr_query(&raw);
        let Some(pulls) = &mut self.pulls else {
            return;
        };
        if pulls.author == author && pulls.text == text {
            return;
        }
        pulls.author = author;
        pulls.text = text;
        pulls.counts = None;
        pulls.restart();
        self.reload_pull_requests(cx);
        cx.notify();
    }

    /// Pick a person from the author list: the box becomes `author:<login>` (any earlier
    /// author word replaced, other words kept) and the search runs.
    pub fn pick_pr_author(&mut self, login: String, window: &mut Window, cx: &mut Context<Self>) {
        let raw = self.pr_search_input.read(cx).value().to_string();
        let mut words: Vec<&str> = raw
            .split_whitespace()
            .filter(|w| author_word(w).is_none())
            .collect();
        let author = format!("author:{login}");
        words.push(&author);
        let next = format!("{} ", words.join(" "));
        self.pr_search_input
            .update(cx, |input, cx| input.set_value(next, window, cx));
        self.apply_pr_search(cx);
    }

    /// The "Author" button: start an `author:` word in the box, which opens the picker.
    pub fn start_pr_author_word(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let raw = self.pr_search_input.read(cx).value().to_string();
        let mut next = raw.trim_end().to_string();
        if !next.is_empty() {
            next.push(' ');
        }
        next.push_str("author:");
        self.pr_search_input.update(cx, |input, cx| {
            input.set_value(next, window, cx);
            input.focus(window, cx);
        });
        cx.notify();
    }

    /// Reacts to the search box: typing refreshes the author picker; Enter runs the search;
    /// emptying the box clears a search that was applied.
    pub(crate) fn on_pr_search_event(&mut self, event: &InputEvent, cx: &mut Context<Self>) {
        match event {
            InputEvent::PressEnter { .. } => self.apply_pr_search(cx),
            InputEvent::Change => {
                let empty = self.pr_search_input.read(cx).value().trim().is_empty();
                let applied = self
                    .pulls
                    .as_ref()
                    .is_some_and(|p| p.author.is_some() || !p.text.is_empty());
                if empty && applied {
                    self.apply_pr_search(cx);
                }
                cx.notify();
            }
            _ => {}
        }
    }

    /// Open one pull request (or refresh the one that's open).
    pub fn open_pull_request(&mut self, number: u64, cx: &mut Context<Self>) {
        let (Some(origin), Some(pulls)) = (self.pulls_origin(), &mut self.pulls) else {
            return;
        };
        if pulls.open != Some(number) {
            pulls.detail = None;
            pulls.merge_error = None;
        }
        pulls.open = Some(number);
        cx.notify();
        let task = crate::bridge::pull_request(cx, origin, number);
        cx.spawn(async move |ws, cx| {
            let detail = task.await.map_err(|e| e.to_string());
            let _ = ws.update(cx, |ws, cx| {
                // Another one may have been clicked while this was loading.
                if let Some(pulls) = &mut ws.pulls
                    && pulls.open == Some(number)
                {
                    pulls.detail = Some(detail);
                    ws.load_pr_pictures(cx);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Merging changes the repository on GitHub for everyone, so it is confirmed first.
    fn confirm_merge(
        &mut self,
        number: u64,
        title: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.has_active_dialog(cx) {
            return;
        }
        let workspace = cx.entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let (workspace, title) = (workspace.clone(), title.clone());
            dialog
                .title(tr!("repo.pulls.merge_title", number = number.to_string()))
                .w(px(440.))
                .content(move |content, _, _| content.child(div().text_sm().child(title.clone())))
                .footer(
                    DialogFooter::new()
                        .child(
                            crate::ui::button("merge-cancel")
                                .outline()
                                .label(tr!("common.cancel"))
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            crate::ui::button("merge-confirm")
                                .primary()
                                .label(tr!("repo.pulls.merge"))
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    workspace
                                        .update(cx, |ws, cx| ws.merge_pull_request(number, cx));
                                }),
                        ),
                )
        });
    }

    fn merge_pull_request(&mut self, number: u64, cx: &mut Context<Self>) {
        let (Some(origin), Some(pulls)) = (self.pulls_origin(), &mut self.pulls) else {
            return;
        };
        pulls.merging = true;
        pulls.merge_error = None;
        cx.notify();
        let task = crate::bridge::merge_pull_request(cx, origin, number);
        cx.spawn(async move |ws, cx| {
            let result = task.await;
            let _ = ws.update(cx, |ws, cx| {
                if let Some(pulls) = &mut ws.pulls {
                    pulls.merging = false;
                    pulls.merge_error = result.err().map(|e| e.to_string());
                }
                // Either way, show what GitHub now says about it.
                ws.open_pull_request(number, cx);
                ws.reload_pull_requests(cx);
                // A merge moved the base branch on the remote.
                ws.fetch(cx);
            });
        })
        .detach();
    }

    fn render_pull_list(&self, pulls: &PullRequests, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, danger, hover, active, border) = (
            theme.muted_foreground,
            theme.danger,
            theme.list_hover,
            theme.list_active,
            theme.border,
        );
        // The switch: Open / Merged / Closed / All, each with the real total from GitHub
        // (not just how many happen to be loaded).
        let filter = pulls.filter;
        let tabs = h_flex()
            .flex_none()
            .gap_1()
            .p_2()
            .border_b_1()
            .border_color(border)
            .children(PrFilter::ALL.into_iter().enumerate().map(|(ix, tab)| {
                let count = pulls.counts.map(|c| match tab {
                    PrFilter::Open => c.open,
                    PrFilter::Merged => c.merged,
                    PrFilter::Closed => c.closed,
                    PrFilter::All => c.all(),
                });
                let label = match count {
                    Some(n) => format!("{} {n}", tab.label()),
                    None => tab.label().to_string(),
                };
                crate::ui::button(("pull-filter", ix))
                    .ghost()
                    .xsmall()
                    .flex_1()
                    .label(label)
                    .when(tab == filter, |button| button.bg(active))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_pr_filter(tab, cx)))
            }));
        // End of the list: how many of the matches are loaded, and "Load more".
        let loaded = match &pulls.list {
            Some(Ok(list)) => list.len(),
            _ => 0,
        };
        let footer = pulls.has_more().then(|| {
            v_flex()
                .flex_none()
                .p_3()
                .gap_2()
                .child(div().text_xs().text_color(muted).child(tr!(
                    "repo.pulls.showing",
                    shown = loaded.to_string(),
                    total = pulls.total.to_string()
                )))
                .children(
                    pulls
                        .more_error
                        .clone()
                        .map(|err| div().text_xs().text_color(danger).child(err)),
                )
                .child(
                    crate::ui::button("pull-load-more")
                        .outline()
                        .small()
                        .w_full()
                        .loading(pulls.loading_more)
                        .disabled(pulls.loading_more)
                        .label(tr!("repo.pulls.load_more"))
                        .on_click(cx.listener(|this, _, _, cx| this.load_more_pull_requests(cx))),
                )
        });
        let searching = pulls.author.is_some() || !pulls.text.is_empty();
        let body = match &pulls.list {
            None => note(tr!("repo.pulls.loading"), muted),
            Some(Err(err)) => note(err.clone().into(), danger),
            Some(Ok(list)) if list.is_empty() => v_flex()
                .id("pull-empty")
                .flex_1()
                .min_h_0()
                .child(note(
                    if searching {
                        tr!("repo.pulls.no_match")
                    } else {
                        filter.empty_text()
                    },
                    muted,
                ))
                .children(footer)
                .into_any_element(),
            Some(Ok(list)) => v_flex()
                .id("pull-list")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .children(list.iter().map(|pr| {
                    let number = pr.number;
                    let selected = pulls.open == Some(number);
                    v_flex()
                        .id(("pull", number as usize))
                        .flex_none()
                        .px_3()
                        .py_2()
                        .gap_1()
                        .border_b_1()
                        .border_color(border)
                        .cursor_pointer()
                        .when(selected, |row| row.bg(active))
                        .hover(move |style| style.bg(hover))
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.open_pull_request(number, cx)),
                        )
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_sm()
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(pr.title.clone()),
                                )
                                .child(state_badge(pr, cx)),
                        )
                        .child(
                            h_flex()
                                .gap_1p5()
                                .items_center()
                                .text_xs()
                                .text_color(muted)
                                .child(crate::ui::avatar(
                                    picture(&self.account_avatars, &pr.author),
                                    &pr.author,
                                    px(14.),
                                    cx,
                                ))
                                .child(div().min_w_0().truncate().child(format!(
                                    "#{} • {} • {}",
                                    pr.number,
                                    pr.author,
                                    crate::ui::time_ago(pr.created_at)
                                ))),
                        )
                }))
                .children(footer)
                .into_any_element(),
        };
        v_flex()
            .size_full()
            .child(tabs)
            .child(body)
            .into_any_element()
    }

    /// The scrollable list of people under the search box while an `author:` word is being
    /// typed: everyone who contributed (most active first) plus anyone seen in the loaded
    /// pull requests, narrowed by what has been typed after `author:`.
    fn render_author_picker(
        &self,
        pulls: &PullRequests,
        partial: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let (muted, border, hover) = (theme.muted_foreground, theme.border, theme.list_hover);
        let partial = partial.to_lowercase();
        let mut names: Vec<String> = pulls.authors.clone();
        if let Some(Ok(list)) = &pulls.list {
            for pr in list {
                if !pr.author.is_empty() && !names.contains(&pr.author) {
                    names.push(pr.author.clone());
                }
            }
        }
        names.retain(|name| name.to_lowercase().contains(&partial));
        let rows: Vec<AnyElement> = names
            .into_iter()
            .enumerate()
            .map(|(ix, login)| {
                let face =
                    crate::ui::avatar(picture(&self.account_avatars, &login), &login, px(20.), cx);
                let pick = login.clone();
                h_flex()
                    .id(("pr-author", ix))
                    .flex_none()
                    .gap_2()
                    .px_2()
                    .py_1p5()
                    .items_center()
                    .rounded(px(6.))
                    .text_sm()
                    .cursor_pointer()
                    .hover(move |style| style.bg(hover))
                    .child(face)
                    .child(div().min_w_0().truncate().child(login))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.pick_pr_author(pick.clone(), window, cx)
                    }))
                    .into_any_element()
            })
            .collect();
        let empty = rows.is_empty().then(|| {
            div()
                .p_2()
                .text_sm()
                .text_color(muted)
                .child(if pulls.authors_loading {
                    tr!("repo.pulls.authors_loading")
                } else {
                    tr!("repo.pulls.authors_none")
                })
        });
        v_flex()
            .id("pr-author-picker")
            // Blocks the mouse for everything behind it: without this the wheel also
            // scrolled the pull request list and detail under the picker.
            .occlude()
            .absolute()
            .top(px(4.))
            .right(px(PICKER_RIGHT))
            .w(px(PICKER_WIDTH))
            .max_h(px(320.))
            .overflow_y_scroll()
            .p_1()
            .rounded(px(8.))
            .border_1()
            .border_color(border)
            .bg(cx.theme().popover)
            .shadow_md()
            .child(
                div()
                    .px_2()
                    .py_1()
                    .text_xs()
                    .text_color(muted)
                    .child(tr!("repo.pulls.author_hint")),
            )
            .children(rows)
            .children(empty)
            .into_any_element()
    }

    fn render_pull_detail(&self, pulls: &PullRequests, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, danger, border) = (theme.muted_foreground, theme.danger, theme.border);
        let pr = match (&pulls.open, &pulls.detail) {
            (None, _) => return note(tr!("repo.pulls.pick_one"), muted),
            (_, None) => return note(tr!("repo.pulls.loading_one"), muted),
            (_, Some(Err(err))) => return note(err.clone().into(), danger),
            (_, Some(Ok(pr))) => pr,
        };
        let (verdict, can_merge) = merge_verdict(pr);
        let section = |title: SharedString| {
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .child(title)
        };
        let (number, title, url) = (pr.summary.number, pr.summary.title.clone(), pr.url.clone());
        let author = pr.summary.author.clone();
        let author_face = |size| {
            crate::ui::avatar(
                picture(&self.account_avatars, &author),
                &author,
                px(size),
                cx,
            )
        };
        let review_button = crate::ui::button("pull-review")
            .primary()
            .label(tr!("repo.pulls.review"))
            .on_click(cx.listener(|this, _, window, cx| this.open_review_dialog(window, cx)));
        // The header stays put; only what is below it scrolls.
        let header = h_flex()
            .flex_none()
            .gap_3()
            .items_start()
            .px_4()
            .py_3()
            .border_b_1()
            .border_color(border)
            .child(author_face(36.))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1p5()
                    .child(div().text_base().font_weight(FontWeight::SEMIBOLD).child(
                        crate::ui::selectable(
                            "pull-title",
                            format!("{} #{}", pr.summary.title, pr.summary.number),
                        ),
                    ))
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .flex_wrap()
                            .text_xs()
                            .text_color(muted)
                            .child(state_badge(&pr.summary, cx))
                            .child(crate::ui::selectable(
                                "pull-meta",
                                tr!(
                                    "repo.pulls.wants_to_merge",
                                    author = pr.summary.author.clone(),
                                    head = pr.summary.head.clone(),
                                    base = pr.summary.base.clone()
                                ),
                            ))
                            .child(format!(
                                "+{} −{} • {}",
                                pr.additions,
                                pr.deletions,
                                tr!(
                                    "repo.history.changed_files",
                                    count = pr.changed_files.to_string()
                                )
                            ))
                            .child(
                                crate::ui::button("pull-open-web")
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::ExternalLink)
                                    .label(tr!("repo.history.view_on_github"))
                                    .on_click(move |_, _, cx| cx.open_url(&url)),
                            ),
                    ),
            )
            .child(review_button);
        let rest = v_flex()
            .id("pull-detail-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p_4()
            .gap_4()
            // The description is the author's own comment, shown like the rest.
            .child(
                h_flex()
                    .gap_3()
                    .items_start()
                    .child(author_face(28.))
                    .child(
                        div().flex_1().min_w_0().child(
                            bubble(cx)
                                .child(byline(
                                    &pr.summary.author,
                                    tr!("repo.pulls.event.opened"),
                                    pr.summary.created_at,
                                    cx,
                                ))
                                .child(if pr.body.trim().is_empty() {
                                    div()
                                        .text_sm()
                                        .text_color(muted)
                                        .child(tr!("repo.pulls.no_description"))
                                        .into_any_element()
                                } else {
                                    markdown(pr.body.clone())
                                        .selectable(true)
                                        .into_any_element()
                                }),
                        ),
                    ),
            )
            .child(section(tr!("repo.pulls.history")))
            .child(
                v_flex().gap_3().children(
                    pr.events
                        .iter()
                        .enumerate()
                        .map(|(ix, event)| event_row(ix, event, &self.account_avatars, cx)),
                ),
            )
            // The end of the page: can it be merged, and the button if so.
            .child(
                v_flex()
                    .gap_2()
                    .p_3()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(border)
                    .child(
                        div()
                            .text_sm()
                            .child(crate::ui::selectable("pull-verdict", verdict)),
                    )
                    // A pull request of the open repository that conflicts with its base:
                    // Claude Code can check it out and resolve the conflicts.
                    .when(
                        pr.summary.state == "OPEN"
                            && pulls.remote.is_none()
                            && (pr.mergeable == "CONFLICTING" || pr.merge_state == "DIRTY"),
                        |merge_box| {
                            let (head, base) = (pr.summary.head.clone(), pr.summary.base.clone());
                            merge_box.child(
                                h_flex().child(
                                    crate::ui::button("pull-fix-conflicts")
                                        .outline()
                                        .label(tr!("repo.claude.fix_pr_button"))
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.fix_pull_request_conflicts(
                                                number,
                                                head.clone(),
                                                base.clone(),
                                                window,
                                                cx,
                                            )
                                        })),
                                ),
                            )
                        },
                    )
                    .children(pulls.merge_error.clone().map(|err| {
                        div()
                            .text_sm()
                            .text_color(danger)
                            .child(crate::ui::selectable("pull-merge-error", err))
                    }))
                    .when(pr.summary.state == "OPEN", |merge_box| {
                        merge_box.child(
                            h_flex().debug_selector(|| "pull-merge".into()).child(
                                crate::ui::button("pull-merge")
                                    .primary()
                                    .label(tr!("repo.pulls.merge"))
                                    .loading(pulls.merging)
                                    .disabled(!can_merge || pulls.merging)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.confirm_merge(number, title.clone(), window, cx)
                                    })),
                            ),
                        )
                    }),
            );
        v_flex()
            .id("pull-detail")
            .size_full()
            .child(header)
            .child(rest)
            .into_any_element()
    }

    pub fn render_pull_requests(
        &self,
        pulls: &PullRequests,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let border = cx.theme().border;
        let slug = self
            .pulls_origin()
            .and_then(|o| anano_core::external::repo_slug(&o))
            .unwrap_or_default();
        // Open while the word being typed is `author:…`; closes once a name is picked.
        let typed = self.pr_search_input.read(cx).value().to_string();
        let picker =
            author_being_typed(&typed).map(|partial| self.render_author_picker(pulls, partial, cx));
        v_flex()
            .id("pull-requests")
            .size_full()
            .child(
                h_flex()
                    .flex_none()
                    .h_10()
                    .px_3()
                    .gap_2()
                    .items_center()
                    .border_b_1()
                    .border_color(border)
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(tr!("repo.pulls.title")),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(slug),
                    )
                    .child(
                        div()
                            .flex_none()
                            .w(px(SEARCH_WIDTH))
                            .rounded(px(6.))
                            .border_1()
                            .border_color(border)
                            .child(
                                Input::new(&self.pr_search_input)
                                    .prefix(
                                        Icon::new(IconName::Search)
                                            .size_4()
                                            .text_color(cx.theme().muted_foreground),
                                    )
                                    .cleanable(true),
                            ),
                    )
                    .child(
                        crate::ui::button("pulls-author")
                            .outline()
                            .xsmall()
                            .w(px(AUTHOR_BUTTON_WIDTH))
                            .label(tr!("repo.pulls.author_button"))
                            .tooltip(tr!("repo.pulls.author_hint"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.start_pr_author_word(window, cx)
                            })),
                    )
                    .child(
                        crate::ui::button("pulls-refresh")
                            .ghost()
                            .xsmall()
                            .icon(Icon::empty().path(ICON_REFRESH))
                            .loading(pulls.revalidating)
                            .tooltip(tr!("repo.clone.refresh"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.reload_pull_requests(cx);
                                if let Some(open) = this.pulls.as_ref().and_then(|p| p.open) {
                                    this.open_pull_request(open, cx);
                                }
                            })),
                    )
                    .child(
                        crate::ui::button("pulls-close")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Close)
                            .tooltip(tr!("repo.close"))
                            .on_click(cx.listener(|this, _, _, cx| this.close_pull_requests(cx))),
                    ),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(
                        h_flex()
                            .size_full()
                            .items_start()
                            .child(
                                div()
                                    .flex_none()
                                    .w(px(LIST_WIDTH))
                                    .h_full()
                                    .border_r_1()
                                    .border_color(border)
                                    .child(self.render_pull_list(pulls, cx)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .h_full()
                                    .child(self.render_pull_detail(pulls, cx)),
                            ),
                    )
                    // Drawn last so it sits on top of the list and the detail.
                    .children(picker),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr(state: &str, draft: bool, mergeable: &str, merge_state: &str) -> PullRequestDetail {
        PullRequestDetail {
            summary: PullRequest {
                number: 1,
                title: "t".into(),
                author: "a".into(),
                state: state.into(),
                draft,
                head: "h".into(),
                base: "main".into(),
                created_at: 0,
            },
            body: String::new(),
            url: String::new(),
            additions: 0,
            deletions: 0,
            changed_files: 0,
            mergeable: mergeable.into(),
            merge_state: merge_state.into(),
            events: Vec::new(),
        }
    }

    /// The Merge button is only live when GitHub itself says the pull request can merge.
    #[test]
    fn merge_button_follows_githubs_verdict() {
        let can = |state, draft, mergeable, merge_state| {
            merge_verdict(&pr(state, draft, mergeable, merge_state)).1
        };
        assert!(can("OPEN", false, "MERGEABLE", "CLEAN"));
        assert!(can("OPEN", false, "MERGEABLE", "UNSTABLE"));
        assert!(!can("OPEN", false, "CONFLICTING", "DIRTY"));
        assert!(!can("OPEN", false, "MERGEABLE", "BLOCKED"));
        assert!(!can("OPEN", false, "MERGEABLE", "BEHIND"));
        assert!(!can("OPEN", true, "MERGEABLE", "CLEAN"));
        assert!(!can("OPEN", false, "UNKNOWN", "UNKNOWN"));
        assert!(!can("MERGED", false, "MERGEABLE", "CLEAN"));
        assert!(!can("CLOSED", false, "MERGEABLE", "CLEAN"));
    }

    /// The whole view draws: list, a Markdown description, each kind of history entry, and
    /// the merge box with its button at the end.
    #[gpui_kit::test]
    fn pull_request_view_draws_list_detail_history_and_merge_box(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use gpui_kit::AppContext as _;
        cx.executor().allow_parking();
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::bridge::init(cx);
        });
        let mut workspace = None;
        let window = cx.add_window(|window, cx| {
            let ws = cx.new(|cx| Workspace::new(window, cx));
            workspace = Some(ws.clone());
            gpui_kit::component::Root::new(ws, window, cx)
        });
        let workspace = workspace.unwrap();
        let cx = &mut gpui_kit::VisualTestContext::from_window(window.into(), cx);
        let mut detail = pr("OPEN", false, "MERGEABLE", "CLEAN");
        detail.body = "# Heading\n\nSome **bold** text and a list:\n\n- one\n- two\n".into();
        detail.events = vec![
            PrEvent {
                kind: PrEventKind::Commit,
                author: "me".into(),
                at: 1,
                text: "abc1234 do it".into(),
            },
            PrEvent {
                kind: PrEventKind::Review,
                author: "you".into(),
                at: 2,
                text: "APPROVED".into(),
            },
            PrEvent {
                kind: PrEventKind::Comment,
                author: "you".into(),
                at: 3,
                text: "looks *good*".into(),
            },
        ];
        workspace.update(cx, |ws, cx| {
            ws.open_repo_path("/tmp/anano-pulls-test".into(), cx);
            ws.pulls = Some(PullRequests {
                list: Some(Ok(vec![detail.summary.clone()])),
                open: Some(1),
                detail: Some(Ok(detail)),
                ..Default::default()
            });
            cx.notify();
        });
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("pull-merge").is_some(),
            "merge box wasn't drawn"
        );
        // Selecting something in the sidebar goes back to the diff.
        workspace.update(cx, |ws, cx| {
            ws.select(
                crate::state::workspace::Selected::Commit("0".repeat(40)),
                cx,
            )
        });
        assert!(workspace.read_with(cx, |ws, _| ws.pulls.is_none()));
    }

    #[test]
    fn the_search_box_splits_into_an_author_and_free_text() {
        assert_eq!(parse_pr_query(""), (None, String::new()));
        assert_eq!(parse_pr_query("fix bug"), (None, "fix bug".to_string()));
        assert_eq!(
            parse_pr_query("fix AUTHOR:me  the bug"),
            (Some("me".to_string()), "fix the bug".to_string())
        );
        // The last author word wins; a bare `author:` (still being typed) is ignored.
        assert_eq!(
            parse_pr_query("author:a author:b"),
            (Some("b".to_string()), String::new())
        );
        assert_eq!(parse_pr_query("author:"), (None, String::new()));
    }

    #[test]
    fn the_author_picker_opens_only_while_an_author_word_is_being_typed() {
        assert_eq!(author_being_typed("author:"), Some(""));
        assert_eq!(author_being_typed("fix author:ar"), Some("ar"));
        assert_eq!(author_being_typed("AUTHOR:Ar"), Some("Ar"));
        // A finished word (space after it), other words, or an empty box: closed.
        assert_eq!(author_being_typed("author:arjun "), None);
        assert_eq!(author_being_typed("author:arjun fix"), None);
        assert_eq!(author_being_typed(""), None);
        assert_eq!(author_being_typed("auth"), None);
    }
}
