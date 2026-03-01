use alloy_primitives::hex;
use dap::types::Variable;
use foundry_debugger::DebugNode;
use revm_inspectors::tracing::types::CallTraceStep;

pub fn stack_variables(step: &CallTraceStep) -> Vec<Variable> {
    let Some(stack) = &step.stack else {
        tracing::debug!("stack is None for pc={}", step.pc);
        return Vec::new();
    };
    tracing::debug!("stack has {} items for pc={}", stack.len(), step.pc);

    stack
        .iter()
        .enumerate()
        .map(|(i, val)| Variable {
            name: format!("[{i}]"),
            value: format!("0x{:x}", val),
            type_field: Some("uint256".to_string()),
            variables_reference: 0,
            ..Default::default()
        })
        .collect()
}

pub fn memory_variables(step: &CallTraceStep) -> Vec<Variable> {
    let Some(memory) = &step.memory else {
        return Vec::new();
    };

    let bytes = memory.as_ref();
    bytes
        .chunks(32)
        .enumerate()
        .map(|(i, chunk)| Variable {
            name: format!("0x{:04x}", i * 32),
            value: format!("0x{}", hex::encode(chunk)),
            type_field: Some("bytes32".to_string()),
            variables_reference: 0,
            ..Default::default()
        })
        .collect()
}

pub fn calldata_variables(node: &DebugNode) -> Vec<Variable> {
    let data = node.calldata.as_ref();
    if data.is_empty() {
        return Vec::new();
    }

    let mut vars = Vec::new();
    if data.len() >= 4 {
        vars.push(Variable {
            name: "selector".to_string(),
            value: format!("0x{}", hex::encode(&data[..4])),
            type_field: Some("bytes4".to_string()),
            variables_reference: 0,
            ..Default::default()
        });
    }

    let args = data.get(4..).unwrap_or_default();
    for (i, chunk) in args.chunks(32).enumerate() {
        vars.push(Variable {
            name: format!("arg[{i}]"),
            value: format!("0x{}", hex::encode(chunk)),
            type_field: Some("bytes32".to_string()),
            variables_reference: 0,
            ..Default::default()
        });
    }

    vars
}

pub fn returndata_variables(step: &CallTraceStep) -> Vec<Variable> {
    let data = step.returndata.as_ref();
    if data.is_empty() {
        return Vec::new();
    }

    data.chunks(32)
        .enumerate()
        .map(|(i, chunk)| Variable {
            name: format!("[{i}]"),
            value: format!("0x{}", hex::encode(chunk)),
            type_field: Some("bytes32".to_string()),
            variables_reference: 0,
            ..Default::default()
        })
        .collect()
}

pub fn gas_info_variables(step: &CallTraceStep) -> Vec<Variable> {
    vec![
        Variable {
            name: "pc".to_string(),
            value: format!("{}", step.pc),
            type_field: Some("uint".to_string()),
            variables_reference: 0,
            ..Default::default()
        },
        Variable {
            name: "opcode".to_string(),
            value: step.op.to_string(),
            type_field: Some("string".to_string()),
            variables_reference: 0,
            ..Default::default()
        },
        Variable {
            name: "gas_remaining".to_string(),
            value: format!("{}", step.gas_remaining),
            type_field: Some("uint64".to_string()),
            variables_reference: 0,
            ..Default::default()
        },
        Variable {
            name: "gas_cost".to_string(),
            value: format!("{}", step.gas_cost),
            type_field: Some("uint64".to_string()),
            variables_reference: 0,
            ..Default::default()
        },
    ]
}
