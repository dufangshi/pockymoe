use std::collections::BTreeSet;

use pockymoe_protocol::{ThreadEventType, ThreadItemKind, ThreadStatus, TurnStatus};

// Compile-time inclusion ensures editing the TS contract reruns this test.
const TYPESCRIPT: &str = include_str!("../../../packages/shared/src/index.ts");

fn literal_union(source: &str, name: &str) -> Vec<String> {
    let declaration = format!("export type {name} =");
    let body = source
        .split_once(&declaration)
        .unwrap_or_else(|| panic!("missing {name}"))
        .1
        .split_once(';')
        .expect("union must end with a semicolon")
        .0;
    body.split('|')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| {
            let literal = part
                .strip_prefix('\'')
                .and_then(|part| part.strip_suffix('\''))
                .expect("contract unions must contain only single-quoted string literals");
            assert!(!literal.is_empty() && !literal.contains('\''));
            literal.to_owned()
        })
        .collect()
}

fn event_keys(source: &str) -> Vec<String> {
    let body = source
        .split_once("export interface ThreadEventPayloadMap {")
        .expect("missing event payload map")
        .1
        .split_once("\n}")
        .expect("missing event payload map end")
        .0;
    let mut depth = 0_i32;
    let mut keys = Vec::new();
    for line in body.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if depth == 0 {
            let (key, _) = line
                .strip_prefix('\'')
                .and_then(|line| line.split_once("':"))
                .expect("top-level events must be single-quoted keys");
            keys.push(key.to_owned());
        }
        depth += line.matches('{').count() as i32 - line.matches('}').count() as i32;
        assert!(depth >= 0, "unbalanced event payload map");
    }
    assert_eq!(depth, 0, "unbalanced event payload map");
    keys
}

fn assert_values(name: &str, rust: &[&str], ts: Vec<String>) {
    let rust_set = rust
        .iter()
        .map(|s| (*s).to_owned())
        .collect::<BTreeSet<_>>();
    let ts_set = ts.iter().cloned().collect::<BTreeSet<_>>();
    assert!(!rust_set.is_empty(), "empty {name}");
    assert_eq!(rust_set.len(), rust.len(), "duplicate Rust {name}");
    assert_eq!(ts_set.len(), ts.len(), "duplicate TS {name}");
    assert_eq!(
        rust_set, ts_set,
        "{name} drift: update Rust and TS together"
    );
}

#[test]
fn rust_and_typescript_thread_discriminants_match() {
    for (name, rust) in [
        ("ThreadItemKindDto", ThreadItemKind::KNOWN_VALUES),
        ("ThreadStatusDto", ThreadStatus::KNOWN_VALUES),
        ("TurnStatusDto", TurnStatus::KNOWN_VALUES),
    ] {
        assert_values(name, rust, literal_union(TYPESCRIPT, name));
    }
    assert_values(
        "ThreadEventType",
        ThreadEventType::KNOWN_VALUES,
        event_keys(TYPESCRIPT),
    );
    // The checked lists must actually be used by the DTOs/envelope.
    for binding in [
        "kind: ThreadItemKindDto;",
        "status: ThreadStatusDto;",
        "status: TurnStatusDto;",
        "[Type in keyof ThreadEventPayloadMap]",
        "payload: ThreadEventPayloadMap[Type];",
    ] {
        assert!(TYPESCRIPT.contains(binding), "missing binding {binding}");
    }
}

#[test]
fn contract_parser_handles_layout_and_nested_payloads() {
    assert_eq!(
        literal_union("export type X =\n | 'a' | 'b';", "X"),
        ["a", "b"]
    );
    assert_eq!(event_keys("export interface ThreadEventPayloadMap {\n 'a': {\n nested: { field: string };\n };\n 'b': { value: string };\n}"), ["a", "b"]);
}

#[test]
#[should_panic(expected = "contract unions must contain only")]
fn contract_parser_rejects_widened_union() {
    literal_union("export type X = 'a' | string;", "X");
}

#[test]
#[should_panic(expected = "drift")]
fn contract_comparison_rejects_new_unmapped_value() {
    assert_values("example", &["a", "b"], vec!["a".into()]);
}
