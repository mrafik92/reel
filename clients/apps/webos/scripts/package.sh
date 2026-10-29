#!/usr/bin/env bash
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
dist="$here/../web/dist"
stage="$here/.staging"
[ -d "$dist" ] || { echo "Build web first: (cd ../.. && bun run build)"; exit 1; }
rm -rf "$stage" && mkdir -p "$stage"
cp -R "$dist"/. "$stage"/
cp "$here/appinfo.json" "$stage"/
for f in icon.png largeIcon.png; do
  [ -f "$here/$f" ] && cp "$here/$f" "$stage"/ || echo "warning: $f missing (placeholder; add a 80x80 icon.png / 130x130 largeIcon.png)"
done
if command -v ares-package >/dev/null; then
  ares-package "$stage" -o "$here/out"
else
  echo "ares-package not found. Install: npm i -g @webos-tooling/cli"
  echo "Then run: ares-package $stage -o $here/out"
fi
