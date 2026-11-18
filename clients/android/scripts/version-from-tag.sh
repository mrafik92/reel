#!/usr/bin/env bash
# Derive the Android versionName / versionCode from a release tag (#195, #232).
#
#   android-vMAJOR.MINOR.PATCH      (Compose app, clients/android)
#   android-web-vMAJOR.MINOR.PATCH  (Reel Web, clients/android-web)
#                                  →  versionName = MAJOR.MINOR.PATCH
#                                     versionCode = MAJOR*1_000_000 + MINOR*1_000 + PATCH
#
# MINOR and PATCH must be 0–999 (no leading zeros) and the code must stay within Android's 1..2_100_000_000,
# so a higher tag always gets a higher code and installs over the previous APK. Anything else is rejected.
# The two apps have different packages, so their version codes are independent.
# Prints `name=…` and `code=…` lines (append to $GITHUB_OUTPUT in CI).
set -euo pipefail

tag="${1:?usage: version-from-tag.sh android-vMAJOR.MINOR.PATCH | android-web-vMAJOR.MINOR.PATCH}"
tag="${tag#refs/tags/}"
if [[ ! "$tag" =~ ^android-(web-)?v(0|[1-9][0-9]{0,3})\.(0|[1-9][0-9]{0,2})\.(0|[1-9][0-9]{0,2})$ ]]; then
  echo "error: tag '$tag' is not android-vMAJOR.MINOR.PATCH or android-web-vMAJOR.MINOR.PATCH (MINOR/PATCH 0-999, no leading zeros)" >&2
  exit 1
fi
major=${BASH_REMATCH[2]} minor=${BASH_REMATCH[3]} patch=${BASH_REMATCH[4]}
code=$((major * 1000000 + minor * 1000 + patch))
if ((code < 1 || code > 2100000000)); then
  echo "error: versionCode $code from '$tag' is outside 1..2100000000" >&2
  exit 1
fi
echo "name=$major.$minor.$patch"
echo "code=$code"
