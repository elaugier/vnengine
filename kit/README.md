# vnengine author kit

Everything needed to write, run, validate and package a visual novel with
vnengine — **no Rust toolchain required**.

## Contents

```text
bin/         vnengine (player/runner) and vnengine-pack (validator/packager)
template/    starter project — copy it to start your own game
docs/        TUTORIAL.md, TUTORIAL.fr.md, DSL_REFERENCE.md
LICENSE-*    engine licenses (MIT OR Apache-2.0)
```

## Quick start

Put `bin/` on your `PATH` (or invoke the binaries by full path), then:

```bash
# 1. Copy the template somewhere writable
cp -r template my-novel && cd my-novel

# 2. Run it
vnengine --project .

# 3. Validate the project (lint + asset checks)
vnengine-pack --project . --check-only

# 4. Produce a distributable .vnpkg
vnengine-pack --project . --output build/my-novel.vnpkg
```

In PowerShell, copy the template with `Copy-Item template my-novel -Recurse`
and enter it with `Set-Location my-novel`.

Edit `my-novel/project.toml` (title, resolution, characters) and
`my-novel/script/main.vn` (the story). The copied project includes
`docs/DSL_REFERENCE.md` for authors and coding agents.

Players run the packaged game with:

```bash
vnengine --package build/my-novel.vnpkg
```

## Editor support

A VS Code extension (`.vsix`, shipped alongside this kit on the releases
page) adds `.vn` syntax highlighting, diagnostics from the engine's own
parser/linter, and Run/Validate/Package commands. Install with:

```bash
code --install-extension vnengine.vsix
```

## Next steps

- `docs/TUTORIAL.md` — full walkthrough: first scene to shipped package.
- `docs/TUTORIAL.fr.md` — le même tutoriel en français.
- `docs/DSL_REFERENCE.md` — complete `.vn` language reference.
- `template/AGENTS.md` — conventions and verification commands for
  AI coding agents working inside the template project.
