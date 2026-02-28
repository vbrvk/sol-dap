use serde_json::Value;
use zed_extension_api::{
    self as zed, serde_json, DebugAdapterBinary, DebugConfig, DebugRequest, DebugScenario,
    DebugTaskDefinition, Result, StartDebuggingRequestArguments,
    StartDebuggingRequestArgumentsRequest, Worktree,
};

struct SolDapExtension;

impl zed::Extension for SolDapExtension {
    fn new() -> Self {
        Self
    }

    fn get_dap_binary(
        &mut self,
        _adapter_name: String,
        config: DebugTaskDefinition,
        user_provided_debug_adapter_path: Option<String>,
        _worktree: &Worktree,
    ) -> Result<DebugAdapterBinary, String> {
        // Use user-provided path if available, otherwise look for sol-dap in PATH.
        let command = if let Some(path) = user_provided_debug_adapter_path {
            path
        } else {
            "sol-dap".to_string()
        };

        Ok(DebugAdapterBinary {
            command: Some(command),
            arguments: vec![],
            envs: vec![],
            cwd: None,
            connection: None, // stdio transport
            request_args: StartDebuggingRequestArguments {
                configuration: config.config,
                request: StartDebuggingRequestArgumentsRequest::Launch,
            },
        })
    }

    fn dap_request_kind(
        &mut self,
        _adapter_name: String,
        _config: Value,
    ) -> Result<StartDebuggingRequestArgumentsRequest, String> {
        // sol-dap only supports launch (post-mortem debugging of recorded traces).
        Ok(StartDebuggingRequestArgumentsRequest::Launch)
    }

    fn dap_config_to_scenario(&mut self, config: DebugConfig) -> Result<DebugScenario, String> {
        let mut dap_config = serde_json::Map::new();
        dap_config.insert("request".to_string(), Value::String("launch".to_string()));

        // Forward all fields from the generic config into the DAP launch config.
        match &config.request {
            DebugRequest::Launch(launch) => {
                // program field maps to project_root for sol-dap.
                if !launch.program.is_empty() {
                    dap_config.insert(
                        "project_root".to_string(),
                        Value::String(launch.program.clone()),
                    );
                }
                if let Some(cwd) = &launch.cwd {
                    dap_config.insert(
                        "project_root".to_string(),
                        Value::String(cwd.clone()),
                    );
                }
                // Pass args as the test name if provided.
                if let Some(first_arg) = launch.args.first() {
                    dap_config.insert("test".to_string(), Value::String(first_arg.clone()));
                }
                if let Some(second_arg) = launch.args.get(1) {
                    dap_config.insert("contract".to_string(), Value::String(second_arg.clone()));
                }
            }
            DebugRequest::Attach(_) => {
                return Err("sol-dap only supports launch requests".to_string());
            }
        }

        Ok(DebugScenario {
            label: config.label.clone(),
            adapter: "sol-dap".to_string(),
            config: serde_json::to_string(&Value::Object(dap_config))
                .map_err(|e| format!("failed to serialize config: {e}"))?,
            tcp_connection: None,
            build: None,
        })
    }
}

zed::register_extension!(SolDapExtension);
