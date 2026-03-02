//! Expression parser for the debug console.
//!
//! Parses expressions like:
//!   - `0xff`                        → HexLiteral
//!   - `42`                          → DecLiteral
//!   - `pc`, `gas`, `this`           → Keyword
//!   - `stack[1]`                    → StackIndex
//!   - `memory[32]`, `memory[0:64]`  → MemoryAccess
//!   - `number`                      → Ident (storage var or other name)
//!   - `deposits[msg.sender]`        → MappingLookup
//!   - `deposits[msg.sender] + 10`   → BinaryOp
//!   - `stack[1] << 8`              → BinaryOp

/// Binary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
}

/// AST node for a debug console expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    /// Hex literal: `0xff`, `0xdeadbeef`
    HexLiteral(String),
    /// Decimal literal: `42`, `1000`
    DecLiteral(String),
    /// Keyword: `pc`, `op`, `gas`, `gas_cost`, `depth`, `step`,
    /// `address`, `this`, `msg.sender`, `caller`,
    /// `memory`, `memory.length`, `calldata`, `msg.data`, `returndata`,
    /// `stack`, `help`, `?`
    Keyword(String),
    /// Identifier (storage variable name, etc.): `number`, `fee`
    Ident(String),
    /// Stack index: `stack[0]`, `stack[12]`
    StackIndex(u64),
    LogAccess(u64),
    EventAccess {
        index: u64,
        field_index: Option<u64>,
    },
    EventField {
        index: u64,
        field: String,
    },
    /// Memory access: `memory[offset]` or `memory[offset:length]`
    MemoryAccess {
        offset: u64,
        length: Option<u64>,
    },
    /// Mapping/array lookup: `deposits[msg.sender]`, `_allowances[from][spender]`
    MappingLookup {
        name: String,
        keys: Vec<Expr>,
    },
    /// Binary operation: `lhs op rhs`
    BinaryOp {
        lhs: Box<Expr>,
        op: BinOp,
        rhs: Box<Expr>,
    },
}

/// Known keywords that map to specific DAP evaluation logic.
const KEYWORDS: &[&str] = &[
    "pc",
    "op",
    "opcode",
    "gas",
    "gas_cost",
    "depth",
    "node",
    "step",
    "address",
    "this",
    "msg.sender",
    "caller",
    "memory",
    "memory.length",
    "calldata",
    "msg.data",
    "returndata",
    "stack",
    "log",
    "help",
    "?",
];

fn is_keyword(s: &str) -> bool {
    KEYWORDS.contains(&s)
}

/// Parse an expression string into an AST.
pub fn parse(input: &str) -> Result<Expr, String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("empty expression".to_string());
    }
    parse_binary(input)
}

/// Try to parse a binary expression: `<atom> <op> <atom>`.
/// Scans for an operator outside of brackets (lowest precedence first).
/// If no operator is found, falls through to parse_atom.
fn parse_binary(input: &str) -> Result<Expr, String> {
    // Operator groups in ascending precedence (we split on lowest first,
    // so the lowest-precedence op becomes the root of the tree).
    // This gives us left-to-right evaluation for same-precedence ops.
    let op_groups: &[&[(&str, BinOp)]] = &[
        &[("|", BinOp::BitOr)],
        &[("^", BinOp::BitXor)],
        &[("&", BinOp::BitAnd)],
        &[("<<", BinOp::Shl), (">>", BinOp::Shr)],
        &[("+", BinOp::Add), ("-", BinOp::Sub)],
        &[("*", BinOp::Mul), ("/", BinOp::Div), ("%", BinOp::Mod)],
    ];

    for group in op_groups {
        // Scan right-to-left for left-associativity
        if let Some((pos, op_str, binop)) = find_rightmost_op(input, group) {
            let lhs = input[..pos].trim();
            let rhs = input[pos + op_str.len()..].trim();
            if lhs.is_empty() || rhs.is_empty() {
                continue; // not a valid binary expression at this position
            }
            let lhs_expr = parse_binary(lhs)?;
            let rhs_expr = parse_binary(rhs)?;
            return Ok(Expr::BinaryOp {
                lhs: Box::new(lhs_expr),
                op: binop,
                rhs: Box::new(rhs_expr),
            });
        }
    }

    parse_atom(input)
}

/// Find the rightmost occurrence of any operator in the group,
/// skipping operators inside brackets.
fn find_rightmost_op<'a>(input: &str, ops: &'a [(&str, BinOp)]) -> Option<(usize, &'a str, BinOp)> {
    let bytes = input.as_bytes();
    let mut best: Option<(usize, &'a str, BinOp)> = None;

    // Track bracket depth
    let mut depth = 0i32;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'[' => {
                depth += 1;
                i += 1;
                continue;
            }
            b']' => {
                depth -= 1;
                i += 1;
                continue;
            }
            _ => {}
        }
        if depth > 0 {
            i += 1;
            continue;
        }

        for &(op_str, binop) in ops {
            let op_bytes = op_str.as_bytes();
            if i + op_bytes.len() > bytes.len() {
                continue;
            }
            if &bytes[i..i + op_bytes.len()] != op_bytes {
                continue;
            }
            // Must not be at very start or end (need operands)
            if i == 0 || i + op_bytes.len() >= bytes.len() {
                continue;
            }
            // Disambiguate: `<<` vs `<`, `>>` vs `>`
            if op_str == "<" && i + 1 < bytes.len() && bytes[i + 1] == b'<' {
                continue;
            }
            if op_str == ">" && i + 1 < bytes.len() && bytes[i + 1] == b'>' {
                continue;
            }
            if op_str == "<" && i > 0 && bytes[i - 1] == b'<' {
                continue;
            }
            if op_str == ">" && i > 0 && bytes[i - 1] == b'>' {
                continue;
            }
            // Rightmost: always update
            best = Some((i, op_str, binop));
        }
        i += 1;
    }
    best
}

/// Parse an atomic expression (no binary operators at the top level).
fn parse_atom(input: &str) -> Result<Expr, String> {
    let input = input.trim();

    // Hex literal: 0x...
    if let Some(hex) = input
        .strip_prefix("0x")
        .or_else(|| input.strip_prefix("0X"))
        && !hex.is_empty()
        && hex.chars().all(|c| c.is_ascii_hexdigit())
    {
        return Ok(Expr::HexLiteral(input.to_string()));
    }

    // Decimal literal
    if input.chars().all(|c| c.is_ascii_digit()) && !input.is_empty() {
        return Ok(Expr::DecLiteral(input.to_string()));
    }

    // stack[N]
    if let Some(inner) = input
        .strip_prefix("stack[")
        .and_then(|s| s.strip_suffix(']'))
    {
        let inner = inner.trim();
        return match inner.parse::<u64>() {
            Ok(idx) => Ok(Expr::StackIndex(idx)),
            Err(_) => Err(format!("invalid stack index: {inner}")),
        };
    }

    if let Some(inner) = input.strip_prefix("log[").and_then(|s| s.strip_suffix(']')) {
        let inner = inner.trim();
        return match inner.parse::<u64>() {
            Ok(idx) => Ok(Expr::LogAccess(idx)),
            Err(_) => Err(format!("invalid log index: {inner}")),
        };
    }

    if let Some(rest) = input.strip_prefix("event[") {
        let close = rest
            .find(']')
            .ok_or_else(|| format!("unclosed bracket in: {input}"))?;
        let idx_str = rest[..close].trim();
        let index = idx_str
            .parse::<u64>()
            .map_err(|_| format!("invalid event index: {idx_str}"))?;

        let tail = rest[close + 1..].trim();
        if tail.is_empty() {
            return Ok(Expr::EventAccess {
                index,
                field_index: None,
            });
        }
        if let Some(tail) = tail.strip_prefix('[') {
            let close2 = tail
                .find(']')
                .ok_or_else(|| format!("unclosed bracket in: {input}"))?;
            let field_str = tail[..close2].trim();
            let field_index = field_str
                .parse::<u64>()
                .map_err(|_| format!("invalid event field index: {field_str}"))?;
            let extra = tail[close2 + 1..].trim();
            if !extra.is_empty() {
                return Err(format!("unexpected trailing characters: {extra}"));
            }
            return Ok(Expr::EventAccess {
                index,
                field_index: Some(field_index),
            });
        }
        if let Some(field) = tail.strip_prefix('.') {
            let field = field.trim();
            if !is_valid_ident_part(field) {
                return Err(format!("invalid event field: {field}"));
            }
            return Ok(Expr::EventField {
                index,
                field: field.to_string(),
            });
        }

        return Err(format!("unexpected trailing characters: {tail}"));
    }

    // memory[offset] or memory[offset:length]
    if let Some(inner) = input
        .strip_prefix("memory[")
        .and_then(|s| s.strip_suffix(']'))
    {
        let inner = inner.trim();
        return if let Some((off_s, len_s)) = inner.split_once(':') {
            let off = off_s
                .trim()
                .parse::<u64>()
                .map_err(|_| format!("invalid memory offset: {off_s}"))?;
            let len = len_s
                .trim()
                .parse::<u64>()
                .map_err(|_| format!("invalid memory length: {len_s}"))?;
            Ok(Expr::MemoryAccess {
                offset: off,
                length: Some(len),
            })
        } else {
            let off = inner
                .parse::<u64>()
                .map_err(|_| format!("invalid memory offset: {inner}"))?;
            Ok(Expr::MemoryAccess {
                offset: off,
                length: None,
            })
        };
    }

    // name[key] or name[key1][key2] — mapping lookup
    if let Some(bracket_pos) = input.find('[') {
        let name = input[..bracket_pos].trim();
        if name.is_empty() {
            return Err(format!("missing name before '[' in: {input}"));
        }
        // Validate the name part is a valid identifier
        if !is_valid_ident(name) {
            return Err(format!("invalid identifier: {name}"));
        }
        let mut keys = Vec::new();
        let mut rest = &input[bracket_pos..];
        while rest.starts_with('[') {
            let close = find_matching_bracket(rest)
                .ok_or_else(|| format!("unclosed bracket in: {input}"))?;
            let key_str = &rest[1..close];
            let key_expr = parse_atom(key_str.trim())?;
            keys.push(key_expr);
            rest = &rest[close + 1..];
        }
        if !rest.trim().is_empty() {
            return Err(format!("unexpected trailing characters: {rest}"));
        }
        return Ok(Expr::MappingLookup {
            name: name.to_string(),
            keys,
        });
    }

    // Keyword check (must come after bracket-based checks)
    if is_keyword(input) {
        return Ok(Expr::Keyword(input.to_string()));
    }

    // Identifier (storage variable name, calldata param, etc.)
    if is_valid_ident(input) {
        return Ok(Expr::Ident(input.to_string()));
    }

    Err(format!("cannot parse expression: '{input}'"))
}

/// Check if a string is a valid Solidity-style identifier.
fn is_valid_ident(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    // Allow dotted keywords like msg.sender, msg.data
    if s.contains('.') {
        return s.split('.').all(is_valid_ident_part);
    }
    is_valid_ident_part(s)
}

fn is_valid_ident_part(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let mut chars = s.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_alphabetic() && first != '_' {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Find the position of the matching `]` for a `[` at position 0.
fn find_matching_bracket(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    if bytes.is_empty() || bytes[0] != b'[' {
        return None;
    }
    let mut depth = 0;
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // ===== Literals =====

    #[test]
    fn parse_hex_literal() {
        assert_eq!(parse("0xff").unwrap(), Expr::HexLiteral("0xff".into()));
        assert_eq!(parse("0x0").unwrap(), Expr::HexLiteral("0x0".into()));
        assert_eq!(parse("0xDEAD").unwrap(), Expr::HexLiteral("0xDEAD".into()));
    }

    #[test]
    fn parse_dec_literal() {
        assert_eq!(parse("42").unwrap(), Expr::DecLiteral("42".into()));
        assert_eq!(parse("0").unwrap(), Expr::DecLiteral("0".into()));
        assert_eq!(
            parse("99000000000000000000").unwrap(),
            Expr::DecLiteral("99000000000000000000".into())
        );
    }

    // ===== Keywords =====

    #[test]
    fn parse_keywords() {
        assert_eq!(parse("pc").unwrap(), Expr::Keyword("pc".into()));
        assert_eq!(parse("gas").unwrap(), Expr::Keyword("gas".into()));
        assert_eq!(
            parse("msg.sender").unwrap(),
            Expr::Keyword("msg.sender".into())
        );
        assert_eq!(parse("stack").unwrap(), Expr::Keyword("stack".into()));
        assert_eq!(parse("help").unwrap(), Expr::Keyword("help".into()));
        assert_eq!(parse("?").unwrap(), Expr::Keyword("?".into()));
    }

    // ===== Identifiers =====

    #[test]
    fn parse_ident() {
        assert_eq!(parse("number").unwrap(), Expr::Ident("number".into()));
        assert_eq!(parse("fee").unwrap(), Expr::Ident("fee".into()));
        assert_eq!(
            parse("totalDeposits").unwrap(),
            Expr::Ident("totalDeposits".into())
        );
        assert_eq!(parse("_private").unwrap(), Expr::Ident("_private".into()));
    }

    // ===== Stack index =====

    #[test]
    fn parse_stack_index() {
        assert_eq!(parse("stack[0]").unwrap(), Expr::StackIndex(0));
        assert_eq!(parse("stack[4]").unwrap(), Expr::StackIndex(4));
        assert_eq!(parse("stack[12]").unwrap(), Expr::StackIndex(12));
    }

    #[test]
    fn parse_stack_index_invalid() {
        assert!(parse("stack[abc]").is_err());
    }

    // ===== Memory access =====

    #[test]
    fn parse_memory_access() {
        assert_eq!(
            parse("memory[32]").unwrap(),
            Expr::MemoryAccess {
                offset: 32,
                length: None
            }
        );
        assert_eq!(
            parse("memory[0:64]").unwrap(),
            Expr::MemoryAccess {
                offset: 0,
                length: Some(64)
            }
        );
    }

    // ===== Mapping lookups =====

    #[test]
    fn parse_mapping_single_key() {
        assert_eq!(
            parse("deposits[msg.sender]").unwrap(),
            Expr::MappingLookup {
                name: "deposits".into(),
                keys: vec![Expr::Keyword("msg.sender".into())],
            }
        );
    }

    #[test]
    fn parse_mapping_hex_key() {
        assert_eq!(
            parse("_balances[0xdead]").unwrap(),
            Expr::MappingLookup {
                name: "_balances".into(),
                keys: vec![Expr::HexLiteral("0xdead".into())],
            }
        );
    }

    #[test]
    fn parse_mapping_nested() {
        assert_eq!(
            parse("_allowances[from][spender]").unwrap(),
            Expr::MappingLookup {
                name: "_allowances".into(),
                keys: vec![Expr::Ident("from".into()), Expr::Ident("spender".into()),],
            }
        );
    }

    // ===== Binary operations =====

    #[test]
    fn parse_add() {
        assert_eq!(
            parse("1 + 1").unwrap(),
            Expr::BinaryOp {
                lhs: Box::new(Expr::DecLiteral("1".into())),
                op: BinOp::Add,
                rhs: Box::new(Expr::DecLiteral("1".into())),
            }
        );
    }

    #[test]
    fn parse_hex_sub() {
        assert_eq!(
            parse("0xff - 32").unwrap(),
            Expr::BinaryOp {
                lhs: Box::new(Expr::HexLiteral("0xff".into())),
                op: BinOp::Sub,
                rhs: Box::new(Expr::DecLiteral("32".into())),
            }
        );
    }

    #[test]
    fn parse_stack_shift() {
        assert_eq!(
            parse("stack[4] << 8").unwrap(),
            Expr::BinaryOp {
                lhs: Box::new(Expr::StackIndex(4)),
                op: BinOp::Shl,
                rhs: Box::new(Expr::DecLiteral("8".into())),
            }
        );
    }

    #[test]
    fn parse_hex_and_ident() {
        assert_eq!(
            parse("0xff & fee").unwrap(),
            Expr::BinaryOp {
                lhs: Box::new(Expr::HexLiteral("0xff".into())),
                op: BinOp::BitAnd,
                rhs: Box::new(Expr::Ident("fee".into())),
            }
        );
    }

    #[test]
    fn parse_mapping_plus_literal() {
        assert_eq!(
            parse("deposits[msg.sender] + 10").unwrap(),
            Expr::BinaryOp {
                lhs: Box::new(Expr::MappingLookup {
                    name: "deposits".into(),
                    keys: vec![Expr::Keyword("msg.sender".into())],
                }),
                op: BinOp::Add,
                rhs: Box::new(Expr::DecLiteral("10".into())),
            }
        );
    }

    #[test]
    fn parse_stack_plus_literal() {
        assert_eq!(
            parse("stack[1] + 2").unwrap(),
            Expr::BinaryOp {
                lhs: Box::new(Expr::StackIndex(1)),
                op: BinOp::Add,
                rhs: Box::new(Expr::DecLiteral("2".into())),
            }
        );
    }

    #[test]
    fn parse_all_operators() {
        for (op_str, expected_op) in [
            ("+", BinOp::Add),
            ("-", BinOp::Sub),
            ("*", BinOp::Mul),
            ("/", BinOp::Div),
            ("%", BinOp::Mod),
            ("&", BinOp::BitAnd),
            ("|", BinOp::BitOr),
            ("^", BinOp::BitXor),
            ("<<", BinOp::Shl),
            (">>", BinOp::Shr),
        ] {
            let input = format!("1 {op_str} 2");
            let expr = parse(&input).unwrap();
            match expr {
                Expr::BinaryOp { op, .. } => assert_eq!(op, expected_op, "failed for op: {op_str}"),
                other => panic!("expected BinaryOp for '{input}', got: {other:?}"),
            }
        }
    }

    #[test]
    fn parse_chained_add() {
        // 1 + 2 + 3 should parse as (1 + 2) + 3 (left-associative)
        let expr = parse("1 + 2 + 3").unwrap();
        assert_eq!(
            expr,
            Expr::BinaryOp {
                lhs: Box::new(Expr::BinaryOp {
                    lhs: Box::new(Expr::DecLiteral("1".into())),
                    op: BinOp::Add,
                    rhs: Box::new(Expr::DecLiteral("2".into())),
                }),
                op: BinOp::Add,
                rhs: Box::new(Expr::DecLiteral("3".into())),
            }
        );
    }

    #[test]
    fn parse_precedence_mul_add() {
        // 2 + 3 * 4 should parse as 2 + (3 * 4)
        let expr = parse("2 + 3 * 4").unwrap();
        assert_eq!(
            expr,
            Expr::BinaryOp {
                lhs: Box::new(Expr::DecLiteral("2".into())),
                op: BinOp::Add,
                rhs: Box::new(Expr::BinaryOp {
                    lhs: Box::new(Expr::DecLiteral("3".into())),
                    op: BinOp::Mul,
                    rhs: Box::new(Expr::DecLiteral("4".into())),
                }),
            }
        );
    }

    // ===== Whitespace handling =====

    #[test]
    fn parse_with_extra_whitespace() {
        assert_eq!(parse("  0xff  ").unwrap(), Expr::HexLiteral("0xff".into()));
        assert_eq!(
            parse("  1  +  2  ").unwrap(),
            Expr::BinaryOp {
                lhs: Box::new(Expr::DecLiteral("1".into())),
                op: BinOp::Add,
                rhs: Box::new(Expr::DecLiteral("2".into())),
            }
        );
    }

    // ===== Errors =====

    #[test]
    fn parse_empty() {
        assert!(parse("").is_err());
        assert!(parse("   ").is_err());
    }

    #[test]
    fn parse_unclosed_bracket() {
        assert!(parse("deposits[msg.sender").is_err());
    }
}
