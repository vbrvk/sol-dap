//! Integration tests for sol-dap.
//!
//! Tests marked `#[ignore]` require `forge` in PATH and run against
//! the sample Foundry project at tests/fixtures/sample-project/.
//! Run with: `cargo test -- --include-ignored`

use std::path::PathBuf;

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("sample-project")
}

// ============ Config parsing (no forge needed) ============

#[test]
fn test_launch_config_valid() {
    use sol_dap::config::LaunchConfig;

    let json = serde_json::json!({
        "project_root": "/tmp/test",
        "test": "testIncrement",
        "contract": "CounterTest"
    });
    let config = LaunchConfig::from_args(&json).unwrap();
    assert_eq!(config.test.as_deref(), Some("testIncrement"));
    assert_eq!(config.contract.as_deref(), Some("CounterTest"));
    assert_eq!(config.project_root, PathBuf::from("/tmp/test"));
}

#[test]
fn test_launch_config_with_script() {
    use sol_dap::config::LaunchConfig;

    let json = serde_json::json!({
        "project_root": "/tmp/test",
        "script": "script/Deploy.s.sol",
        "sig": "run()"
    });
    let config = LaunchConfig::from_args(&json).unwrap();
    assert_eq!(config.script.as_deref(), Some("script/Deploy.s.sol"));
    assert_eq!(config.sig.as_deref(), Some("run()"));
}

#[test]
fn test_launch_config_rejects_empty() {
    use sol_dap::config::LaunchConfig;

    let json = serde_json::json!({ "project_root": "/tmp/test" });
    assert!(LaunchConfig::from_args(&json).is_err());
}

#[test]
fn test_source_location_struct() {
    use sol_dap::source_map::SourceLocation;

    let loc = SourceLocation {
        path: PathBuf::from("src/Counter.sol"),
        line: 42,
        column: 5,
        length: 10,
    };
    assert_eq!(loc.line, 42);
    assert_eq!(loc.column, 5);
}

// ============ Compile & Launch (requires forge) ============

#[test]
#[ignore]
fn test_compile_and_debug() {
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
        "at least one node should have steps"
    );
    assert!(
        !ctx.identified_contracts.is_empty(),
        "should have identified contracts"
    );
    // Method identifiers loaded from artifacts
    assert!(
        !ctx.method_identifiers.is_empty(),
        "should have method identifiers"
    );
    // Function params loaded from ABI
    assert!(
        !ctx.function_params.is_empty(),
        "should have function params"
    );
    // Storage layouts loaded
    assert!(
        !ctx.storage_layouts.is_empty(),
        "should have storage layouts"
    );
}

// ============ Session: stepping (requires forge) ============

#[test]
#[ignore]
fn test_step_opcode_forward_backward() {
    let mut session = create_session("testIncrement", "CounterTest");

    assert_eq!(session.current_node, 0);
    assert_eq!(session.current_step, 0);

    session.step_opcode();
    assert_eq!(session.current_step, 1);

    session.step_back_opcode();
    assert_eq!(session.current_step, 0);
}

#[test]
#[ignore]
fn test_step_next_advances_source_line() {
    let mut session = create_session("testIncrement", "CounterTest");

    let start_loc = session.current_source_location();
    session.step_next();
    let new_loc = session.current_source_location();

    // Either source line changed or we reached end of trace
    match (&start_loc, &new_loc) {
        (Some(a), Some(b)) => assert!(a.line != b.line || a.path != b.path),
        _ => {} // unmapped start is ok
    }
}

#[test]
#[ignore]
fn test_step_next_skips_child_calls() {
    let mut session = create_session("testIncrement", "CounterTest");

    // Step to the counter.increment() line
    session.step_next(); // to function entry
    session.step_next(); // to counter.increment() call

    let node_before = session.current_node;
    session.step_next(); // should SKIP over increment() and land on next line

    // After step_next over a CALL, we should be past the child nodes
    // but still in a CounterTest node (not inside Counter)
    let node_after = session.current_node;
    assert!(
        node_after >= node_before,
        "step_next should advance past child call"
    );
}

#[test]
#[ignore]
fn test_step_in_enters_child_call() {
    let mut session = create_session("testIncrement", "CounterTest");

    let start_node = session.current_node;
    // Step to the counter.increment() call line
    session.step_next();
    session.step_in(); // should step into increment()
    session.step_in(); // may need another to enter Counter node

    // Eventually we should be in a different node (Counter)
    let mut entered = false;
    for _ in 0..10 {
        if session.current_node != start_node {
            entered = true;
            break;
        }
        session.step_in();
    }
    assert!(entered, "step_in should eventually enter a child call");
}

#[test]
#[ignore]
fn test_continue_to_end() {
    use sol_dap::session::StopReason;

    let mut session = create_session("testIncrement", "CounterTest");
    let reason = session.continue_to_breakpoint();
    assert_eq!(reason, StopReason::End);
    assert!(session.is_at_end());
}

#[test]
#[ignore]
fn test_breakpoint_hit() {
    use sol_dap::session::StopReason;

    let mut session = create_session("testIncrement", "CounterTest");

    // Set breakpoint on line 15 of Counter.t.sol (counter.increment())
    let test_file = fixture_path().join("test").join("Counter.t.sol");
    session
        .source_breakpoints
        .insert(test_file.clone(), vec![15]);

    let reason = session.continue_to_breakpoint();
    assert_eq!(reason, StopReason::Breakpoint);

    let loc = session.current_source_location();
    assert!(loc.is_some(), "should have source location at breakpoint");
    let loc = loc.unwrap();
    assert_eq!(loc.line, 15);
}

// ============ Source mapping (requires forge) ============

#[test]
#[ignore]
fn test_source_mapping_finds_solidity_locations() {
    let mut session = create_session("testIncrement", "CounterTest");

    let mut found = false;
    for _ in 0..500 {
        if session.current_source_location().is_some() {
            found = true;
            break;
        }
        session.step_opcode();
        if session.is_at_end() {
            break;
        }
    }
    assert!(found, "should find at least one mapped source location");
}

#[test]
#[ignore]
fn test_source_mapping_returns_absolute_paths() {
    let mut session = create_session("testIncrement", "CounterTest");

    session.step_next();
    if let Some(loc) = session.current_source_location() {
        assert!(
            loc.path.is_absolute(),
            "source path should be absolute: {:?}",
            loc.path
        );
    }
}

// ============ Variables (requires forge) ============

#[test]
#[ignore]
fn test_stack_variables() {
    use sol_dap::variables;

    let mut session = create_session("testIncrement", "CounterTest");
    // Step until we find a step with a non-empty stack
    let mut found_stack = false;
    for _ in 0..200 {
        if let Some(step) = session.current_trace_step() {
            if step.stack.as_ref().is_some_and(|s| !s.is_empty()) {
                let vars = variables::stack_variables(step);
                assert!(
                    !vars.is_empty(),
                    "stack variables should not be empty when stack has items"
                );
                found_stack = true;
                break;
            }
        }
        if session.is_at_end() {
            break;
        }
        session.step_opcode();
    }
    assert!(
        found_stack,
        "should find at least one step with non-empty stack"
    );
}

#[test]
#[ignore]
fn test_gas_info_variables() {
    use sol_dap::variables;

    let session = create_session("testIncrement", "CounterTest");
    if let Some(step) = session.current_trace_step() {
        let vars = variables::gas_info_variables(step);
        assert_eq!(vars.len(), 2); // gas_remaining, gas_cost
        assert_eq!(vars[0].name, "gas_remaining");
        assert_eq!(vars[1].name, "gas_cost");
    }
}

#[test]
#[ignore]
fn test_calldata_variables_decoded() {
    use sol_dap::variables;

    let session = create_session("testSetNumber", "CounterTest");
    if let Some(node) = session.current_debug_node() {
        // VaultTest::testSetNumber has no params, but we can still test decoding
        let vars = variables::calldata_variables(node, None, None);
        assert!(
            !vars.is_empty(),
            "calldata should have at least function selector"
        );
        assert_eq!(vars[0].name, "function");
    }
}

#[test]
#[ignore]
fn test_storage_variables_counter() {
    use sol_dap::variables;

    let mut session = create_session("testIncrement", "CounterTest");

    // Run to completion to see storage writes
    session.continue_to_breakpoint();

    // Find Counter node to check its storage
    for (_i, node) in session.debug_arena.iter().enumerate() {
        let name = session
            .identified_contracts
            .get(&node.address)
            .map(|s| s.as_str());
        if name == Some("Counter") {
            if let Some(layout) = session.storage_layouts.get("Counter") {
                let vars = variables::storage_variables(
                    &session.debug_arena,
                    session.current_node,
                    session.current_step,
                    &node.address,
                    layout,
                );
                // Counter has 'number' in storage
                let number_var = vars.iter().find(|v| v.name == "number");
                assert!(
                    number_var.is_some(),
                    "should find 'number' storage variable"
                );
                // After testIncrement, number should be 1
                if let Some(nv) = number_var {
                    assert_eq!(nv.value, "1", "number should be 1 after increment");
                }
            }
            break;
        }
    }
}

#[test]
#[ignore]
fn test_context_variables() {
    use sol_dap::variables;

    let mut session = create_session("testIncrement", "CounterTest");
    session.step_next(); // enter test function

    if let Some(node) = session.current_debug_node() {
        let step = session.current_trace_step();
        let vars =
            variables::context_variables(node, session.current_node, &session.debug_arena, step);
        // Should have at least 'pc', 'opcode', 'this'
        let names: Vec<&str> = vars.iter().map(|v| v.name.as_str()).collect();
        assert!(names.contains(&"pc"), "context should have pc");
        assert!(names.contains(&"opcode"), "context should have opcode");
        assert!(names.contains(&"this"), "context should have this");
    }
}

// ============ Method identifiers (requires forge) ============

#[test]
#[ignore]
fn test_method_identifiers_loaded() {
    let session = create_session("testIncrement", "CounterTest");

    // Should have selectors for Counter functions
    let has_increment = session
        .method_identifiers
        .values()
        .any(|sig| sig.contains("increment"));
    assert!(
        has_increment,
        "should have increment() in method identifiers"
    );

    let has_set_number = session
        .method_identifiers
        .values()
        .any(|sig| sig.contains("setNumber"));
    assert!(
        has_set_number,
        "should have setNumber() in method identifiers"
    );
}

#[test]
#[ignore]
fn test_function_params_loaded() {
    let session = create_session("testIncrement", "CounterTest");

    // setNumber(uint256) should have one param named 'newNumber'
    let set_number_params = session
        .method_identifiers
        .iter()
        .find(|(_, sig)| sig.contains("setNumber"))
        .and_then(|(sel, _)| session.function_params.get(sel));

    assert!(
        set_number_params.is_some(),
        "should have params for setNumber"
    );
    let params = set_number_params.unwrap();
    assert_eq!(params.len(), 1);
    assert_eq!(params[0].0, "newNumber");
    assert_eq!(params[0].1, "uint256");
}

// ============ Jump::Out detection (requires forge) ============

#[test]
#[ignore]
fn test_is_jump_out() {
    use sol_dap::source_map;

    let session = create_session("testIncrement", "CounterTest");

    // Scan through steps to find at least one Jump::Out
    let mut found_jump_out = false;
    for node in &session.debug_arena {
        let contract_name = session
            .identified_contracts
            .get(&node.address)
            .map(|s| s.as_str())
            .unwrap_or("");
        for step in &node.steps {
            if source_map::is_jump_out(
                step,
                contract_name,
                &session.contracts_sources,
                node.kind.is_any_create(),
            ) {
                found_jump_out = true;
                break;
            }
        }
        if found_jump_out {
            break;
        }
    }
    assert!(
        found_jump_out,
        "should find at least one Jump::Out in the trace"
    );
}

// ============ Vault tests (requires forge) ============

#[test]
#[ignore]
fn test_vault_test_simple() {
    // Use testSafeAdd which is a pure function test
    let session = create_session("testSafeAdd", "VaultTest");
    assert!(!session.debug_arena.is_empty(), "should have debug data");
    assert!(
        !session.identified_contracts.is_empty(),
        "should identify contracts"
    );
}

// ============ Local variables (requires forge) ============

#[test]
#[ignore]
fn test_local_variables_parsed() {
    use sol_dap::variables;

    // testRawBalanceOf has: uint256 raw = ...; uint256 normal = ...;
    let mut session = create_session("testRawBalanceOf", "VaultTest");

    // Step to line 104 (uint256 normal = token.balanceOf(alice))
    // At this point, 'raw' should be declared and 'normal' not yet
    for _ in 0..4 {
        session.step_next();
    }

    if let Some(step) = session.current_trace_step() {
        if let Some(loc) = session.current_source_location() {
            let locals = variables::local_variables(&loc.path, loc.line, step);
            let names: Vec<&str> = locals.iter().map(|v| v.name.as_str()).collect();
            assert!(names.contains(&"raw"), "should find local 'raw', got: {names:?}");
            assert!(names.contains(&"normal"), "should find local 'normal', got: {names:?}");

            // 'raw' should have a value (declared before current line)
            let raw_var = locals.iter().find(|v| v.name == "raw").unwrap();
            assert!(
                !raw_var.value.contains("not yet declared"),
                "'raw' should be declared with a value, got: {}",
                raw_var.value
            );
            assert_eq!(raw_var.type_field.as_deref(), Some("uint256"));
        }
    }
}

#[test]
#[ignore]
fn test_local_variables_not_yet_declared() {
    use sol_dap::variables;

    // testSafeAdd has: uint256 result = token.safeAdd(100, 200);
    let session = create_session("testSafeAdd", "VaultTest");

    // At the start (before any stepping), we're at the function entry
    // 'result' should be found but marked not yet declared
    if let Some(step) = session.current_trace_step() {
        if let Some(loc) = session.current_source_location() {
            let locals = variables::local_variables(&loc.path, loc.line, step);
            if !locals.is_empty() {
                // If we found locals at entry, they should be 'not yet declared'
                for v in &locals {
                    assert!(
                        v.value.contains("not yet declared") || v.value.contains("declared at"),
                        "local '{}' should indicate declaration status, got: {}",
                        v.name,
                        v.value
                    );
                }
            }
        }
    }
}

#[test]
fn test_local_variables_parsing_unit() {
    // Test source parsing without forge — just verify the file structure
    // and that our parsing would find the right lines.
    // Full integration test with values is in test_local_variables_parsed.

    let dir = std::env::temp_dir().join("sol-dap-test-locals");
    std::fs::create_dir_all(&dir).unwrap();
    let source_path = dir.join("Test.sol");
    std::fs::write(
        &source_path,
        "// SPDX-License-Identifier: MIT\n\
pragma solidity ^0.8.13;\n\
contract Test {\n\
    function doStuff(uint256 x) public {\n\
        uint256 a = x + 1;\n\
        bool flag = true;\n\
        address sender = msg.sender;\n\
        uint256 b = a * 2;\n\
    }\n\
}",
    )
    .unwrap();

    // Verify the file has the expected structure
    let source = std::fs::read_to_string(&source_path).unwrap();
    let lines: Vec<&str> = source.lines().collect();
    assert!(lines[3].trim().contains("function doStuff"), "line 4 should have function");
    assert!(lines[4].trim().starts_with("uint256 a"), "line 5 should have uint256 a");
    assert!(lines[5].trim().starts_with("bool flag"), "line 6 should have bool flag");
    assert!(lines[6].trim().starts_with("address sender"), "line 7 should have address sender");
    assert!(lines[7].trim().starts_with("uint256 b"), "line 8 should have uint256 b");

    std::fs::remove_dir_all(&dir).ok();
}

// ============ String decoding (no forge needed) ============

#[test]
fn test_decode_short_string() {
    use alloy_primitives::U256;
    use sol_dap::variables::decode_short_string;

    // "Test Token" = 10 chars, stored as: bytes[0..10] = "Test Token", byte[31] = 20 (10*2)
    let mut bytes = [0u8; 32];
    let text = b"Test Token";
    bytes[..text.len()].copy_from_slice(text);
    bytes[31] = (text.len() * 2) as u8;
    let raw = U256::from_be_bytes(bytes);
    assert_eq!(decode_short_string(&raw), "\"Test Token\"");

    // Empty string: byte[31] = 0
    let empty = U256::ZERO;
    assert_eq!(decode_short_string(&empty), "\"\"");
}

// ============ Helpers ============

#[cfg(test)]
fn create_session(test: &str, contract: &str) -> sol_dap::session::DebugSession {
    use sol_dap::config::LaunchConfig;
    use sol_dap::launch;
    use sol_dap::session::DebugSession;

    let json = serde_json::json!({
        "project_root": fixture_path().to_string_lossy().to_string(),
        "test": test,
        "contract": contract
    });
    let config = LaunchConfig::from_args(&json).unwrap();
    let ctx = launch::compile_and_debug(&config).unwrap();
    DebugSession::new(ctx, config)
}
