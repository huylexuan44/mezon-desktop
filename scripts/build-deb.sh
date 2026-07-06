#!/usr/bin/env bash
# Build a .deb package for Mezon Desktop.
#
# Prerequisites (on Linux):
#   ./scripts/linux-deps          # native libraries for GPUI/GTK
#   rustup + toolchain from rust-toolchain.toml
#
# Output: target/debian/mezon_<version>_<arch>.deb

set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

if [ "$(uname -s)" != "Linux" ]; then
  echo "error: .deb packages must be built on Linux." >&2
  echo "hint: use a Linux VM, container, or CI runner with this script." >&2
  exit 1
fi

if ! command -v cargo-deb >/dev/null 2>&1; then
  echo "Installing cargo-deb..."
  cargo install cargo-deb --locked
fi

echo "Building release binary and .deb package..."
cargo deb -p mezon-app --locked

echo ""
echo "Built package(s) in target/debian/:"
ls -1 target/debian/*.deb
