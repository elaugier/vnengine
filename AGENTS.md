# vnengine

## Build & Test Commands
- Build: `cargo build`
- Build (release): `cargo build --release`
- Check: `cargo check`
- Run tests: `cargo test`
- Run demo: `cargo run -- --project demo`
- Run a package: `cargo run -p vnengine-cli -- --package build/demo.vnpkg`
- Package a project: `cargo run -p vnengine-pack -- --project demo --output build/demo.vnpkg`
- Validate a project: `cargo run -p vnengine-pack -- --project demo --check-only`
- Validate (JSON diagnostics): `... --check-only --json`
- Build the editor linter: `cargo build -p vnengine-lint-wasm --target wasm32-unknown-unknown --release` → copy `target/wasm32-unknown-unknown/release/vnengine_lint_wasm.wasm` to `editors/vscode/media/vnengine-lint.wasm`
- Package the VS Code extension: `cd editors/vscode && npx -y @vscode/vsce package`

## Architecture
- `vnengine-core`: Shared types, config, save/load, scene model
- `vnengine-script`: `.vn` DSL: lexer, parser, AST, VM + `lint` module (static analysis shared by `vnengine-pack --check-only` and the WASM linter; returns positioned `Diagnostic`s)
- `vnengine-runtime`: Headless game state (VM + scene + history + rollback), no OS access
- `vnengine-render`: wgpu renderer (sprites, backgrounds, text, UI)
- `vnengine-audio`: kira audio engine (BGM/SFX/voice)
- `vnengine-app`: Main loop, input, scene management, UI
- `vnengine-cli`: Command-line runner
- `vnengine-pack`: `.vnpkg` build/validate/open (lib + `vnengine-pack` bin)
- `vnengine-plugins`: sandboxed WASM plugins (wasmtime 48)
- `vnengine-lint-wasm`: no-bindgen `cdylib` exposing the linter to JS
  (`vn_alloc`/`vn_free`/`vn_lint`/`vn_result_ptr`/`vn_result_len`; JSON
  in/out). Used by `editors/vscode`.

## Distribution
- `scripts/package-kit.sh <bin_dir> <name> <exe_ext>` assembles the
  author kit (bin + `demo/` as `template/` + `docs/` + `kit/README.md`)
  under `dist/`.
- `.github/workflows/release.yml` (tags `v*`): kits for
  `windows-x64`/`windows-arm64` (native `windows-11-arm`
  runner)/`linux-x64` + a `.vsix` built from `editors/vscode`.
- `editors/vscode` is plain CommonJS (no build step): TextMate grammar,
  diagnostics via the embedded WASM linter (`vnengine.diagnostics=wasm`,
  default) or `vnengine-pack --check-only --json` (`binary`), and
  Run/Validate/Package commands. Rebuild `media/vnengine-lint.wasm`
  after changing `vnengine-script`'s parser or `lint`.
- `crates/vnengine-script/tests/doc_examples.rs` parses every ```vn
  block in `docs/DSL_REFERENCE.md` — keep doc examples valid.

## Plugins
- A plugin is `plugins/<name>.wasm` (or `.wat`) + `plugins/<name>.toml`
  manifest (`name`, `version`, `permissions` = `log`/`kv`,
  `fuel_per_tick`, `memory_max_bytes`). Permissions are enforced:
  unlisted `env.*` imports are rejected at load time.
- Exports: `memory`, `vn_init()`, `vn_tick(f32)`, `vn_on_choice(i32)`,
  `vn_alloc(len)->ptr` + `vn_on_command(ptr,len)` for script commands.
- Imports: `env.vn_log`, `env.vn_kv_get`, `env.vn_kv_set` — KV keys are
  namespaced `<plugin>.<key>` and live in `Runtime::plugin_kv`, which is
  part of saves and rollback snapshots.
- DSL: `plugin <name> [<expr>, ...]` → `Command::PluginCall` with a
  plain-JSON payload (`["unlock","chapel"]`, not the tagged `Value`
  encoding). Demo plugin: `demo/plugins/journal.wat`. (`--project` dir or `--package` .vnpkg)
- `vnengine-pack`: `.vnpkg` builder/reader (manifest, asset index with sha256, licenses)

## Key Dependencies
- wgpu 30, winit 0.30, glyphon 0.12 (text), kira 0.12 (audio), gilrs 0.11 (gamepad)
- serde + serde_json for save files (bincode 3.0 is a joke release, avoid)

## Script DSL
- Values: `None`/`Bool`/`Int`/`Float`/`Str`/`List`/`Dict` (`BTreeMap`, stable order). Dict literals `{k: v}` (ident or string keys), `d["k"]`, `d.k`, slices `a[1:3]`/`s[:-1]` (Python-style bounds), `in`/`not in`/`is`/`is not`, `"ab" * 3` string repeat.
- Builtin calls: `random()` → float in [0,1), `randint(lo, hi)`, `srand(seed)` reseed, plus `len str int float bool abs min max round choice sample shuffle range sorted reversed keys values items has get` and string helpers `upper lower strip lstrip rstrip split replace startswith endswith count find index join` (also usable as methods: `name.upper()`, `items.count(x)`). Read-only methods only — values are immutable.
- User functions: `def name(params): body` with `return [expr]` (bare `return` → `None`). In expressions `f(args)` runs a pure interpreter (`funcs.rs`) — only side-effect-free statements (assign/if/while/for/break/continue/return/def) are allowed; `say`/`menu`/scene/etc. are rejected. `call f(args)` and `call name` run the body as a call frame (blocking statements allowed); args bind as regular variables and persist after the call. `call label(arg, ...)` binds a label's declared params (`label name(a, b):`) the same way. `call [expr]`/`jump [expr]` evaluate a dynamic target; a bare identifier stays a literal label for backward compatibility.
- `menu [prompt] [with <trans>] [set <var>]:` — `set` stores the chosen 0-based index into `<var>` on selection (`PendingMenu.set_var`, serialized).
- Dynamic assets: `scene [expr]`, `show e happy [expr]` (suffix appended), `play music|sfx [expr]`, `voice [expr]`, `music layer "n" [expr]` accept `[expr]` or plain expressions evaluated at runtime.
- Transitions: `with <name> [dur]` standalone; also `say`/`narrate`/`menu ... with <trans>` (transition plays first, the line/menu is deferred one step). `pause` without a duration emits `Command::WaitInput` (blocks until advance). `hide`/`hide all` → `Command::HideAll`; `show ... layer=<layer>` sets the render layer. Transition names: `cut`, `fade`, `fadewhite`, `dissolve`, `slide{left,right,up,down}`, `zoom`, `push{left,right,up,down}`, `blinds`/`blinds{v,h}`, `iris{in,out}`, `pixelate`, `squares`. Push animates `Scene.bg_offset`/`prev_bg_offset` + sprite offsets (CPU-side); the last four render through a WGSL fragment mask (`Scene.trans_fx`, `Transition::mask_kind`/`mask_param`, `var<immediate>` params in `textured.wgsl` — needs `Features::IMMEDIATES`, requested by the app when available; the shader falls back to a no-mask build otherwise).
- ATL (Phase 2): `transform <name>:` at top level defines an animation block — per-line `[warper] <dur> <prop> <expr>...` tweens (`linear ease easein easeout bounce elastic`, unknown → linear), bare `<prop> <expr>` lines set instantly, `pause <s>` waits, `time <t>` syncs to track elapsed, `repeat [n]` wraps the preceding steps (`repeat` alone = forever), `parallel:` splits the sequence into a second track, consecutive `choice [weight]:` blocks pick one branch through the serialized PRNG, `on show:`/`on replace:`/`on hide:` compile into `anim_events` (fired on attach/replace/hide by `Scene::set_sprite`/`hide_sprite` — `on hide` runs on the exiting sprite). Props: `xpos ypos xalign yalign zoom xzoom yzoom alpha rotate xoffset yoffset xanchor yanchor z` (`vnengine-core/src/atl.rs`; rotate is radians, matching `transform <c> rotation=`). `show <c> <img> at <anchor|transform>,...` — known anchors stay anchors, other names compile via `Vm::compile_atl` into `SpriteSpec.anims`/`anim_events` (expressions evaluated at show time, serializable).
- `scene <img> [with <t>] [onlayer <layer>]` — `onlayer` targets `Scene.layer_bgs[<layer>]` (with a `LayerFade`) and hides that layer's sprites; plain `scene` hides Foreground sprites (Ren'Py-style) and swaps `Scene.background`. `image <name>:` defines composite images — `<asset> [when <e>] [alpha=<e>] [offset=(<e>,<e>)] [zoom=<e>]` layers resolved by `Vm::resolve_image_layers` into `SpriteLayer`s on `Scene`/`Show` commands; the renderer draws each layer with the parent transform. Sprite `scale` is `[x, y]` (`xzoom`/`yzoom`); `anchor_frac` shifts the draw pivot.
- Screen language (Phase 3): `screen <name>:` at top level defines declarative UI — `text <e> x=<e> y=<e> [size=] [color=]`, `box x= y= w= h= [color=]`, `image <e> x= y= [w= h= alpha=]`, `button <e> x= y= [w= h= color=]:` followed by an indented action block (`jump`, `set <var> = <e>`, `return [<e>]`, `show screen`, `hide screen` — several actions per button allowed), `if <e>:`/`for <v> in <e>:` expanded at resolution. `rect` coords are normalized 0..1. `show screen <name>`, `hide screen <name>`, `call screen <name> [set <var>]` (blocks until a `return`/`jump` action; `set <var>` receives the return value). A bare screen name is a literal (like `jump` labels); other exprs evaluate. The VM resolves a `ScreenInst` (`vnengine-core/src/screen.rs`) — shown screens live in `Runtime.screens` (serialized); raw button action exprs stay in `VmState.shown_screen_actions`/`PendingScreen.node_actions` so `set`/`return` **evaluate at click time**, not show time. `input <var> ["prompt"]` and `confirm <var> ["msg"]` block for text/yes-no and bind the var; `nvl clear` advances `Runtime.nvl_start` (NVL page = `history[nvl_start..]`); `window show|hide|auto` sets `Runtime.window_mode` (the app drops the dialogue box on `hide`). `Character(..., voice=<e>, image=<e>, kind=<e>)` — `kind="nvl"` marks `Say.nvl` (full-window text); `voice`/`image` are per-line defaults. Journal: `Screen(idx)`/`Input(text)`/`Confirm(bool)` entries replay deterministically.
- Control flow: `break`/`continue` work in `while`/`for` (frame unwinding in `Vm::step`).
- Interpolation `[expr]` in dialogue/menu text evaluates full expressions (nested brackets supported); unresolvable content stays literal `[like this]`.
- The PRNG seed lives in `VmState` — it is serialized, so saves and replays reproduce the same draws. `choice`/`sample`/`shuffle`/`randint`/`random` all draw from it.
- `Runtime.journal` records menu choices; `Runtime::replay(script, journal)` re-runs deterministically.
- `Runtime.music` tracks the current BGM across commands/saves/rollback — hosts resync audio against it.
- 2.5D lighting/camera: `light "<name>" x=<e> y=<e> radius=<e> intensity=<e> [color="#hex"]`, `unlight "<name>"`, `ambient <r> <g> <b>`, `camera [x=<e>] [y=<e>] [zoom=<e>] [over=<sec>]` (`over` = smoothstep animation on the logical clock).
- Adaptive music: `music layer "<n>" ["<track>"] [volume=<e>] [fade=<e>]` upserts a stem loop seeked to the base BGM position; `music layer "<n>" off` removes it. `Runtime::music_layers` is in saves/rollback; the app's `resync_layers` diffs it against the audio engine. `play`/`stop music` clears all stems.
- Named audio channels (Phase 4): `play channel <n> <track> [volume=<e>] [fade=<e>] [loop]` replaces the channel and clears its queue; `queue channel <n> <track> [...] ` appends (starts immediately when idle); `stop channel <n> [fade=<e>]`; `stop all [fade=<e>]` (channels + music + voice); `volume channel <n> <e> [fade=<e>]`; `filter channel <n> lowpass|highpass|bandpass|notch [cutoff=<e>] [resonance=<e>] [mix=<e>]` / `reverb [feedback=<e>] [damping=<e>] [width=<e>] [mix=<e>]` / `off`. Channel name is literal ident/string or `[expr]`. `Runtime::channels` (`ChannelState{current,queue,volume,filter}`) is serialized in saves/rollback; the app's per-frame `resync_channels` diffs it into kira sub-tracks (each carrying a bypassed filter+reverb retuned live via handles), and `Runtime::channel_advance` pops the queue when the host sees a non-looping track end. `settings.voice_duck` (0.5) ducks BGM+stems while a voice line plays (`AudioEngine::update`).
- Lvalue assignment (Phase 5): `set`/`$` and `set <t> op= <e>` accept dict/list targets, not just names — `$ d.k = v`, `$ d["k"] = v`, `$ list[i] += 1`, `$ list[-1] = v` (index out of range errors). Screen-button `set` actions accept the same targets.
- Persistent store & achievements (Phase 5): `persistent` is a dict auto-injected into VM variables — mutations via lvalue assigns live outside saves and are never reverted by rollback (the runtime re-injects it after every snapshot restore; `persistent_dirty`/`mark_persistent_clean` drive host flushing). `achievements` is an injected list of granted names (`in`/`has` checks). `achievement <name> ["desc"]` declares achievements at top level; `grant <name>` / `grant [expr]` emits `Command::Grant` (error on undeclared names), the runtime records it and the app shows a toast. The app loads `persistent.json` (`{vars, achievements}`) from `save_root/save_dir` at startup via `Runtime::set_persistent`, flushes when dirty and on close.
- Lint (Phase 5): `--check-only`/build runs `vnengine_script::lint` — warns on missing `scene`/`show`/`play`/`screen`-image assets (literal paths only), `jump`/`call` to unknown labels (literal only), and unused screens/transforms/achievements/defaults; unknown `with` transitions and unknown `grant` names too. Warnings carry a `needle` resolved to `line`/`col`/`span` against the source (`check_source`); `--check-only --json` emits a `CheckReport` (`{ok,title,labels,assets,licenses,diagnostics}`) consumed by the VS Code extension's `binary` mode.
- Studio overlay: F8 (`Action::Debug`, `debug_panel`) shows frame/label, vars, camera/lights, sprites, music layers, plugin KV. `[`/`]` pick a label, F10 (`DebugJump`) calls `Runtime::jump_to_label` — stack reset to the label body, pause cleared, journal untouched (dev action, not replayable).
- Hot reload: dir mode watches `entry_script` mtime (~2 Hz, `poll_hot_reload`); on change, `Runtime::reload_script` swaps the script while preserving the execution point (label frames re-resolve by name; parse errors keep the old script). Textures are invalidated via `TextureCache::clear`. `run_source` takes `watch: Option<PathBuf>` — packages pass `None`. Vertex lighting via `light_at`; `parallax(layer)` gives per-layer camera depth. All in `Scene` → serialized.
- Assets are read through `AssetSource` (`DirAssets` in dev, `Package` in .vnpkg) — never hardcode `project_root.join(...)` for assets; saves/settings still live under `save_root` on disk.
- i18n/a11y: `translations/<lang>.json` maps original text → localized text (demo ships `fr.json`); `settings.language` cycles shipped locales, `App::tr` translates Say/Narrate/speaker/menu strings at dispatch (identity when absent). `settings.text_scale` scales every `UiText.font_size` in `build_ui`; `settings.reduce_motion` flattens scene/sprite/camera animations to their end state and drops `shake`/`PlayTransition` in `apply_effect`. Translation dirs are packed via `asset_dirs` in vnengine-pack.

## Notes
- Script-language breaking change (Phases 1+2): `Stmt`/`Expr`/`Value`/`Scene`/`ActiveSprite`/`SpriteSpec`/`Command` shapes changed (dynamic assets, dicts, ATL tracks, composite layers, `scale: [f32;2]`, `layer_bgs`, `trans_fx`, new transitions). Old `.vnpkg` files and old savegames do NOT deserialize — rebuild packages with `vnengine-pack` and start new games.
- Windows builds may hit transient file lock errors (os error 32) on proc-macro crates. Retry the build if this happens.
- The wgpu 30 API has breaking changes from earlier versions: `bind_group_layouts` takes `&[Some(...)]`, `PipelineLayoutDescriptor` needs `immediate_size`, `RenderPipelineDescriptor` needs `multiview_mask`, `SurfaceConfiguration` needs `color_space`, vertex buffers are `Option<VertexBufferLayout>`.
- Web stretch: `vnengine-core`/`vnengine-script`/`vnengine-runtime` compile clean for `wasm32-unknown-unknown` (`cargo check --target wasm32-unknown-unknown`). `ahash` was removed from those crates' deps (unused there — only render uses `AHashMap`) because it pulls `getrandom`, which needs JS on wasm. The render/app/audio layer still targets native (wgpu/winit/kira); a browser host would need wgpu-web + WebAudio bindings.
