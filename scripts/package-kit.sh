#!/usr/bin/env bash
# Assemble the author kit for one platform.
#
#   scripts/package-kit.sh <release_bin_dir> <kit_name> <exe_ext>
#
# Example: scripts/package-kit.sh target/x86_64-pc-windows-msvc/release \
#            vnengine-kit-windows-x64 .exe
#
# Produces dist/<kit_name>.zip (7z available) or .tar.gz otherwise.
set -euo pipefail

BIN_DIR="$1"
KIT="$2"
EXT="${3:-}"
STAGE="target/kit/$KIT"

rm -rf "$STAGE"
mkdir -p "$STAGE/bin" "$STAGE/docs"

# Engine binaries — the kit works without a Rust toolchain.
cp "$BIN_DIR/vnengine$EXT" "$BIN_DIR/vnengine-pack$EXT" "$STAGE/bin/"
chmod +x "$STAGE/bin/"* 2>/dev/null || true

# Template project (the demo doubles as the starter project).
cp -r demo "$STAGE/template"
rm -rf "$STAGE/template/saves"

# Documentation shipped with the kit.
cp docs/TUTORIAL.md docs/TUTORIAL.fr.md docs/DSL_REFERENCE.md "$STAGE/docs/"
mkdir -p "$STAGE/template/docs"
cp docs/DSL_REFERENCE.md "$STAGE/template/docs/"
cp kit/README.md "$STAGE/README.md"
cp LICENSE-MIT LICENSE-APACHE "$STAGE/"

mkdir -p dist
cd "target/kit"
if command -v 7z >/dev/null 2>&1; then
    7z a -tzip "../../dist/$KIT.zip" "$KIT" >/dev/null
    echo "dist/$KIT.zip"
else
    tar -czf "../../dist/$KIT.tar.gz" "$KIT"
    echo "dist/$KIT.tar.gz"
fi
