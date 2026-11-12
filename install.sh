#!/bin/sh
# Install the meta CLI from a GitHub release. No sudo needed.
#   curl -fsSL https://raw.githubusercontent.com/galaxy-gateway/glxy-repo/main/install.sh | sh
# Env: VERSION (default: latest release), INSTALL_DIR (default: $HOME/.local/bin)
set -eu

REPO="galaxy-gateway/glxy-repo"
INSTALL_DIR="${INSTALL_DIR:-$HOME/.local/bin}"

die() {
    echo "install.sh: $*" >&2
    exit 1
}

case "$(uname -s)" in
    Linux) os="unknown-linux-gnu" ;;
    Darwin) os="apple-darwin" ;;
    *) die "unsupported OS: $(uname -s) (on Windows, download the zip from https://github.com/$REPO/releases)" ;;
esac
case "$(uname -m)" in
    x86_64 | amd64) cpu="x86_64" ;;
    arm64 | aarch64) cpu="aarch64" ;;
    *) die "unsupported CPU: $(uname -m)" ;;
esac
target="$cpu-$os"

version="${VERSION:-}"
if [ -z "$version" ]; then
    # The latest-release URL redirects to .../releases/tag/vX.Y.Z
    version="$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$REPO/releases/latest")" ||
        die "could not look up the latest release"
    version="${version##*/}"
fi
version="${version#v}"

name="meta-$version-$target"
url="https://github.com/$REPO/releases/download/v$version/$name.tar.gz"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

echo "Downloading $name.tar.gz"
curl -fsSL -o "$tmp/$name.tar.gz" "$url" || die "download failed: $url"
curl -fsSL -o "$tmp/$name.tar.gz.sha256" "$url.sha256" || die "download failed: $url.sha256"

if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "$tmp/$name.tar.gz")"
elif command -v shasum >/dev/null 2>&1; then
    actual="$(shasum -a 256 "$tmp/$name.tar.gz")"
else
    die "need sha256sum or shasum to verify the download"
fi
actual="${actual%% *}"
read -r expected _ <"$tmp/$name.tar.gz.sha256" || true
if [ -z "$expected" ] || [ "$actual" != "$expected" ]; then
    die "checksum mismatch for $name.tar.gz (expected ${expected:-nothing}, got $actual)"
fi

tar -xzf "$tmp/$name.tar.gz" -C "$tmp" || die "could not unpack $name.tar.gz"
mkdir -p "$INSTALL_DIR"
install -m 755 "$tmp/$name/meta" "$INSTALL_DIR/meta" || die "could not install into $INSTALL_DIR"
echo "Installed meta $version to $INSTALL_DIR/meta"

case ":$PATH:" in
    *":$INSTALL_DIR:"*) ;;
    *) echo "Note: $INSTALL_DIR is not on your PATH. Add it, for example: export PATH=\"$INSTALL_DIR:\$PATH\"" ;;
esac
