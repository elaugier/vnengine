# vnengine tutorial — from zero to a packaged game

*(Ce tutoriel existe aussi en français : `TUTORIAL.fr.md`.)*

This walkthrough takes you from the author kit to a distributable
`.vnpkg` — without installing Rust. Estimated time: ~30 minutes.

## 1. Install the author kit

1. Download `vnengine-kit-<platform>.zip` (or `.tar.gz`) from the
   releases page: `windows-x64`, `windows-arm64` or `linux-x64`.
2. Unpack it anywhere. Inside you'll find:

   ```text
   bin/         vnengine, vnengine-pack
   template/    starter project
   docs/        this tutorial + the DSL reference
   ```

3. Put `bin/` on your `PATH`, or remember its absolute path — every
   command below works with a full path too (`kit/bin/vnengine --project .`).

Optional but recommended: install the VS Code extension (`vnengine.vsix`
from the same release) for syntax highlighting, in-editor diagnostics and
run/validate/package commands:

```bash
code --install-extension vnengine.vsix
```

## 2. Start from the template

The `template/` directory is a complete mini visual novel. Copy it so the
kit stays pristine:

```bash
cp -r template my-novel
cd my-novel
```

In PowerShell, use `Copy-Item template my-novel -Recurse`, then
`Set-Location my-novel`.

Project layout:

```text
project.toml       engine config: title, resolution, dirs, characters
script/main.vn     your story (the .vn DSL)
images/            backgrounds and sprites
audio/             music and sound effects
voice/             voiced lines
plugins/           optional WASM plugins
saves/             created at runtime — never ship it
```

## 3. Run it

```bash
vnengine --project .
```

Controls: **Space/Enter/click** advance · **F5** save · **F9** load ·
**F11** fullscreen · **A** auto · **S** skip · **F8** debug overlay ·
**F4/Q** quit.

While `vnengine` is running from a project directory it **hot-reloads**
`script/main.vn` (~2s poll): edit the script, save, watch the change.

## 4. Write your first scene

Open `script/main.vn`. The essentials:

```vn
define e = Character("Eileen", color = "#ff4040")

label start:
    scene bg school with dissolve
    show eileen happy at center
    e "Hello, world!"
    menu:
        "Say hi back":
            e "She smiles."
        "Stay silent":
            e "She shrugs."
    end
```

Assets are referenced **without extension** and relative to their dir:
`scene bg school` loads `images/bg school.png` (or `.jpg`/`.webp`);
`play music "theme"` loads `audio/theme.ogg`.

Everything the language can do is in `docs/DSL_REFERENCE.md` — sprites,
transitions, variables, menus, screens, audio channels, achievements,
persistent data, plugins.

## 5. Validate while you write

```bash
vnengine-pack --project . --check-only
```

This parses the script and lints it: unknown labels, missing assets,
undeclared speakers, unknown transforms/screens/achievements, unreachable
labels. Machine-readable output for editors and CI:

```bash
vnengine-pack --project . --check-only --json
```

With the VS Code extension, these same checks appear as squiggles while
you type — powered by the engine's parser compiled to WASM, no binary
required.

## 6. Package the game

```bash
vnengine-pack --project . --output build/my-novel.vnpkg
```

The `.vnpkg` is a deterministic archive: manifest, compiled script,
hashed assets, licenses. It ships to players as a single file.

## 7. Ship it

Players run the package with the engine binary for their platform:

```bash
vnengine --package my-novel.vnpkg
```

Typical release layout:

```text
my-novel-windows-x64/
├── vnengine.exe          (from the kit's bin/)
└── my-novel.vnpkg
```

## Troubleshooting

| Problem | Fix |
|---|---|
| `vnengine` not found | Add the kit's `bin/` to `PATH`, or use the full path. |
| `warning: missing image asset` | The name after `scene`/`show` must match a file under `images/` (extension omitted). |
| Parse error at line N | Indentation is significant — use spaces, keep blocks consistent. |
| Package missing a file | Check `vnengine-pack --check-only` warnings; assets live under `images/`, `audio/`, `voice/`, `translations/`, `plugins/`. |
