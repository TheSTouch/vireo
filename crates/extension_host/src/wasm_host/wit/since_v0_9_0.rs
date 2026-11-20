//! Vireo's v0.9.0 extension API.
//!
//! v0.9.0 is v0.8.0 plus the addon UI interfaces. Every interface the two
//! versions share is mapped onto the v0.8.0 bindings, so the host
//! implementations in `since_v0_8_0` serve both versions. Only the world-level
//! items, which `bindgen!` regenerates for each world, are forwarded here.

use crate::wasm_host::WasmState;
use anyhow::Result;
use extension::{
    AddonDecoration, AddonElement, AddonElementKind, AddonPosition, AddonSeverity, AddonStatusItem,
    AddonTextRange, AddonTreeItem, AddonViewEvent, AddonViewTree, ExtensionAddonProxy as _,
    ExtensionHostProxy, KeyValueStoreDelegate, ProjectDelegate, WorktreeDelegate,
};
use gpui::{App, BackgroundExecutor};
use semver::Version;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use wasmtime::component::{Linker, Resource};

use super::since_v0_8_0 as v0_8_0;

pub const MIN_VERSION: Version = Version::new(0, 9, 0);
pub const MAX_VERSION: Version = Version::new(0, 9, 0);

wasmtime::component::bindgen!({
    imports: {
        default: async | trappable,
    },
    exports: {
        default: async,
    },
    path: "../extension_api/wit/since_v0.9.0",
    with: {
        "worktree": ExtensionWorktree,
        "project": ExtensionProject,
        "key-value-store": ExtensionKeyValueStore,
        "zed:extension/common": v0_8_0::zed::extension::common,
        "zed:extension/context-server": v0_8_0::zed::extension::context_server,
        "zed:extension/dap": v0_8_0::zed::extension::dap,
        "zed:extension/github": v0_8_0::zed::extension::github,
        "zed:extension/http-client": v0_8_0::zed::extension::http_client,
        "zed:extension/lsp": v0_8_0::zed::extension::lsp,
        "zed:extension/nodejs": v0_8_0::zed::extension::nodejs,
        "zed:extension/platform": v0_8_0::zed::extension::platform,
        "zed:extension/process": v0_8_0::zed::extension::process,
        "zed:extension/slash-command": v0_8_0::zed::extension::slash_command,
    },
});

pub use self::zed::extension::*;

pub type ExtensionWorktree = Arc<dyn WorktreeDelegate>;
pub type ExtensionProject = Arc<dyn ProjectDelegate>;
pub type ExtensionKeyValueStore = Arc<dyn KeyValueStoreDelegate>;

pub fn linker(executor: &BackgroundExecutor) -> &'static Linker<WasmState> {
    static LINKER: OnceLock<Linker<WasmState>> = OnceLock::new();
    LINKER.get_or_init(|| {
        super::new_linker(executor, |linker| {
            Extension::add_to_linker::<_, WasmState>(linker, |s| s)
        })
    })
}

impl From<CodeLabel> for v0_8_0::CodeLabel {
    fn from(value: CodeLabel) -> Self {
        Self {
            code: value.code,
            spans: value.spans.into_iter().map(Into::into).collect(),
            filter_range: value.filter_range,
        }
    }
}

impl From<CodeLabelSpan> for v0_8_0::CodeLabelSpan {
    fn from(value: CodeLabelSpan) -> Self {
        match value {
            CodeLabelSpan::CodeRange(range) => Self::CodeRange(range),
            CodeLabelSpan::Literal(literal) => Self::Literal(literal.into()),
        }
    }
}

impl From<CodeLabelSpanLiteral> for v0_8_0::CodeLabelSpanLiteral {
    fn from(value: CodeLabelSpanLiteral) -> Self {
        Self {
            text: value.text,
            highlight_name: value.highlight_name,
        }
    }
}

impl From<SettingsLocation> for v0_8_0::SettingsLocation {
    fn from(value: SettingsLocation) -> Self {
        Self {
            worktree_id: value.worktree_id,
            path: value.path,
        }
    }
}

impl From<LanguageServerInstallationStatus> for v0_8_0::LanguageServerInstallationStatus {
    fn from(value: LanguageServerInstallationStatus) -> Self {
        match value {
            LanguageServerInstallationStatus::None => Self::None,
            LanguageServerInstallationStatus::Downloading => Self::Downloading,
            LanguageServerInstallationStatus::CheckingForUpdate => Self::CheckingForUpdate,
            LanguageServerInstallationStatus::Failed(error) => Self::Failed(error),
        }
    }
}

impl From<DownloadedFileType> for v0_8_0::DownloadedFileType {
    fn from(value: DownloadedFileType) -> Self {
        match value {
            DownloadedFileType::Bzip2 => Self::Bzip2,
            DownloadedFileType::Bzip2Tar => Self::Bzip2Tar,
            DownloadedFileType::Gzip => Self::Gzip,
            DownloadedFileType::GzipTar => Self::GzipTar,
            DownloadedFileType::Zip => Self::Zip,
            DownloadedFileType::Uncompressed => Self::Uncompressed,
        }
    }
}

impl HostKeyValueStore for WasmState {
    async fn insert(
        &mut self,
        kv_store: Resource<ExtensionKeyValueStore>,
        key: String,
        value: String,
    ) -> wasmtime::Result<Result<(), String>> {
        v0_8_0::HostKeyValueStore::insert(self, kv_store, key, value).await
    }

    async fn drop(&mut self, _kv_store: Resource<ExtensionKeyValueStore>) -> wasmtime::Result<()> {
        // We only ever hand out borrows of key-value stores.
        Ok(())
    }
}

impl HostProject for WasmState {
    async fn worktree_ids(
        &mut self,
        project: Resource<ExtensionProject>,
    ) -> wasmtime::Result<Vec<u64>> {
        v0_8_0::HostProject::worktree_ids(self, project).await
    }

    async fn drop(&mut self, _project: Resource<Project>) -> wasmtime::Result<()> {
        // We only ever hand out borrows of projects.
        Ok(())
    }
}

impl HostWorktree for WasmState {
    async fn id(&mut self, delegate: Resource<Arc<dyn WorktreeDelegate>>) -> wasmtime::Result<u64> {
        v0_8_0::HostWorktree::id(self, delegate).await
    }

    async fn root_path(
        &mut self,
        delegate: Resource<Arc<dyn WorktreeDelegate>>,
    ) -> wasmtime::Result<String> {
        v0_8_0::HostWorktree::root_path(self, delegate).await
    }

    async fn read_text_file(
        &mut self,
        delegate: Resource<Arc<dyn WorktreeDelegate>>,
        path: String,
    ) -> wasmtime::Result<Result<String, String>> {
        v0_8_0::HostWorktree::read_text_file(self, delegate, path).await
    }

    async fn shell_env(
        &mut self,
        delegate: Resource<Arc<dyn WorktreeDelegate>>,
    ) -> wasmtime::Result<EnvVars> {
        v0_8_0::HostWorktree::shell_env(self, delegate).await
    }

    async fn which(
        &mut self,
        delegate: Resource<Arc<dyn WorktreeDelegate>>,
        binary_name: String,
    ) -> wasmtime::Result<Option<String>> {
        v0_8_0::HostWorktree::which(self, delegate, binary_name).await
    }

    async fn drop(&mut self, _worktree: Resource<Worktree>) -> wasmtime::Result<()> {
        // We only ever hand out borrows of worktrees.
        Ok(())
    }
}

impl ExtensionImports for WasmState {
    async fn get_settings(
        &mut self,
        location: Option<self::SettingsLocation>,
        category: String,
        key: Option<String>,
    ) -> wasmtime::Result<Result<String, String>> {
        v0_8_0::ExtensionImports::get_settings(
            self,
            location.map(|location| location.into()),
            category,
            key,
        )
        .await
    }

    async fn set_language_server_installation_status(
        &mut self,
        server_name: String,
        status: LanguageServerInstallationStatus,
    ) -> wasmtime::Result<()> {
        v0_8_0::ExtensionImports::set_language_server_installation_status(
            self,
            server_name,
            status.into(),
        )
        .await
    }

    async fn download_file(
        &mut self,
        url: String,
        path: String,
        file_type: DownloadedFileType,
    ) -> wasmtime::Result<Result<(), String>> {
        v0_8_0::ExtensionImports::download_file(self, url, path, file_type.into()).await
    }

    async fn make_file_executable(&mut self, path: String) -> wasmtime::Result<Result<(), String>> {
        v0_8_0::ExtensionImports::make_file_executable(self, path).await
    }
}

impl ui::Host for WasmState {
    async fn set_status_item(
        &mut self,
        id: String,
        text: String,
        tooltip: Option<String>,
        command: Option<String>,
    ) -> wasmtime::Result<()> {
        let item = AddonStatusItem {
            id: id.into(),
            text,
            tooltip,
            command: command.map(Into::into),
        };
        self.on_addon_ui(move |proxy, extension_id, cx| {
            proxy.set_status_item(extension_id, item, cx)
        })
        .await
    }

    async fn remove_status_item(&mut self, id: String) -> wasmtime::Result<()> {
        self.on_addon_ui(move |proxy, extension_id, cx| {
            proxy.remove_status_item(extension_id, id.into(), cx)
        })
        .await
    }

    async fn refresh_view(&mut self, view_id: String) -> wasmtime::Result<()> {
        self.on_addon_ui(move |proxy, extension_id, cx| {
            proxy.refresh_view(extension_id, view_id.into(), cx)
        })
        .await
    }

    async fn open_view_tab(&mut self, view_id: String, title: String) -> wasmtime::Result<()> {
        self.on_addon_ui(move |proxy, extension_id, cx| {
            proxy.open_view_tab(extension_id, view_id.into(), title, cx)
        })
        .await
    }
}

impl From<ui::ViewTree> for AddonViewTree {
    fn from(tree: ui::ViewTree) -> Self {
        Self {
            elements: tree
                .elements
                .into_iter()
                .map(|element| AddonElement {
                    id: element.id,
                    kind: match element.kind {
                        ui::ElementKind::Column => AddonElementKind::Column,
                        ui::ElementKind::Row => AddonElementKind::Row,
                        ui::ElementKind::Scroll => AddonElementKind::Scroll,
                        ui::ElementKind::List => AddonElementKind::List,
                        ui::ElementKind::Label(text) => AddonElementKind::Label(text),
                        ui::ElementKind::Icon(name) => AddonElementKind::Icon(name),
                        ui::ElementKind::Button(label) => AddonElementKind::Button(label),
                        ui::ElementKind::TextInput(input) => AddonElementKind::TextInput {
                            value: input.value,
                            placeholder: input.placeholder,
                        },
                        ui::ElementKind::Checkbox(checkbox) => AddonElementKind::Checkbox {
                            label: checkbox.label,
                            checked: checkbox.checked,
                        },
                        ui::ElementKind::Divider => AddonElementKind::Divider,
                    },
                    children: element.children,
                })
                .collect(),
        }
    }
}

impl From<AddonViewEvent> for ui::ViewEvent {
    fn from(event: AddonViewEvent) -> Self {
        match event {
            AddonViewEvent::Click => Self::Click,
            AddonViewEvent::Change(text) => Self::Change(text),
            AddonViewEvent::Submit(text) => Self::Submit(text),
            AddonViewEvent::Toggle(checked) => Self::Toggle(checked),
        }
    }
}

impl From<editor::Position> for AddonPosition {
    fn from(position: editor::Position) -> Self {
        Self {
            line: position.line,
            column: position.column,
        }
    }
}

impl From<editor::TextRange> for AddonTextRange {
    fn from(range: editor::TextRange) -> Self {
        Self {
            start: range.start.into(),
            end: range.end.into(),
        }
    }
}

impl From<editor::Severity> for AddonSeverity {
    fn from(severity: editor::Severity) -> Self {
        match severity {
            editor::Severity::Error => Self::Error,
            editor::Severity::Warning => Self::Warning,
            editor::Severity::Info => Self::Info,
            editor::Severity::Hint => Self::Hint,
        }
    }
}

impl From<editor::Decoration> for AddonDecoration {
    fn from(decoration: editor::Decoration) -> Self {
        match decoration {
            editor::Decoration::Highlight((range, color)) => Self::Highlight {
                range: range.into(),
                color,
            },
            editor::Decoration::Underline((range, severity, message)) => Self::Underline {
                range: range.into(),
                severity: severity.into(),
                message,
            },
            editor::Decoration::InlayHint((position, text)) => Self::InlayHint {
                position: position.into(),
                text,
            },
            editor::Decoration::Hover((range, markdown)) => Self::Hover {
                range: range.into(),
                markdown,
            },
            editor::Decoration::GutterIcon((line, severity, tooltip)) => Self::GutterIcon {
                line,
                severity: severity.into(),
                tooltip,
            },
        }
    }
}

impl editor::Host for WasmState {
    async fn set_decorations(
        &mut self,
        path: String,
        collection: String,
        decorations: Vec<editor::Decoration>,
    ) -> wasmtime::Result<()> {
        let decorations = decorations.into_iter().map(Into::into).collect();
        self.on_addon_ui(move |proxy, extension_id, cx| {
            proxy.set_decorations(
                extension_id,
                PathBuf::from(path),
                collection.into(),
                decorations,
                cx,
            )
        })
        .await
    }

    async fn clear_decorations(&mut self, collection: String) -> wasmtime::Result<()> {
        self.on_addon_ui(move |proxy, extension_id, cx| {
            proxy.clear_decorations(extension_id, collection.into(), cx)
        })
        .await
    }
}

impl From<ui::TreeItem> for AddonTreeItem {
    fn from(item: ui::TreeItem) -> Self {
        Self {
            id: item.id,
            label: item.label,
            description: item.description,
            icon: item.icon,
            collapsible: item.collapsible,
            command: item.command,
            context_value: item.context_value,
        }
    }
}

impl WasmState {
    /// Runs `f` on the main thread with the addon UI proxy and this extension's ID.
    fn on_addon_ui<T: 'static + Send>(
        &self,
        f: impl 'static + Send + FnOnce(&ExtensionHostProxy, Arc<str>, &mut App) -> T,
    ) -> impl 'static + Future<Output = wasmtime::Result<T>> {
        let proxy = self.host.proxy.clone();
        let extension_id = self.manifest.id.clone();
        let result =
            self.on_main_thread(async move |cx| cx.update(|cx| f(&proxy, extension_id, cx)));
        async move { Ok(result.await) }
    }
}

impl super::Extension {
    pub async fn call_run_command(
        &self,
        store: &mut wasmtime::Store<WasmState>,
        command_id: &str,
        args: &[String],
    ) -> wasmtime::Result<Result<(), String>> {
        match self {
            super::Extension::V0_9_0(extension) => {
                extension.call_run_command(store, command_id, args).await
            }
            _ => Ok(Err(
                "addon commands require extension API v0.9.0 or later".to_string()
            )),
        }
    }

    pub async fn call_tree_children(
        &self,
        store: &mut wasmtime::Store<WasmState>,
        view_id: &str,
        parent: Option<&str>,
    ) -> wasmtime::Result<Result<Vec<AddonTreeItem>, String>> {
        match self {
            super::Extension::V0_9_0(extension) => Ok(extension
                .call_tree_children(store, view_id, parent)
                .await?
                .map(|items| items.into_iter().map(Into::into).collect())),
            _ => Ok(Err(
                "addon views require extension API v0.9.0 or later".to_string()
            )),
        }
    }

    pub async fn call_render_view(
        &self,
        store: &mut wasmtime::Store<WasmState>,
        view_id: &str,
    ) -> wasmtime::Result<Result<AddonViewTree, String>> {
        match self {
            super::Extension::V0_9_0(extension) => Ok(extension
                .call_render_view(store, view_id)
                .await?
                .map(Into::into)),
            _ => Ok(Err(
                "addon views require extension API v0.9.0 or later".to_string()
            )),
        }
    }

    pub async fn call_handle_view_event(
        &self,
        store: &mut wasmtime::Store<WasmState>,
        view_id: &str,
        element_id: &str,
        event: AddonViewEvent,
    ) -> wasmtime::Result<Result<(), String>> {
        match self {
            super::Extension::V0_9_0(extension) => {
                extension
                    .call_handle_view_event(store, view_id, element_id, &event.into())
                    .await
            }
            _ => Ok(Err(
                "addon views require extension API v0.9.0 or later".to_string()
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Extension as VersionedExtension, is_supported_wasm_api_version};
    use super::ui;
    use crate::capability_granter::CapabilityGranter;
    use crate::wasm_host::{WasmHost, WasmState};
    use extension::{
        AddonDecoration, AddonStatusItem, ExtensionAddonProxy, ExtensionHostProxy,
        ExtensionManifest,
    };
    use fs::FakeFs;
    use gpui::{App, TestAppContext};
    use http_client::FakeHttpClient;
    use node_runtime::NodeRuntime;
    use parking_lot::Mutex;
    use release_channel::ReleaseChannel;
    use semver::Version;
    use settings::SettingsStore;
    use std::{path::PathBuf, sync::Arc};
    use wasmtime::component::{Component, ResourceTable};
    use wasmtime_wasi::WasiCtxBuilder;

    /// Builds a component that implements the given WIT version with stub
    /// functions, which trap if called.
    fn stub_component(engine: &wasmtime::Engine, wit_version_dir: &str) -> Component {
        use wasm_encoder::{
            CodeSection, ExportKind, ExportSection, Function, FunctionSection, Instruction,
            MemorySection, MemoryType, Module, TypeSection, ValType,
        };
        use wit_parser::{
            ManglingAndAbi, WasmExport, WasmExportKind, WorldItem,
            abi::{AbiVariant, WasmType},
        };

        let wit_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../extension_api/wit")
            .join(wit_version_dir);
        let mut resolve = wit_parser::Resolve::default();
        let (package, _) = resolve.push_dir(&wit_dir).expect("WIT should parse");
        let world = resolve
            .select_world(&[package], Some("extension"))
            .expect("world should exist");

        let value_type = |wasm_type: &WasmType| match wasm_type {
            WasmType::I32 | WasmType::Pointer | WasmType::Length => ValType::I32,
            WasmType::I64 | WasmType::PointerOrI64 => ValType::I64,
            WasmType::F32 => ValType::F32,
            WasmType::F64 => ValType::F64,
        };
        let mut types = TypeSection::new();
        let mut functions = FunctionSection::new();
        let mut exports = ExportSection::new();
        let mut code = CodeSection::new();
        let mut add_function =
            |name: &str, params: Vec<ValType>, results: Vec<ValType>, traps: bool| {
                let index = types.len();
                types.ty().function(params, results);
                functions.function(index);
                exports.export(name, ExportKind::Func, index);
                let mut body = Function::new([]);
                if traps {
                    body.instruction(&Instruction::Unreachable);
                }
                body.instruction(&Instruction::End);
                code.function(&body);
            };

        let mangling = ManglingAndAbi::Standard32;
        for (_, item) in resolve.worlds[world].exports.iter() {
            let WorldItem::Function(function) = item else {
                panic!("the extension world only exports functions");
            };
            let signature = resolve.wasm_signature(AbiVariant::GuestExport, function);
            let params: Vec<_> = signature.params.iter().map(value_type).collect();
            let results: Vec<_> = signature.results.iter().map(value_type).collect();
            for is_post_return in [false, true] {
                let kind = if is_post_return {
                    WasmExportKind::PostReturn
                } else {
                    WasmExportKind::Normal
                };
                let name = resolve.wasm_export_name(
                    mangling,
                    WasmExport::Func {
                        interface: None,
                        func: function,
                        kind,
                    },
                );
                if is_post_return {
                    add_function(&name, results.clone(), Vec::new(), true);
                } else {
                    add_function(&name, params.clone(), results.clone(), true);
                }
            }
        }
        add_function(
            &resolve.wasm_export_name(mangling, WasmExport::Realloc),
            vec![ValType::I32; 4],
            vec![ValType::I32],
            true,
        );
        add_function(
            &resolve.wasm_export_name(mangling, WasmExport::Initialize),
            Vec::new(),
            Vec::new(),
            false,
        );

        let mut memories = MemorySection::new();
        memories.memory(MemoryType {
            minimum: 0,
            maximum: None,
            memory64: false,
            shared: false,
            page_size_log2: None,
        });
        exports.export(
            &resolve.wasm_export_name(mangling, WasmExport::Memory),
            ExportKind::Memory,
            0,
        );

        let mut module = Module::new();
        module.section(&types);
        module.section(&functions);
        module.section(&memories);
        module.section(&exports);
        module.section(&code);
        let mut module = module.finish();

        wit_component::embed_component_metadata(
            &mut module,
            &resolve,
            world,
            wit_component::StringEncoding::UTF8,
        )
        .expect("metadata should embed");
        let component_bytes = wit_component::ComponentEncoder::default()
            .module(&module)
            .expect("module should be accepted")
            .validate(true)
            .encode()
            .expect("component should encode");
        Component::from_binary(engine, &component_bytes).expect("component should compile")
    }

    fn test_host(cx: &mut TestAppContext, proxy: Arc<ExtensionHostProxy>) -> Arc<WasmHost> {
        cx.update(|cx| {
            let settings_store = SettingsStore::test(cx);
            cx.set_global(settings_store);
            release_channel::init(Version::new(0, 0, 0), cx);
        });
        cx.update(|cx| {
            WasmHost::new(
                FakeFs::new(cx.background_executor().clone()),
                FakeHttpClient::with_404_response(),
                NodeRuntime::unavailable(),
                proxy,
                PathBuf::from("/work"),
                cx,
            )
        })
    }

    fn test_manifest() -> Arc<ExtensionManifest> {
        Arc::new(
            toml::from_str(
                r#"
                id = "stub"
                name = "Stub"
                version = "0.1.0"
                schema_version = 1
                "#,
            )
            .expect("manifest should parse"),
        )
    }

    fn test_state(host: &Arc<WasmHost>, manifest: &Arc<ExtensionManifest>) -> WasmState {
        WasmState {
            manifest: manifest.clone(),
            table: ResourceTable::new(),
            ctx: WasiCtxBuilder::new().build(),
            host: host.clone(),
            capability_granter: CapabilityGranter::new(Vec::new(), manifest.clone()),
            language_server_status_source: None,
        }
    }

    #[gpui::test]
    async fn test_v0_8_0_and_v0_9_0_extensions_load_on_every_channel(cx: &mut TestAppContext) {
        let host = test_host(cx, Arc::new(ExtensionHostProxy::new()));
        let manifest = test_manifest();
        let executor = cx.background_executor.clone();

        for (wit_version_dir, version) in [
            ("since_v0.8.0", Version::new(0, 8, 0)),
            ("since_v0.9.0", Version::new(0, 9, 0)),
        ] {
            let component = stub_component(&host.engine, wit_version_dir);
            for channel in [
                ReleaseChannel::Stable,
                ReleaseChannel::Preview,
                ReleaseChannel::Nightly,
                ReleaseChannel::Dev,
            ] {
                assert!(
                    is_supported_wasm_api_version(channel, version.clone()),
                    "{version} should be supported on {channel:?}"
                );
                let mut store = wasmtime::Store::new(&host.engine, test_state(&host, &manifest));
                store.set_epoch_deadline(1);
                store.epoch_deadline_async_yield_and_update(1);
                let extension = VersionedExtension::instantiate_async(
                    &executor,
                    &mut store,
                    channel,
                    version.clone(),
                    &component,
                )
                .await
                .unwrap_or_else(|error| {
                    panic!("{version} extension should load on {channel:?}: {error:#}")
                });
                let loaded_version = match extension {
                    VersionedExtension::V0_9_0(_) => Version::new(0, 9, 0),
                    VersionedExtension::V0_8_0(_) => Version::new(0, 8, 0),
                    _ => Version::new(0, 0, 0),
                };
                assert_eq!(loaded_version, version);
            }
        }
    }

    /// Records what the host asks of the addon UI.
    #[derive(Clone, Default)]
    struct RecordingAddonProxy {
        calls: Arc<Mutex<Vec<String>>>,
    }

    impl ExtensionAddonProxy for RecordingAddonProxy {
        fn register_addon(&self, _extension: Arc<dyn extension::Extension>, _cx: &mut App) {}

        fn unregister_addon(&self, _extension_id: Arc<str>, _cx: &mut App) {}

        fn set_status_item(&self, extension_id: Arc<str>, item: AddonStatusItem, _cx: &mut App) {
            self.calls.lock().push(format!(
                "set {extension_id}/{} {:?} {:?} {:?}",
                item.id, item.text, item.tooltip, item.command
            ));
        }

        fn remove_status_item(&self, extension_id: Arc<str>, item_id: Arc<str>, _cx: &mut App) {
            self.calls
                .lock()
                .push(format!("remove {extension_id}/{item_id}"));
        }

        fn refresh_view(&self, extension_id: Arc<str>, view_id: Arc<str>, _cx: &mut App) {
            self.calls
                .lock()
                .push(format!("refresh {extension_id}/{view_id}"));
        }

        fn open_view_tab(
            &self,
            extension_id: Arc<str>,
            view_id: Arc<str>,
            title: String,
            _cx: &mut App,
        ) {
            self.calls
                .lock()
                .push(format!("open {extension_id}/{view_id} {title:?}"));
        }

        fn set_decorations(
            &self,
            extension_id: Arc<str>,
            path: PathBuf,
            collection: Arc<str>,
            decorations: Vec<AddonDecoration>,
            _cx: &mut App,
        ) {
            self.calls.lock().push(format!(
                "decorate {extension_id}/{collection} {} {decorations:?}",
                path.display()
            ));
        }

        fn clear_decorations(&self, extension_id: Arc<str>, collection: Arc<str>, _cx: &mut App) {
            self.calls
                .lock()
                .push(format!("clear {extension_id}/{collection}"));
        }
    }

    #[gpui::test]
    async fn test_ui_imports_reach_the_addon_ui(cx: &mut TestAppContext) {
        let proxy = Arc::new(ExtensionHostProxy::new());
        let recorder = RecordingAddonProxy::default();
        proxy.register_addon_proxy(recorder.clone());
        let host = test_host(cx, proxy);
        let mut state = test_state(&host, &test_manifest());

        ui::Host::set_status_item(
            &mut state,
            "count".into(),
            "Count: 1".into(),
            Some("Say hello".into()),
            Some("say-hello".into()),
        )
        .await
        .unwrap();
        ui::Host::remove_status_item(&mut state, "count".into())
            .await
            .unwrap();
        ui::Host::refresh_view(&mut state, "files".into())
            .await
            .unwrap();
        ui::Host::open_view_tab(&mut state, "dashboard".into(), "Dashboard".into())
            .await
            .unwrap();
        let position = |line, column| super::editor::Position { line, column };
        super::editor::Host::set_decorations(
            &mut state,
            "/project/main.rs".into(),
            "lint".into(),
            vec![super::editor::Decoration::GutterIcon((
                3,
                super::editor::Severity::Warning,
                "careful".into(),
            ))],
        )
        .await
        .unwrap();
        super::editor::Host::set_decorations(
            &mut state,
            "/project/main.rs".into(),
            "lint".into(),
            vec![super::editor::Decoration::InlayHint((
                position(1, 2),
                "hint".into(),
            ))],
        )
        .await
        .unwrap();
        super::editor::Host::clear_decorations(&mut state, "lint".into())
            .await
            .unwrap();

        assert_eq!(
            recorder.calls.lock().as_slice(),
            [
                r#"set stub/count "Count: 1" Some("Say hello") Some("say-hello")"#,
                "remove stub/count",
                "refresh stub/files",
                r#"open stub/dashboard "Dashboard""#,
                r#"decorate stub/lint /project/main.rs [GutterIcon { line: 3, severity: Warning, tooltip: "careful" }]"#,
                r#"decorate stub/lint /project/main.rs [InlayHint { position: AddonPosition { line: 1, column: 2 }, text: "hint" }]"#,
                "clear stub/lint",
            ]
        );
    }
}
