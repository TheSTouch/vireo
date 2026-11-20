use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Result, bail};
use async_trait::async_trait;
use collections::HashMap;
use editor::{Editor, HighlightKey};
use extension::{AddonDecoration, AddonPosition, AddonSeverity, AddonTextRange};
use extension::{AddonElement, AddonElementKind, AddonTreeItem, AddonViewEvent, AddonViewTree};
use extension::{
    AddonStatusItem, BuildTaskTemplate, CodeLabel, Command, Completion, ContextServerConfiguration,
    DebugAdapterBinary, DebugRequest, DebugScenario, DebugTaskDefinition, Extension,
    ExtensionHostProxy, ExtensionManifest, KeyValueStoreDelegate, ProjectDelegate, SlashCommand,
    SlashCommandArgumentCompletion, SlashCommandOutput, StartDebuggingRequestArgumentsRequest,
    Symbol, WorktreeDelegate,
};
use gpui::{Entity, EntityId, TestAppContext, VisualTestContext};
use language::LanguageName;
use lsp::LanguageServerName;
use parking_lot::Mutex;
use project::Project;
use task::{SpawnInTerminal, ZedDebugConfig};
use util::path;
use workspace::Item as _;
use workspace::dock::{DockPosition, Panel as _};
use workspace::{AppState, MultiWorkspace};

use crate::*;

/// An addon that records the commands it is asked to run and answers with
/// canned view content.
pub(crate) struct FakeAddon {
    pub manifest: Arc<ExtensionManifest>,
    pub commands_run: Mutex<Vec<(Arc<str>, Vec<String>)>>,
    pub tree_children: Mutex<HashMap<Option<String>, Vec<AddonTreeItem>>>,
    pub view_events: Mutex<Vec<(String, AddonViewEvent)>>,
}

impl FakeAddon {
    pub fn new(manifest_toml: &str) -> Arc<Self> {
        Arc::new(Self {
            manifest: Arc::new(toml::from_str(manifest_toml).expect("manifest should parse")),
            commands_run: Mutex::default(),
            tree_children: Mutex::default(),
            view_events: Mutex::default(),
        })
    }

    pub fn set_tree_children(&self, parent: Option<&str>, children: Vec<AddonTreeItem>) {
        self.tree_children
            .lock()
            .insert(parent.map(ToString::to_string), children);
    }
}

#[async_trait]
impl Extension for FakeAddon {
    fn manifest(&self) -> Arc<ExtensionManifest> {
        self.manifest.clone()
    }

    fn work_dir(&self) -> Arc<Path> {
        PathBuf::from("/work").into()
    }

    async fn run_addon_command(&self, command_id: Arc<str>, args: Vec<String>) -> Result<()> {
        self.commands_run.lock().push((command_id, args));
        Ok(())
    }

    /// A counter view: a label showing how often the button was clicked, the
    /// button, a text input and a checkbox.
    async fn addon_render_view(&self, view_id: Arc<str>) -> Result<AddonViewTree> {
        let clicks = self
            .view_events
            .lock()
            .iter()
            .filter(|(_, event)| *event == AddonViewEvent::Click)
            .count();
        let element = |id: &str, kind, children: Vec<u32>| AddonElement {
            id: id.into(),
            kind,
            children,
        };
        Ok(AddonViewTree {
            elements: vec![
                element("root", AddonElementKind::Column, vec![1, 2, 3, 4, 5]),
                element(
                    "count",
                    AddonElementKind::Label(format!("{view_id}: {clicks} clicks")),
                    vec![],
                ),
                element("inc", AddonElementKind::Button("Click".into()), vec![]),
                element(
                    "name",
                    AddonElementKind::TextInput {
                        value: "world".into(),
                        placeholder: Some("Name".into()),
                    },
                    vec![],
                ),
                element(
                    "shout",
                    AddonElementKind::Checkbox {
                        label: "Shout".into(),
                        checked: false,
                    },
                    vec![],
                ),
                // A child that points back at the root must not loop forever.
                element("cycle", AddonElementKind::Row, vec![0]),
            ],
        })
    }

    async fn addon_handle_view_event(
        &self,
        _view_id: Arc<str>,
        element_id: String,
        event: AddonViewEvent,
    ) -> Result<()> {
        self.view_events.lock().push((element_id, event));
        Ok(())
    }

    async fn addon_tree_children(
        &self,
        _view_id: Arc<str>,
        parent: Option<String>,
    ) -> Result<Vec<AddonTreeItem>> {
        Ok(self
            .tree_children
            .lock()
            .get(&parent)
            .cloned()
            .unwrap_or_default())
    }

    async fn language_server_command(
        &self,
        _: LanguageServerName,
        _: LanguageName,
        _: Arc<dyn WorktreeDelegate>,
        _: EntityId,
    ) -> Result<Command> {
        bail!("unused")
    }

    async fn language_server_initialization_options(
        &self,
        _: LanguageServerName,
        _: LanguageName,
        _: Arc<dyn WorktreeDelegate>,
        _: EntityId,
    ) -> Result<Option<String>> {
        bail!("unused")
    }

    async fn language_server_workspace_configuration(
        &self,
        _: LanguageServerName,
        _: Arc<dyn WorktreeDelegate>,
        _: EntityId,
    ) -> Result<Option<String>> {
        bail!("unused")
    }

    async fn language_server_initialization_options_schema(
        &self,
        _: LanguageServerName,
        _: Arc<dyn WorktreeDelegate>,
        _: EntityId,
    ) -> Result<Option<String>> {
        bail!("unused")
    }

    async fn language_server_workspace_configuration_schema(
        &self,
        _: LanguageServerName,
        _: Arc<dyn WorktreeDelegate>,
        _: EntityId,
    ) -> Result<Option<String>> {
        bail!("unused")
    }

    async fn language_server_additional_initialization_options(
        &self,
        _: LanguageServerName,
        _: LanguageServerName,
        _: Arc<dyn WorktreeDelegate>,
        _: EntityId,
    ) -> Result<Option<String>> {
        bail!("unused")
    }

    async fn language_server_additional_workspace_configuration(
        &self,
        _: LanguageServerName,
        _: LanguageServerName,
        _: Arc<dyn WorktreeDelegate>,
        _: EntityId,
    ) -> Result<Option<String>> {
        bail!("unused")
    }

    async fn labels_for_completions(
        &self,
        _: LanguageServerName,
        _: Vec<Completion>,
    ) -> Result<Vec<Option<CodeLabel>>> {
        bail!("unused")
    }

    async fn labels_for_symbols(
        &self,
        _: LanguageServerName,
        _: Vec<Symbol>,
    ) -> Result<Vec<Option<CodeLabel>>> {
        bail!("unused")
    }

    async fn complete_slash_command_argument(
        &self,
        _: SlashCommand,
        _: Vec<String>,
    ) -> Result<Vec<SlashCommandArgumentCompletion>> {
        bail!("unused")
    }

    async fn run_slash_command(
        &self,
        _: SlashCommand,
        _: Vec<String>,
        _: Option<Arc<dyn WorktreeDelegate>>,
    ) -> Result<SlashCommandOutput> {
        bail!("unused")
    }

    async fn context_server_command(
        &self,
        _: Arc<str>,
        _: Arc<dyn ProjectDelegate>,
    ) -> Result<Command> {
        bail!("unused")
    }

    async fn context_server_configuration(
        &self,
        _: Arc<str>,
        _: Arc<dyn ProjectDelegate>,
    ) -> Result<Option<ContextServerConfiguration>> {
        bail!("unused")
    }

    async fn suggest_docs_packages(&self, _: Arc<str>) -> Result<Vec<String>> {
        bail!("unused")
    }

    async fn index_docs(
        &self,
        _: Arc<str>,
        _: Arc<str>,
        _: Arc<dyn KeyValueStoreDelegate>,
    ) -> Result<()> {
        bail!("unused")
    }

    async fn get_dap_binary(
        &self,
        _: Arc<str>,
        _: DebugTaskDefinition,
        _: Option<PathBuf>,
        _: Arc<dyn WorktreeDelegate>,
    ) -> Result<DebugAdapterBinary> {
        bail!("unused")
    }

    async fn dap_request_kind(
        &self,
        _: Arc<str>,
        _: serde_json::Value,
    ) -> Result<StartDebuggingRequestArgumentsRequest> {
        bail!("unused")
    }

    async fn dap_config_to_scenario(&self, _: ZedDebugConfig) -> Result<DebugScenario> {
        bail!("unused")
    }

    async fn dap_locator_create_scenario(
        &self,
        _: String,
        _: BuildTaskTemplate,
        _: String,
        _: String,
    ) -> Result<Option<DebugScenario>> {
        bail!("unused")
    }

    async fn run_dap_locator(&self, _: String, _: SpawnInTerminal) -> Result<DebugRequest> {
        bail!("unused")
    }
}

pub(crate) const HELLO_MANIFEST: &str = r#"
id = "hello-ui"
name = "Hello UI"
version = "0.1.0"
schema_version = 1

[commands.say-hello]
title = "Say Hello"
"#;

pub(crate) fn init_test(cx: &mut TestAppContext) -> (Arc<AppState>, Arc<ExtensionHostProxy>) {
    cx.update(|cx| {
        let app_state = AppState::test(cx);
        theme_settings::init(theme::LoadThemes::JustBase, cx);
        command_palette_hooks::init(cx);
        workspace::init(app_state.clone(), cx);
        editor::init(cx);
        let proxy = Arc::new(ExtensionHostProxy::new());
        crate::init(proxy.clone(), cx);
        (app_state, proxy)
    })
}

#[gpui::test]
async fn test_addon_commands_appear_in_the_palette(cx: &mut TestAppContext) {
    let (_, proxy) = init_test(cx);
    let addon = FakeAddon::new(HELLO_MANIFEST);
    cx.update(|cx| proxy.register_addon(addon.clone(), cx));

    let commands = cx.update(|cx| GlobalCommandPaletteExtraCommands::commands(cx));
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].name, "Hello UI: Say Hello");
    assert!(commands[0].action.partial_eq(&RunCommand {
        extension: "hello-ui".into(),
        command: "say-hello".into(),
        args: Vec::new(),
    }));

    cx.update(|cx| proxy.unregister_addon("hello-ui".into(), cx));
    let commands = cx.update(|cx| GlobalCommandPaletteExtraCommands::commands(cx));
    assert!(commands.is_empty());
}

#[gpui::test]
async fn test_run_command_action_runs_the_addon_command(cx: &mut TestAppContext) {
    let (app_state, proxy) = init_test(cx);
    let addon = FakeAddon::new(HELLO_MANIFEST);
    cx.update(|cx| proxy.register_addon(addon.clone(), cx));

    let project = Project::test(app_state.fs.clone(), [], cx).await;
    let (_multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));

    let action = cx.update(|_, cx| {
        cx.build_action(
            "addon::RunCommand",
            Some(serde_json::json!({
                "extension": "hello-ui",
                "command": "say-hello",
                "args": ["from-keymap"],
            })),
        )
    });
    let action = action.expect("keymap-style action should build");
    cx.update(|window, cx| window.dispatch_action(action, cx));
    cx.run_until_parked();

    assert_eq!(
        addon.commands_run.lock().as_slice(),
        &[("say-hello".into(), vec!["from-keymap".to_string()])]
    );
}

#[gpui::test]
async fn test_unknown_addon_command_fails(cx: &mut TestAppContext) {
    let (_, proxy) = init_test(cx);
    let addon = FakeAddon::new(HELLO_MANIFEST);
    cx.update(|cx| proxy.register_addon(addon.clone(), cx));

    let registry = cx.update(|cx| AddonRegistry::global(cx).expect("registry is initialized"));
    let undeclared = cx
        .update(|cx| {
            registry
                .read(cx)
                .run_command("hello-ui", "not-declared", Vec::new(), cx)
        })
        .await;
    assert!(undeclared.is_err());
    let unknown_addon = cx
        .update(|cx| {
            registry
                .read(cx)
                .run_command("missing", "say-hello", Vec::new(), cx)
        })
        .await;
    assert!(unknown_addon.is_err());
    assert!(addon.commands_run.lock().is_empty());
}

fn status_item(id: &str, text: &str) -> AddonStatusItem {
    AddonStatusItem {
        id: id.into(),
        text: text.into(),
        tooltip: Some("Say hello".into()),
        command: Some("say-hello".into()),
    }
}

#[gpui::test]
async fn test_status_items_show_in_the_status_bar(cx: &mut TestAppContext) {
    let (app_state, proxy) = init_test(cx);
    // Addons set their status items from `init`, before they are registered.
    cx.update(|cx| proxy.set_status_item("hello-ui".into(), status_item("hello", "Hello 0"), cx));
    let addon = FakeAddon::new(HELLO_MANIFEST);
    cx.update(|cx| proxy.register_addon(addon.clone(), cx));
    cx.update(|cx| proxy.set_status_item("hello-ui".into(), status_item("other", "Other"), cx));
    // Setting an existing item again updates it in place.
    cx.update(|cx| proxy.set_status_item("hello-ui".into(), status_item("hello", "Hello 1"), cx));

    let registry = cx.update(|cx| AddonRegistry::global(cx).expect("registry is initialized"));
    let texts = |cx: &mut TestAppContext| {
        cx.update(|cx| {
            registry
                .read(cx)
                .status_items()
                .into_iter()
                .map(|(extension_id, item)| format!("{extension_id}/{}: {}", item.id, item.text))
                .collect::<Vec<_>>()
        })
    };
    assert_eq!(
        texts(cx),
        ["hello-ui/hello: Hello 1", "hello-ui/other: Other"]
    );

    let project = Project::test(app_state.fs.clone(), [], cx).await;
    let (multi_workspace, window_cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));
    let workspace = multi_workspace.read_with(window_cx, |mw, _| mw.workspace().clone());
    let status_items = workspace.read_with(window_cx, |workspace, cx| {
        workspace
            .status_bar()
            .read(cx)
            .item_of_type::<status_bar::AddonStatusItems>()
    });
    assert!(status_items.is_some(), "the status bar shows addon items");

    cx.update(|cx| proxy.remove_status_item("hello-ui".into(), "other".into(), cx));
    assert_eq!(texts(cx), ["hello-ui/hello: Hello 1"]);

    cx.update(|cx| proxy.unregister_addon("hello-ui".into(), cx));
    assert!(texts(cx).is_empty(), "unloading an addon removes its items");
}

pub(crate) const PANEL_MANIFEST: &str = r#"
id = "tree-addon"
name = "Tree Addon"
version = "0.1.0"
schema_version = 1

[commands.open-item]
title = "Open"

[commands.show-path]
title = "Show Path"

[panels.files]
title = "Files"
icon = "folder"
dock = "right"

[[panels.files.context_menu]]
command = "open-item"
when = "file"

[[panels.files.context_menu]]
command = "show-path"
"#;

fn tree_item(id: &str, collapsible: bool) -> AddonTreeItem {
    AddonTreeItem {
        id: id.into(),
        label: id.rsplit('/').next().unwrap_or(id).into(),
        description: None,
        icon: Some(if collapsible { "folder" } else { "file" }.into()),
        collapsible,
        command: (!collapsible).then(|| "open-item".into()),
        context_value: Some(if collapsible { "folder" } else { "file" }.into()),
    }
}

fn visible_labels(view: &Entity<AddonView>, cx: &mut VisualTestContext) -> Vec<String> {
    view.read_with(cx, |view, _| {
        view.visible_items()
            .into_iter()
            .map(|(depth, item)| format!("{}{}", "  ".repeat(depth), item.label))
            .collect()
    })
}

#[gpui::test]
async fn test_addon_panels_show_tree_views(cx: &mut TestAppContext) {
    let (app_state, proxy) = init_test(cx);
    let project = Project::test(app_state.fs.clone(), [], cx).await;
    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));
    let workspace = multi_workspace.read_with(cx, |mw, _| mw.workspace().clone());

    let addon = FakeAddon::new(PANEL_MANIFEST);
    addon.set_tree_children(None, vec![tree_item("root", true)]);
    addon.set_tree_children(
        Some("root"),
        vec![
            tree_item("root/src", true),
            tree_item("root/README.md", false),
        ],
    );
    cx.update(|_, cx| proxy.register_addon(addon.clone(), cx));
    cx.run_until_parked();

    // Registering the addon adds its panel to the workspace.
    let panel = workspace
        .read_with(cx, |workspace, cx| workspace.panel::<AddonPanel<0>>(cx))
        .expect("the addon panel is added");
    let (key, position) = panel.update_in(cx, |panel, window, cx| {
        (panel.key().clone(), panel.position(window, cx))
    });
    assert_eq!(key, AddonViewKey::new("tree-addon", "files"));
    assert_eq!(position, DockPosition::Right);

    // The tree shows the root items, and expanding an item asks for its children.
    let view = panel.read_with(cx, |panel, _| panel.view().clone());
    assert_eq!(visible_labels(&view, cx), ["root"]);
    view.update(cx, |view, cx| view.toggle_expanded("root", cx));
    cx.run_until_parked();
    assert_eq!(visible_labels(&view, cx), ["root", "  src", "  README.md"]);

    // Context menu entries are filtered by the item's context value.
    let registry = cx.update(|_, cx| AddonRegistry::global(cx).expect("registry is initialized"));
    let titles = |context_value: Option<&str>, cx: &mut VisualTestContext| {
        registry.read_with(cx, |registry, _| {
            registry
                .context_menu_entries(&key, context_value)
                .into_iter()
                .map(|(command, title)| format!("{command}: {title}"))
                .collect::<Vec<_>>()
        })
    };
    assert_eq!(
        titles(Some("file"), cx),
        ["open-item: Open", "show-path: Show Path"]
    );
    assert_eq!(titles(Some("folder"), cx), ["show-path: Show Path"]);

    // `refresh-view` asks the addon again, keeping expanded items expanded.
    addon.set_tree_children(Some("root"), vec![tree_item("root/new.rs", false)]);
    cx.update(|_, cx| proxy.refresh_view("tree-addon".into(), "files".into(), cx));
    cx.run_until_parked();
    assert_eq!(visible_labels(&view, cx), ["root", "  new.rs"]);

    // The panel is listed in the command palette.
    let commands = cx.update(|_, cx| GlobalCommandPaletteExtraCommands::commands(cx));
    assert!(
        commands
            .iter()
            .any(|command| command.name == "Tree Addon: Toggle Files"),
        "palette commands: {:?}",
        commands
            .iter()
            .map(|command| &command.name)
            .collect::<Vec<_>>()
    );

    // Unloading the addon removes its panel.
    cx.update(|_, cx| proxy.unregister_addon("tree-addon".into(), cx));
    cx.run_until_parked();
    assert!(
        workspace
            .read_with(cx, |workspace, cx| workspace.panel::<AddonPanel<0>>(cx))
            .is_none()
    );
}

fn count_label(view: &Entity<AddonView>, cx: &mut VisualTestContext) -> Option<String> {
    view.read_with(cx, |view, _| {
        view.view_tree()?
            .elements
            .iter()
            .find_map(|element| match &element.kind {
                AddonElementKind::Label(text) if element.id == "count" => Some(text.clone()),
                _ => None,
            })
    })
}

#[gpui::test]
async fn test_custom_views_open_in_editor_tabs(cx: &mut TestAppContext) {
    let (app_state, proxy) = init_test(cx);
    let addon = FakeAddon::new(HELLO_MANIFEST);
    cx.update(|cx| proxy.register_addon(addon.clone(), cx));
    let project = Project::test(app_state.fs.clone(), [], cx).await;
    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));
    let workspace = multi_workspace.read_with(cx, |mw, _| mw.workspace().clone());

    workspace.update_in(cx, |workspace, window, cx| {
        let registry = AddonRegistry::global(cx).expect("registry is initialized");
        let key = AddonViewKey::new("hello-ui", "dashboard");
        view::open_view_tab(
            workspace,
            key.clone(),
            "Dashboard".into(),
            registry.clone(),
            window,
            cx,
        );
        // Opening the same view again activates its tab instead of adding one.
        view::open_view_tab(workspace, key, "Dashboard".into(), registry, window, cx);
    });
    cx.run_until_parked();

    let items: Vec<_> = workspace.read_with(cx, |workspace, cx| {
        workspace.items_of_type::<AddonViewItem>(cx).collect()
    });
    assert_eq!(items.len(), 1);
    let tab_title = items[0].read_with(cx, |item, cx| item.tab_content_text(0, cx));
    assert_eq!(tab_title, "Dashboard");
    let view = items[0].read_with(cx, |item, _| item.view().clone());
    assert_eq!(
        count_label(&view, cx).as_deref(),
        Some("dashboard: 0 clicks")
    );

    // A click goes to the addon, and the view renders again.
    view.update_in(cx, |view, window, cx| {
        view.send_event("inc".into(), AddonViewEvent::Click, window, cx)
    });
    cx.run_until_parked();
    assert_eq!(
        count_label(&view, cx).as_deref(),
        Some("dashboard: 1 clicks")
    );

    // Text inputs show the addon's value and send what the user types.
    let input = view
        .read_with(cx, |view, _| view.text_input("name"))
        .expect("the text input has an editor");
    assert_eq!(input.read_with(cx, |editor, cx| editor.text(cx)), "world");
    input.update_in(cx, |editor, window, cx| {
        editor.set_text("Vireo", window, cx)
    });
    cx.run_until_parked();
    assert!(
        addon
            .view_events
            .lock()
            .contains(&("name".to_string(), AddonViewEvent::Change("Vireo".into()))),
        "events: {:?}",
        addon.view_events.lock()
    );
    // Setting the addon's own value did not echo back as a change.
    assert!(
        !addon
            .view_events
            .lock()
            .contains(&("name".to_string(), AddonViewEvent::Change("world".into())))
    );
}

fn range(start: (u32, u32), end: (u32, u32)) -> AddonTextRange {
    AddonTextRange {
        start: AddonPosition {
            line: start.0,
            column: start.1,
        },
        end: AddonPosition {
            line: end.0,
            column: end.1,
        },
    }
}

fn hovers_at(
    editor: &Entity<Editor>,
    line: u32,
    column: u32,
    cx: &mut VisualTestContext,
) -> Vec<String> {
    editor.read_with(cx, |editor, cx| {
        let snapshot = editor.buffer().read(cx).snapshot(cx);
        let anchor = snapshot.anchor_before(text::Point::new(line, column));
        editor
            .addon_hovers_at(anchor, &snapshot)
            .into_iter()
            .map(|(_, text)| text.to_string())
            .collect()
    })
}

#[gpui::test]
async fn test_decorations_reach_editors_showing_the_file(cx: &mut TestAppContext) {
    let (app_state, proxy) = init_test(cx);
    app_state
        .fs
        .as_fake()
        .insert_tree(
            path!("/project"),
            serde_json::json!({ "main.rs": "fn main() {\n    let x = 1;\n}\n" }),
        )
        .await;
    let project = Project::test(app_state.fs.clone(), [path!("/project").as_ref()], cx).await;
    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));
    let workspace = multi_workspace.read_with(cx, |mw, _| mw.workspace().clone());
    let main_rs = PathBuf::from(path!("/project/main.rs"));
    let editor = workspace
        .update_in(cx, |workspace, window, cx| {
            workspace.open_abs_path(main_rs.clone(), Default::default(), window, cx)
        })
        .await
        .expect("main.rs opens")
        .downcast::<Editor>()
        .expect("main.rs opens in an editor");

    cx.update(|_, cx| {
        proxy.set_decorations(
            "hello-ui".into(),
            main_rs.clone(),
            "hello".into(),
            vec![
                AddonDecoration::Highlight {
                    range: range((1, 0), (2, 0)),
                    color: Some("#ff000080".into()),
                },
                AddonDecoration::Underline {
                    range: range((1, 8), (1, 9)),
                    severity: AddonSeverity::Warning,
                    message: "unused `x`".into(),
                },
                AddonDecoration::InlayHint {
                    position: AddonPosition { line: 1, column: 9 },
                    text: ": i32".into(),
                },
                AddonDecoration::Hover {
                    range: range((0, 3), (0, 7)),
                    markdown: "**main**".into(),
                },
                AddonDecoration::GutterIcon {
                    line: 2,
                    severity: AddonSeverity::Info,
                    tooltip: "the end".into(),
                },
            ],
            cx,
        );
        // Another addon's collection lives beside it.
        proxy.set_decorations(
            "other-addon".into(),
            main_rs.clone(),
            "hello".into(),
            vec![AddonDecoration::Hover {
                range: range((0, 3), (0, 7)),
                markdown: "other".into(),
            }],
            cx,
        );
    });
    cx.run_until_parked();

    editor.update_in(cx, |editor, window, cx| {
        assert_eq!(
            editor.display_text(cx),
            "fn main() {\n    let x: i32 = 1;\n}\n",
            "the inlay hint is shown"
        );
        let red = decorations::parse_color("#ff000080").unwrap();
        assert!(
            editor
                .all_text_background_highlights(window, cx)
                .iter()
                .any(|(_, color)| *color == red),
            "the highlight is shown"
        );
    });
    let underlined = editor.read_with(cx, |editor, cx| {
        (0..4)
            .flat_map(|collection| {
                editor
                    .text_highlights(
                        HighlightKey::AddonUnderline(collection, AddonSeverity::Warning as u8),
                        cx,
                    )
                    .map(|(_, ranges)| ranges.len())
            })
            .sum::<usize>()
    });
    assert_eq!(underlined, 1, "the underline is shown");
    assert_eq!(hovers_at(&editor, 0, 4, cx), ["**main**", "other"]);
    assert_eq!(hovers_at(&editor, 1, 8, cx), ["unused `x`"]);
    assert_eq!(hovers_at(&editor, 2, 0, cx), ["the end"]);

    // Clearing a collection removes its decorations and leaves other addons' alone.
    cx.update(|_, cx| proxy.clear_decorations("hello-ui".into(), "hello".into(), cx));
    cx.run_until_parked();
    assert_eq!(
        editor.update(cx, |editor, cx| editor.display_text(cx)),
        "fn main() {\n    let x = 1;\n}\n"
    );
    assert_eq!(hovers_at(&editor, 0, 4, cx), ["other"]);
    assert!(hovers_at(&editor, 2, 0, cx).is_empty());

    // Unloading an addon removes its decorations.
    cx.update(|_, cx| proxy.unregister_addon("other-addon".into(), cx));
    cx.run_until_parked();
    assert!(hovers_at(&editor, 0, 4, cx).is_empty());
}
