use serde_json::Value;
use zed_extension_api::{
    self as zed, serde_json, DebugAdapterBinary, DebugConfig, DebugRequest, DebugScenario,
    DebugTaskDefinition, Result, StartDebuggingRequestArguments,
    StartDebuggingRequestArgumentsRequest, TaskTemplate, Worktree,
};

struct SolDapExtension;

impl SolDapExtension {
    /// Extract test name and contract name from forge test command args.
    /// Recognizes patterns like:
    ///   forge test --match-test testFoo --match-contract Bar
    ///   forge test --mt testFoo
    fn parse_forge_test_args(args: &[String]) -> Option<(String, Option<String>)> {
        let mut test_name = None;
        let mut contract_name = None;
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--match-test" | "--mt" | "-t" => {
                    test_name = iter.next().cloned();
                }
                "--match-contract" | "--mc" => {
                    contract_name = iter.next().cloned();
                }
                other => {
                    // Handle --match-test=foo or --mt=foo
                    for prefix in ["--match-test=", "--mt="] {
                        if let Some(val) = other.strip_prefix(prefix) {
                            test_name = Some(val.to_string());
                        }
                    }
                    for prefix in ["--match-contract=", "--mc="] {
                        if let Some(val) = other.strip_prefix(prefix) {
                            contract_name = Some(val.to_string());
                        }
                    }
                }
            }
        }
        test_name.map(|t| (t, contract_name))
    }
}

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
        let command = user_provided_debug_adapter_path
            .unwrap_or_else(|| "sol-dap".to_string());

        Ok(DebugAdapterBinary {
            command: Some(command),
            arguments: vec![],
            envs: vec![],
            cwd: None,
            connection: None,
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
        Ok(StartDebuggingRequestArgumentsRequest::Launch)
    }

    fn dap_config_to_scenario(&mut self, config: DebugConfig) -> Result<DebugScenario, String> {
        let mut dap_config = serde_json::Map::new();
        dap_config.insert("request".to_string(), Value::String("launch".to_string()));

        match &config.request {
            DebugRequest::Launch(launch) => {
                if !launch.program.is_empty() {
                    dap_config.insert("project_root".to_string(), Value::String(launch.program.clone()));
                }
                if let Some(cwd) = &launch.cwd {
                    dap_config.insert("project_root".to_string(), Value::String(cwd.clone()));
                }
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

    /// Locator: converts `forge test --match-test testFoo` tasks into debug scenarios.
    /// This is what enables the "Debug" button next to forge test tasks in Zed's UI.
    fn dap_locator_create_scenario(
        &mut self,
        _locator_name: String,
        build_task: TaskTemplate,
        resolved_label: String,
        _debug_adapter_name: String,
    ) -> Option<DebugScenario> {
        // Only handle forge test commands.
        if build_task.command != "forge" {
            return None;
        }
        if !build_task.args.iter().any(|a| a == "test") {
            return None;
        }

        // Extract test name from args.
        let (test_name, contract_name) = Self::parse_forge_test_args(&build_task.args)?;

        // Build the sol-dap launch config.
        let mut dap_config = serde_json::Map::new();
        dap_config.insert("request".to_string(), Value::String("launch".to_string()));
        if let Some(cwd) = &build_task.cwd {
            dap_config.insert("project_root".to_string(), Value::String(cwd.clone()));
        }
        dap_config.insert("test".to_string(), Value::String(test_name.clone()));
        if let Some(contract) = &contract_name {
            dap_config.insert("contract".to_string(), Value::String(contract.clone()));
        }

        let config_str = serde_json::to_string(&Value::Object(dap_config)).ok()?;

        Some(DebugScenario {
            label: format!("Debug {resolved_label}"),
            adapter: "sol-dap".to_string(),
            config: config_str,
            tcp_connection: None,
            build: None, // No build step needed — sol-dap runs forge internally.
        })
    }
}

zed::register_extension!(SolDapExtension);
