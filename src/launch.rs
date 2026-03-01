use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use alloy_primitives::{map::AddressHashMap, Address};
use eyre::{Context, OptionExt};
use foundry_common::compile::ProjectCompiler;
use foundry_config::Config as FoundryConfig;
use foundry_debugger::DebugNode;
use foundry_evm_core::Breakpoints;
use foundry_evm_traces::debug::ContractSources;
use serde::Deserialize;

use crate::config::LaunchConfig;
/// Parsed Solidity storage layout for a contract.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct StorageLayout {
    #[serde(default)]
    pub storage: Vec<StorageEntry>,
    #[serde(default)]
    pub types: HashMap<String, StorageType>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StorageEntry {
    pub label: String,
    pub slot: String,
    pub offset: u64,
    #[serde(rename = "type")]
    pub type_key: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StorageType {
    pub label: String,
    #[serde(rename = "numberOfBytes")]
    pub number_of_bytes: String,
}

/// Maps a 4-byte function selector to its signature.
pub type MethodIdentifiers = HashMap<String, String>;

#[derive(Debug)]
pub struct DebuggerContext {
    pub debug_arena: Vec<DebugNode>,
    pub identified_contracts: AddressHashMap<String>,
    pub contracts_sources: ContractSources,
    pub breakpoints: Breakpoints,
    /// Contract name → storage layout (for displaying Solidity variable values)
    pub storage_layouts: HashMap<String, StorageLayout>,
    /// Function selector (4-byte hex) → function signature (e.g. "increment()")
    pub method_identifiers: HashMap<String, String>,
    /// Function selector → parameter names and types for stack labeling
    pub function_params: HashMap<String, Vec<(String, String)>>,
}

pub fn compile_and_debug(launch_config: &LaunchConfig) -> eyre::Result<DebuggerContext> {
    let project_root = &launch_config.project_root;

    let test = launch_config
        .test
        .as_deref()
        .ok_or_eyre("only test debugging is supported for now (missing `test`)")?;

    let (match_contract, match_test) = split_contract_test(launch_config.contract.as_deref(), test);
    let dump_path = temp_dump_path(project_root);

    // Run forge first (it compiles internally), then build ContractSources from artifacts.
    // This ordering avoids file-lock contention between ProjectCompiler and forge.
    let dump = run_forge_debug_dump(
        project_root,
        &dump_path,
        match_contract.as_deref(),
        &match_test,
    )
    .wrap_err("forge test --debug failed")?;

    // Now build ContractSources from the compilation artifacts forge left behind.
    // Change to project_root so Config::load_with_root resolves paths correctly.
    // Without this, it walks up and finds the wrong root (e.g., sol-dap's root).
    let original_dir = std::env::current_dir().ok();
    std::env::set_current_dir(project_root).wrap_err("failed to chdir to project_root")?;
    let foundry_config = FoundryConfig::load_with_root(project_root).wrap_err_with(|| {
        format!("failed to load foundry config from {}", project_root.display())
    })?;
    let project = foundry_config.project().wrap_err_with(|| {
        format!("failed to create foundry project for {}", project_root.display())
    })?;
    // Restore original directory
    if let Some(dir) = original_dir {
        let _ = std::env::set_current_dir(dir);
    }

    tracing::info!("building source maps from cached compilation artifacts");
    // Use Project::compile() directly instead of ProjectCompiler::compile().
    tracing::info!("project root: {}", project.root().display());
    tracing::info!("sources dir: {}", project.paths.sources.display());
    tracing::info!("artifacts dir: {}", project.paths.artifacts.display());
    // Read all source files and pass them to the compiler explicitly.
    // Project::compile() returns empty output when everything is cached.
    // foundry_common's ProjectCompiler does the same but adds stdout printing
    // and calls process::exit(0) on 'nothing to compile', which kills our DAP server.
    // Disable caching AND force recompile with explicit sources.
    let mut project = project;
    project.cached = false;
    let sources_input = project.paths.read_input_files().wrap_err("failed to read source files")?;
    tracing::info!("read {} source files", sources_input.len());
    let compiler = foundry_compilers::project::ProjectCompiler::with_sources(&project, sources_input)
        .wrap_err("failed to create compiler")?;
    let output = compiler.compile().wrap_err("foundry compilation failed")?;
    let sources = ContractSources::from_project_output(&output, project_root, None)
        .wrap_err("failed to build ContractSources from compilation output")?;
    let artifact_count = output.artifact_ids().count();
    tracing::info!("compilation produced {artifact_count} artifacts");
    for (id, artifact) in output.artifact_ids().take(5) {
        tracing::info!("  artifact: {} (file_id={:?})", id.name, artifact.id);
    }
    let sources = ContractSources::from_project_output(&output, project_root, None)
        .wrap_err("failed to build ContractSources from compilation output")?;
    tracing::info!("source maps ready ({} entries)", sources.entries().count());
    // Load storage layouts from artifacts directory.
    let (storage_layouts, method_identifiers, function_params) = load_artifact_metadata(&project.paths.artifacts)?;
    tracing::info!("loaded storage layouts for {} contracts, {} method selectors",
        storage_layouts.len(), method_identifiers.len());

    let identified_contracts = parse_identified_contracts(dump.contracts.identified_contracts)
        .wrap_err("failed to parse identified_contracts from forge dump")?;

    Ok(DebuggerContext {
        debug_arena: dump.debug_arena.into_iter().map(DebugNode::from).collect(),
        identified_contracts,
        contracts_sources: sources,
        breakpoints: Breakpoints::default(),
        storage_layouts,
        method_identifiers,
        function_params,
    })
}

/// Intermediate type for deserializing the forge dump.
/// We use our own node type because the installed forge version may not
/// include all fields that the pinned foundry crate expects (e.g. `gas_limit`).
#[derive(Debug, Deserialize)]
struct ForgeDebuggerDump {
    contracts: ForgeContractsDump,
    debug_arena: Vec<RawDebugNode>,
}

#[derive(Debug, Deserialize)]
struct RawDebugNode {
    pub address: Address,
    pub kind: revm_inspectors::tracing::types::CallKind,
    pub calldata: alloy_primitives::Bytes,
    #[serde(default)]
    pub gas_limit: u64,
    pub steps: Vec<revm_inspectors::tracing::types::CallTraceStep>,
}

impl From<RawDebugNode> for DebugNode {
    fn from(raw: RawDebugNode) -> Self {
        Self {
            address: raw.address,
            kind: raw.kind,
            calldata: raw.calldata,
            gas_limit: raw.gas_limit,
            steps: raw.steps,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ForgeContractsDump {
    identified_contracts: HashMap<String, String>,
}

fn split_contract_test(contract: Option<&str>, test: &str) -> (Option<String>, String) {
    if let Some(contract) = contract {
        return (Some(contract.to_string()), test.to_string());
    }

    if let Some((c, t)) = test.split_once("::") {
        if !c.is_empty() && !t.is_empty() {
            return (Some(c.to_string()), t.to_string());
        }
    }

    (None, test.to_string())
}

fn temp_dump_path(project_root: &Path) -> PathBuf {
    let filename = format!(".sol-dap-debugger-dump-{}.json", std::process::id());
    project_root.join(filename)
}

fn run_forge_debug_dump(
    project_root: &Path,
    dump_path: &Path,
    match_contract: Option<&str>,
    match_test: &str,
) -> eyre::Result<ForgeDebuggerDump> {
    let mut cmd = Command::new("forge");
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env("TERM", "dumb")
        .current_dir(project_root)
        .arg("test")
        .arg("--debug")
        .arg("--dump")
        .arg(dump_path)
        .arg("--match-test")
        .arg(match_test);

    if let Some(contract) = match_contract {
        cmd.arg("--match-contract").arg(contract);
    }

    tracing::info!("spawning forge in {}", project_root.display());

    // Spawn the process and wait for the dump file to appear.
    // forge test --debug --dump writes the file then opens the TUI,
    // so we poll for the file and kill the process once it exists.
    let mut child = cmd.spawn().wrap_err("failed to spawn forge")?;

    // Wait for the dump file to appear (forge writes it before opening TUI)
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    loop {
        tracing::trace!("poll: dump_exists={}", dump_path.exists());
        if dump_path.exists() && std::fs::metadata(dump_path).map(|m| m.len() > 0).unwrap_or(false) {
            // Give forge a moment to finish writing
            std::thread::sleep(std::time::Duration::from_millis(200));
            break;
        }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            eyre::bail!("timed out waiting for forge to produce debug dump");
        }
        // Check if the process exited with an error
        if let Some(status) = child.try_wait().wrap_err("failed to wait for forge")? {
            if !status.success() {
                eyre::bail!("forge exited with {status}");
            }
            if !dump_path.exists() {
                eyre::bail!(
                    "forge exited without creating debug dump. \
                     Check that project_root points to a valid Foundry project \
                     and the test/contract names are correct."
                );
            }
            break; // Process exited successfully
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    tracing::info!("killing forge process");
    let _ = child.kill();
    tracing::info!("waiting for forge to exit");
    let _ = child.wait();
    tracing::info!("forge exited, reading dump");

    let dump_bytes = std::fs::read(dump_path)
        .wrap_err_with(|| format!("failed to read forge dump at {}", dump_path.display()))?;

    let _ = std::fs::remove_file(dump_path);

    serde_json::from_slice(&dump_bytes).wrap_err("failed to deserialize forge debugger dump")
}

fn parse_identified_contracts(
    raw: HashMap<String, String>,
) -> eyre::Result<AddressHashMap<String>> {
    let mut out = AddressHashMap::default();
    for (addr, name) in raw {
        let address: Address = addr
            .parse()
            .wrap_err_with(|| format!("invalid address key: {addr}"))?;
        out.insert(address, name);
    }
    Ok(out)
}

/// Load storage layouts, method identifiers, and function parameter names from artifacts.
fn load_artifact_metadata(artifacts_dir: &Path) -> eyre::Result<(
    HashMap<String, StorageLayout>,
    HashMap<String, String>,
    HashMap<String, Vec<(String, String)>>,
)> {
    let mut layouts = HashMap::new();
    let mut methods: HashMap<String, String> = HashMap::new();
    // selector -> [(param_name, param_type), ...]
    let mut params: HashMap<String, Vec<(String, String)>> = HashMap::new();
    if !artifacts_dir.exists() {
        return Ok((layouts, methods, params));
    }
    for entry in std::fs::read_dir(artifacts_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() { continue; }
        for file in std::fs::read_dir(entry.path())? {
            let file = file?;
            let path = file.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") { continue; }
            let contract_name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
            if contract_name.is_empty() { continue; }
            let data = match std::fs::read(&path) { Ok(d) => d, Err(_) => continue };
            #[derive(Deserialize)]
            struct AbiInput {
                name: String,
                #[serde(rename = "type")]
                type_field: String,
            }
            #[derive(Deserialize)]
            #[serde(tag = "type")]
            enum AbiItem {
                #[serde(rename = "function")]
                Function { name: String, #[serde(default)] inputs: Vec<AbiInput> },
                #[serde(other)]
                Other,
            }
            #[derive(Deserialize)]
            struct ArtifactPartial {
                #[serde(default, rename = "storageLayout")]
                storage_layout: Option<StorageLayout>,
                #[serde(default, rename = "methodIdentifiers")]
                method_identifiers: Option<HashMap<String, String>>,
                #[serde(default)]
                abi: Vec<AbiItem>,
            }
            let artifact: ArtifactPartial = match serde_json::from_slice(&data) { Ok(a) => a, Err(_) => continue };

            // Build selector -> param names from ABI + methodIdentifiers
            let mi = artifact.method_identifiers.as_ref();
            for item in &artifact.abi {
                if let AbiItem::Function { name, inputs } = item {
                    // Build the signature to look up selector
                    let sig = format!("{}({})", name, inputs.iter().map(|i| i.type_field.as_str()).collect::<Vec<_>>().join(","));
                    if let Some(mi) = mi {
                        if let Some(selector) = mi.get(&sig) {
                            let param_list: Vec<(String, String)> = inputs.iter()
                                .map(|i| (i.name.clone(), i.type_field.clone()))
                                .collect();
                            if !param_list.is_empty() {
                                params.insert(format!("0x{selector}"), param_list);
                            }
                        }
                    }
                }
            }

            if let Some(layout) = artifact.storage_layout {
                if !layout.storage.is_empty() {
                    layouts.insert(contract_name, layout);
                }
            }
            if let Some(mi) = artifact.method_identifiers {
                for (sig, selector) in mi {
                    methods.insert(format!("0x{selector}"), sig);
                }
            }
        }
    }
    Ok((layouts, methods, params))
}
