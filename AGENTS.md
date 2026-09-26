# vnengine

## Build & Test Commands
- Build: `cargo build`
- Build (release): `cargo build --release`
- Check: `cargo check`
- Run tests: `cargo test`
- Run demo: `cargo run -- --project demo`

## Architecture
- `vnengine-core`: Shared types, config, save/load, scene model
- `vnengine-script`: `.vn` DSL: lexer, parser, AST, VM
- `vnengine-runtime`: Headless game state (VM + scene + history + rollback), no OS access
- `vnengine-render`: wgpu renderer (sprites, backgrounds, text, UI)
- `vnengine-audio`: kira audio engine (BGM/SFX/voice)
- `vnengine-app`: Main loop, input, scene management, UI
- `vnengine-cli`: Command-line runner

## Key Dependencies
- wgpu 30, winit 0.30, glyphon 0.12 (text), kira 0.12 (audio), gilrs 0.11 (gamepad)
- serde + serde_json for save files (bincode 3.0 is a joke release, avoid)

## Script DSL
- Builtin calls: `random()` → float in [0,1), `randint(lo, hi)` → int in range, `srand(seed)` → reseed PRNG.
- The PRNG seed lives in `VmState` — it is serialized, so saves and replays reproduce the same draws.
- `pause <seconds>` is driven by the runtime's logical clock (`Runtime::update`), not wall time.
- `Runtime.journal` records menu choices; `Runtime::replay(script, journal)` re-runs deterministically.
- `Runtime.music` tracks the current BGM across commands/saves/rollback — hosts resync audio against it.

## Notes
- Windows builds may hit transient file lock errors (os error 32) on proc-macro crates. Retry the build if this happens.
- The wgpu 30 API has breaking changes from earlier versions: `bind_group_layouts` takes `&[Some(...)]`, `PipelineLayoutDescriptor` needs `immediate_size`, `RenderPipelineDescriptor` needs `multiview_mask`, `SurfaceConfiguration` needs `color_space`, vertex buffers are `Option<VertexBufferLayout>`.
