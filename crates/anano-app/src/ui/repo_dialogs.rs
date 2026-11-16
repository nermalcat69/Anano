//! The three "Add" dialogs: Clone a Repository, Create a New Repository, Add Local
//! Repository. Laid out after GitHub Desktop's: every question (which repository, where on
//! disk, what to initialise it with) is answered inside the dialog, and nothing happens
//! until its primary button is pressed.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use anano_core::external;
use gpui_kit::component::{
    ActiveTheme, Disableable, Icon, IconName, IndexPath, Sizable, WindowExt,
    button::{Button, ButtonVariants},
    checkbox::Checkbox,
    dialog::DialogFooter,
    h_flex,
    input::{Input, InputState},
    searchable_list::SearchableVec,
    select::{Select, SelectState},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext, Context, Div, Entity, FontWeight, Hsla, InteractiveElement, IntoElement,
    ParentElement, PathPromptOptions, SharedString, StatefulInteractiveElement, Styled, Window,
    div, px,
};

use crate::assets::{ICON_BOOK, ICON_LOCK, ICON_REFRESH};
use crate::bridge::{self, CreateOptions};
use crate::i18n::tr;
use crate::state::workspace::Workspace;

const DIALOG_WIDTH: f32 = 480.;
/// Rows drawn in the repository list; the filter box narrows anything beyond that.
const MAX_REPO_ROWS: usize = 300;

type Picked = Rc<RefCell<Option<String>>>;
type Options = SelectState<SearchableVec<SharedString>>;

/// A labelled form row.
fn field(label: SharedString, body: impl IntoElement) -> Div {
    v_flex()
        .gap_1()
        .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(label))
        .child(body)
}

fn boxed(input: &Entity<InputState>, border: Hsla) -> Div {
    div()
        .rounded(px(6.))
        .border_1()
        .border_color(border)
        .child(Input::new(input))
}

/// "Local Path": an editable folder path plus a Choose… button backed by the native picker.
fn path_field(input: &Entity<InputState>, id: &'static str, border: Hsla) -> Div {
    let target = input.clone();
    field(
        tr!("repo.dialog.local_path"),
        h_flex()
            .gap_2()
            .child(boxed(input, border).flex_1().min_w_0())
            .child(
                crate::ui::button(id)
                    .outline()
                    .label(tr!("repo.dialog.choose"))
                    .on_click(move |_, window, cx| {
                        let picked = cx.prompt_for_paths(PathPromptOptions {
                            files: false,
                            directories: true,
                            multiple: false,
                            prompt: Some(tr!("repo.dialog.choose_prompt")),
                        });
                        let target = target.clone();
                        window
                            .spawn(cx, async move |cx| {
                                if let Ok(Ok(Some(mut paths))) = picked.await
                                    && let Some(path) = paths.pop()
                                {
                                    let _ = target.update_in(cx, |input, window, cx| {
                                        input.set_value(path.display().to_string(), window, cx)
                                    });
                                }
                            })
                            .detach();
                    }),
            ),
    )
}

/// Cancel + the dialog's primary action.
fn footer(confirm: Button) -> DialogFooter {
    DialogFooter::new()
        .child(
            crate::ui::button("dialog-cancel")
                .outline()
                .label(tr!("common.cancel"))
                .on_click(|_, window, cx| window.close_dialog(cx)),
        )
        .child(confirm.primary())
}

/// A segmented tab: the selected one is filled, not underlined.
fn tab(id: &'static str, label: SharedString, selected: bool, active_bg: Hsla) -> Button {
    crate::ui::button(id)
        .ghost()
        .flex_1()
        .label(label)
        .when(selected, |b| b.bg(active_bg))
}

/// One button per connected account; the picked one is filled. `allow_none` lets a second
/// click unpick it (create dialog: "don't publish"); the clone list always needs an owner.
fn account_picker(
    logins: Vec<String>,
    picked: Picked,
    id: &'static str,
    allow_none: bool,
    on_change: impl Fn(Option<String>, &mut App) + Clone + 'static,
) -> Div {
    let current = picked.borrow().clone();
    h_flex()
        .gap_2()
        .flex_wrap()
        .children(logins.into_iter().enumerate().map(|(ix, login)| {
            let selected = current.as_deref() == Some(login.as_str());
            let picked = picked.clone();
            let on_change = on_change.clone();
            let button = crate::ui::button((id, ix)).small().label(login.clone());
            if selected {
                button.primary()
            } else {
                button.outline()
            }
            .on_click(move |_, window, cx| {
                let next = match selected {
                    true if allow_none => None,
                    true => return,
                    false => Some(login.clone()),
                };
                *picked.borrow_mut() = next.clone();
                on_change(next, cx);
                window.refresh();
            })
        }))
}

fn text_of(input: &Entity<InputState>, cx: &App) -> String {
    input.read(cx).value().trim().to_string()
}

/// `["None", ...items]` for a Git Ignore / License select.
fn with_none(items: impl IntoIterator<Item = String>) -> SearchableVec<SharedString> {
    let mut all = vec![tr!("repo.create.none")];
    all.extend(items.into_iter().map(SharedString::from));
    SearchableVec::from(all)
}

/// The chosen entry of a Git Ignore / License select, `None` for the "None" row.
///
/// Compared by value, not by row: these selects are searchable, and after typing "mit" the
/// MIT row is row 0 of the *filtered* list, which a row check mistook for "None".
fn chosen(select: &Entity<Options>, cx: &App) -> Option<String> {
    let none = tr!("repo.create.none");
    select
        .read(cx)
        .selected_value()
        .filter(|value| **value != none)
        .map(|value| value.to_string())
}

/// Everything the create dialog's controls write to. Kept on the workspace while the dialog
/// is open so its answers are read from one place (and so tests can drive it).
#[derive(Clone)]
pub(crate) struct CreateForm {
    pub name: Entity<InputState>,
    pub description: Entity<InputState>,
    pub path: Entity<InputState>,
    pub readme: Rc<Cell<bool>>,
    pub private: Rc<Cell<bool>>,
    pub account: Picked,
    pub gitignore: Entity<Options>,
    pub license: Entity<Options>,
}

impl CreateForm {
    /// The dialog's answers, or `None` while the name or the folder is still empty.
    pub fn options(
        &self,
        templates: Option<&external::Templates>,
        cx: &App,
    ) -> Option<CreateOptions> {
        let (name, parent) = (text_of(&self.name, cx), text_of(&self.path, cx));
        if name.is_empty() || parent.is_empty() {
            return None;
        }
        // The select shows license names; the API wants the key.
        let license = chosen(&self.license, cx).and_then(|picked| {
            let found = templates?.licenses.iter().find(|l| l.1 == picked);
            found.map(|(key, _)| key.clone())
        });
        Some(CreateOptions {
            parent: PathBuf::from(parent),
            name,
            description: text_of(&self.description, cx),
            readme: self.readme.get(),
            gitignore: chosen(&self.gitignore, cx),
            license,
            github: self
                .account
                .borrow()
                .clone()
                .map(|l| (l, self.private.get())),
        })
    }
}

impl Workspace {
    /// Where new clones / repositories go by default: next to the repository that's open,
    /// else `~/Documents/GitHub` if that exists, else the home directory.
    fn default_parent(&self) -> String {
        let beside_active = self
            .active_repo()
            .and_then(|r| r.entry.path.parent().map(PathBuf::from))
            .filter(|p| p.is_dir());
        let home = std::env::home_dir().unwrap_or_default();
        let github = Some(home.join("Documents/GitHub")).filter(|p| p.is_dir());
        beside_active
            .or(github)
            .unwrap_or(home)
            .display()
            .to_string()
    }

    /// Add → Clone Repository…
    ///
    /// "GitHub.com" lists what the picked account can reach (filter box + a refresh button
    /// that re-reads the list from GitHub); "URL" takes any git URL. Both clone into
    /// `<Local Path>/<repository name>`.
    pub fn open_clone_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_repo_switcher(cx);
        if window.has_active_dialog(cx) {
            return;
        }
        let owner = self
            .accounts
            .iter()
            .find(|a| a.active)
            .or(self.accounts.first())
            .map(|a| a.login.clone());
        if let Some(login) = &owner {
            self.load_account_repos(login.clone(), false, cx);
        }
        let on_github = Rc::new(Cell::new(owner.is_some()));
        let account: Picked = Rc::new(RefCell::new(owner));
        let repo: Picked = Rc::default();
        let filter = cx.new(|cx| {
            InputState::new(window, cx).placeholder(tr!("repo.clone.filter_placeholder"))
        });
        let url =
            cx.new(|cx| InputState::new(window, cx).placeholder(tr!("repo.clone.url_placeholder")));
        let parent = self.default_parent();
        let path = cx.new(|cx| InputState::new(window, cx).default_value(parent));
        let workspace = cx.entity();

        window.open_dialog(cx, move |dialog, _, _| {
            let (ws_c, on_github_c, account_c, repo_c) = (
                workspace.clone(),
                on_github.clone(),
                account.clone(),
                repo.clone(),
            );
            let (filter_c, url_c, path_c) = (filter.clone(), url.clone(), path.clone());
            let (ws_f, on_github_f, account_f, repo_f) = (
                workspace.clone(),
                on_github.clone(),
                account.clone(),
                repo.clone(),
            );
            let (url_f, path_f) = (url.clone(), path.clone());
            dialog
                .title(tr!("repo.clone.title"))
                .w(px(DIALOG_WIDTH))
                .content(move |content, _, cx| {
                    let theme = cx.theme();
                    let (muted, danger, border) =
                        (theme.muted_foreground, theme.danger, theme.border);
                    let (hover, active, radius) =
                        (theme.list_hover, theme.list_active, theme.radius);
                    let logins = ws_c.read(cx).account_logins();
                    let github = on_github_c.get() && !logins.is_empty();

                    let tabs = (!logins.is_empty()).then(|| {
                        let (to_github, to_url) = (on_github_c.clone(), on_github_c.clone());
                        h_flex()
                            .gap_1()
                            .p_0p5()
                            .rounded(radius)
                            .border_1()
                            .border_color(border)
                            .child(
                                tab("clone-tab-github", "GitHub.com".into(), github, active)
                                    .on_click(move |_, window, _| {
                                        to_github.set(true);
                                        window.refresh();
                                    }),
                            )
                            .child(
                                tab("clone-tab-url", "URL".into(), !github, active).on_click(
                                    move |_, window, _| {
                                        to_url.set(false);
                                        window.refresh();
                                    },
                                ),
                            )
                    });

                    let body = if github {
                        let login = account_c.borrow().clone().unwrap_or_default();
                        let listed = ws_c.read(cx).account_repos.get(&login).cloned();
                        let query = filter_c.read(cx).value().trim().to_lowercase();
                        let picked = repo_c.borrow().clone();
                        let note = |text: SharedString, color: Hsla| {
                            div().py_4().text_sm().text_color(color).child(text)
                        };
                        let list = match listed {
                            None => note(tr!("repo.clone.repos_loading"), muted),
                            Some(Err(err)) => note(err.into(), danger),
                            Some(Ok(repos)) => {
                                let rows: Vec<_> = repos
                                    .into_iter()
                                    .filter(|r| r.name.to_lowercase().contains(&query))
                                    .take(MAX_REPO_ROWS)
                                    .enumerate()
                                    .map(|(ix, r)| {
                                        let selected = picked.as_deref() == Some(r.name.as_str());
                                        let repo_c = repo_c.clone();
                                        let icon = if r.private { ICON_LOCK } else { ICON_BOOK };
                                        let name = r.name.clone();
                                        h_flex()
                                            .id(("clone-repo", ix))
                                            .gap_2p5()
                                            .px_2()
                                            .py_1p5()
                                            .rounded(radius)
                                            .text_sm()
                                            .cursor_pointer()
                                            .when(selected, |row| row.bg(active))
                                            .hover(move |style| style.bg(hover))
                                            .child(
                                                Icon::empty().path(icon).size_4().text_color(muted),
                                            )
                                            .child(div().min_w_0().truncate().child(r.name))
                                            .on_click(move |_, window, _| {
                                                *repo_c.borrow_mut() = Some(name.clone());
                                                window.refresh();
                                            })
                                    })
                                    .collect();
                                if rows.is_empty() {
                                    note(tr!("repo.clone.no_match"), muted)
                                } else {
                                    div().child(
                                        v_flex()
                                            .id("clone-repo-list")
                                            .h(px(220.))
                                            .overflow_y_scroll()
                                            .children(rows),
                                    )
                                }
                            }
                        };
                        let (ws_pick, repo_pick) = (ws_c.clone(), repo_c.clone());
                        let (ws_refresh, login_refresh) = (ws_c.clone(), login.clone());
                        let loading = ws_c.read(cx).repos_loading.contains(&login);
                        v_flex()
                            .gap_3()
                            .when(logins.len() > 1, |v| {
                                v.child(account_picker(
                                    logins.clone(),
                                    account_c.clone(),
                                    "clone-account",
                                    false,
                                    move |login, cx| {
                                        *repo_pick.borrow_mut() = None;
                                        if let Some(login) = login {
                                            ws_pick.update(cx, |ws, cx| {
                                                ws.load_account_repos(login, false, cx)
                                            });
                                        }
                                    },
                                ))
                            })
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .rounded(px(6.))
                                            .border_1()
                                            .border_color(border)
                                            .child(
                                                Input::new(&filter_c).prefix(
                                                    Icon::new(IconName::Search)
                                                        .size_4()
                                                        .text_color(muted),
                                                ),
                                            ),
                                    )
                                    .child(
                                        crate::ui::button("clone-refresh")
                                            .outline()
                                            .icon(Icon::empty().path(ICON_REFRESH).size_4())
                                            .tooltip(tr!("repo.clone.refresh"))
                                            .loading(loading)
                                            .disabled(loading)
                                            .on_click(move |_, _, cx| {
                                                let login = login_refresh.clone();
                                                ws_refresh.update(cx, |ws, cx| {
                                                    ws.load_account_repos(login, true, cx)
                                                });
                                            }),
                                    ),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(tr!("repo.clone.your_repos")),
                            )
                            .child(list)
                    } else {
                        v_flex().child(field(tr!("repo.clone.url_label"), boxed(&url_c, border)))
                    };

                    content.child(
                        v_flex()
                            .gap_4()
                            .py_1()
                            .children(tabs)
                            .child(body)
                            .child(path_field(&path_c, "clone-choose", border)),
                    )
                })
                .footer(footer(
                    crate::ui::button("clone-confirm")
                        .label(tr!("repo.clone.action"))
                        .on_click(move |_, window, cx| {
                            let parent = text_of(&path_f, cx);
                            let login = account_f.borrow().clone();
                            let github = on_github_f.get() && login.is_some();
                            let url = if github {
                                // Listed repositories clone as the account that listed them.
                                repo_f.borrow().as_ref().zip(login).map(|(name, login)| {
                                    let url = format!("https://github.com/{name}.git");
                                    external::with_account(&url, &login)
                                })
                            } else {
                                Some(text_of(&url_f, cx)).filter(|u| !u.is_empty())
                            };
                            // Nothing picked yet (or no folder): leave the dialog up.
                            let (Some(url), false) = (url, parent.is_empty()) else {
                                return;
                            };
                            window.close_dialog(cx);
                            ws_f.update(cx, |ws, cx| ws.clone_into(url, PathBuf::from(parent), cx));
                        }),
                ))
        });
    }

    /// Add → Create New Repository…
    ///
    /// Name, Local Path, Description, README, Git Ignore and License as in GitHub Desktop,
    /// plus one thing it leaves for later: optionally creating the repository on GitHub
    /// under a connected account and wiring it up as `origin`.
    pub fn open_create_repo_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_repo_switcher(cx);
        if window.has_active_dialog(cx) {
            return;
        }
        let name = cx
            .new(|cx| InputState::new(window, cx).placeholder(tr!("repo.create.name_placeholder")));
        let description = cx.new(|cx| InputState::new(window, cx));
        let parent = self.default_parent();
        let path = cx.new(|cx| InputState::new(window, cx).default_value(parent));
        let readme = Rc::new(Cell::new(false));
        let private = Rc::new(Cell::new(true));
        let account: Picked = Rc::default();

        let templates = self.templates.clone().unwrap_or_default();
        let select =
            |items: SearchableVec<SharedString>, window: &mut Window, cx: &mut Context<Self>| {
                cx.new(|cx| {
                    SelectState::new(items, Some(IndexPath::default()), window, cx).searchable(true)
                })
            };
        let gitignore = select(with_none(templates.gitignores), window, cx);
        let license = select(
            with_none(templates.licenses.into_iter().map(|(_, name)| name)),
            window,
            cx,
        );
        // First time: the template lists come from GitHub; fill the selects when they land.
        if self.templates.is_none() {
            let task = bridge::templates(cx);
            // Weak: a dialog closed before the lists arrive shouldn't be kept alive by them.
            let (gitignore, license) = (gitignore.downgrade(), license.downgrade());
            cx.spawn_in(window, async move |ws, cx| {
                let Ok(templates) = task.await else { return };
                let _ = ws.update_in(cx, |ws, window, cx| {
                    // Cache first: worth keeping even if this dialog is already gone.
                    ws.templates = Some(templates.clone());
                    let (Some(gitignore), Some(license)) = (gitignore.upgrade(), license.upgrade())
                    else {
                        return;
                    };
                    let first = Some(IndexPath::default());
                    gitignore.update(cx, |s, cx| {
                        s.set_items(with_none(templates.gitignores.clone()), window, cx);
                        s.set_selected_index(first, window, cx);
                    });
                    license.update(cx, |s, cx| {
                        let names = templates.licenses.iter().map(|(_, name)| name.clone());
                        s.set_items(with_none(names), window, cx);
                        s.set_selected_index(first, window, cx);
                    });
                });
            })
            .detach();
        }
        let form = CreateForm {
            name: name.clone(),
            description: description.clone(),
            path: path.clone(),
            readme: readme.clone(),
            private: private.clone(),
            account: account.clone(),
            gitignore: gitignore.clone(),
            license: license.clone(),
        };
        self.create_form = Some(form.clone());
        let workspace = cx.entity();

        window.open_dialog(cx, move |dialog, _, _| {
            let (ws_c, name_c, description_c, path_c) = (
                workspace.clone(),
                name.clone(),
                description.clone(),
                path.clone(),
            );
            let (readme_c, private_c, account_c) =
                (readme.clone(), private.clone(), account.clone());
            let (gitignore_c, license_c) = (gitignore.clone(), license.clone());
            let (ws_f, form) = (workspace.clone(), form.clone());
            dialog
                .title(tr!("repo.create.title"))
                .w(px(DIALOG_WIDTH))
                .content(move |content, _, cx| {
                    let border = cx.theme().border;
                    let muted = cx.theme().muted_foreground;
                    let logins = ws_c.read(cx).account_logins();
                    let publishing = account_c.borrow().is_some();
                    let (readme_set, private_set) = (readme_c.clone(), private_c.clone());
                    content.child(
                        v_flex()
                            .gap_4()
                            .py_1()
                            .child(field(tr!("repo.create.name"), boxed(&name_c, border)))
                            .child(path_field(&path_c, "create-choose", border))
                            .child(field(
                                tr!("repo.create.description"),
                                boxed(&description_c, border),
                            ))
                            .child(
                                div().debug_selector(|| "create-readme".into()).child(
                                    Checkbox::new("create-readme")
                                        .label(tr!("repo.create.readme"))
                                        .checked(readme_c.get())
                                        .on_click(move |checked, window, _| {
                                            readme_set.set(*checked);
                                            window.refresh();
                                        }),
                                ),
                            )
                            .child(field(
                                tr!("repo.create.gitignore"),
                                Select::new(&gitignore_c).menu_max_h(px(260.)),
                            ))
                            .child(field(
                                tr!("repo.create.license"),
                                Select::new(&license_c).menu_max_h(px(260.)),
                            ))
                            .when(!logins.is_empty(), |form| {
                                form.child(
                                    field(
                                        tr!("repo.create.account"),
                                        account_picker(
                                            logins,
                                            account_c.clone(),
                                            "create-account",
                                            true,
                                            |_, _| {},
                                        ),
                                    )
                                    .child(if publishing {
                                        Checkbox::new("create-private")
                                            .label(tr!("repo.create.private"))
                                            .checked(private_c.get())
                                            .on_click(move |checked, window, _| {
                                                private_set.set(*checked);
                                                window.refresh();
                                            })
                                            .into_any_element()
                                    } else {
                                        div()
                                            .text_xs()
                                            .text_color(muted)
                                            .child(tr!("repo.create.local_only"))
                                            .into_any_element()
                                    }),
                                )
                            }),
                    )
                })
                .footer(footer(
                    crate::ui::button("create-repo-confirm")
                        .label(tr!("repo.create.action"))
                        .on_click(move |_, window, cx| {
                            let templates = ws_f.read(cx).templates.clone();
                            // Name or folder still empty: leave the dialog up.
                            let Some(options) = form.options(templates.as_ref(), cx) else {
                                return;
                            };
                            window.close_dialog(cx);
                            ws_f.update(cx, |ws, cx| ws.create_repo_from(options, cx));
                        }),
                ))
        });
    }

    /// Add → Add Existing Repository…: a path box + Choose…, then open whatever repository
    /// contains that folder (same `discover_root` step as the rail's "open" button).
    pub fn add_existing_repo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_repo_switcher(cx);
        if window.has_active_dialog(cx) {
            return;
        }
        let parent = self.default_parent();
        let path = cx.new(|cx| InputState::new(window, cx).default_value(parent));
        let workspace = cx.entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let (path_c, path_f, ws_f) = (path.clone(), path.clone(), workspace.clone());
            dialog
                .title(tr!("repo.add.title"))
                .w(px(DIALOG_WIDTH))
                .content(move |content, _, cx| {
                    content.child(path_field(&path_c, "add-choose", cx.theme().border))
                })
                .footer(footer(
                    crate::ui::button("add-repo-confirm")
                        .label(tr!("repo.add.action"))
                        .on_click(move |_, window, cx| {
                            let path = text_of(&path_f, cx);
                            if path.is_empty() {
                                return;
                            }
                            window.close_dialog(cx);
                            ws_f.update(cx, |ws, cx| ws.open_repo_at(PathBuf::from(path), cx));
                        }),
                ))
        });
    }
}

impl Workspace {
    /// "Create a Branch": a name, and a line saying what it will be based on (the checked
    /// out branch, or a commit when opened from the history's context menu). Creating
    /// switches to the new branch.
    pub fn open_new_branch_dialog(
        &mut self,
        from: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_branch_popover(cx);
        if window.has_active_dialog(cx) {
            return;
        }
        let based_on = match &from {
            Some(sha) => tr!(
                "repo.branch.based_on_commit",
                sha = sha[..7.min(sha.len())].to_string()
            ),
            None => {
                let current = self.active_repo().and_then(|r| r.current_branch());
                let name = current.map_or_else(|| "HEAD".to_string(), |b| b.name.clone());
                tr!("repo.branch.based_on_branch", name = name)
            }
        };
        let name = cx.new(|cx| InputState::new(window, cx));
        name.update(cx, |input, cx| input.focus(window, cx));
        let workspace = cx.entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let (name_c, name_f, ws_f) = (name.clone(), name.clone(), workspace.clone());
            let (based_on, from) = (based_on.clone(), from.clone());
            dialog
                .title(tr!("repo.branch.create_title"))
                .w(px(DIALOG_WIDTH))
                .content(move |content, _, cx| {
                    content.child(
                        v_flex()
                            .gap_3()
                            .py_1()
                            .child(field(
                                tr!("repo.branch.name"),
                                boxed(&name_c, cx.theme().border),
                            ))
                            .child(div().text_sm().child(based_on.clone())),
                    )
                })
                .footer(footer(
                    crate::ui::button("new-branch-confirm")
                        .label(tr!("repo.branch.create_action"))
                        .on_click(move |_, window, cx| {
                            // Spaces can't be in a branch name; GitHub Desktop turns them
                            // into dashes too.
                            let name = text_of(&name_f, cx).replace(char::is_whitespace, "-");
                            if name.is_empty() {
                                return;
                            }
                            window.close_dialog(cx);
                            let from = from.clone();
                            ws_f.update(cx, |ws, cx| ws.create_branch(name, from, cx));
                        }),
                ))
        });
    }

    /// "Switch Branch" with uncommitted changes: leave them (stashed) on the branch being
    /// left, or bring them along.
    pub fn open_switch_branch_dialog(
        &mut self,
        target: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.has_active_dialog(cx) {
            return;
        }
        let current = self
            .active_repo()
            .and_then(|r| r.current_branch())
            .map_or_else(|| "HEAD".to_string(), |b| b.name.clone());
        let leave = Rc::new(Cell::new(true));
        let workspace = cx.entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let (leave_c, leave_f, ws_f) = (leave.clone(), leave.clone(), workspace.clone());
            let (current, target_c, target_f) = (current.clone(), target.clone(), target.clone());
            dialog
                .title(tr!("repo.branch.switch_title"))
                .w(px(DIALOG_WIDTH))
                .content(move |content, _, cx| {
                    let theme = cx.theme();
                    let (border, muted, active, primary) = (
                        theme.border,
                        theme.muted_foreground,
                        theme.list_active,
                        theme.primary,
                    );
                    let choice =
                        |id: &'static str, value: bool, title: SharedString, desc: SharedString| {
                            let picked = leave_c.get() == value;
                            let leave_c = leave_c.clone();
                            h_flex()
                                .id(id)
                                .debug_selector(move || id.into())
                                .gap_3()
                                .p_3()
                                .items_start()
                                .cursor_pointer()
                                .when(picked, |row| row.bg(active))
                                .child(
                                    // The radio dot.
                                    div()
                                        .mt_0p5()
                                        .size_4()
                                        .flex_none()
                                        .rounded_full()
                                        .border_1()
                                        .border_color(if picked { primary } else { muted })
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .when(picked, |dot| {
                                            dot.child(div().size_2().rounded_full().bg(primary))
                                        }),
                                )
                                .child(
                                    v_flex()
                                        .min_w_0()
                                        .gap_0p5()
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .child(title),
                                        )
                                        .child(div().text_sm().text_color(muted).child(desc)),
                                )
                                .on_click(move |_, window, _| {
                                    leave_c.set(value);
                                    window.refresh();
                                })
                        };
                    content.child(
                        v_flex()
                            .gap_3()
                            .py_1()
                            .child(div().text_sm().child(tr!("repo.branch.switch_question")))
                            .child(
                                v_flex()
                                    .rounded(px(6.))
                                    .border_1()
                                    .border_color(border)
                                    .overflow_hidden()
                                    .child(choice(
                                        "switch-leave",
                                        true,
                                        tr!("repo.branch.leave_title", name = current.clone()),
                                        tr!("repo.branch.leave_desc"),
                                    ))
                                    .child(div().h(px(1.)).bg(border))
                                    .child(choice(
                                        "switch-bring",
                                        false,
                                        tr!("repo.branch.bring_title", name = target_c.clone()),
                                        tr!("repo.branch.bring_desc"),
                                    )),
                            ),
                    )
                })
                .footer(footer(
                    crate::ui::button("switch-branch-confirm")
                        .label(tr!("repo.branch.switch_action"))
                        .on_click(move |_, window, cx| {
                            window.close_dialog(cx);
                            let (target, leave) = (target_f.clone(), leave_f.get());
                            ws_f.update(cx, |ws, cx| ws.switch_branch(target, leave, cx));
                        }),
                ))
        });
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::AppContext as _;
    use gpui_kit::component::{Root, WindowExt as _};

    use crate::state::workspace::Workspace;

    /// Each of the three "Add" entries must actually put a dialog on screen, and that
    /// dialog must survive being drawn (its content closures read the workspace).
    #[gpui_kit::test]
    fn every_add_entry_opens_a_dialog_that_renders(cx: &mut gpui_kit::TestAppContext) {
        // git / gh work finishes on tokio's threads, outside the test scheduler.
        cx.executor().allow_parking();
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::bridge::init(cx);
        });
        let mut workspace = None;
        let window = cx.add_window(|window, cx| {
            let ws = cx.new(|cx| Workspace::new(window, cx));
            workspace = Some(ws.clone());
            Root::new(ws, window, cx)
        });
        let workspace = workspace.unwrap();
        type Open = fn(&mut Workspace, &mut gpui_kit::Window, &mut gpui_kit::Context<Workspace>);
        let entries: [Open; 3] = [
            Workspace::open_clone_dialog,
            Workspace::open_create_repo_dialog,
            Workspace::add_existing_repo,
        ];
        for open in entries {
            // Through the untyped handle: the typed one would hold `Root` for the whole
            // closure, and asking about dialogs reads `Root`.
            cx.update_window(window.into(), |_, window, cx| {
                workspace.update(cx, |ws, cx| open(ws, window, cx));
                assert!(window.has_active_dialog(cx), "no dialog was opened");
                let _ = window.draw(cx);
                window.close_dialog(cx);
                assert!(!window.has_active_dialog(cx));
            })
            .unwrap();
        }
    }

    /// The real path a user takes: open the repository switcher, click an "Add" entry.
    /// This is the one that silently did nothing while the entries were a dropdown menu
    /// nested in the switcher popover.
    #[gpui_kit::test]
    fn clicking_an_add_entry_in_the_switcher_opens_its_dialog(cx: &mut gpui_kit::TestAppContext) {
        // git / gh work finishes on tokio's threads, outside the test scheduler.
        cx.executor().allow_parking();
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::bridge::init(cx);
        });
        let mut workspace = None;
        let window = cx.add_window(|window, cx| {
            let ws = cx.new(|cx| Workspace::new(window, cx));
            workspace = Some(ws.clone());
            Root::new(ws, window, cx)
        });
        let workspace = workspace.unwrap();
        let cx = &mut gpui_kit::VisualTestContext::from_window(window.into(), cx);
        // With a repository open, the entries live inside the switcher popover.
        workspace.update(cx, |ws, cx| {
            ws.open_repo_path("/tmp/anano-switcher-test".into(), cx)
        });
        cx.run_until_parked();

        for entry in ["repo-add-clone", "repo-add-create", "repo-add-existing"] {
            let trigger = cx
                .debug_bounds("repo-switcher")
                .expect("switcher is on screen");
            cx.simulate_click(trigger.center(), gpui_kit::Modifiers::none());
            cx.run_until_parked();
            let button = cx
                .debug_bounds(entry)
                .unwrap_or_else(|| panic!("{entry} isn't in the open switcher"));
            cx.simulate_click(button.center(), gpui_kit::Modifiers::none());
            cx.run_until_parked();
            cx.update(|window, cx| {
                assert!(window.has_active_dialog(cx), "{entry} opened no dialog");
                assert!(!workspace.read(cx).switcher_open, "switcher stayed open");
                window.close_dialog(cx);
            });
            cx.run_until_parked();
        }
    }

    /// The create dialog's controls must reach `CreateOptions`: a ticked README box and a
    /// picked Git Ignore / License are what the user asked for, so losing them is a bug.
    #[gpui_kit::test]
    fn create_dialog_answers_reach_the_create_options(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::component::IndexPath;
        cx.executor().allow_parking();
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::bridge::init(cx);
        });
        let mut workspace = None;
        let window = cx.add_window(|window, cx| {
            let ws = cx.new(|cx| Workspace::new(window, cx));
            workspace = Some(ws.clone());
            Root::new(ws, window, cx)
        });
        let workspace = workspace.unwrap();
        let cx = &mut gpui_kit::VisualTestContext::from_window(window.into(), cx);
        let templates = anano_core::external::Templates {
            gitignores: vec!["Node".into(), "Rust".into()],
            licenses: vec![("mit".into(), "MIT License".into())],
        };
        let form = cx.update(|window, cx| {
            workspace.update(cx, |ws, cx| {
                ws.templates = Some(templates.clone());
                ws.open_create_repo_dialog(window, cx);
                ws.create_form.clone().unwrap()
            })
        });
        cx.run_until_parked();
        assert!(
            cx.update(|_, cx| form.options(Some(&templates), cx))
                .is_none(),
            "no name yet"
        );

        cx.update(|window, cx| {
            form.name
                .update(cx, |i, cx| i.set_value("demo", window, cx));
            // Row 0 is "None"; row 2 is the second gitignore, row 1 the first license.
            form.gitignore.update(cx, |s, cx| {
                s.set_selected_index(Some(IndexPath::default().row(2)), window, cx)
            });
            form.license.update(cx, |s, cx| {
                s.set_selected_index(Some(IndexPath::default().row(1)), window, cx)
            });
        });
        let readme = cx
            .debug_bounds("create-readme")
            .expect("README checkbox is on screen");
        cx.simulate_click(
            gpui_kit::point(readme.left() + gpui_kit::px(8.), readme.center().y),
            gpui_kit::Modifiers::none(),
        );
        cx.run_until_parked();

        let options = cx
            .update(|_, cx| form.options(Some(&templates), cx))
            .unwrap();
        assert_eq!(options.name, "demo");
        assert!(options.readme, "ticking the README box was lost");
        assert_eq!(options.gitignore.as_deref(), Some("Rust"));
        assert_eq!(options.license.as_deref(), Some("mit"));
        assert!(options.github.is_none());
    }
}
