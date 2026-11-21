//! The Develop Vireo page, opened from the activity bar: Vireo's sources on
//! this machine, the tools a build needs, and prepared Claude Code sessions
//! that change Vireo itself. Anyone who downloads Vireo can get its sources
//! here and ask, in the editor, for a change.

mod sessions;
mod sources;

use std::path::PathBuf;

use collections::HashMap;
use db::kvp::KeyValueStore;
use editor::Editor;
use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, PathPromptOptions, Render, Task,
    WeakEntity, Window,
};
use ui::prelude::*;
use util::{ResultExt as _, paths::PathExt as _};
use workspace::{
    OpenMode, OpenOptions, Workspace,
    item::{Item, ItemEvent},
};
use zed_actions::develop_vireo::Open;

pub use sessions::{PreparedSession, SESSIONS_DIR};
pub use sources::{SourcesState, default_sources_path};

const TITLE: &str = "Develop Vireo";
const SUBTITLE: &str = "Vireo is open source and built with Vireo. Get its sources, then ask Claude Code to change anything.";

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, _: &Open, window, cx| {
            open_page(workspace, window, cx);
        });
    })
    .detach();
}

/// Shows the page in the active pane, reusing the one already there.
pub fn open_page(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let existing = workspace
        .active_pane()
        .read(cx)
        .items()
        .find_map(|item| item.downcast::<DevelopVireoPage>());
    if let Some(existing) = existing {
        workspace.activate_item(&existing, true, true, window, cx);
    } else {
        let page = cx.new(|cx| DevelopVireoPage::new(workspace.weak_handle(), window, cx));
        workspace.add_item_to_active_pane(Box::new(page), None, true, window, cx);
    }
}

struct SessionCard {
    session: PreparedSession,
    input: Option<Entity<Editor>>,
    error: Option<SharedString>,
}

pub struct DevelopVireoPage {
    workspace: WeakEntity<Workspace>,
    focus_handle: FocusHandle,
    sources_path: PathBuf,
    sources: SourcesState,
    prerequisites: Option<Vec<sources::Prerequisite>>,
    cards: Vec<SessionCard>,
    _refresh: Option<Task<()>>,
    _download: Option<Task<()>>,
    _choose_folder: Option<Task<()>>,
    _prerequisites: Option<Task<()>>,
}

impl DevelopVireoPage {
    pub fn new(
        workspace: WeakEntity<Workspace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let sources_path = KeyValueStore::global(cx)
            .read_kvp(sources::SOURCES_PATH_KEY)
            .log_err()
            .flatten()
            .map(PathBuf::from)
            .unwrap_or_else(default_sources_path);
        let mut this = Self {
            workspace,
            focus_handle: cx.focus_handle(),
            sources_path,
            sources: SourcesState::Checking,
            prerequisites: None,
            cards: Vec::new(),
            _refresh: None,
            _download: None,
            _choose_folder: None,
            _prerequisites: None,
        };
        this.set_sessions(sessions::built_in(), window, cx);
        // Tests must not run git or cargo from the machine they run on.
        if !cx.is_test() {
            this.refresh(window, cx);
            let check = cx.background_spawn(sources::check_prerequisites());
            this._prerequisites = Some(cx.spawn(async move |this, cx| {
                let prerequisites = check.await;
                this.update(cx, |this, cx| {
                    this.prerequisites = Some(prerequisites);
                    cx.notify();
                })
                .ok();
            }));
        }
        this
    }

    pub fn sources_path(&self) -> &PathBuf {
        &self.sources_path
    }

    pub fn session_titles(&self) -> Vec<String> {
        self.cards
            .iter()
            .map(|card| card.session.title.clone())
            .collect()
    }

    /// Reads the state of the sources, and the prepared sessions they hold.
    fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.sources_path.clone();
        self.sources = SourcesState::Checking;
        cx.notify();
        let check = cx.background_spawn(async move {
            let state = sources::check(&path).await;
            let sessions = state.is_present().then(|| sessions::load(&path));
            (state, sessions)
        });
        self._refresh = Some(cx.spawn_in(window, async move |this, cx| {
            let (state, sessions) = check.await;
            this.update_in(cx, |this, window, cx| {
                this.sources = state;
                this.set_sessions(sessions.unwrap_or_else(sessions::built_in), window, cx);
                cx.notify();
            })
            .ok();
        }));
    }

    /// Replaces the session cards, keeping what was typed in a card whose
    /// session is still there.
    fn set_sessions(
        &mut self,
        sessions: Vec<PreparedSession>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut inputs: HashMap<String, Entity<Editor>> = self
            .cards
            .drain(..)
            .filter_map(|card| Some((card.session.file_name, card.input?)))
            .collect();
        self.cards = sessions
            .into_iter()
            .map(|session| {
                let input = session.input.as_ref().map(|placeholder| {
                    let input = inputs
                        .remove(&session.file_name)
                        .unwrap_or_else(|| cx.new(|cx| Editor::single_line(window, cx)));
                    input.update(cx, |input, cx| {
                        input.set_placeholder_text(placeholder, window, cx)
                    });
                    input
                });
                SessionCard {
                    session,
                    input,
                    error: None,
                }
            })
            .collect();
    }

    fn download(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.sources_path.clone();
        self.sources = SourcesState::Downloading;
        cx.notify();
        let download = cx.background_spawn(async move { sources::download(&path).await });
        self._download = Some(cx.spawn_in(window, async move |this, cx| {
            let result = download.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(()) => this.refresh(window, cx),
                Err(error) => {
                    this.sources = SourcesState::Failed(format!("Download failed: {error:#}"));
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    fn choose_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Use This Folder".into()),
        });
        self._choose_folder = Some(cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                this.set_sources_path(path, window, cx)
            })
            .ok();
        }));
    }

    fn set_sources_path(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let value = path.to_string_lossy().into_owned();
        self.sources_path = path;
        let store = KeyValueStore::global(cx);
        cx.background_spawn(async move {
            store
                .write_kvp(sources::SOURCES_PATH_KEY.to_string(), value)
                .await
                .log_err();
        })
        .detach();
        self.refresh(window, cx);
    }

    fn start_session(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(card) = self.cards.get_mut(index) else {
            return;
        };
        let typed = card
            .input
            .as_ref()
            .map(|input| input.read(cx).text(cx))
            .unwrap_or_default();
        if card.session.takes_input() && typed.trim().is_empty() {
            card.error = Some("Describe it first: Claude starts from what you write.".into());
            cx.notify();
            return;
        }
        card.error = None;
        let prompt = card.session.prompt_for(&typed);
        if let Some(input) = &card.input {
            input.update(cx, |input, cx| input.set_text("", window, cx));
        }
        self.open_sources(Some(prompt), cx);
    }

    /// Opens the sources in a window of their own (or the one already
    /// showing them) and, with a prompt, a new Claude Code conversation there
    /// with the prompt in its message box, for the person to read and send.
    fn open_sources(&mut self, prompt: Option<String>, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        let app_state = workspace.read(cx).app_state().clone();
        let open = workspace::open_paths(
            std::slice::from_ref(&self.sources_path),
            app_state,
            OpenOptions {
                open_mode: OpenMode::NewWindow,
                ..OpenOptions::default()
            },
            cx,
        );
        cx.spawn(async move |_, cx| {
            let opened = open.await?;
            let Some(prompt) = prompt else {
                return anyhow::Ok(());
            };
            let workspace = opened.workspace.clone();
            let panels = opened.window.update(cx, |_, _, cx| {
                workspace.update(cx, |workspace, _| workspace.take_panels_task())
            })?;
            if let Some(panels) = panels {
                panels.await.log_err();
            }
            opened.window.update(cx, |_, window, cx| {
                workspace.update(cx, |workspace, cx| {
                    claude_code_ui::open_with_prompt(workspace, prompt, window, cx);
                });
            })?;
            anyhow::Ok(())
        })
        .detach_and_log_err(cx);
    }

    fn render_sources(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let path = self.sources_path.compact().to_string_lossy().into_owned();
        let (status, color) = match &self.sources {
            SourcesState::Checking => ("Checking…".to_string(), Color::Muted),
            SourcesState::Missing => ("Not downloaded yet.".to_string(), Color::Muted),
            SourcesState::NotACheckout => (
                "This folder is not a git checkout of Vireo.".to_string(),
                Color::Warning,
            ),
            SourcesState::Present(checkout) => {
                let mut status = format!("{} at {}", checkout.branch, checkout.commit);
                match checkout.local_changes {
                    0 => {}
                    1 => status.push_str(" · 1 file changed"),
                    count => status.push_str(&format!(" · {count} files changed")),
                }
                (status, Color::Success)
            }
            SourcesState::Downloading => (
                "Downloading Vireo's sources… this takes a few minutes.".to_string(),
                Color::Accent,
            ),
            SourcesState::Failed(error) => (error.clone(), Color::Error),
        };
        let can_download = matches!(
            self.sources,
            SourcesState::Missing | SourcesState::Failed(_)
        );
        v_flex()
            .gap_3()
            .child(section_title("Sources"))
            .child(
                v_flex()
                    .gap_0p5()
                    .child(Label::new(path).color(Color::Muted).truncate())
                    .child(Label::new(status).color(color)),
            )
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .when(can_download, |this| {
                        this.child(
                            Button::new("download-sources", "Download Sources")
                                .style(ButtonStyle::Filled)
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.download(window, cx)),
                                ),
                        )
                    })
                    .when(self.sources.is_present(), |this| {
                        this.child(
                            Button::new("open-sources", "Open Sources")
                                .style(ButtonStyle::Filled)
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.open_sources(None, cx)),
                                ),
                        )
                    })
                    .when(self.sources != SourcesState::Downloading, |this| {
                        this.child(
                            Button::new("choose-folder", "Use Another Folder…").on_click(
                                cx.listener(|this, _, window, cx| this.choose_folder(window, cx)),
                            ),
                        )
                    }),
            )
    }

    fn render_prerequisites(&self) -> impl IntoElement {
        let rows = match &self.prerequisites {
            None => vec![
                Label::new("Checking…")
                    .color(Color::Muted)
                    .into_any_element(),
            ],
            Some(prerequisites) => prerequisites
                .iter()
                .map(|prerequisite| {
                    let (icon, color, detail) = match &prerequisite.found {
                        Some(version) => (IconName::Check, Color::Success, version.clone()),
                        None => (IconName::Close, Color::Error, "Not found".to_string()),
                    };
                    h_flex()
                        .gap_2()
                        .child(Icon::new(icon).size(IconSize::Small).color(color))
                        .child(Label::new(prerequisite.name))
                        .child(Label::new(detail).color(Color::Muted).truncate())
                        .into_any_element()
                })
                .collect(),
        };
        v_flex()
            .gap_3()
            .child(section_title("Build Tools"))
            .child(v_flex().gap_1().children(rows))
            .child(
                Button::new("build-docs", "How to install them")
                    .style(ButtonStyle::Transparent)
                    .color(Color::Accent)
                    .on_click(|_, _, cx| cx.open_url(sources::BUILD_DOCS_URL)),
            )
    }

    fn render_sessions(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let ready = self.sources.is_present();
        let colors = cx.theme().colors();
        v_flex()
            .gap_3()
            .child(section_title("Prepared Sessions"))
            .child(
                Label::new(if ready {
                    "Each opens Vireo's sources and a Claude Code conversation with its prompt, ready for you to read and send."
                } else {
                    "Download the sources first: these sessions work in them."
                })
                .color(Color::Muted),
            )
            .children(self.cards.iter().enumerate().map(|(index, card)| {
                v_flex()
                    .gap_2()
                    .p_3()
                    .rounded_md()
                    .border_1()
                    .border_color(colors.border_variant)
                    .bg(colors.surface_background)
                    .child(Label::new(card.session.title.clone()).weight(gpui::FontWeight::SEMIBOLD))
                    .child(
                        Label::new(card.session.description.clone())
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    )
                    .when_some(card.input.clone(), |this, input| {
                        this.child(
                            div()
                                .px_2()
                                .py_1()
                                .rounded_sm()
                                .border_1()
                                .border_color(colors.border)
                                .bg(colors.editor_background)
                                .child(input),
                        )
                    })
                    .when_some(card.error.clone(), |this, error| {
                        this.child(Label::new(error).size(LabelSize::Small).color(Color::Error))
                    })
                    .child(
                        h_flex().justify_end().child(
                            Button::new(("start-session", index), "Start in Claude Code")
                                .style(ButtonStyle::Filled)
                                .disabled(!ready)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.start_session(index, window, cx)
                                })),
                        ),
                    )
                    .into_any_element()
            }))
    }
}

fn section_title(title: &'static str) -> impl IntoElement {
    Label::new(title).size(LabelSize::Large)
}

impl Render for DevelopVireoPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .key_context("DevelopVireo")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(cx.theme().colors().editor_background)
            .justify_center()
            .child(
                v_flex()
                    .id("develop-vireo")
                    .px_8()
                    .py_12()
                    .max_w(px(1000.))
                    .size_full()
                    .gap_10()
                    .overflow_y_scroll()
                    .child(
                        v_flex()
                            .child(
                                Label::new(TITLE)
                                    .size(LabelSize::Custom(rems_from_px(36_f32)))
                                    .weight(gpui::FontWeight::LIGHT),
                            )
                            .child(
                                Label::new(SUBTITLE)
                                    .size(LabelSize::Large)
                                    .color(Color::Muted),
                            ),
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .items_start()
                            .gap_16()
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_8()
                                    .child(self.render_sources(cx))
                                    .child(self.render_prerequisites()),
                            )
                            .child(v_flex().flex_1().min_w_0().child(self.render_sessions(cx))),
                    ),
            )
    }
}

impl EventEmitter<ItemEvent> for DevelopVireoPage {}

impl Focusable for DevelopVireoPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Item for DevelopVireoPage {
    type Event = ItemEvent;

    fn tab_content_text(&self, _detail: usize, _cx: &App) -> SharedString {
        TITLE.into()
    }

    fn tab_icon(&self, _window: &Window, _cx: &App) -> Option<Icon> {
        Some(Icon::new(IconName::ActivityDevelop))
    }

    fn show_toolbar(&self) -> bool {
        false
    }

    fn to_item_events(event: &Self::Event, f: &mut dyn FnMut(ItemEvent)) {
        f(*event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fs::FakeFs;
    use gpui::TestAppContext;
    use project::Project;
    use settings::SettingsStore;

    fn init_test(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let settings_store = SettingsStore::test(cx);
            cx.set_global(settings_store);
            theme_settings::init(theme::LoadThemes::JustBase, cx);
            editor::init(cx);
            crate::init(cx);
        });
    }

    #[gpui::test]
    async fn open_shows_one_page_with_the_built_in_sessions(cx: &mut TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        for _ in 0..2 {
            workspace.update_in(cx, |workspace, window, cx| open_page(workspace, window, cx));
        }
        cx.run_until_parked();

        workspace.update(cx, |workspace, cx| {
            let pages = workspace
                .active_pane()
                .read(cx)
                .items()
                .filter_map(|item| item.downcast::<DevelopVireoPage>())
                .collect::<Vec<_>>();
            assert_eq!(pages.len(), 1, "Open reuses the page");
            let page = pages[0].read(cx);
            assert_eq!(page.sources_path(), &default_sources_path());
            assert_eq!(page.session_titles().len(), 4);
        });
    }
}
