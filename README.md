# vnengine

A cross-platform visual novel engine written in Rust, inspired by Ren'Py.

## Features

- **Custom DSL** (`.vn` files) for writing visual novels with a Ren'Py-like syntax
- **Cross-platform desktop** rendering via wgpu (Windows, Linux, macOS). Web and mobile are planned targets — see [docs/DEPLOYMENT.md](docs/DEPLOYMENT.md) for the roadmap.
- **Text rendering** with glyphon (cosmic-text + wgpu) supporting Unicode and emoji
- **Audio** engine (BGM, SFX, voice) powered by kira
- **Save/load** system with multiple slots
- **CG gallery** tracking
- **Transitions**: fade, dissolve, slide, zoom, cut
- **Typewriter** effect with configurable speed
- **Auto-advance** and **skip** modes
- **Menu choices** with conditional visibility
- **Variables** with a full expression evaluator (arithmetic, comparison, logic)
- **Gamepad** support via gilrs

## Project Structure

```
vnengine/
├── crates/
│   ├── vnengine-core/     # Shared types, config, save/load, scene model
│   ├── vnengine-script/   # .vn DSL: lexer, parser, AST, VM, linter
│   ├── vnengine-runtime/  # Headless game state (no OS access)
│   ├── vnengine-render/   # wgpu renderer (sprites, backgrounds, text, UI)
│   ├── vnengine-audio/    # kira audio engine (BGM/SFX/voice)
│   ├── vnengine-app/      # Main loop, input, scene management, UI
│   ├── vnengine-cli/      # `vnengine` command-line runner
│   ├── vnengine-pack/     # `vnengine-pack` .vnpkg builder/validator
│   ├── vnengine-plugins/  # sandboxed WASM plugins (wasmtime)
│   └── vnengine-lint-wasm/# WASM build of the linter for editors
├── editors/vscode/        # VS Code extension (highlighting + diagnostics)
├── demo/                  # Example VN — doubles as the kit's template
├── docs/                  # Tutorial (EN/FR), DSL reference, deployment
├── kit/                   # Author-kit README template
└── scripts/               # Release packaging helpers
```

## For authors (no Rust required)

Releases ship an **author kit** per platform (`windows-x64`,
`windows-arm64`, `linux-x64`): the `vnengine` and `vnengine-pack`
binaries, the `demo/` project as a template, and the docs — everything
needed to write, run, validate and package a game. A `.vsix` VS Code
extension provides `.vn` syntax highlighting and diagnostics from the
engine's own parser/linter.

- `docs/TUTORIAL.md` / `docs/TUTORIAL.fr.md` — end-to-end walkthrough
- `docs/DSL_REFERENCE.md` — complete `.vn` language reference (all
  examples are validated by the test suite)

## Getting Started

### Running the demo

```bash
# Build the engine
cargo build --release

# Run the demo project
cargo run --release -- --project demo
```

### Creating your own project

1. Create a `project.toml`:
```toml
title = "My Visual Novel"
width = 1280
height = 720
fullscreen = false
entry_script = "script/main.vn"
image_dir = "images"
audio_dir = "audio"
voice_dir = "voice"
save_dir = "saves"
text_speed = 40.0

[[characters]]
id = "e"
name = "Eileen"
color = "#ff4040"
```

2. Write your script in `script/main.vn`:
```vn
define e = Character("Eileen", color = "#ff4040")

label start:
    scene bg school with dissolve
    show eileen happy at center
    e "Hello, world!"
    menu:
        "Option A":
            e "You chose A!"
        "Option B":
            e "You chose B!"
    end
```

3. Run it:
```bash
vnengine --project /path/to/your/project
```

## DSL Reference

### Definitions
```vn
define e = Character("Eileen", color = "#ff4040")
define max_score = 100
```

### Scenes and sprites
```vn
scene bg school with dissolve 1.0
show eileen happy at left
show eileen sad at center with dissolve
hide eileen with fade 0.5
```

### Dialogue
```vn
e "Hello there!" voice "e_hello.ogg"
"This is narration."
```

### Choices
```vn
menu:
    "Go left":
        $ went_left = True
        jump left_path
    "Go right" if score > 50:
        jump right_path
```

### Control flow
```vn
label start:
    jump next_scene

label next_scene:
    call subroutine
    return
```

### Conditionals
```vn
if score > 50:
    e "You did great!"
elif score > 20:
    e "Not bad."
else:
    e "Try harder next time."
```

### Variables and expressions
```vn
$ score = 0
$ score = score + 10
$ name = "Alice"
if name == "Alice" and score >= 100:
    e "Perfect score, Alice!"
```

### Audio
```vn
play music "bgm_calm.ogg" fade 2.0
stop music fade 1.0
play sfx "click.wav" volume 0.8
voice "e_line_01.ogg"
stop voice
```

### Transitions
```vn
scene bg school with dissolve 1.0
scene bg night with fade 2.0
scene bg city with slideleft 0.8
```

Available transitions: `cut`, `fade`, `fadewhite`, `fadeblack`, `dissolve`, `slideleft`, `slideright`, `slideup`, `slidedown`, `zoom`

## Controls

| Key | Action |
|-----|--------|
| Space / Enter | Advance dialogue / Confirm |
| Escape / Backspace | Cancel / Back |
| Left Click | Advance / Confirm |
| F5 | Save |
| F9 | Load |
| F11 | Toggle fullscreen |
| A | Toggle auto-advance |
| S | Toggle skip |
| F1 | Settings |
| F4 / Q | Quit |

## License

MIT OR Apache-2.0
