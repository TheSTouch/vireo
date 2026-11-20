//! Vireo's v0.9.0 extension API.
//!
//! v0.9.0 is v0.8.0 plus the addon UI interfaces. Every interface the two
//! versions share is mapped onto the v0.8.0 bindings, so the host
//! implementations in `since_v0_8_0` serve both versions. Only the world-level
//! items, which `bindgen!` regenerates for each world, are forwarded here.

use crate::wasm_host::WasmState;
use anyhow::Result;
use extension::{KeyValueStoreDelegate, ProjectDelegate, WorktreeDelegate};
use gpui::BackgroundExecutor;
use semver::Version;
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

impl ui::Host for WasmState {}

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
}

#[cfg(test)]
mod tests {
    use super::super::{Extension as VersionedExtension, is_supported_wasm_api_version};
    use crate::capability_granter::CapabilityGranter;
    use crate::wasm_host::{WasmHost, WasmState};
    use extension::{ExtensionHostProxy, ExtensionManifest};
    use fs::FakeFs;
    use gpui::TestAppContext;
    use http_client::FakeHttpClient;
    use node_runtime::NodeRuntime;
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

    #[gpui::test]
    async fn test_v0_8_0_and_v0_9_0_extensions_load_on_every_channel(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let settings_store = SettingsStore::test(cx);
            cx.set_global(settings_store);
            release_channel::init(Version::new(0, 0, 0), cx);
        });
        let host = cx.update(|cx| {
            WasmHost::new(
                FakeFs::new(cx.background_executor().clone()),
                FakeHttpClient::with_404_response(),
                NodeRuntime::unavailable(),
                Arc::new(ExtensionHostProxy::new()),
                PathBuf::from("/work"),
                cx,
            )
        });
        let manifest: Arc<ExtensionManifest> = Arc::new(
            toml::from_str(
                r#"
                id = "stub"
                name = "Stub"
                version = "0.1.0"
                schema_version = 1
                "#,
            )
            .expect("manifest should parse"),
        );
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
                let mut store = wasmtime::Store::new(
                    &host.engine,
                    WasmState {
                        manifest: manifest.clone(),
                        table: ResourceTable::new(),
                        ctx: WasiCtxBuilder::new().build(),
                        host: host.clone(),
                        capability_granter: CapabilityGranter::new(Vec::new(), manifest.clone()),
                        language_server_status_source: None,
                    },
                );
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
}
