#!/usr/bin/env bash
# Build VELA for Linux: a .deb (libmpv from the distro) and a portable tarball.
#
#   sudo apt install libmpv-dev        # Debian/Ubuntu (Fedora: mpv-libs-devel)
#   cargo install cargo-deb
#   packaging/linux/build.sh
set -euo pipefail
cd "$(dirname "$0")/../.."
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
mkdir -p dist

cargo build --release
cargo deb --no-build --output "dist/vela_${VERSION}_amd64.deb"

T=$(mktemp -d)/vela-$VERSION
mkdir -p "$T"
cp target/release/vela packaging/linux/vela.desktop build/icon.png LICENSE installer/THIRD-PARTY-NOTICES.txt "$T/"
cat > "$T/install.sh" <<'EOF'
#!/bin/sh
# Per-user install: binary, menu entry, icon and "Open With" registration.
set -e
cd "$(dirname "$0")"
install -Dm755 vela "$HOME/.local/bin/vela"
install -Dm644 vela.desktop "$HOME/.local/share/applications/vela.desktop"
install -Dm644 icon.png "$HOME/.local/share/icons/hicolor/1024x1024/apps/vela.png"
update-desktop-database "$HOME/.local/share/applications" 2>/dev/null || true
echo "VELA installed. It needs libmpv (e.g. sudo apt install libmpv2)."
EOF
chmod +x "$T/install.sh"
tar -C "$(dirname "$T")" -czf "dist/vela-$VERSION-linux-x86_64.tar.gz" "$(basename "$T")"
echo "built dist/vela_${VERSION}_amd64.deb and dist/vela-$VERSION-linux-x86_64.tar.gz"
