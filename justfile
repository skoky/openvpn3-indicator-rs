# openvpn3-indicator-rs tasks. Run `just` to list them.

prefix := env("PREFIX", "/usr/local")
destdir := env("DESTDIR", "")
root := destdir + prefix
app_id := "net.openvpn.openvpn3_indicator_rs"

# List available recipes
default:
    @just --list

# Install the system packages needed to build (Debian/Ubuntu)
deps:
    sudo apt install build-essential pkg-config libgtk-4-dev libdbus-1-dev

# Fail early with a hint when the GTK 4 / D-Bus development files are missing
[private]
check-deps:
    #!/usr/bin/env sh
    missing=""
    for pc in gtk4 graphene-gobject-1.0 dbus-1; do
        pkg-config --exists "$pc" || missing="$missing $pc"
    done
    if [ -n "$missing" ]; then
        echo "error: missing development files for:$missing" >&2
        echo "       install them with: just deps   (libgtk-4-dev, libdbus-1-dev)" >&2
        exit 1
    fi

# Build an optimised binary
build: check-deps
    cargo build --release

# Run from the source tree (debug build, icons from data/icons)
run *ARGS: check-deps
    cargo run -- {{ARGS}}

# Print configurations and sessions as seen over D-Bus
dump:
    cargo run -- --dump

# Format, lint and test
check:
    cargo fmt --check
    cargo clippy --all-targets -- -D warnings
    cargo test

# Build a Debian package into target/debian/ (needs `cargo install cargo-deb`)
deb: check-deps
    cargo deb --locked

# Tag the current Cargo.toml version and push the tag; CI builds and publishes the .deb
release:
    #!/usr/bin/env sh
    set -e
    version=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
    if [ -n "$(git status --porcelain)" ]; then
        echo "error: working tree is not clean; commit first" >&2
        exit 1
    fi
    git tag -a "v${version}" -m "Release v${version}"
    git push origin "v${version}"
    echo "Pushed v${version}; GitHub Actions will publish the .deb release."

# Regenerate the SVG icons from tools/gen-icons.py
icons:
    python3 tools/gen-icons.py

# Copy the built binary, desktop file and icons into place (run `just build` first;
# PREFIX=/usr/local, DESTDIR supported). Does not compile, so it is safe under sudo.
install:
    @test -x target/release/openvpn3-indicator-rs || { echo "error: target/release/openvpn3-indicator-rs not found; run 'just build' first (as your user, not root)" >&2; exit 1; }
    install -Dm755 target/release/openvpn3-indicator-rs "{{root}}/bin/openvpn3-indicator-rs"
    install -Dm644 data/applications/{{app_id}}.desktop "{{root}}/share/applications/{{app_id}}.desktop"
    cd data/icons && find hicolor -name '*.svg' -exec install -Dm644 {} "{{root}}/share/icons/{}" \;
    -gtk-update-icon-cache -q "{{root}}/share/icons/hicolor"

# Remove everything `install` put in place
uninstall:
    rm -f "{{root}}/bin/openvpn3-indicator-rs"
    rm -f "{{root}}/share/applications/{{app_id}}.desktop"
    find "{{root}}/share/icons/hicolor" -name 'openvpn3-indicator-rs*.svg' -delete
