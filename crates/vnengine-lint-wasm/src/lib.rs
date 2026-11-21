//! Minimal no-bindgen WASM interface to the `.vn` linter, built for
//! `wasm32-unknown-unknown` and driven by the VS Code extension (plain
//! `WebAssembly` JS API — no wasm-bindgen glue).
//!
//! ABI:
//! - `vn_alloc(len) -> ptr` — allocate `len` bytes for the input JSON.
//! - `vn_free(ptr, len)` — release a buffer obtained from `vn_alloc`.
//! - `vn_lint(ptr, len)` — consume a UTF-8 JSON [`LintRequest`], store the
//!   JSON response in the reusable output buffer.
//! - `vn_result_ptr()` / `vn_result_len()` — read the response after
//!   `vn_lint`. The buffer is overwritten by the next call.
//!
//! Request JSON:
//! ```json
//! {
//!   "source": "label start: ...",
//!   "file": "script/main.vn (optional — enables multi-file lint)",
//!   "files": {"script/ch1.vn": "label ch1: ... (import candidates)"},
//!   "project_toml": "title = \"...\" (optional raw project.toml text)",
//!   "assets": ["images/bg.png", "audio/theme.ogg"]
//! }
//! ```
//! When `file` is present, `import` directives resolve through `files`
//! (the host gathers the import closure — open documents first, then
//! disk). Response JSON: `{ "diagnostics": [Diagnostic, ...] }` where
//! each entry is the [`vnengine_script::lint::Diagnostic`] shape
//! (`severity`, `message`, optional `line`/`col`/`span`) plus, in
//! multi-file mode, a `file` field naming the source it was found in.

use serde::Deserialize;
use std::cell::RefCell;
use std::collections::BTreeMap;
use vnengine_core::EngineConfig;
use vnengine_script::lint::{check_source, check_source_multi, LintOptions};

thread_local! {
    static OUT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

#[derive(Deserialize)]
struct LintRequest {
    /// Full text of the `.vn` file being edited.
    source: String,
    /// Project-relative path of `source` (e.g. `script/main.vn`). When
    /// present, `import` directives are resolved through `files`.
    #[serde(default)]
    file: Option<String>,
    /// Import candidates gathered by the host: project-relative path →
    /// source text. Missing entries fail the import with a parse error.
    #[serde(default)]
    files: BTreeMap<String, String>,
    /// Raw `project.toml` text, when the file lives inside a project.
    /// Absent → "loose file" mode: default config, speaker checks off.
    #[serde(default)]
    project_toml: Option<String>,
    /// Project-relative asset paths (`dir/file`, `/`-separated).
    #[serde(default)]
    assets: Vec<String>,
}

/// Allocate `len` bytes and return a pointer the host writes the request
/// JSON into. Pair with [`vn_free`].
#[no_mangle]
pub extern "C" fn vn_alloc(len: usize) -> *mut u8 {
    let mut buf = Vec::<u8>::with_capacity(len);
    let ptr = buf.as_mut_ptr();
    std::mem::forget(buf);
    ptr
}

/// Release a buffer previously returned by [`vn_alloc`].
///
/// # Safety
/// `ptr`/`len` must come from a `vn_alloc` call and be passed at most once.
#[no_mangle]
pub unsafe extern "C" fn vn_free(ptr: *mut u8, len: usize) {
    if !ptr.is_null() {
        drop(Vec::from_raw_parts(ptr, len, len));
    }
}

/// Run the linter on a UTF-8 JSON [`LintRequest`]. The response is written
/// into a reusable buffer read via [`vn_result_ptr`]/[`vn_result_len`].
///
/// # Safety
/// `ptr` must point to `len` readable bytes (typically from `vn_alloc`).
#[no_mangle]
pub unsafe extern "C" fn vn_lint(ptr: *const u8, len: usize) {
    let input = std::slice::from_raw_parts(ptr, len);
    let response = match serde_json::from_slice::<LintRequest>(input) {
        Ok(req) => run_lint(req),
        Err(e) => serde_json::json!({
            "diagnostics": [{
                "severity": "error",
                "message": format!("invalid lint request: {e}"),
            }]
        })
        .to_string()
        .into_bytes(),
    };
    OUT.with(|out| *out.borrow_mut() = response);
}

/// Pointer to the response buffer written by [`vn_lint`].
#[no_mangle]
pub extern "C" fn vn_result_ptr() -> *const u8 {
    OUT.with(|out| out.borrow().as_ptr())
}

/// Length in bytes of the response buffer.
#[no_mangle]
pub extern "C" fn vn_result_len() -> usize {
    OUT.with(|out| out.borrow().len())
}

fn run_lint(req: LintRequest) -> Vec<u8> {
    let (config, opts) = match &req.project_toml {
        Some(text) => match toml::from_str::<EngineConfig>(text) {
            Ok(c) => (c, LintOptions::default()),
            // Unparseable project.toml: report it, still lint loosely.
            Err(e) => {
                return serde_json::json!({
                    "diagnostics": [
                        {"severity": "error", "message": format!("project.toml: {e}")}
                    ]
                })
                .to_string()
                .into_bytes();
            }
        },
        None => {
            // Loose file: no declared characters, no project layout —
            // lint labels/screens/transforms but skip speaker checks.
            (
                EngineConfig::default(),
                LintOptions {
                    check_assets: false,
                    check_speakers: false,
                },
            )
        }
    };
    let assets: std::collections::BTreeSet<String> = req.assets.into_iter().collect();
    let asset_set = if opts.check_assets {
        Some(&assets)
    } else {
        None
    };
    let diagnostics: Vec<serde_json::Value> = match &req.file {
        // Project mode: resolve `import` through the host-provided
        // sources and report each diagnostic with its file of origin.
        Some(entry) => {
            let files = req.files;
            let mut resolver = |resolved: &str, _cur: &str| -> Result<String, String> {
                files
                    .get(resolved)
                    .cloned()
                    .ok_or_else(|| format!("{resolved}: not provided by host"))
            };
            check_source_multi(&req.source, entry, &mut resolver, &config, asset_set, &opts)
                .into_iter()
                .map(|(file, d)| {
                    let mut v = serde_json::to_value(&d).unwrap_or_default();
                    if let Some(obj) = v.as_object_mut() {
                        obj.insert("file".to_string(), file.into());
                    }
                    v
                })
                .collect()
        }
        None => check_source(&req.source, &config, asset_set, &opts)
            .into_iter()
            .map(|d| serde_json::to_value(&d).unwrap_or_default())
            .collect(),
    };
    serde_json::json!({ "diagnostics": diagnostics })
        .to_string()
        .into_bytes()
}
