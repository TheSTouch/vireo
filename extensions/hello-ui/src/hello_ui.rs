//! A sample Vireo addon that exercises every addon UI API: commands, a status
//! bar item, a tree view panel with a context menu, a custom view in a panel
//! and in an editor tab, editor decorations, and workspace services.

use zed_extension_api::{
    self as zed, Result,
    addon::{
        editor::{self, Decoration, Position, Severity, TextRange},
        ui::{self, Element, TreeItem, ViewEvent, ViewTree},
        workspace::{self, NotificationLevel, QuickPickItem},
    },
};

const DASHBOARD_VIEW: &str = "dashboard";
const FILES_VIEW: &str = "files";
const DECORATIONS: &str = "hello";

struct HelloUi {
    greeting: String,
    shout: bool,
    watch_id: Option<u64>,
    changed_files: usize,
}

impl HelloUi {
    fn hello_count() -> u64 {
        workspace::storage_get("hello-count")
            .and_then(|count| count.parse().ok())
            .unwrap_or(0)
    }

    fn update_status_item(&self) {
        ui::set_status_item(
            "hello",
            &format!("Hello UI: {}", Self::hello_count()),
            Some("Say hello"),
            Some("say-hello"),
        );
    }

    fn message(&self) -> String {
        if self.shout {
            self.greeting.to_uppercase()
        } else {
            self.greeting.clone()
        }
    }

    fn say_hello(&mut self) -> Result<()> {
        let count = Self::hello_count() + 1;
        workspace::storage_set("hello-count", Some(&count.to_string()))?;
        workspace::show_notification(
            NotificationLevel::Info,
            &format!("{} (said {count} times)", self.message()),
        );
        self.update_status_item();
        ui::refresh_view(DASHBOARD_VIEW);
        Ok(())
    }

    fn pick_greeting(&mut self) -> Result<()> {
        let items = ["Hello", "Bonjour", "Hola", "Hallo"]
            .into_iter()
            .map(|greeting| QuickPickItem {
                id: greeting.to_string(),
                label: greeting.to_string(),
                description: None,
            })
            .collect::<Vec<_>>();
        let Some(picked) = workspace::quick_pick(&items, Some("Pick a greeting")) else {
            return Ok(());
        };
        let name = workspace::input_box("Who should we greet?", Some("world")).unwrap_or_default();
        self.greeting = format!("{picked}, {name}!");
        ui::refresh_view(DASHBOARD_VIEW);
        Ok(())
    }

    fn decorate_active_file(&mut self) -> Result<()> {
        let active = editor::get_active_editor().ok_or("no active editor")?;
        let path = active.path.ok_or("the active editor has no file")?;
        let line = active.selection.start.line;
        let whole_line = TextRange {
            start: Position { line, column: 0 },
            end: Position {
                line: line + 1,
                column: 0,
            },
        };
        editor::set_decorations(
            &path,
            DECORATIONS,
            &[
                Decoration::Highlight((whole_line, Some("#ffd70040".to_string()))),
                Decoration::Underline((
                    active.selection,
                    Severity::Warning,
                    format!("Selected: {}", active.selected_text),
                )),
                Decoration::InlayHint((
                    Position { line, column: 0 },
                    format!("{} ", self.message()),
                )),
                Decoration::Hover((whole_line, format!("**{}** from Hello UI", self.message()))),
                Decoration::GutterIcon((line, Severity::Info, "Hello UI was here".to_string())),
            ],
        );
        Ok(())
    }

    fn dashboard(&self) -> ViewTree {
        let folders = workspace::workspace_folders();
        let settings = workspace::get_addon_settings().unwrap_or_else(|| "none".to_string());
        Element::scroll(
            "root",
            [
                Element::label("greeting", self.message()),
                Element::row(
                    "actions",
                    [
                        Element::button("hello", "Say hello"),
                        Element::button("pick", "Pick greeting"),
                    ],
                ),
                Element::text_input(
                    "greeting-input",
                    self.greeting.clone(),
                    Some("Type a greeting".to_string()),
                ),
                Element::checkbox("shout", "Shout", self.shout),
                Element::divider("divider"),
                Element::label("count", format!("Said hello {} times", Self::hello_count())),
                Element::label(
                    "changes",
                    format!("{} file changes seen", self.changed_files),
                ),
                Element::label("settings", format!("Settings: {settings}")),
                Element::list(
                    "folders",
                    folders.into_iter().enumerate().map(|(index, folder)| {
                        Element::row(
                            format!("folder-{index}"),
                            [
                                Element::icon(format!("folder-icon-{index}"), "folder"),
                                Element::label(format!("folder-label-{index}"), folder),
                            ],
                        )
                    }),
                ),
            ],
        )
        .into_view_tree()
    }
}

impl zed::Extension for HelloUi {
    fn new() -> Self {
        let mut this = Self {
            greeting: "Hello, world!".to_string(),
            shout: false,
            watch_id: None,
            changed_files: 0,
        };
        if let Some(folder) = workspace::workspace_folders().into_iter().next() {
            this.watch_id = workspace::watch_path(&folder).ok();
        }
        this.update_status_item();
        this
    }

    fn run_command(&mut self, command_id: &str, args: Vec<String>) -> Result<()> {
        match command_id {
            "say-hello" => self.say_hello(),
            "pick-greeting" => self.pick_greeting(),
            "open-dashboard" => {
                ui::open_view_tab(DASHBOARD_VIEW, "Hello Dashboard");
                Ok(())
            }
            "decorate-active-file" => self.decorate_active_file(),
            "clear-decorations" => {
                editor::clear_decorations(DECORATIONS);
                Ok(())
            }
            "open-item" => {
                let path = args.first().ok_or("missing item")?;
                editor::open_file(path, Some(Position { line: 0, column: 0 }))
            }
            "show-item-path" => {
                let path = args.first().ok_or("missing item")?;
                workspace::show_notification(NotificationLevel::Info, path);
                Ok(())
            }
            _ => Err(format!("unknown command `{command_id}`")),
        }
    }

    fn tree_children(&mut self, view_id: &str, parent: Option<String>) -> Result<Vec<TreeItem>> {
        if view_id != FILES_VIEW {
            return Err(format!("unknown tree view `{view_id}`"));
        }
        let Some(parent) = parent else {
            return Ok(workspace::workspace_folders()
                .into_iter()
                .map(|folder| tree_item(&folder, true))
                .collect());
        };
        let mut entries = workspace::read_dir(&parent)?;
        entries.sort();
        Ok(entries
            .into_iter()
            .map(|entry| match entry.strip_suffix('/') {
                Some(directory) => tree_item(&format!("{parent}/{directory}"), true),
                None => tree_item(&format!("{parent}/{entry}"), false),
            })
            .collect())
    }

    fn render_view(&mut self, view_id: &str) -> Result<ViewTree> {
        match view_id {
            DASHBOARD_VIEW => Ok(self.dashboard()),
            _ => Err(format!("unknown view `{view_id}`")),
        }
    }

    fn handle_view_event(
        &mut self,
        view_id: &str,
        element_id: &str,
        event: ViewEvent,
    ) -> Result<()> {
        if view_id != DASHBOARD_VIEW {
            return Ok(());
        }
        match (element_id, event) {
            ("hello", ViewEvent::Click) => self.say_hello()?,
            ("pick", ViewEvent::Click) => self.pick_greeting()?,
            ("greeting-input", ViewEvent::Change(text) | ViewEvent::Submit(text)) => {
                self.greeting = text
            }
            ("shout", ViewEvent::Toggle(checked)) => self.shout = checked,
            _ => {}
        }
        Ok(())
    }

    fn files_changed(&mut self, watch_id: u64, paths: Vec<String>) {
        if Some(watch_id) == self.watch_id {
            self.changed_files += paths.len();
            ui::refresh_view(DASHBOARD_VIEW);
        }
    }
}

fn tree_item(path: &str, is_directory: bool) -> TreeItem {
    TreeItem {
        id: path.to_string(),
        label: path.rsplit(['/', '\\']).next().unwrap_or(path).to_string(),
        description: None,
        icon: Some(if is_directory { "folder" } else { "file" }.to_string()),
        collapsible: is_directory,
        command: (!is_directory).then(|| "open-item".to_string()),
        context_value: Some(if is_directory { "folder" } else { "file" }.to_string()),
    }
}

zed::register_extension!(HelloUi);
