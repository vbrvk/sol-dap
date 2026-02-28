use foundry_evm_traces::debug::ContractSources;
use revm_inspectors::tracing::types::CallTraceStep;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct SourceLocation {
    pub path: PathBuf,
    pub line: i64,
    pub column: i64,
    pub length: usize,
}

pub fn step_to_source(
    step: &CallTraceStep,
    contract_name: &str,
    sources: &ContractSources,
    is_create: bool,
    project_root: &std::path::Path,
) -> Option<SourceLocation> {
    tracing::debug!("find_source_mapping: contract={contract_name}, pc={}, is_create={is_create}", step.pc);
    let (source_element, source_data) =
        sources.find_source_mapping(contract_name, step.pc as u32, is_create)?;

    let offset = source_element.offset() as usize;
    let length = source_element.length() as usize;
    let source_text: &str = source_data.source.as_str();

    let (line, column) = byte_offset_to_line_col(source_text, offset);

    tracing::debug!("source_map hit: path={}, line={line}, col={column}", source_data.path.display());
    Some(SourceLocation {
        path: if source_data.path.is_absolute() {
            source_data.path.clone()
        } else {
            project_root.join(&source_data.path)
        },
        line,
        column,
        length,
    })
}

fn byte_offset_to_line_col(source: &str, offset: usize) -> (i64, i64) {
    let offset = offset.min(source.len());
    let bytes = source.as_bytes();

    let mut line: i64 = 1;
    let mut last_newline: usize = 0;
    for (i, &b) in bytes.iter().enumerate().take(offset) {
        if b == b'\n' {
            line += 1;
            last_newline = i + 1;
        }
    }

    let column = (offset.saturating_sub(last_newline)) as i64 + 1;
    (line, column)
}
