#!/usr/bin/env bash
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
dist="$here/../web/dist"
stage="$here/.staging"
[ -d "$dist" ] || { echo "Build web first: (cd ../.. && bun run build)"; exit 1; }
rm -rf "$stage" && mkdir -p "$stage"
cp -R "$dist"/. "$stage"/
cp "$here/config.xml" "$stage"/
[ -f "$here/icon.png" ] && cp "$here/icon.png" "$stage"/ || echo "warning: icon.png missing (add a 117x117 icon.png)"
if command -v tizen >/dev/null; then
  tizen package -t wgt -s "${TIZEN_CERT_PROFILE:?set TIZEN_CERT_PROFILE to your signing profile name}" -o "$here/out" -- "$stage"
else
  echo "tizen CLI not found. Install Tizen Studio (with TV Extensions) and add tizen to PATH."
  echo "Then run: tizen package -t wgt -s <cert-profile> -o $here/out -- $stage"
fi
