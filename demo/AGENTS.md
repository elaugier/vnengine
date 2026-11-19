# vnengine project — agent guide

You are working inside a **vnengine visual novel project**. The game is
data + scripts, not engine code: don't modify engine binaries or add a
Rust toolchain — everything runs through the prebuilt `vnengine` and
`vnengine-pack` binaries from the author kit (on `PATH`, or ask where the
kit is installed).

## Layout

- `project.toml` — title, resolution, asset dir names, `[[characters]]`.
- `script/main.vn` — the story (`.vn` DSL). `entry_script` in
  `project.toml` points here.
- `images/`, `audio/`, `voice/` — assets, referenced **without extension**
  and relative to their dir (`scene bg school` → `images/bg school.png`).
- `translations/` — `<lang>.json` maps original text → localized text.
- `plugins/` — optional sandboxed WASM plugins (`<name>.wasm|.wat` +
  `<name>.toml` manifest).
- `saves/` — runtime save files. Never edit, never ship.
- `persistent.json` (under `saves/`) — cross-save store, also not shipped.

## Commands

Run these from the project root (the directory containing
`project.toml`):

```bash
vnengine --project .                          # run the game
vnengine-pack --project . --check-only        # parse + lint (must stay clean)
vnengine-pack --project . --check-only --json # same, machine-readable
vnengine-pack --project . --output build/game.vnpkg   # distributable package
```

## Verification

After any script change, run `vnengine-pack --project . --check-only` and
fix every error **and** warning you introduced. The linter reports
unknown labels/screens/transforms/achievements, missing literal assets,
undeclared speakers and unreachable labels. Dynamic targets
(`jump [expr]`, `scene [expr]`) are legitimate and unchecked — warnings
about them are false positives you may leave, but prefer literals when
the target is static so the linter can help you.

## Conventions

- Indentation is significant — spaces, consistent depth.
- New story flow goes in `label` blocks; keep `label start` as the entry.
- Declare every `Character(...)` id in `project.toml` `[[characters]]`
  **or** via `define` — the linter checks speakers against both.
- Declare achievements with `achievement <name>` before `grant <name>`.
- Keep asset names kebab/snake without spaces where possible; quoted
  names may contain spaces.
- Persist cross-run state in `persistent` (dict), not in globals you
  expect to survive loading an old save.

## Language reference

`docs/DSL_REFERENCE.md` — the complete, test-validated `.vn` language
reference, included inside the starter project. When working on the
demo in the engine repository, use `../docs/DSL_REFERENCE.md` from the
demo directory. The reference's ```vn examples are checked by the parser.

When unsure whether a statement exists: check the reference, then
validate a scratch edit with `vnengine-pack --check-only` — parse errors
report exact line numbers.
