# vnengine for VS Code

Language support for [vnengine](https://github.com/example/vnengine) `.vn`
visual novel scripts.

## Features

- **Syntax highlighting** for `.vn` files (labels, strings, interpolation,
  keywords, screen/transform/ATL blocks).
- **Diagnostics from the engine itself** — parse errors and lint warnings
  (unknown labels/screens/transforms, missing assets, undeclared speakers,
  unreachable labels) with squiggles at the offending token. Powered by the
  same parser/linter that `vnengine-pack --check-only` runs, compiled to
  WebAssembly and embedded in the extension: no engine install required.
- **Commands** (Command Palette):
  - `vnengine: Run Project` — launch the current project and show logs in the vnengine output channel.
  - `vnengine: Validate Project` — full `vnengine-pack --check-only` pass,
    diagnostics pushed to the Problems panel.
  - `vnengine: Package Project (.vnpkg)` — build a distributable package
    under `build/`.

## Requirements

Diagnostics work out of the box (embedded WASM linter). The Run/Validate/
Package commands need the **vnengine author kit**: put `vnengine` and
`vnengine-pack` on your `PATH`, or set the paths in Settings.

## Settings

| Setting | Default | Description |
|---|---|---|
| `vnengine.enginePath` | `""` | Path to the `vnengine` binary (empty = PATH). |
| `vnengine.packPath` | `""` | Path to the `vnengine-pack` binary (empty = PATH). |
| `vnengine.diagnostics` | `"wasm"` | Diagnostics source: `wasm` (embedded), `binary` (vnengine-pack for saved files, WASM for unsaved edits), `off`. |
| `vnengine.diagnosticsDebounceMs` | `300` | Delay before live diagnostics refresh after an edit. |

## Notes

- Inside a project (a directory containing `project.toml`), the extension
  feeds the linter your declared characters and the file list so asset and
  speaker checks match `vnengine-pack` exactly. A `.vn` file opened outside
  a project still gets parse errors and label/screen/transform checks.
- The WASM linter is built from the engine sources with
  `cargo build -p vnengine-lint-wasm --target wasm32-unknown-unknown --release`.
