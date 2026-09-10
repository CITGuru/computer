#!/bin/sh
# Install `computer` and `computerd` from a GitHub release.
#
#   curl -fsSL https://raw.githubusercontent.com/CITGuru/computer/main/scripts/install.sh | sh
#
# Reads:
#   COMPUTER_VERSION      a tag such as v0.1.0. Default: the latest release.
#   COMPUTER_INSTALL_DIR  where the binaries go. Default: ~/.local/bin.
#
# Flags:
#   --print-target        say which build this machine wants, and stop.
#   --version <tag>       same as COMPUTER_VERSION.
#   --dir <path>          same as COMPUTER_INSTALL_DIR.
#
# POSIX sh on purpose: this is piped into whatever /bin/sh happens to be.

set -eu

REPO="CITGuru/computer"
VERSION="${COMPUTER_VERSION:-}"
INSTALL_DIR="${COMPUTER_INSTALL_DIR:-$HOME/.local/bin}"
PRINT_TARGET=""

die() {
    echo "install: $*" >&2
    exit 1
}

need() {
    command -v "$1" >/dev/null 2>&1 || die "$1 is needed and was not found"
}

detect() {
    kernel="$(uname -s)"
    machine="$(uname -m)"

    case "$kernel" in
    Darwin)
        case "$machine" in
        arm64 | aarch64) echo "aarch64-apple-darwin" ;;
        x86_64) echo "x86_64-apple-darwin" ;;
        *) die "no build for macOS on $machine; try: cargo install computer" ;;
        esac
        ;;
    Linux)
        case "$machine" in
        x86_64 | amd64) echo "x86_64-unknown-linux-gnu" ;;
        aarch64 | arm64) echo "aarch64-unknown-linux-gnu" ;;
        *) die "no build for Linux on $machine; try: cargo install computer" ;;
        esac
        ;;
    *)
        die "no build for $kernel; try: cargo install computer"
        ;;
    esac
}

latest() {
    url="$(curl -fsSLI -o /dev/null -w '%{url_effective}' \
        "https://github.com/$REPO/releases/latest")" ||
        die "could not ask $REPO what its latest release is"

    case "$url" in
    */releases/tag/*) echo "${url##*/}" ;;
    *) die "$REPO has published no release yet; try: cargo install computer" ;;
    esac
}

verify() {
    archive="$1"
    sums="$2"

    if command -v sha256sum >/dev/null 2>&1; then
        grep " $archive\$" "$sums" | sha256sum -c - >/dev/null
    elif command -v shasum >/dev/null 2>&1; then
        grep " $archive\$" "$sums" | shasum -a 256 -c - >/dev/null
    else
        die "neither sha256sum nor shasum is here, so the download cannot be checked"
    fi
}

while [ $# -gt 0 ]; do
    case "$1" in
    --print-target)
        PRINT_TARGET=1
        shift
        ;;
    --version)
        [ $# -ge 2 ] || die "--version needs a tag"
        VERSION="$2"
        shift 2
        ;;
    --dir)
        [ $# -ge 2 ] || die "--dir needs a path"
        INSTALL_DIR="$2"
        shift 2
        ;;
    -h | --help)
        sed -n '2,18p' "$0" | sed 's/^# \{0,1\}//'
        exit 0
        ;;
    *)
        die "unknown option: $1"
        ;;
    esac
done

TARGET="$(detect)"

if [ -n "$PRINT_TARGET" ]; then
    echo "$TARGET"
    exit 0
fi

need curl
need tar

[ -n "$VERSION" ] || VERSION="$(latest)"

ARCHIVE="computer-$VERSION-$TARGET.tar.gz"
BASE="https://github.com/$REPO/releases/download/$VERSION"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT INT TERM

echo "install: fetching computer $VERSION for $TARGET"
curl -fsSL "$BASE/$ARCHIVE" -o "$WORK/$ARCHIVE" 2>/dev/null ||
    die "no $ARCHIVE in $VERSION; see https://github.com/$REPO/releases"
curl -fsSL "$BASE/SHA256SUMS" -o "$WORK/SHA256SUMS" 2>/dev/null ||
    die "$VERSION has no SHA256SUMS, so the download cannot be checked"

(cd "$WORK" && verify "$ARCHIVE" SHA256SUMS) ||
    die "$ARCHIVE does not match its checksum; nothing was installed"

tar -xzf "$WORK/$ARCHIVE" -C "$WORK"

mkdir -p "$INSTALL_DIR"
for binary in computer computerd; do
    [ -f "$WORK/$binary" ] || die "$ARCHIVE holds no $binary"
    cp "$WORK/$binary" "$INSTALL_DIR/$binary.new"
    chmod +x "$INSTALL_DIR/$binary.new"
    mv "$INSTALL_DIR/$binary.new" "$INSTALL_DIR/$binary"
    echo "install: $INSTALL_DIR/$binary"
done

case ":${PATH}:" in
*":$INSTALL_DIR:"*) ;;
*)
    echo "install: $INSTALL_DIR is not on your PATH. Add it:" >&2
    echo "  export PATH=\"$INSTALL_DIR:\$PATH\"" >&2
    ;;
esac

echo "install: done. Try: computer up"
