//! Reference Zed debug adapter for Foundry/Solidity debugging.
//!
//! This file shows how to register sol-dap as a debug adapter in the Zed editor.
//! To contribute this upstream, this file would go into:
//!   zed-industries/zed/crates/dap_adapters/src/solidity.rs
//!
//! And be registered in:
//!   zed-industries/zed/crates/dap_adapters/src/dap_adapters.rs
//!
//! # Integration Steps
//!
//! 1. Copy this file to `zed/crates/dap_adapters/src/solidity.rs`
//! 2. Add to `zed/crates/dap_adapters/src/dap_adapters.rs`:
//!    ```ignore
//!    mod solidity;
//!    use solidity::SolidityDebugAdapter;
//!    ```
//! 3. Register in the adapter registry:
//!    ```ignore
//!    registry.register(Arc::new(SolidityDebugAdapter));
//!    ```
//! 4. Add Solidity language support to `zed/crates/languages/src/solidity.rs`

// NOTE: These imports reference Zed's internal crates, not sol-dap's dependencies.
// Uncomment when integrating into Zed's codebase.
//
// use anyhow::Result;
// use async_trait::async_trait;
// use collections::HashMap;
// use dap::adapters::*;
// use gpui::AsyncApp;
// use language::LanguageName;
// use std::path::PathBuf;
// use std::sync::Arc;

/// Debug adapter for Foundry Solidity projects using sol-dap.
///
/// This adapter enables debugging of Solidity smart contracts in Zed by:
/// - Launching sol-dap as a DAP server
/// - Configuring test and script debugging
/// - Supporting fork testing with custom RPC URLs
/// - Providing Foundry profile and verbosity options
pub struct SolidityDebugAdapter;

// #[async_trait(?Send)]
// impl DebugAdapter for SolidityDebugAdapter {
//     fn name(&self) -> DebugAdapterName {
//         DebugAdapterName("Foundry".into())
//     }
//
//     fn adapter_language_name(&self) -> Option<LanguageName> {
//         Some(LanguageName::new("Solidity"))
//     }
//
//     async fn get_binary(
//         &self,
//         delegate: &Arc<dyn DapDelegate>,
//         config: &DebugTaskDefinition,
//         user_installed_path: Option<PathBuf>,
//         user_args: Option<Vec<String>>,
//         user_env: Option<HashMap<String, String>>,
//         _cx: &mut AsyncApp,
//     ) -> Result<DebugAdapterBinary> {
//         // Find sol-dap binary in PATH or use user-provided path
//         let binary_path = if let Some(path) = user_installed_path {
//             path
//         } else {
//             delegate
//                 .which(std::ffi::OsStr::new("sol-dap"))
//                 .await
//                 .ok_or_else(|| anyhow::anyhow!(
//                     "sol-dap not found in PATH. Install with: cargo install sol-dap"
//                 ))?
//         };
//
//         let request_kind = self.request_kind(&config.config).await?;
//
//         Ok(DebugAdapterBinary {
//             command: Some(binary_path.to_string_lossy().into_owned()),
//             arguments: user_args.unwrap_or_default(),
//             envs: user_env.unwrap_or_default(),
//             cwd: Some(delegate.worktree_root_path().to_path_buf()),
//             connection: None, // stdio transport
//             request_args: StartDebuggingRequestArguments {
//                 request: request_kind,
//                 configuration: config.config.clone(),
//             },
//         })
//     }
//
//     fn dap_schema(&self) -> serde_json::Value {
//         serde_json::json!({
//             "type": "object",
//             "required": ["project_root"],
//             "properties": {
//                 "request": {
//                     "type": "string",
//                     "enum": ["launch"],
//                     "default": "launch"
//                 },
//                 "project_root": {
//                     "type": "string",
//                     "description": "Path to the Foundry project root"
//                 },
//                 "test": {
//                     "type": "string",
//                     "description": "Test function name to debug (e.g., 'testTransfer')"
//                 },
//                 "contract": {
//                     "type": "string",
//                     "description": "Test contract name (e.g., 'TokenTest')"
//                 },
//                 "script": {
//                     "type": "string",
//                     "description": "Script path to debug (e.g., 'script/Deploy.s.sol')"
//                 },
//                 "sig": {
//                     "type": "string",
//                     "description": "Script function signature (e.g., 'run()')"
//                 },
//                 "profile": {
//                     "type": "string",
//                     "description": "Foundry profile name"
//                 },
//                 "fork_url": {
//                     "type": "string",
//                     "description": "RPC URL for forked testing"
//                 },
//                 "fork_block_number": {
//                     "type": "integer",
//                     "description": "Block number to fork from"
//                 },
//                 "verbosity": {
//                     "type": "integer",
//                     "minimum": 0,
//                     "maximum": 5,
//                     "description": "Verbosity level (0-5)"
//                 }
//             }
//         })
//     }
//
//     async fn config_from_zed_format(
//         &self,
//         zed_scenario: task::ZedDebugConfig,
//     ) -> Result<task::DebugScenario> {
//         Ok(task::DebugScenario {
//             adapter: zed_scenario.adapter,
//             label: zed_scenario.label,
//             build: None,
//             config: serde_json::to_value(zed_scenario.request)?,
//             tcp_connection: None,
//         })
//     }
// }

// # Example Zed Configuration
//
// Users would configure sol-dap debugging in their `.zed/settings.json`:
//
// ```json
// {
//   "debug": {
//     "configurations": [
//       {
//         "name": "Debug Solidity Test",
//         "type": "Foundry",
//         "request": "launch",
//         "project_root": "${workspaceFolder}",
//         "contract": "TokenTest",
//         "test": "testTransfer"
//       },
//       {
//         "name": "Debug Script",
//         "type": "Foundry",
//         "request": "launch",
//         "project_root": "${workspaceFolder}",
//         "script": "script/Deploy.s.sol",
//         "sig": "run()"
//       },
//       {
//         "name": "Debug with Fork",
//         "type": "Foundry",
//         "request": "launch",
//         "project_root": "${workspaceFolder}",
//         "contract": "IntegrationTest",
//         "test": "testMainnetIntegration",
//         "fork_url": "https://eth-mainnet.g.alchemy.com/v2/YOUR_KEY",
//         "fork_block_number": 19000000
//       }
//     ]
//   }
// }
// ```
