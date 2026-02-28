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
#[derive(Debug)]
pub struct DebuggerContext {
    pub debug_arena: Vec<DebugNode>,
    pub identified_contracts: AddressHashMap<String>,
    pub contracts_sources: ContractSources,
    pub breakpoints: Breakpoints,
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
    let foundry_config = FoundryConfig::load_with_root(project_root).wrap_err_with(|| {
        format!(
            "failed to load foundry config from {}",
            project_root.display()
        )
    })?;

    let project = foundry_config.project().wrap_err_with(|| {
        format!(
            "failed to create foundry project for {}",
            project_root.display()
        )
    })?;

    tracing::info!("starting compilation");
    // TODO: ProjectCompiler::compile() hangs when stdout is redirected via dup2.
    // For now, skip ContractSources — we have debug_arena and identified_contracts
    // from the forge dump, which is enough for stepping. Source mapping (for
    // showing Solidity source in the debugger) will need a different approach.
    let sources = ContractSources::default();
    tracing::info!("skipping compilation (source mapping not yet available)");
    let identified_contracts = parse_identified_contracts(dump.contracts.identified_contracts)
        .wrap_err("failed to parse identified_contracts from forge dump")?;

    Ok(DebuggerContext {
        debug_arena: dump.debug_arena.into_iter().map(DebugNode::from).collect(),
        identified_contracts,
        contracts_sources: sources,
        breakpoints: Breakpoints::default(),
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
