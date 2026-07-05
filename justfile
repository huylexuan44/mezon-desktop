# ------------------------------------------------------------------------------
# GENERAL
# ------------------------------------------------------------------------------

# Crates we own — vendored Zed crates are excluded (we don't lint/test their code;
# some of their test targets don't even compile against our pinned deps).
pkgs := "-p mezon-app -p mezon-ui -p mezon-store -p mezon-client -p mezon-native -p mezon-proto -p mezon-i18n -p mezon-updater -p mezon-audio"

# Formatting scope — pkgs plus mezon-voice (excluded from clippy/test above),
# still excluding vendored crates (read-only, carry upstream fmt drift).
fmt_pkgs := pkgs + " -p mezon-voice"

# List available recipes
default:
    @just help

help:
    @echo ""
    @echo "  Mezon Desktop (Rust/GPUI)"
    @echo ""
    @echo "  Usage: just <recipe>"
    @echo ""
    @echo "  Development"
    @echo "  ---------------------------------------------"
    @echo "  install           Install development tools (via cargo-binstall)"
    @echo "  install-linux-deps Install Linux system libraries for GPUI/GTK"
    @echo "  run             Build (debug) and run the app"
    @echo "  watch           Hot-reload development (requires cargo-watch)"
    @echo "  check           Fast clippy checks"
    @echo "  lint            Strict linting before commit"
    @echo "  fix             Auto-fix formatting and clippy suggestions"
    @echo ""
    @echo "  Testing"
    @echo "  ---------------------------------------------"
    @echo "  test            Run all tests in the workspace"
    @echo "  test <args>     Forward args to cargo-nextest"
    @echo "                  e.g. just test -p my_crate"
    @echo "                  e.g. just test my_test_name"
    @echo ""
    @echo "  Coverage"
    @echo "  ---------------------------------------------"
    @echo "  cov             Generate and open HTML coverage report"
    @echo "  cov-summary     Show coverage summary in terminal"
    @echo ""
    @echo "  Security & Maintenance"
    @echo "  ---------------------------------------------"
    @echo "  safety          Run security and license checks"
    @echo "  audit           Audit dependencies for advisories"
    @echo "  outdated        Check for outdated dependencies"
    @echo "  update          Update Cargo dependencies"
    @echo ""

# ------------------------------------------------------------------------------
# DEVELOPMENT
# ------------------------------------------------------------------------------

# Install all necessary CLI tools via cargo-binstall
install:
    @echo "Installing development tools..."
    cargo install cargo-binstall || true
    cargo binstall -y cargo-watch cargo-nextest cargo-deny cargo-outdated cargo-llvm-cov

# Install Linux system libraries required to build GPUI, GTK tray, and accessibility
install-linux-deps:
    @bash scripts/linux-deps

# Run the project with optional arguments
run *args:
    cargo run {{args}}

# Hot-reload development (requires cargo-watch)
watch:
    cargo watch -x run

# Profile with Tracy (open Tracy 0.11.x GUI to connect; CPU + memory + frames)
tracy:
    cargo run --profile profiling --features tracy

# Fast check for errors during development
check:
    cargo clippy {{pkgs}} -- -D warnings

# Strict linting (Use before commit/push)
lint:
    cargo clippy {{pkgs}} --all-targets --all-features --locked -- -D warnings
    cargo fmt {{fmt_pkgs}} -- --check

# Auto-fix formatting and clippy suggestions
fix:
    cargo fmt {{fmt_pkgs}}
    cargo clippy {{pkgs}} --fix --allow-dirty --allow-staged

# ------------------------------------------------------------------------------
# TESTING (Nextest)
# ------------------------------------------------------------------------------

# Run all tests in the workspace, or pass args straight to cargo-nextest
test *args:
    sh -c 'if [ "$#" -eq 0 ]; then exec cargo nextest run {{pkgs}} --all-targets; fi; exec cargo nextest run "$@"' sh {{args}}

# ------------------------------------------------------------------------------
# CODE COVERAGE (llvm-cov)
# ------------------------------------------------------------------------------

# Generate and open HTML coverage report
cov:
    cargo llvm-cov {{pkgs}} --all-features --open

# Run coverage and show summary in terminal
cov-summary:
    cargo llvm-cov {{pkgs}} --all-features

# ------------------------------------------------------------------------------
# SECURITY & MAINTENANCE
# ------------------------------------------------------------------------------

# Run all security and license checks
safety:
    cargo deny check

# Audit dependencies for security vulnerabilities
audit:
    cargo deny check advisories

bans:
    cargo deny check bans

# Check for outdated dependencies
outdated:
    cargo outdated -R

# Update dependencies
update:
    cargo update

# ------------------------------------------------------------------------------
# BUILD & CLEAN
# ------------------------------------------------------------------------------

# Build production release
release:
    cargo build --release

# Bundle the release binary into a macOS Mezon.app
bundle: release
    #!/usr/bin/env bash
    set -euo pipefail
    app="target/release/bundle/Mezon.app"
    rm -rf "$app"
    mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
    cp crates/mezon-app/Info.plist "$app/Contents/Info.plist"
    cp target/release/mezon "$app/Contents/MacOS/mezon"
    chmod +x "$app/Contents/MacOS/mezon"
    for icon in assets/icons/app.icns crates/mezon-app/app.icns assets/app.icns; do
        if [ -f "$icon" ]; then cp "$icon" "$app/Contents/Resources/app.icns"; break; fi
    done
    codesign --force --deep --sign - "$app" >/dev/null 2>&1 || true
    echo "Built $app"
    echo "Run: open \"$app\"  (or double-click in Finder)"

# Clean build artifacts
clean:
    cargo clean
    @echo "Cleaned target directory."
    


