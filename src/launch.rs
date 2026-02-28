use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Command,
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

    let compiler = ProjectCompiler::new();
    let output = compiler
        .compile(&project)
        .wrap_err("foundry compilation failed")?;

    if output.has_compiler_errors() {
        eyre::bail!("compilation produced errors; see compiler output above");
    }

    let sources = ContractSources::from_project_output(&output, project_root, None)
        .wrap_err("failed to build ContractSources from compilation output")?;

    let test = launch_config
        .test
        .as_deref()
        .ok_or_eyre("only test debugging is supported for now (missing `test`)")?;

    let (match_contract, match_test) = split_contract_test(launch_config.contract.as_deref(), test);
    let dump_path = temp_dump_path(project_root);

    let dump = run_forge_debug_dump(
        project_root,
        &dump_path,
        match_contract.as_deref(),
        &match_test,
    )
    .wrap_err("forge test --debug failed")?;

    let identified_contracts = parse_identified_contracts(dump.contracts.identified_contracts)
        .wrap_err("failed to parse identified_contracts from forge dump")?;

    Ok(DebuggerContext {
        debug_arena: dump.debug_arena,
        identified_contracts,
        contracts_sources: sources,
        breakpoints: Breakpoints::default(),
    })
}

#[derive(Debug, Deserialize)]
struct ForgeDebuggerDump {
    contracts: ForgeContractsDump,
    debug_arena: Vec<DebugNode>,
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
    cmd.current_dir(project_root)
        .arg("test")
        .arg("--debug")
        .arg("--dump")
        .arg(dump_path)
        .arg("--match-test")
        .arg(match_test);

    if let Some(contract) = match_contract {
        cmd.arg("--match-contract").arg(contract);
    }

    let out = cmd.output().wrap_err("failed to spawn forge")?;
    if !out.status.success() {
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        eyre::bail!(
            "forge exited with {}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
            out.status
        );
    }

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
