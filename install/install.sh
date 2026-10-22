#!/bin/sh
# Install cosmoscope on Linux or macOS from the latest GitHub release.
#   curl -fsSL https://raw.githubusercontent.com/phanminhtai1029/cosmoscope/main/install/install.sh | sh
# Installs into $PREFIX/bin (default ~/.local/bin). No sudo needed.
set -e

REPO="phanminhtai1029/cosmoscope"
PREFIX="${PREFIX:-$HOME/.local}"
BIN="$PREFIX/bin"

os=$(uname -s)
arch=$(uname -m)
case "$os" in
    Linux)
        case "$arch" in
            x86_64|amd64) asset="cosmoscope-linux-x86_64" ;;
            aarch64|arm64) asset="cosmoscope-linux-arm64" ;;
            *) echo "No prebuilt binary for Linux/$arch. Try: cargo install --git https://github.com/$REPO" >&2; exit 1 ;;
        esac ;;
    Darwin) asset="cosmoscope-macos-universal" ;;
    *) echo "Unsupported system: $os. On Windows use install/install.ps1." >&2; exit 1 ;;
esac

url="https://github.com/$REPO/releases/latest/download/$asset"
mkdir -p "$BIN"
echo "Downloading $url"
tmp="$BIN/.cosmoscope.download"
if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$url" -o "$tmp"
elif command -v wget >/dev/null 2>&1; then
    wget -qO "$tmp" "$url"
else
    echo "Need curl or wget to download cosmoscope." >&2; exit 1
fi
chmod 755 "$tmp"
mv "$tmp" "$BIN/cosmoscope"

echo "Installed: $BIN/cosmoscope"
case ":$PATH:" in
    *":$BIN:"*) echo "Run it with: cosmoscope" ;;
    *)
        case "$(basename "${SHELL:-sh}")" in
            zsh)  rc="$HOME/.zshrc" ;;
            bash) rc="$HOME/.bashrc" ;;
            fish) rc="" ;;
            *)    rc="$HOME/.profile" ;;
        esac
        echo ""
        echo "$BIN is not on your PATH yet. Add it with:"
        if [ -z "$rc" ]; then
            echo "    fish_add_path $BIN"
        else
            echo "    echo 'export PATH=\"$BIN:\$PATH\"' >> $rc && . $rc"
        fi
        echo "or run it directly: $BIN/cosmoscope" ;;
esac
