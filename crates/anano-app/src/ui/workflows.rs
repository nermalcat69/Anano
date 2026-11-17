//! The Workflows view (toolbar → Workflows): GitHub Actions for the repository. Runs down the
//! left (filterable by workflow and by status, newest first, "Load more"); the open run on the
//! right with its jobs and their steps, and the selected job's log with errors, warnings,
//! commands and step headers highlighted. Read-only: nothing is re-run or cancelled.

use std::collections::HashMap;
use std::rc::Rc;

use anano_core::workflows::{
    LogKind, LogLine, RUNS_PER_PAGE, RunJob, RunStatus, Workflow, WorkflowRun,
};
use gpui_kit::component::{
    ActiveTheme, Disableable, Icon, IconName, Sizable,
    button::ButtonVariants,
    h_flex,
    input::{Input, InputEvent, InputState},
    popover::Popover,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, Entity, FontWeight, Hsla, InteractiveElement, IntoElement,
    ParentElement, ScrollStrategy, SharedString, StatefulInteractiveElement, StyleRefinement,
    Styled, div, px, uniform_list,
};

use crate::assets::ICON_REFRESH;
use crate::i18n::tr;
use crate::state::workspace::Workspace;

const LIST_WIDTH: f32 = 320.;
const JOBS_WIDTH: f32 = 250.;
const ROW_HEIGHT: f32 = 20.;

/// A job's log: being fetched, ready, or why it couldn't be had (they expire on GitHub).
#[derive(Clone)]
pub enum LogState {
    Loading,
    Ready(Rc<Vec<LogLine>>),
    Failed(String),
}

/// What the Workflows view is showing. `None` in a slot means "loading".
#[derive(Default)]
pub struct WorkflowsState {
    pub workflows: Option<Result<Vec<Workflow>, String>>,
    /// The workflow the run list is narrowed to; `None` is every workflow.
    pub workflow: Option<u64>,
    pub status: RunStatus,
    pub runs: Option<Result<Vec<WorkflowRun>, String>>,
    /// How many runs match the filters in all.
    pub total: usize,
    /// A page came back short: there is nothing after what is loaded.
    pub exhausted: bool,
    pub loading: bool,
    pub loading_more: bool,
    pub more_error: Option<String>,
    /// Bumped whenever the filters change, so answers to an older query are dropped.
    pub generation: u64,
    pub picker_open: bool,
    pub open: Option<WorkflowRun>,
    pub jobs: Option<Result<Vec<RunJob>, String>>,
    pub job: Option<u64>,
    pub logs: HashMap<u64, LogState>,
    /// Show only errors and warnings in the log.
    pub errors_only: bool,
}

impl WorkflowsState {
    fn has_more(&self) -> bool {
        match &self.runs {
            Some(Ok(runs)) => !self.exhausted && runs.len() < self.total,
            _ => false,
        }
    }

    /// The filters changed: forget the list and drop answers to the old query.
    fn restart(&mut self) {
        self.generation += 1;
        self.runs = None;
        self.total = 0;
        self.exhausted = false;
        self.loading = false;
        self.loading_more = false;
        self.more_error = None;
    }
}

/// `3m 12s`, `45s`, `1h 02m`.
pub(crate) fn duration(secs: i64) -> String {
    let secs = secs.max(0);
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m {:02}s", secs / 60, secs % 60),
        _ => format!("{}h {:02}m", secs / 3600, secs % 3600 / 60),
    }
}

/// A run's or job's state as a glyph, a colour and a word. `conclusion` is empty until the
/// thing has finished.
pub(crate) fn outcome(
    status: &str,
    conclusion: &str,
    cx: &App,
) -> (&'static str, Hsla, SharedString) {
    let theme = cx.theme();
    match (status, conclusion) {
        ("completed", "success") => ("✓", theme.success, tr!("repo.workflows.state.success")),
        ("completed", "failure") => ("✕", theme.danger, tr!("repo.workflows.state.failure")),
        ("completed", "timed_out") => ("✕", theme.danger, tr!("repo.workflows.state.timed_out")),
        ("completed", "cancelled") => (
            "⊘",
            theme.muted_foreground,
            tr!("repo.workflows.state.cancelled"),
        ),
        ("completed", "skipped") => (
            "–",
            theme.muted_foreground,
            tr!("repo.workflows.state.skipped"),
        ),
        ("completed", "action_required") => (
            "!",
            theme.warning,
            tr!("repo.workflows.state.action_required"),
        ),
        ("completed", _) => (
            "•",
            theme.muted_foreground,
            tr!("repo.workflows.state.neutral"),
        ),
        ("in_progress", _) => ("●", theme.warning, tr!("repo.workflows.state.running")),
        _ => (
            "○",
            theme.muted_foreground,
            tr!("repo.workflows.state.queued"),
        ),
    }
}

fn badge(status: &str, conclusion: &str, size: f32, cx: &App) -> impl IntoElement {
    let (glyph, color, label) = outcome(status, conclusion, cx);
    div()
        .id(SharedString::from(format!("state-{glyph}-{size}")))
        .flex_none()
        .size(px(size))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .border_1()
        .border_color(color)
        .bg(color.opacity(0.12))
        .text_color(color)
        .text_xs()
        .font_weight(FontWeight::BOLD)
        .child(glyph)
        .tooltip(move |window, cx| {
            gpui_kit::component::tooltip::Tooltip::new(label.clone()).build(window, cx)
        })
}

fn note(text: SharedString, color: Hsla) -> AnyElement {
    div()
        .p_4()
        .text_sm()
        .text_color(color)
        .child(text)
        .into_any_element()
}

fn run_seconds(run: &WorkflowRun) -> i64 {
    let end = if run.status == "completed" {
        run.updated_at
    } else {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64)
    };
    end - run.started_at.max(run.created_at)
}

/// One row of the log: line number, time of day, then the text styled for what it is.
fn log_row(number: usize, line: &LogLine, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let row = h_flex()
        .when(number == 1, |r| r.debug_selector(|| "log-row-1".into()))
        .h(px(ROW_HEIGHT))
        .w_full()
        .font_family(theme.mono_font_family.clone())
        .text_xs();
    let (tint, color, bold, prefix) = match line.kind {
        LogKind::Group => (Some(theme.secondary), theme.foreground, true, "▸ "),
        LogKind::Error => (Some(theme.danger.opacity(0.14)), theme.danger, false, ""),
        LogKind::Warning => (Some(theme.warning.opacity(0.14)), theme.warning, false, ""),
        LogKind::Notice => (None, theme.primary, false, ""),
        LogKind::Command => (None, theme.primary, false, "$ "),
        LogKind::Debug => (None, muted, false, ""),
        LogKind::Plain => (None, theme.foreground, false, ""),
    };
    row.when_some(tint, |r, tint| r.bg(tint))
        .child(
            div()
                .w(px(52.))
                .flex_none()
                .flex()
                .justify_end()
                .pr_2()
                .text_color(muted)
                .child(number.to_string()),
        )
        .child(
            div()
                .w(px(64.))
                .flex_none()
                .text_color(muted)
                .children(line.time.clone()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_color(color)
                .when(bold, |d| d.font_weight(FontWeight::SEMIBOLD))
                .child(crate::ui::selectable(
                    ("log-text", number),
                    format!("{prefix}{}", line.text.replace('\t', "    ")),
                )),
        )
        .into_any_element()
}

/// The workflow picker's panel: a filter box and every workflow, "All workflows" first.
fn workflow_picker(
    workflows: &[Workflow],
    selected: Option<u64>,
    search: &Entity<InputState>,
    workspace: &Entity<Workspace>,
    cx: &App,
) -> impl IntoElement + use<> {
    let theme = cx.theme();
    let (muted, border, hover, active) = (
        theme.muted_foreground,
        theme.border,
        theme.list_hover,
        theme.list_active,
    );
    let query = search.read(cx).value().trim().to_lowercase();
    let row = |ix: usize, id: Option<u64>, name: String, hint: String, enabled: bool| {
        let workspace = workspace.clone();
        h_flex()
            .id(("workflow-row", ix))
            .flex_none()
            .gap_2()
            .px_2()
            .py_1p5()
            .rounded(px(6.))
            .items_center()
            .text_sm()
            .cursor_pointer()
            .when(selected == id, |r| r.bg(active))
            .hover(move |style| style.bg(hover))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .when(!enabled, |d| d.text_color(muted))
                    .child(name),
            )
            .child(
                div()
                    .flex_none()
                    .max_w(px(110.))
                    .truncate()
                    .text_xs()
                    .text_color(muted)
                    .child(hint),
            )
            .on_click(move |_, _, cx| workspace.update(cx, |ws, cx| ws.set_workflow_filter(id, cx)))
    };
    let rows: Vec<_> = std::iter::once(row(
        0,
        None,
        tr!("repo.workflows.all_workflows").to_string(),
        String::new(),
        true,
    ))
    .filter(|_| query.is_empty())
    .chain(
        workflows
            .iter()
            .filter(|w| w.name.to_lowercase().contains(&query))
            .enumerate()
            .map(|(ix, w)| {
                let file = w.path.rsplit('/').next().unwrap_or(&w.path).to_string();
                row(ix + 1, Some(w.id), w.name.clone(), file, w.active)
            }),
    )
    .collect();
    v_flex()
        .w(px(340.))
        .p_2()
        .gap_2()
        .child(div().rounded(px(6.)).border_1().border_color(border).child(
            Input::new(search).prefix(Icon::new(IconName::Search).size_4().text_color(muted)),
        ))
        .child(
            v_flex()
                .id("workflow-list")
                .gap_0p5()
                .max_h(px(360.))
                .overflow_y_scroll()
                .children(rows),
        )
}

impl Workspace {
    /// Toolbar → Workflows: show the view and load the runs and the workflow list.
    pub fn open_workflows(&mut self, cx: &mut Context<Self>) {
        self.pulls = None;
        self.account = None;
        if self.workflows.is_none() {
            self.workflows = Some(WorkflowsState::default());
        }
        self.reload_runs(cx);
        self.load_workflow_list(cx);
        cx.notify();
    }

    pub fn close_workflows(&mut self, cx: &mut Context<Self>) {
        self.workflows = None;
        cx.notify();
    }

    fn workflows_origin(&self) -> Option<String> {
        self.active_repo().and_then(|r| r.origin.clone())
    }

    fn load_workflow_list(&mut self, cx: &mut Context<Self>) {
        let (Some(origin), Some(state)) = (self.workflows_origin(), &mut self.workflows) else {
            return;
        };
        if state.workflows.is_some() {
            return;
        }
        // Saved from an earlier visit: shown now, replaced when GitHub answers.
        if let Some(cached) = anano_core::workflows::cached_workflows(&origin) {
            state.workflows = Some(Ok(cached));
        }
        let task = crate::bridge::list_workflows(cx, origin.clone());
        cx.spawn(async move |ws, cx| {
            let result = task.await.map_err(|e| e.to_string());
            let _ = ws.update(cx, |ws, cx| {
                if ws.workflows_origin().as_deref() == Some(origin.as_str())
                    && let Some(state) = &mut ws.workflows
                {
                    // A failed refresh keeps what the cache showed.
                    match result {
                        Err(_) if matches!(state.workflows, Some(Ok(_))) => {}
                        other => state.workflows = Some(other),
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// (Re)load the first page of runs for the current filters. What is on screen stays
    /// until the new page arrives; a failed refresh keeps it.
    pub fn reload_runs(&mut self, cx: &mut Context<Self>) {
        let Some(origin) = self.workflows_origin() else {
            if let Some(state) = &mut self.workflows {
                state.runs = Some(Err(tr!("repo.pulls.no_origin").to_string()));
            }
            return;
        };
        let Some(state) = &mut self.workflows else {
            return;
        };
        if state.loading {
            return;
        }
        state.loading = true;
        let generation = state.generation;
        // The first page as last saved for these filters, until the fresh one arrives.
        if state.runs.is_none()
            && let Some(page) =
                anano_core::workflows::cached_runs(&origin, state.workflow, state.status, 1)
        {
            state.exhausted = page.items.len() < RUNS_PER_PAGE;
            state.total = page.total;
            state.runs = Some(Ok(page.items));
        }
        let task = crate::bridge::list_runs(cx, origin.clone(), state.workflow, state.status, 1);
        cx.notify();
        cx.spawn(async move |ws, cx| {
            let result = task.await.map_err(|e| e.to_string());
            let _ = ws.update(cx, |ws, cx| {
                if ws.workflows_origin().as_deref() != Some(origin.as_str()) {
                    return;
                }
                let Some(state) = &mut ws.workflows else {
                    return;
                };
                if state.generation != generation {
                    return;
                }
                state.loading = false;
                match result {
                    Ok((runs, total)) => {
                        state.exhausted = runs.len() < RUNS_PER_PAGE;
                        state.runs = Some(Ok(runs));
                        state.total = total;
                        state.more_error = None;
                    }
                    Err(err) if !matches!(state.runs, Some(Ok(_))) => state.runs = Some(Err(err)),
                    Err(_) => {}
                }
                ws.load_run_pictures(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// "Load more": the next page of runs, appended.
    pub fn load_more_runs(&mut self, cx: &mut Context<Self>) {
        let Some(origin) = self.workflows_origin() else {
            return;
        };
        let Some(state) = &mut self.workflows else {
            return;
        };
        let Some(Ok(runs)) = &state.runs else {
            return;
        };
        if state.loading_more || !state.has_more() {
            return;
        }
        let page = runs.len().div_ceil(RUNS_PER_PAGE) + 1;
        let generation = state.generation;
        state.loading_more = true;
        state.more_error = None;
        let task = crate::bridge::list_runs(cx, origin.clone(), state.workflow, state.status, page);
        cx.notify();
        cx.spawn(async move |ws, cx| {
            let result = task.await.map_err(|e| e.to_string());
            let _ = ws.update(cx, |ws, cx| {
                if ws.workflows_origin().as_deref() != Some(origin.as_str()) {
                    return;
                }
                let Some(state) = &mut ws.workflows else {
                    return;
                };
                if state.generation != generation {
                    return;
                }
                state.loading_more = false;
                match result {
                    Ok((more, total)) => {
                        state.exhausted = more.len() < RUNS_PER_PAGE;
                        state.total = total;
                        if let Some(Ok(runs)) = &mut state.runs {
                            for run in more {
                                if !runs.iter().any(|r| r.id == run.id) {
                                    runs.push(run);
                                }
                            }
                        }
                    }
                    Err(err) => state.more_error = Some(err),
                }
                ws.load_run_pictures(cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn load_run_pictures(&mut self, cx: &mut Context<Self>) {
        let Some(Ok(runs)) = self.workflows.as_ref().and_then(|w| w.runs.as_ref()) else {
            return;
        };
        let logins = runs.iter().map(|r| r.actor.clone()).collect();
        self.load_logins(logins, cx);
    }

    /// Narrow the runs to one workflow (`None`: all of them).
    pub fn set_workflow_filter(&mut self, workflow: Option<u64>, cx: &mut Context<Self>) {
        let Some(state) = &mut self.workflows else {
            return;
        };
        state.picker_open = false;
        if state.workflow == workflow {
            cx.notify();
            return;
        }
        state.workflow = workflow;
        state.restart();
        self.reload_runs(cx);
        cx.notify();
    }

    pub fn set_run_status(&mut self, status: RunStatus, cx: &mut Context<Self>) {
        let Some(state) = &mut self.workflows else {
            return;
        };
        if state.status == status {
            return;
        }
        state.status = status;
        state.restart();
        self.reload_runs(cx);
        cx.notify();
    }

    /// Open a run: fetch its jobs, and show the log of the one that failed (else the first).
    pub fn open_run(&mut self, run: WorkflowRun, cx: &mut Context<Self>) {
        let Some(origin) = self.workflows_origin() else {
            return;
        };
        let Some(state) = &mut self.workflows else {
            return;
        };
        let id = run.id;
        state.open = Some(run);
        state.jobs = anano_core::workflows::cached_run_jobs(&origin, id).map(Ok);
        state.job = None;
        // Saved jobs: pick the one to read right away (its log is cached too).
        let early_pick = match &state.jobs {
            Some(Ok(jobs)) => jobs
                .iter()
                .find(|j| matches!(j.conclusion.as_str(), "failure" | "timed_out"))
                .or(jobs.first())
                .map(|j| j.id),
            _ => None,
        };
        let task = crate::bridge::run_jobs(cx, origin.clone(), id);
        if let Some(job) = early_pick {
            self.open_job(job, cx);
        }
        cx.notify();
        cx.spawn(async move |ws, cx| {
            let result = task.await.map_err(|e| e.to_string());
            let _ = ws.update(cx, |ws, cx| {
                let Some(state) = &mut ws.workflows else {
                    return;
                };
                // Another run may have been clicked while this was loading.
                if state.open.as_ref().map(|r| r.id) != Some(id) {
                    return;
                }
                let pick = match &result {
                    Ok(jobs) => jobs
                        .iter()
                        .find(|j| matches!(j.conclusion.as_str(), "failure" | "timed_out"))
                        .or(jobs.first())
                        .map(|j| j.id),
                    Err(_) => None,
                };
                // A failed refresh keeps the saved jobs.
                match result {
                    Err(_) if matches!(state.jobs, Some(Ok(_))) => {}
                    other => state.jobs = Some(other),
                }
                // Open the log unless the saved jobs already did.
                let already = state.job.is_some();
                if let (Some(job), false) = (pick, already) {
                    ws.open_job(job, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Show one job's log, fetching it the first time.
    pub fn open_job(&mut self, job: u64, cx: &mut Context<Self>) {
        let Some(origin) = self.workflows_origin() else {
            return;
        };
        let Some(state) = &mut self.workflows else {
            return;
        };
        state.job = Some(job);
        self.log_scroll.scroll_to_item(0, ScrollStrategy::Top);
        if state.logs.contains_key(&job) {
            cx.notify();
            return;
        }
        state.logs.insert(job, LogState::Loading);
        // A job that has finished never changes its log: the saved copy is the answer, and
        // GitHub is only asked when there is none (or the job is still running).
        let finished = matches!(&state.jobs, Some(Ok(jobs))
            if jobs.iter().any(|j| j.id == job && !j.conclusion.is_empty()));
        let saved = crate::bridge::cached_job_log(cx, origin.clone(), job);
        let task = crate::bridge::job_log(cx, origin, job);
        cx.notify();
        cx.spawn(async move |ws, cx| {
            let saved = saved.await.ok().flatten();
            let had_saved = saved.is_some();
            if let Some(lines) = saved {
                let _ = ws.update(cx, |ws, cx| {
                    if let Some(state) = &mut ws.workflows {
                        state.logs.insert(job, LogState::Ready(Rc::new(lines)));
                        cx.notify();
                    }
                });
            }
            if had_saved && finished {
                return; // nothing newer can exist
            }
            let result = task.await;
            let _ = ws.update(cx, |ws, cx| {
                if let Some(state) = &mut ws.workflows {
                    match result {
                        Ok(lines) => {
                            state.logs.insert(job, LogState::Ready(Rc::new(lines)));
                        }
                        // A failed refresh keeps the saved log on screen.
                        Err(_) if had_saved => {}
                        Err(err) => {
                            state.logs.insert(job, LogState::Failed(err.to_string()));
                        }
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub fn toggle_errors_only(&mut self, cx: &mut Context<Self>) {
        if let Some(state) = &mut self.workflows {
            state.errors_only = !state.errors_only;
        }
        self.log_scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    /// Scroll the log to its first error.
    pub fn jump_to_first_error(&mut self, cx: &mut Context<Self>) {
        let Some(state) = &self.workflows else {
            return;
        };
        let Some(LogState::Ready(lines)) = state.job.and_then(|j| state.logs.get(&j)) else {
            return;
        };
        let Some(first) = lines.iter().position(|l| l.kind == LogKind::Error) else {
            return;
        };
        // With "errors only" the list holds only errors and warnings, so count those.
        let row = if state.errors_only {
            lines[..first]
                .iter()
                .filter(|l| matches!(l.kind, LogKind::Error | LogKind::Warning))
                .count()
        } else {
            first
        };
        self.log_scroll.scroll_to_item(row, ScrollStrategy::Top);
        cx.notify();
    }

    pub(crate) fn on_workflow_search_event(&mut self, event: &InputEvent, cx: &mut Context<Self>) {
        if matches!(event, InputEvent::Change) {
            cx.notify();
        }
    }

    fn render_run_list(&self, state: &WorkflowsState, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, danger, border, hover, active) = (
            theme.muted_foreground,
            theme.danger,
            theme.border,
            theme.list_hover,
            theme.list_active,
        );
        // Which workflow, and which status.
        let picked = match (&state.workflow, &state.workflows) {
            (Some(id), Some(Ok(list))) => list
                .iter()
                .find(|w| w.id == *id)
                .map(|w| w.name.clone())
                .unwrap_or_else(|| id.to_string()),
            _ => tr!("repo.workflows.all_workflows").to_string(),
        };
        let workflows = match &state.workflows {
            Some(Ok(list)) => list.clone(),
            _ => Vec::new(),
        };
        let (selected, search, workspace) = (
            state.workflow,
            self.workflow_search_input.clone(),
            cx.entity(),
        );
        let picker = Popover::new("workflow-picker")
            .open(state.picker_open)
            .on_open_change(cx.listener(|this, open: &bool, _, cx| {
                if let Some(state) = &mut this.workflows {
                    state.picker_open = *open;
                    cx.notify();
                }
            }))
            .trigger_style(StyleRefinement::default())
            .trigger(
                crate::ui::button("workflow-picker-button")
                    .outline()
                    .small()
                    .w_full()
                    .child(
                        h_flex()
                            .w_full()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_left()
                                    .child(picked),
                            )
                            .child(Icon::new(IconName::ChevronDown).size_4()),
                    ),
            )
            .content(move |_, _, cx| {
                workflow_picker(&workflows, selected, &search, &workspace, cx)
            });
        let chips = h_flex().gap_1().children(
            [
                (RunStatus::All, tr!("repo.workflows.filter.all")),
                (RunStatus::Failed, tr!("repo.workflows.filter.failed")),
                (RunStatus::Succeeded, tr!("repo.workflows.filter.success")),
                (RunStatus::Running, tr!("repo.workflows.filter.running")),
            ]
            .into_iter()
            .enumerate()
            .map(|(ix, (status, label))| {
                crate::ui::button(("run-status", ix))
                    .ghost()
                    .xsmall()
                    .flex_1()
                    .label(label)
                    .when(state.status == status, |b| b.bg(active))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_run_status(status, cx)))
            }),
        );
        let filters = v_flex()
            .flex_none()
            .gap_2()
            .p_2()
            .border_b_1()
            .border_color(border)
            .child(picker)
            .child(chips);

        let footer = state.has_more().then(|| {
            let shown = match &state.runs {
                Some(Ok(runs)) => runs.len(),
                _ => 0,
            };
            v_flex()
                .flex_none()
                .p_3()
                .gap_2()
                .child(div().text_xs().text_color(muted).child(tr!(
                    "repo.pulls.showing",
                    shown = shown.to_string(),
                    total = state.total.to_string()
                )))
                .children(
                    state
                        .more_error
                        .clone()
                        .map(|err| div().text_xs().text_color(danger).child(err)),
                )
                .child(
                    crate::ui::button("runs-load-more")
                        .outline()
                        .small()
                        .w_full()
                        .loading(state.loading_more)
                        .disabled(state.loading_more)
                        .label(tr!("repo.pulls.load_more"))
                        .on_click(cx.listener(|this, _, _, cx| this.load_more_runs(cx))),
                )
        });
        let open_id = state.open.as_ref().map(|r| r.id);
        let body = match &state.runs {
            None => crate::ui::skeleton_list(8),
            Some(Err(err)) => note(err.clone().into(), danger),
            Some(Ok(runs)) if runs.is_empty() => note(tr!("repo.workflows.none"), muted),
            Some(Ok(runs)) => v_flex()
                .id("run-list")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .children(runs.iter().map(|run| {
                    let clicked = run.clone();
                    let face = crate::ui::avatar(
                        self.account_avatars
                            .get(&run.actor)
                            .and_then(|p| p.as_ref()),
                        &run.actor,
                        px(14.),
                        cx,
                    );
                    h_flex()
                        .id(("run", run.id as usize))
                        .flex_none()
                        .gap_2()
                        .px_3()
                        .py_2()
                        .items_start()
                        .border_b_1()
                        .border_color(border)
                        .cursor_pointer()
                        .when(open_id == Some(run.id), |r| r.bg(active))
                        .hover(move |style| style.bg(hover))
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.open_run(clicked.clone(), cx)),
                        )
                        .child(badge(&run.status, &run.conclusion, 18., cx))
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .gap_0p5()
                                .child(
                                    div()
                                        .truncate()
                                        .text_sm()
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(run.title.clone()),
                                )
                                .child(div().truncate().text_xs().text_color(muted).child(format!(
                                    "{} • #{} • {}",
                                    run.workflow, run.number, run.branch
                                )))
                                .child(
                                    h_flex()
                                        .gap_1p5()
                                        .items_center()
                                        .text_xs()
                                        .text_color(muted)
                                        .child(face)
                                        .child(div().min_w_0().truncate().child(format!(
                                            "{} • {}",
                                            run.actor,
                                            crate::ui::time_ago(run.created_at)
                                        ))),
                                ),
                        )
                }))
                .children(footer)
                .into_any_element(),
        };
        v_flex()
            .size_full()
            .child(filters)
            .child(body)
            .into_any_element()
    }

    fn render_run_detail(&self, state: &WorkflowsState, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, danger, border, hover, active) = (
            theme.muted_foreground,
            theme.danger,
            theme.border,
            theme.list_hover,
            theme.list_active,
        );
        let Some(run) = &state.open else {
            return note(tr!("repo.workflows.pick_run"), muted);
        };
        let url = run.url.clone();
        let face = crate::ui::avatar(
            self.account_avatars
                .get(&run.actor)
                .and_then(|p| p.as_ref()),
            &run.actor,
            px(16.),
            cx,
        );
        let (_, _, label) = outcome(&run.status, &run.conclusion, cx);
        // Sticky: it sits above the scrolling parts.
        let header = h_flex()
            .flex_none()
            .gap_3()
            .items_start()
            .px_4()
            .py_3()
            .border_b_1()
            .border_color(border)
            .child(badge(&run.status, &run.conclusion, 28., cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(
                        div()
                            .truncate()
                            .text_base()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(crate::ui::selectable("run-title", run.title.clone())),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .flex_wrap()
                            .text_xs()
                            .text_color(muted)
                            .child(label)
                            .child(format!("{} #{}", run.workflow, run.number))
                            .child(run.branch.clone())
                            .child(
                                div()
                                    .font_family(theme.mono_font_family.clone())
                                    .child(run.sha.chars().take(7).collect::<String>()),
                            )
                            .child(run.event.clone())
                            .child(face)
                            .child(run.actor.clone())
                            .child(crate::ui::time_ago(run.created_at))
                            .child(duration(run_seconds(run))),
                    ),
            )
            .child(
                crate::ui::button("run-open-web")
                    .outline()
                    .small()
                    .icon(IconName::ExternalLink)
                    .label(tr!("repo.history.view_on_github"))
                    .on_click(move |_, _, cx| cx.open_url(&url)),
            );

        // ---- jobs, the selected one with its steps
        let jobs: AnyElement = match &state.jobs {
            None => crate::ui::skeleton_list(4),
            Some(Err(err)) => note(err.clone().into(), danger),
            Some(Ok(jobs)) if jobs.is_empty() => note(tr!("repo.workflows.no_jobs"), muted),
            Some(Ok(jobs)) => v_flex()
                .id("run-jobs")
                .h_full()
                .overflow_y_scroll()
                .children(jobs.iter().map(|job| {
                    let id = job.id;
                    let selected = state.job == Some(id);
                    let took =
                        (job.completed_at > 0).then(|| duration(job.completed_at - job.started_at));
                    v_flex()
                        .id(("job", id as usize))
                        .flex_none()
                        .border_b_1()
                        .border_color(border)
                        .child(
                            h_flex()
                                .id(("job-head", id as usize))
                                .gap_2()
                                .px_3()
                                .py_2()
                                .items_center()
                                .cursor_pointer()
                                .when(selected, |r| r.bg(active))
                                .hover(move |style| style.bg(hover))
                                .on_click(cx.listener(move |this, _, _, cx| this.open_job(id, cx)))
                                .child(badge(&job.status, &job.conclusion, 16., cx))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_sm()
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(job.name.clone()),
                                )
                                .children(took.map(|t| {
                                    div().flex_none().text_xs().text_color(muted).child(t)
                                })),
                        )
                        .when(selected, |card| {
                            card.children(job.steps.iter().map(|step| {
                                let took = (step.completed_at > 0)
                                    .then(|| duration(step.completed_at - step.started_at));
                                h_flex()
                                    .gap_2()
                                    .pl_6()
                                    .pr_3()
                                    .py_1()
                                    .items_center()
                                    .text_xs()
                                    .child(badge(&step.status, &step.conclusion, 12., cx))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .child(format!("{}. {}", step.number, step.name)),
                                    )
                                    .children(
                                        took.map(|t| div().flex_none().text_color(muted).child(t)),
                                    )
                            }))
                        })
                }))
                .into_any_element(),
        };

        // ---- the log
        let log: AnyElement = match state.job.and_then(|j| state.logs.get(&j)) {
            None => note(tr!("repo.workflows.pick_job"), muted),
            Some(LogState::Loading) => crate::ui::skeleton_text(16),
            Some(LogState::Failed(err)) => note(err.clone().into(), danger),
            Some(LogState::Ready(lines)) => {
                let errors = lines.iter().filter(|l| l.kind == LogKind::Error).count();
                let warnings = lines.iter().filter(|l| l.kind == LogKind::Warning).count();
                // The rows shown: every line, or only the errors and warnings.
                let visible: Rc<Vec<usize>> = Rc::new(
                    lines
                        .iter()
                        .enumerate()
                        .filter(|(_, l)| {
                            !state.errors_only
                                || matches!(l.kind, LogKind::Error | LogKind::Warning)
                        })
                        .map(|(ix, _)| ix)
                        .collect(),
                );
                let (rows, shared) = (visible.len(), lines.clone());
                let list = if rows == 0 {
                    note(tr!("repo.workflows.log_empty"), muted)
                } else {
                    uniform_list("job-log", rows, move |range, _, cx| {
                        range
                            .map(|i| {
                                let line = visible[i];
                                log_row(line + 1, &shared[line], cx)
                            })
                            .collect()
                    })
                    .track_scroll(&self.log_scroll)
                    .flex_1()
                    .min_h_0()
                    .into_any_element()
                };
                let bar = h_flex()
                    .flex_none()
                    .h_9()
                    .px_3()
                    .gap_3()
                    .items_center()
                    .border_b_1()
                    .border_color(border)
                    .text_xs()
                    .child(
                        div()
                            .text_color(muted)
                            .child(tr!("repo.workflows.lines", count = lines.len().to_string())),
                    )
                    .child(
                        div()
                            .text_color(if errors > 0 { danger } else { muted })
                            .child(tr!("repo.workflows.errors", count = errors.to_string())),
                    )
                    .child(
                        div()
                            .text_color(if warnings > 0 { theme.warning } else { muted })
                            .child(tr!("repo.workflows.warnings", count = warnings.to_string())),
                    )
                    .child(div().flex_1())
                    .child(
                        crate::ui::button("log-first-error")
                            .ghost()
                            .xsmall()
                            .label(tr!("repo.workflows.first_error"))
                            .disabled(errors == 0)
                            .on_click(cx.listener(|this, _, _, cx| this.jump_to_first_error(cx))),
                    )
                    .child({
                        let button = crate::ui::button("log-errors-only")
                            .xsmall()
                            .label(tr!("repo.workflows.errors_only"));
                        if state.errors_only {
                            button.primary()
                        } else {
                            button.outline()
                        }
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_errors_only(cx)))
                    });
                v_flex()
                    .size_full()
                    .child(bar)
                    .child(list)
                    .into_any_element()
            }
        };
        v_flex()
            .id("workflow-detail")
            .size_full()
            .child(header)
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_start()
                    .child(
                        div()
                            .flex_none()
                            .w(px(JOBS_WIDTH))
                            .h_full()
                            .border_r_1()
                            .border_color(border)
                            .child(jobs),
                    )
                    .child(div().flex_1().min_w_0().h_full().child(log)),
            )
            .into_any_element()
    }

    pub fn render_workflows(
        &self,
        state: &WorkflowsState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let border = cx.theme().border;
        let slug = self
            .workflows_origin()
            .and_then(|o| anano_core::external::repo_slug(&o))
            .unwrap_or_default();
        v_flex()
            .id("workflows")
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
                            .child(tr!("repo.workflows.title")),
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
                        crate::ui::button("workflows-refresh")
                            .ghost()
                            .xsmall()
                            .icon(Icon::empty().path(ICON_REFRESH))
                            .loading(state.loading)
                            .tooltip(tr!("repo.clone.refresh"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.reload_runs(cx);
                                if let Some(run) =
                                    this.workflows.as_ref().and_then(|w| w.open.clone())
                                {
                                    this.open_run(run, cx);
                                }
                            })),
                    )
                    .child(
                        crate::ui::button("workflows-close")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Close)
                            .tooltip(tr!("repo.close"))
                            .on_click(cx.listener(|this, _, _, cx| this.close_workflows(cx))),
                    ),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_start()
                    .child(
                        div()
                            .flex_none()
                            .w(px(LIST_WIDTH))
                            .h_full()
                            .border_r_1()
                            .border_color(border)
                            .child(self.render_run_list(state, cx)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .child(self.render_run_detail(state, cx)),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anano_core::workflows::RunStep;
    use gpui_kit::AppContext as _;

    fn run(id: u64, conclusion: &str) -> WorkflowRun {
        WorkflowRun {
            id,
            workflow_id: 1,
            workflow: "CI".into(),
            title: format!("Run {id}"),
            number: id,
            event: "push".into(),
            status: "completed".into(),
            conclusion: conclusion.into(),
            branch: "main".into(),
            actor: "me".into(),
            sha: "abcdef0123".into(),
            created_at: 1_789_427_775,
            started_at: 1_789_427_780,
            updated_at: 1_789_427_900,
            url: "https://github.com/o/r/actions/runs/1".into(),
        }
    }

    fn line(kind: LogKind, text: &str) -> LogLine {
        LogLine {
            kind,
            time: Some("07:51:27".into()),
            text: text.into(),
        }
    }

    #[test]
    fn durations_and_paging_read_sensibly() {
        assert_eq!(duration(-5), "0s");
        assert_eq!(duration(45), "45s");
        assert_eq!(duration(192), "3m 12s");
        assert_eq!(duration(3_725), "1h 02m");

        let mut state = WorkflowsState {
            runs: Some(Ok((1..=30).map(|i| run(i, "success")).collect())),
            total: 412,
            ..Default::default()
        };
        assert!(state.has_more());
        state.exhausted = true; // a short page came back: GitHub has no more
        assert!(!state.has_more());
        state.exhausted = false;
        state.total = 30;
        assert!(!state.has_more());
    }

    /// The whole view draws: runs, a run's jobs and steps, and a log with every kind of
    /// line, in full and with "errors only"; and the log-less states.
    #[gpui_kit::test]
    fn the_workflows_view_draws_runs_jobs_and_a_highlighted_log(cx: &mut gpui_kit::TestAppContext) {
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
        let draw = |cx: &mut gpui_kit::VisualTestContext| {
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
        };
        let cx = &mut gpui_kit::VisualTestContext::from_window(window.into(), cx);
        let log = Rc::new(vec![
            line(LogKind::Group, "Run cargo test"),
            line(LogKind::Command, "cargo test"),
            line(LogKind::Plain, "running 3 tests"),
            line(LogKind::Warning, "warning: unused variable"),
            line(LogKind::Error, "error: test failed"),
            line(LogKind::Debug, "debug detail"),
        ]);
        let job = RunJob {
            id: 7,
            name: "build".into(),
            status: "completed".into(),
            conclusion: "failure".into(),
            started_at: 1_789_427_780,
            completed_at: 1_789_427_900,
            steps: vec![RunStep {
                number: 1,
                name: "Set up job".into(),
                status: "completed".into(),
                conclusion: "success".into(),
                started_at: 1_789_427_780,
                completed_at: 1_789_427_782,
            }],
        };
        workspace.update(cx, |ws, cx| {
            ws.workflows = Some(WorkflowsState {
                workflows: Some(Ok(vec![Workflow {
                    id: 1,
                    name: "CI".into(),
                    path: ".github/workflows/ci.yml".into(),
                    active: true,
                }])),
                runs: Some(Ok(vec![run(1, "failure"), run(2, "success"), run(3, "")])),
                total: 3,
                open: Some(run(1, "failure")),
                jobs: Some(Ok(vec![job])),
                job: Some(7),
                logs: HashMap::from([(7, LogState::Ready(log))]),
                ..Default::default()
            });
            cx.notify();
        });
        draw(cx); // everything in the log
        workspace.update(cx, |ws, cx| ws.toggle_errors_only(cx));
        draw(cx); // only the error and the warning
        workspace.update(cx, |ws, cx| ws.jump_to_first_error(cx));
        draw(cx);
        // The in-between states.
        workspace.update(cx, |ws, cx| {
            let state = ws.workflows.as_mut().unwrap();
            state.logs.insert(7, LogState::Loading);
            state
                .logs
                .insert(7, LogState::Failed("logs expired".into()));
            state.jobs = None;
            cx.notify();
        });
        draw(cx);
    }

    /// Dragging the mouse over log text selects it, and ⌘C copies it: plain div text
    /// can't be selected in gpui, which is why log lines go through `selectable`.
    #[gpui_kit::test]
    fn log_text_can_be_selected_and_copied(cx: &mut gpui_kit::TestAppContext) {
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
        let log = Rc::new(vec![line(LogKind::Plain, "selectable words here")]);
        workspace.update(cx, |ws, cx| {
            ws.workflows = Some(WorkflowsState {
                runs: Some(Ok(vec![run(1, "failure")])),
                total: 1,
                open: Some(run(1, "failure")),
                jobs: Some(Ok(vec![])),
                job: Some(7),
                logs: HashMap::from([(7, LogState::Ready(log))]),
                ..Default::default()
            });
            cx.notify();
        });
        cx.run_until_parked();
        let row = cx
            .debug_bounds("log-row-1")
            .expect("log row isn't on screen");
        // Past the line number and the time, across the whole text.
        let y = row.center().y;
        let (from, to) = (row.left() + px(117.), row.right() - px(4.));
        cx.simulate_mouse_down(
            gpui_kit::point(from, y),
            gpui_kit::MouseButton::Left,
            gpui_kit::Modifiers::none(),
        );
        cx.simulate_event(gpui_kit::MouseMoveEvent {
            position: gpui_kit::point(to, y),
            pressed_button: Some(gpui_kit::MouseButton::Left),
            modifiers: gpui_kit::Modifiers::none(),
        });
        cx.simulate_mouse_up(
            gpui_kit::point(to, y),
            gpui_kit::MouseButton::Left,
            gpui_kit::Modifiers::none(),
        );
        cx.run_until_parked();
        cx.simulate_keystrokes("cmd-c ctrl-c");
        let copied = cx.read_from_clipboard().and_then(|item| item.text());
        assert_eq!(copied.as_deref(), Some("selectable words here"));
    }
}
