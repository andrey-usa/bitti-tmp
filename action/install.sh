#!/usr/bin/env bash
# The action's install step: a release binary for this runner (checked
# against the SHA-256 published beside it), else a build from the action's
# own commit with navigera beside it. Puts `bitti` on the PATH.
set -euo pipefail

# Git Bash on Windows: POSIX paths here, a Windows one for GITHUB_PATH.
unix() { cygpath -u "$1" 2>/dev/null || printf '%s\n' "$1"; }
BT_ACTION=$(unix "$BT_ACTION")
temp=$(unix "$RUNNER_TEMP")
root="$temp/bitti"
mkdir -p "$root/bin"
cygpath -w "$root/bin" >> "$GITHUB_PATH" 2>/dev/null || echo "$root/bin" >> "$GITHUB_PATH"

case "$RUNNER_OS/$RUNNER_ARCH" in
  Linux/X64) target=x86_64-unknown-linux-musl ext=tar.gz ;;
  Linux/ARM64) target=aarch64-unknown-linux-musl ext=tar.gz ;;
  macOS/ARM64) target=aarch64-apple-darwin ext=tar.gz ;;
  macOS/X64) target=x86_64-apple-darwin ext=tar.gz ;;
  Windows/X64) target=x86_64-pc-windows-msvc ext=zip ;;
  *) target="" ext="" ;;
esac

sha256() { if command -v sha256sum >/dev/null; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1; }

release() {
  [ -n "$target" ] || return 1
  case "$BT_VERSION" in
    latest) url="https://github.com/andrey-usa/bitti/releases/latest/download/bitti-$target.$ext" ;;
    *) url="https://github.com/andrey-usa/bitti/releases/download/$BT_VERSION/bitti-$target.$ext" ;;
  esac
  tmp=$(mktemp -d)
  curl --proto '=https' --tlsv1.2 -fsSL --retry 3 -o "$tmp/bt.$ext" "$url" || return 1
  curl --proto '=https' --tlsv1.2 -fsSL --retry 3 -o "$tmp/bt.sha256" "$url.sha256" || {
    echo "::error title=bitti::no checksum beside $url"; exit 1; }
  [ "$(cut -d' ' -f1 < "$tmp/bt.sha256")" = "$(sha256 "$tmp/bt.$ext")" ] || {
    echo "::error title=bitti::$url does not match its checksum"; exit 1; }
  if [ "$ext" = zip ]; then
    # Git Bash may have no unzip; PowerShell always can.
    pwsh -NoProfile -Command "Expand-Archive -Force -Path '$(cygpath -w "$tmp/bt.zip")' -DestinationPath '$(cygpath -w "$root/bin")'" || return 1
  else
    tar -C "$root/bin" -xzf "$tmp/bt.tar.gz" || return 1
  fi
  echo "bitti $BT_VERSION from $url"
}

source_build() {
  # Cargo.toml takes navigera from ../navigera.
  nav="$BT_ACTION/../navigera"
  if [ ! -f "$nav/Cargo.toml" ]; then
    git clone --quiet --filter=blob:none https://github.com/andrey-usa/navigera "$nav"
    git -C "$nav" checkout --quiet "$BT_NAVIGERA_REF" || {
      echo "::error title=bitti::navigera has no branch, tag or commit '$BT_NAVIGERA_REF' (the action's navigera-ref input)"; exit 1; }
  fi
  echo "building bitti from $BT_ACTION with navigera $(git -C "$nav" rev-parse --short HEAD)"
  CARGO_TARGET_DIR="$temp/bitti-target" cargo install --quiet --locked --path "$BT_ACTION" --root "$root"
}

if [ "$BT_VERSION" = source ] || ! release; then
  [ "$BT_VERSION" = source ] || echo "::notice title=bitti::no release binary for ${target:-this runner} ($BT_VERSION); building from source"
  source_build
fi
"$root/bin/bitti" --version
