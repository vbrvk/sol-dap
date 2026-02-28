use std::path::PathBuf;

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("sample-project")
}

#[test]
fn test_launch_config_parsing() {
    use sol_dap::config::LaunchConfig;

    let json = serde_json::json!({
        "project_root": "/tmp/test",
        "test": "testIncrement",
        "contract": "CounterTest"
    });

    let config = LaunchConfig::from_args(&json).unwrap();
    assert_eq!(config.test.as_deref(), Some("testIncrement"));
    assert_eq!(config.contract.as_deref(), Some("CounterTest"));
}

#[test]
fn test_launch_config_validation_fails_without_test_or_script() {
    use sol_dap::config::LaunchConfig;

    let json = serde_json::json!({
        "project_root": "/tmp/test"
    });

    assert!(LaunchConfig::from_args(&json).is_err());
}

#[test]
fn test_source_map_module_is_accessible() {
    use sol_dap::source_map::SourceLocation;

    let loc = SourceLocation {
        path: PathBuf::from("/tmp/example.sol"),
        line: 1,
        column: 1,
        length: 0,
    };

    assert_eq!(loc.line, 1);
    assert_eq!(loc.column, 1);
}

#[test]
#[ignore]
fn test_compile_and_debug_with_fixture() {
    use sol_dap::config::LaunchConfig;
    use sol_dap::launch;

    let json = serde_json::json!({
        "project_root": fixture_path().to_string_lossy().to_string(),
        "test": "testIncrement",
        "contract": "CounterTest"
    });

    let config = LaunchConfig::from_args(&json).unwrap();
    let ctx = launch::compile_and_debug(&config).unwrap();

    assert!(
        !ctx.debug_arena.is_empty(),
        "debug_arena should not be empty"
    );
    assert!(
        ctx.debug_arena.iter().any(|n| !n.steps.is_empty()),
        "expected at least one debug node with steps"
    );
}

#[test]
#[ignore]
fn test_session_stepping_and_basic_inspection() {
    use sol_dap::config::LaunchConfig;
    use sol_dap::launch;
    use sol_dap::session::DebugSession;
    use sol_dap::{source_map, variables};

    let json = serde_json::json!({
        "project_root": fixture_path().to_string_lossy().to_string(),
        "test": "testIncrement",
        "contract": "CounterTest"
    });

    let config = LaunchConfig::from_args(&json).unwrap();
    let ctx = launch::compile_and_debug(&config).unwrap();
    let mut session = DebugSession::new(ctx, config);

    assert_eq!(session.current_node, 0);
    assert_eq!(session.current_step, 0);

    let before = (session.current_node, session.current_step);
    session.step_opcode();
    let after = (session.current_node, session.current_step);
    assert_ne!(after, before);

    session.step_back_opcode();
    let back = (session.current_node, session.current_step);
    assert_eq!(back, before);

    session.step_next();
    let advanced = (session.current_node, session.current_step);
    assert!(advanced != before || session.is_at_end());

    if let Some(step) = session.current_trace_step() {
        let vars = variables::gas_info_variables(step);
        assert_eq!(vars.len(), 4);
    }

    let mut found_loc = None;
    for _ in 0..512 {
        if let Some(node) = session.current_debug_node() {
            if let Some(step) = session.current_trace_step() {
                let contract_name = session.current_contract_name().unwrap_or("Unknown");
                found_loc = source_map::step_to_source(
                    step,
                    contract_name,
                    &session.contracts_sources,
                    node.kind.is_any_create(),
                );
                if found_loc.is_some() {
                    break;
                }
            }
        }
        if session.is_at_end() {
            break;
        }
        session.step_opcode();
    }

    assert!(
        found_loc.is_some(),
        "expected at least one step to map to a source location"
    );
}
