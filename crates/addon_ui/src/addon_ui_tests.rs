use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Result, bail};
use async_trait::async_trait;
use extension::{
    BuildTaskTemplate, CodeLabel, Command, Completion, ContextServerConfiguration,
    DebugAdapterBinary, DebugRequest, DebugScenario, DebugTaskDefinition, Extension,
    ExtensionHostProxy, ExtensionManifest, KeyValueStoreDelegate, ProjectDelegate, SlashCommand,
    SlashCommandArgumentCompletion, SlashCommandOutput, StartDebuggingRequestArgumentsRequest,
    Symbol, WorktreeDelegate,
};
use gpui::{EntityId, TestAppContext};
use language::LanguageName;
use lsp::LanguageServerName;
use parking_lot::Mutex;
use project::Project;
use task::{SpawnInTerminal, ZedDebugConfig};
use workspace::{AppState, MultiWorkspace};

use crate::*;

/// An addon that records the commands it is asked to run.
pub(crate) struct FakeAddon {
    pub manifest: Arc<ExtensionManifest>,
    pub commands_run: Mutex<Vec<(Arc<str>, Vec<String>)>>,
}

impl FakeAddon {
    pub fn new(manifest_toml: &str) -> Arc<Self> {
        Arc::new(Self {
            manifest: Arc::new(toml::from_str(manifest_toml).expect("manifest should parse")),
            commands_run: Mutex::default(),
        })
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
