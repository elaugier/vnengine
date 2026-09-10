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
- `vnengine-render`: wgpu renderer (sprites, backgrounds, text, UI)
- `vnengine-audio`: kira audio engine (BGM/SFX/voice)
- `vnengine-app`: Main loop, input, scene management, UI
- `vnengine-cli`: Command-line runner

## Key Dependencies
- wgpu 30, winit 0.30, glyphon 0.12 (text), kira 0.12 (audio), gilrs 0.11 (gamepad)
- serde + serde_json for save files (bincode 3.0 is a joke release, avoid)

## Notes
- Windows builds may hit transient file lock errors (os error 32) on proc-macro crates. Retry the build if this happens.
- The wgpu 30 API has breaking changes from earlier versions: `bind_group_layouts` takes `&[Some(...)]`, `PipelineLayoutDescriptor` needs `immediate_size`, `RenderPipelineDescriptor` needs `multiview_mask`, `SurfaceConfiguration` needs `color_space`, vertex buffers are `Option<VertexBufferLayout>`.
