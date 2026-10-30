#!/usr/bin/env bash
# Build apps/web for webOS, stage it with appinfo.json + icons, and run ares-package if available.
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
web="$here/../web"
stage="$here/.staging"
out="$here/out"

[ -f "$here/assets/icon.png" ] || bun "$here/scripts/gen-icons.ts"

echo "==> Building apps/web (VITE_CLIENT=webos, VITE_TARGET=tv)"
rm -rf "$stage" && mkdir -p "$stage"
(cd "$web" && VITE_CLIENT=webos VITE_TARGET=tv bun run build --outDir "$stage" --emptyOutDir)

echo "==> Staging"
cp "$here/appinfo.json" "$stage"/
cp "$here/assets/icon.png" "$here/assets/largeIcon.png" "$here/assets/splash.png" "$stage"/

mkdir -p "$out"
if command -v ares-package >/dev/null; then
  echo "==> ares-package"
  ares-package "$stage" -o "$out"
  echo "Package(s) in $out:"; ls -1 "$out"
else
  cat <<MSG
ares-package not found — staged app is ready in: $stage
Install the CLI:   npm i -g @webos-tooling/cli
Then package:      ares-package "$stage" -o "$out"
Install + launch:  ares-install --device tv "$out"/dev.reel.app_0.0.1_all.ipk && ares-launch --device tv dev.reel.app
(see apps/webos/README.md)
MSG
fi
