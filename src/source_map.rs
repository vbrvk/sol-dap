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

/// Check if a step's source mapping indicates a function return (Jump::Out).
pub fn is_jump_out(
    step: &CallTraceStep,
    contract_name: &str,
    sources: &ContractSources,
    is_create: bool,
) -> bool {
    use foundry_compilers::artifacts::sourcemap::Jump;
    sources
        .find_source_mapping(contract_name, step.pc as u32, is_create)
        .is_some_and(|(elem, _)| elem.jump() == Jump::Out)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_offset_first_char() {
        let (line, col) = byte_offset_to_line_col("hello\nworld", 0);
        assert_eq!(line, 1);
        assert_eq!(col, 1);
    }

    #[test]
    fn byte_offset_mid_first_line() {
        let (line, col) = byte_offset_to_line_col("hello\nworld", 3);
        assert_eq!(line, 1);
        assert_eq!(col, 4); // 'l' at index 3
    }

    #[test]
    fn byte_offset_second_line_start() {
        // "hello\n" = 6 bytes, so offset 6 = first char of line 2
        let (line, col) = byte_offset_to_line_col("hello\nworld", 6);
        assert_eq!(line, 2);
        assert_eq!(col, 1);
    }

    #[test]
    fn byte_offset_second_line_mid() {
        let (line, col) = byte_offset_to_line_col("hello\nworld", 8);
        assert_eq!(line, 2);
        assert_eq!(col, 3); // 'r' at offset 8
    }

    #[test]
    fn byte_offset_beyond_end_clamped() {
        let (line, col) = byte_offset_to_line_col("ab", 100);
        // Should clamp to end (offset=2), line=1, col=3
        assert_eq!(line, 1);
        assert_eq!(col, 3);
    }

    #[test]
    fn byte_offset_empty_source() {
        let (line, col) = byte_offset_to_line_col("", 0);
        assert_eq!(line, 1);
        assert_eq!(col, 1);
    }

    #[test]
    fn byte_offset_multiple_newlines() {
        let source = "a\nb\nc\nd";
        let (line, col) = byte_offset_to_line_col(source, 6); // 'c' at index 4, then newline at 5, 'd' at 6
        assert_eq!(line, 4);
        assert_eq!(col, 1);
    }

    #[test]
    fn source_location_fields_populated() {
        let loc = SourceLocation {
            path: PathBuf::from("test.sol"),
            line: 10,
            column: 5,
            length: 20,
        };
        assert_eq!(loc.path, PathBuf::from("test.sol"));
        assert_eq!(loc.line, 10);
        assert_eq!(loc.column, 5);
        assert_eq!(loc.length, 20);
    }
}
