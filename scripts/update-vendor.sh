#!/bin/sh
# Update the vendored Bela headers in bela-sys/vendor/bela.
#
# Usage: scripts/update-vendor.sh <git-ref>
#   <git-ref> is a branch, tag or commit sha of BelaPlatform/Bela.
#   Branches/tags are resolved to a commit sha so the vendored copy is pinned.
#
# After updating, regenerate the bindings (see bela-sys/README.md):
#   cargo xtask bindgen --sysroot <aarch64-sysroot>
set -eu

REF="${1:?usage: $0 <bela-git-ref>}"
REPO_URL="https://github.com/BelaPlatform/Bela"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEST="$ROOT/bela-sys/vendor/bela"

# The include closure of wrapper.h. Extend this list if Bela.h grows new
# includes (the bindgen run will fail on a missing header).
HEADERS="Bela.h GPIOcontrol.h Utilities.h"

SHA="$(git ls-remote "$REPO_URL.git" "refs/heads/$REF" "refs/tags/$REF" | head -n1 | cut -f1)"
[ -n "$SHA" ] || SHA="$REF"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
curl -sfL "$REPO_URL/archive/$SHA.tar.gz" | tar xz -C "$TMP" --strip-components=1

mkdir -p "$DEST/include"
for h in $HEADERS; do
  cp "$TMP/include/$h" "$DEST/include/"
done
cp "$TMP/LICENSE" "$DEST/LICENSE"
printf '%s\n' "$SHA" > "$DEST/COMMIT"

echo "Vendored Bela headers from $SHA"
echo "Next: regenerate the bindings with \`cargo xtask bindgen\`"
