//! Validates every ```vn fenced block in docs/DSL_REFERENCE.md: each
//! block must parse either as a complete script or as a label body.

use vnengine_script::Parser;

fn extract_vn_blocks(md: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut in_block = false;
    let mut cur = String::new();
    for line in md.lines() {
        let t = line.trim();
        if !in_block && (t == "```vn" || t.starts_with("```vn ")) {
            in_block = true;
            cur.clear();
        } else if in_block && t == "```" {
            in_block = false;
            blocks.push(std::mem::take(&mut cur));
        } else if in_block {
            cur.push_str(line);
            cur.push('\n');
        }
    }
    blocks
}

/// Parse `src` as a complete script; if that fails, retry as the body of
/// a synthetic label (most snippets are statements, not top-level items).
fn parse_snippet(src: &str) -> Result<(), String> {
    if let Err(e) = Parser::parse(src) {
        // Keep the original error for reporting if the body retry fails.
        let original = e.to_string();
        return parse_as_body(src)
            .map_err(|wrapped| format!("as script: {original}; as label body: {wrapped}"));
    }
    Ok(())
}

fn parse_as_body(src: &str) -> Result<(), String> {
    let mut wrapped = String::from("label __doc_snippet:\n");
    for line in src.lines() {
        if line.trim().is_empty() {
            wrapped.push('\n');
        } else {
            wrapped.push_str("    ");
            wrapped.push_str(line);
            wrapped.push('\n');
        }
    }
    Parser::parse(&wrapped)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[test]
fn dsl_reference_examples_parse() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/DSL_REFERENCE.md");
    let md = match std::fs::read_to_string(path) {
        Ok(m) => m,
        // Doc not present (e.g. crate built outside the repo): skip.
        Err(_) => return,
    };
    let blocks = extract_vn_blocks(&md);
    assert!(!blocks.is_empty(), "no ```vn blocks found in {path}");
    let mut failures = Vec::new();
    for (i, block) in blocks.iter().enumerate() {
        if let Err(e) = parse_snippet(block) {
            failures.push(format!("block #{i}:\n{block}\n  -> {e}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} ```vn blocks failed to parse:\n\n{}",
        failures.len(),
        blocks.len(),
        failures.join("\n\n")
    );
}
