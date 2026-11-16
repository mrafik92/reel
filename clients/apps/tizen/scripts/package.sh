#!/usr/bin/env bash
# Build apps/web for Tizen, stage with config.xml + icon, then `tizen build-web` + `tizen package` if available.
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
web="$here/../web"
stage="$here/.staging"
out="$here/out"

echo "==> Building apps/web (VITE_CLIENT=tizen, VITE_TARGET=tv)"
rm -rf "$stage" && mkdir -p "$stage"
(cd "$web" && VITE_CLIENT=tizen VITE_TARGET=tv bun run build --outDir "$stage" --emptyOutDir)

echo "==> Staging"
cp "$here/config.xml" "$stage"/
cp "$here/assets/icon.png" "$stage"/

mkdir -p "$out"
if command -v tizen >/dev/null; then
  : "${TIZEN_CERT_PROFILE:?set TIZEN_CERT_PROFILE to your signing profile name (Tizen Studio > Certificate Manager)}"
  echo "==> tizen build-web"
  tizen build-web -out .buildResult -- "$stage"
  echo "==> tizen package"
  tizen package -t wgt -s "$TIZEN_CERT_PROFILE" -o "$out" -- "$stage/.buildResult"
  echo "Package(s) in $out:"; ls -1 "$out"
else
  cat <<MSG
tizen CLI not found — staged app is ready in: $stage
Install Tizen Studio (or the CLI-only package) with the TV Extensions and add \$HOME/tizen-studio/tools/ide/bin to PATH.
Then:  cd "$stage" && tizen build-web -out .buildResult -- . \\
         && tizen package -t wgt -s <cert-profile> -o "$out" -- .buildResult
Or re-run: TIZEN_CERT_PROFILE=<cert-profile> bun run package
(see apps/tizen/README.md)
MSG
fi
