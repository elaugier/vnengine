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
//!   "project_toml": "title = \"...\" (optional raw project.toml text)",
//!   "assets": ["images/bg.png", "audio/theme.ogg"]
//! }
//! ```
//! Response JSON: `{ "diagnostics": [Diagnostic, ...] }` where each entry
//! is the [`vnengine_script::lint::Diagnostic`] shape (`severity`,
//! `message`, optional `line`/`col`/`span`).

use serde::Deserialize;
use std::cell::RefCell;
use vnengine_core::EngineConfig;
use vnengine_script::lint::{check_source, LintOptions};

thread_local! {
    static OUT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

#[derive(Deserialize)]
struct LintRequest {
    /// Full text of the `.vn` file being edited.
    source: String,
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
    let diagnostics = check_source(&req.source, &config, asset_set, &opts);
    serde_json::json!({ "diagnostics": diagnostics })
        .to_string()
        .into_bytes()
}
