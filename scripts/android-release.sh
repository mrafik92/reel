#!/usr/bin/env bash
# Local signed Android release (#232): GitHub Actions is unavailable, so release APKs for both Android apps
# are built on the Mac, then optionally sent to the phone with Taildrop and/or attached to a GitHub release.
#
#   scripts/android-release.sh web|compose [--version X.Y.Z] [--taildrop] [--taildrop-target NAME]
#                                          [--github-release] [--dry-run]
#
# Docs: clients/android-web/README.md and clients/android/README.md ("Release builds"), docs/OPERATIONS.md.
# Written for macOS's /bin/bash 3.2: no associative arrays, no ${x,,}, no empty "${arr[@]}" under set -u.
#
# Passwords never go on disk, into argv or into the log. They come from, in this order:
#   1. REEL_KEYSTORE_PASSWORD (and REEL_KEY_PASSWORD, defaulting to the keystore password: PKCS12 uses one)
#   2. the macOS Keychain: generic password, service $REEL_KEYCHAIN_SERVICE (reel-android-keystore),
#      account = the key alias (reel). Skipped when REEL_NO_KEYCHAIN=1.
#   3. a hidden prompt, when stdin is a terminal.
# Without any of them the script stops before building. They reach only keytool (-storepass:env) and Gradle,
# as environment variables of those two commands; nothing else the script runs inherits them.
set -euo pipefail

REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
VERSION_SCRIPT="$REPO_ROOT/clients/android/scripts/version-from-tag.sh"
DIST_DIR=${REEL_DIST_DIR:-"$REPO_ROOT/dist"}
GH_REPO=${REEL_GH_REPO:-mrafik92/reel}
KEYCHAIN_SERVICE=${REEL_KEYCHAIN_SERVICE:-reel-android-keystore}
TAILSCALE_BIN=${TAILSCALE_BIN:-/Applications/Tailscale.app/Contents/MacOS/Tailscale}
# SHA-256 of the release certificate (CN=Reel, O=Reel) that signed android-v0.1.0..0.3.0. Public: every APK
# carries it. An APK signed by any other key can't update the installed apps.
RELEASE_CERT_SHA256=${REEL_RELEASE_CERT_SHA256:-a48654758e7b9b85a5825e7f7a55bfebc8b2d5370691d7d056124ee4c9a58ecb}

# Exit codes
EXIT_ERROR=1
EXIT_USAGE=2
EXIT_TAILDROP_UNDELIVERED=3   # the APK was built (and released, if asked) but the phone didn't get it

usage() {
  cat <<'EOF'
Usage: scripts/android-release.sh web|compose [options]

Builds a signed, R8-shrunk release APK of Reel Web (web: clients/android-web) or the Compose app
(compose: clients/android). Runs lint and unit tests first, verifies the signature against the release
key, and writes dist/ReelWeb-X.Y.Z.apk or dist/Reel-X.Y.Z.apk with a .sha256 next to it.

Options:
  --version X.Y.Z        Version to build. Default: the next patch after the app's newest release tag
                         (android-web-v* for web, android-v* for compose; tags are fetched from origin).
  --taildrop             Send the APK to the phone with Taildrop afterwards.
  --taildrop-target NAME Taildrop device (default: $REEL_TAILDROP_TARGET or rafiiiki). Implies --taildrop.
  --github-release       Tag HEAD (android-web-vX.Y.Z / android-vX.Y.Z), push the tag and create a GitHub
                         release with the APK and its .sha256 (gh release create; works without Actions).
                         Needs a clean tree, HEAD on origin/main and a tag that doesn't exist yet.
  --dry-run              Build for real, but only print the Taildrop and GitHub commands.
  -h, --help             This help.

Environment: REEL_NO_FETCH=1 skips fetching tags (uses local tags), REEL_DIST_DIR (default dist/),
TAILSCALE_BIN (default /Applications/Tailscale.app/Contents/MacOS/Tailscale).

Signing (passwords are never written anywhere):
  REEL_KEYSTORE_FILE     keystore (default ~/.reel-android/reel-release.jks)
  REEL_KEY_ALIAS         key alias (default reel)
  REEL_KEYSTORE_PASSWORD keystore password; else the macOS Keychain item
                         (service reel-android-keystore, account = alias), else a hidden prompt
  REEL_KEY_PASSWORD      key password (default: the keystore password)
  A key whose certificate isn't the Reel release certificate gets a warning (its APK can't update the
  installed apps); --github-release refuses it. REEL_RELEASE_CERT_SHA256 overrides the expected digest.

Exit codes: 0 done, 1 error, 2 usage, 3 built but Taildrop couldn't deliver (phone offline).
EOF
}

die() {
  echo "error: $*" >&2
  exit "$EXIT_ERROR"
}

step() { printf '\n==> %s\n' "$*"; }

# Prints a command as one shell-quoted line (for dry runs and "run this later" hints).
quote_cmd() {
  local out="" a
  for a in "$@"; do out+="$(printf '%q' "$a") "; done
  printf '%s\n' "${out% }"
}

# --- app and version ------------------------------------------------------------------------------------

# Sets APP_DIR, APP_ID, TAG_PREFIX, APK_BASENAME, RELEASE_TITLE_PREFIX for `web` or `compose`.
app_config() {
  case "$1" in
    web)
      APP_DIR="$REPO_ROOT/clients/android-web"
      APP_ID=app.reel.android.web
      TAG_PREFIX=android-web-v
      APK_BASENAME=ReelWeb
      RELEASE_TITLE_PREFIX="Reel Web"
      ;;
    compose)
      APP_DIR="$REPO_ROOT/clients/android"
      APP_ID=app.reel.android
      TAG_PREFIX=android-v
      APK_BASENAME=Reel
      # Same title as the releases android-release.yml made (Obtainium filters on it).
      RELEASE_TITLE_PREFIX="Android"
      ;;
    *) return 1 ;;
  esac
}

# version_code PREFIX VERSION → prints the versionCode (fails on a version the scheme rejects).
version_code() {
  local out
  out=$("$VERSION_SCRIPT" "$1$2") || return 1
  printf '%s\n' "$out" | sed -n 's/^code=//p'
}

# Newest tag (by versionCode) among the tag names on stdin that match PREFIX exactly; prints nothing if none.
# `android-v*` never matches `android-web-v*` tags and vice versa, so the two apps' versions stay apart.
newest_tag() {
  local prefix=$1 tag code best="" best_code=0
  while IFS= read -r tag; do
    [[ "$tag" == "$prefix"* ]] || continue
    code=$(version_code "$prefix" "${tag#"$prefix"}" 2>/dev/null) || continue
    if ((code > best_code)); then best=$tag best_code=$code; fi
  done
  [[ -n "$best" ]] && printf '%s\n' "$best"
  return 0
}

# next_patch X.Y.Z → X.Y.(Z+1); "" → 0.1.0.
next_patch() {
  local v=$1
  if [[ -z "$v" ]]; then echo 0.1.0; return; fi
  local major=${v%%.*} rest=${v#*.}
  local minor=${rest%%.*} patch=${rest#*.}
  if ((patch >= 999)); then
    echo "error: $v has patch 999; pass --version with a higher minor" >&2
    return 1
  fi
  echo "$major.$minor.$((patch + 1))"
}

fetch_release_tags() {
  # Only this app's tags. Read-only for the remote. REEL_NO_FETCH=1 uses the local tags only (offline).
  [[ "${REEL_NO_FETCH:-}" == 1 ]] && return 0
  git -C "$REPO_ROOT" fetch --quiet --no-tags origin "+refs/tags/${TAG_PREFIX}*:refs/tags/${TAG_PREFIX}*"
}

# --- Taildrop -------------------------------------------------------------------------------------------

# True when `tailscale file cp` output says the target is offline. Tailscale prints
# "... is reportedly offline" and then fails with "502 Bad Gateway".
taildrop_offline() {
  printf '%s' "$1" | grep -qiE 'reportedly offline|is offline|502 Bad Gateway'
}

# taildrop_send APK TARGET DRY_RUN → 0 sent (or dry run), 3 offline, 1 other failure.
taildrop_send() {
  local apk=$1 target=$2 dry=$3 bin=$TAILSCALE_BIN out rc
  if [[ ! -x "$bin" ]]; then
    if command -v tailscale >/dev/null 2>&1; then bin=$(command -v tailscale); else bin=""; fi
  fi
  if [[ "$dry" == 1 ]]; then
    echo "dry run: would run: $(quote_cmd "${bin:-$TAILSCALE_BIN}" file cp "$apk" "$target:")"
    return 0
  fi
  [[ -n "$bin" ]] || { echo "error: Tailscale CLI not found at $TAILSCALE_BIN (set TAILSCALE_BIN)" >&2; return "$EXIT_ERROR"; }
  rc=0
  out=$("$bin" file cp "$apk" "$target:" 2>&1) || rc=$?
  if ((rc == 0)); then
    echo "Sent $(basename "$apk") to $target with Taildrop. On the phone: open the Tailscale notification (or Files → Downloads) and tap Install."
    return 0
  fi
  if taildrop_offline "$out"; then
    echo "Taildrop: the device '$target' is offline, so nothing was sent." >&2
    echo "  Open Tailscale on the phone (or wake it), then run:" >&2
    echo "  $(quote_cmd "$bin" file cp "$apk" "$target:")" >&2
    return "$EXIT_TAILDROP_UNDELIVERED"
  fi
  echo "error: Taildrop to '$target' failed (exit $rc):" >&2
  printf '%s\n' "$out" >&2
  return "$EXIT_ERROR"
}

# --- GitHub release -------------------------------------------------------------------------------------

# Read-only checks before anything is built. In a dry run, problems are printed as warnings.
github_release_preflight() {
  local tag=$1 dry=$2 problems="" p
  command -v gh >/dev/null 2>&1 || problems+="gh (GitHub CLI) is not installed"$'\n'
  if [[ -n "$(git -C "$REPO_ROOT" status --porcelain --untracked-files=no)" ]]; then
    problems+="the working tree has uncommitted changes; a release must match a pushed commit"$'\n'
  fi
  if ! git -C "$REPO_ROOT" fetch --quiet --no-tags origin main 2>/dev/null; then
    problems+="can't fetch origin/main"$'\n'
  elif ! git -C "$REPO_ROOT" merge-base --is-ancestor HEAD FETCH_HEAD; then
    problems+="HEAD ($(git -C "$REPO_ROOT" rev-parse --short HEAD)) is not on origin/main; release from merged code"$'\n'
  fi
  if git -C "$REPO_ROOT" rev-parse -q --verify "refs/tags/$tag" >/dev/null; then
    problems+="tag $tag already exists locally"$'\n'
  fi
  if git -C "$REPO_ROOT" ls-remote --exit-code --tags origin "refs/tags/$tag" >/dev/null 2>&1; then
    problems+="tag $tag already exists on origin"$'\n'
  fi
  [[ -z "$problems" ]] && return 0
  while IFS= read -r p; do
    [[ -n "$p" ]] || continue
    if [[ "$dry" == 1 ]]; then echo "dry run: --github-release would refuse: $p" >&2; else echo "error: $p" >&2; fi
  done <<<"$problems"
  [[ "$dry" == 1 ]] && return 0
  return "$EXIT_ERROR"
}

# github_release TAG TITLE NOTES DRY_RUN FILE...
github_release() {
  local tag=$1 title=$2 notes=$3 dry=$4
  shift 4
  if [[ "$dry" == 1 ]]; then
    echo "dry run: would run: $(quote_cmd git tag -a "$tag" -m "$title" HEAD)"
    echo "dry run: would run: $(quote_cmd git push origin "refs/tags/$tag")"
    echo "dry run: would run: $(quote_cmd gh release create "$tag" "$@" -R "$GH_REPO" --title "$title" --notes "$notes" --verify-tag)"
    return 0
  fi
  git -C "$REPO_ROOT" tag -a "$tag" -m "$title" HEAD
  git -C "$REPO_ROOT" push --quiet origin "refs/tags/$tag"
  if ! gh release create "$tag" "$@" -R "$GH_REPO" --title "$title" --notes "$notes" --verify-tag; then
    echo "error: the tag $tag is pushed but the release wasn't created. Retry with:" >&2
    echo "  $(quote_cmd gh release create "$tag" "$@" -R "$GH_REPO" --title "$title" --notes "$notes" --verify-tag)" >&2
    return "$EXIT_ERROR"
  fi
}

# --- signing --------------------------------------------------------------------------------------------

# Sets STORE_PW and KEY_PW (plain, unexported shell variables) and PW_SOURCE. Never prints a password.
resolve_passwords() {
  local alias=$1 ks=$2
  STORE_PW="" KEY_PW="" PW_SOURCE=""
  # Keep the passwords out of the environment of everything else this script runs (git, gh, tailscale).
  export -n REEL_KEYSTORE_PASSWORD REEL_KEY_PASSWORD 2>/dev/null || true
  if [[ -n "${REEL_KEYSTORE_PASSWORD:-}" ]]; then
    STORE_PW=$REEL_KEYSTORE_PASSWORD PW_SOURCE="REEL_KEYSTORE_PASSWORD"
  elif [[ "${REEL_NO_KEYCHAIN:-}" != 1 ]] && command -v security >/dev/null 2>&1 &&
    STORE_PW=$(security find-generic-password -s "$KEYCHAIN_SERVICE" -a "$alias" -w 2>/dev/null) &&
    [[ -n "$STORE_PW" ]]; then
    PW_SOURCE="macOS Keychain ($KEYCHAIN_SERVICE)"
  elif [[ -t 0 ]]; then
    STORE_PW=""
    read -rs -p "Password for $ks (hidden): " STORE_PW </dev/tty
    echo >&2
    PW_SOURCE="prompt"
  fi
  if [[ -z "$STORE_PW" ]]; then
    cat >&2 <<EOF
error: no keystore password, so nothing was built. Give it in one of these ways:
  - in this shell, hidden:  read -rs REEL_KEYSTORE_PASSWORD && export REEL_KEYSTORE_PASSWORD
  - once, in the macOS Keychain:  security add-generic-password -s $KEYCHAIN_SERVICE -a $alias -w
    (prompts for the password; it's in the owner's password manager)
  - run the script in a terminal and type it at the hidden prompt
EOF
    return "$EXIT_ERROR"
  fi
  KEY_PW=${REEL_KEY_PASSWORD:-$STORE_PW}
  unset REEL_KEYSTORE_PASSWORD REEL_KEY_PASSWORD
}

# Checks the keystore opens with STORE_PW and has ALIAS. Prints the certificate's SHA-256 (lowercase hex).
keystore_cert_sha256() {
  local ks=$1 alias=$2 out rc=0
  out=$(REEL_KS_PW="$STORE_PW" keytool -list -v -keystore "$ks" -alias "$alias" -storepass:env REEL_KS_PW 2>&1) || rc=$?
  if ((rc != 0)); then
    case "$out" in
      *"password was incorrect"* | *"tampered"* | *"BadPaddingException"*)
        echo "error: the keystore password is wrong for $ks" >&2 ;;
      *"does not exist"*)
        echo "error: $ks has no key with alias '$alias' (set REEL_KEY_ALIAS)" >&2 ;;
      *)
        echo "error: keytool can't read $ks:" >&2
        printf '%s\n' "$out" | grep -i 'error' | head -3 >&2 ;;
    esac
    return "$EXIT_ERROR"
  fi
  printf '%s\n' "$out" | sed -n 's/^[[:space:]]*SHA256:[[:space:]]*//p' | head -1 | tr -d ':' | tr 'A-F' 'a-f'
}

# check_release_cert CERT GH_RELEASE DRY_RUN: a different key is only a warning for a local build, but a
# GitHub release must use the release key (Obtainium would offer an update that can't install).
check_release_cert() {
  local cert=$1 gh=$2 dry=$3
  [[ "$cert" == "$RELEASE_CERT_SHA256" ]] && return 0
  local msg="this key's certificate ($cert) is not the Reel release certificate ($RELEASE_CERT_SHA256); its APK won't install over release builds"
  if [[ "$gh" == 1 && "$dry" != 1 ]]; then
    echo "error: $msg, so --github-release refuses it" >&2
    return "$EXIT_ERROR"
  fi
  echo "warning: $msg" >&2
}

# --- toolchain ------------------------------------------------------------------------------------------

java_major() { "$1/bin/java" -version 2>&1 | sed -n '1s/.*version "\([0-9]*\).*/\1/p'; }

# AGP 8.7 needs JDK 17–21. Uses JAVA_HOME if it fits, else Homebrew's openjdk@21/@17.
find_java_home() {
  local c m
  for c in "${JAVA_HOME:-}" /opt/homebrew/opt/openjdk@21/libexec/openjdk.jdk/Contents/Home \
    /opt/homebrew/opt/openjdk@17/libexec/openjdk.jdk/Contents/Home \
    "$(/usr/libexec/java_home -v 21 2>/dev/null || true)" "$(/usr/libexec/java_home -v 17 2>/dev/null || true)"; do
    [[ -n "$c" && -x "$c/bin/java" ]] || continue
    m=$(java_major "$c")
    if [[ -n "$m" ]] && ((m >= 17 && m <= 21)); then
      echo "$c"
      return 0
    fi
  done
  echo "error: no JDK 17–21 found (set JAVA_HOME; see clients/android/README.md)" >&2
  return 1
}

find_sdk() {
  local c
  for c in "${ANDROID_HOME:-}" "${ANDROID_SDK_ROOT:-}" \
    "$(sed -n 's/^sdk\.dir=//p' "$APP_DIR/local.properties" 2>/dev/null)" \
    /opt/homebrew/share/android-commandlinetools "$HOME/Library/Android/sdk"; do
    [[ -n "$c" && -d "$c/build-tools" ]] && { echo "$c"; return 0; }
  done
  echo "error: no Android SDK found (set ANDROID_HOME; see clients/android/README.md)" >&2
  return 1
}

# The build-tools AGP uses (35.x), else the newest installed.
# shellcheck disable=SC2012 # SDK directory names are plain version numbers
find_build_tools() {
  local bt
  bt=$(ls -d "$1"/build-tools/35.* 2>/dev/null | sort -V | tail -1)
  [[ -n "$bt" ]] || bt=$(ls -d "$1"/build-tools/* 2>/dev/null | sort -V | tail -1)
  [[ -n "$bt" && -x "$bt/apksigner" ]] || { echo "error: no build-tools with apksigner under $1" >&2; return 1; }
  echo "$bt"
}

# --- main -----------------------------------------------------------------------------------------------

main() {
  local app="" version="" taildrop=0 target=${REEL_TAILDROP_TARGET:-rafiiiki} gh_release=0 dry=0
  while (($#)); do
    case "$1" in
      web | compose) [[ -z "$app" ]] || { usage >&2; exit "$EXIT_USAGE"; }; app=$1 ;;
      --version) [[ $# -ge 2 ]] || { usage >&2; exit "$EXIT_USAGE"; }; version=${2#v}; shift ;;
      --version=*) version=${1#--version=}; version=${version#v} ;;
      --taildrop) taildrop=1 ;;
      --taildrop-target) [[ $# -ge 2 ]] || { usage >&2; exit "$EXIT_USAGE"; }; taildrop=1 target=$2; shift ;;
      --taildrop-target=*) taildrop=1 target=${1#--taildrop-target=} ;;
      --github-release) gh_release=1 ;;
      --dry-run) dry=1 ;;
      -h | --help) usage; exit 0 ;;
      *) echo "error: unknown argument: $1" >&2; usage >&2; exit "$EXIT_USAGE" ;;
    esac
    shift
  done
  [[ -n "$app" ]] || { usage >&2; exit "$EXIT_USAGE"; }
  app_config "$app"

  # Version: explicit, or the next patch after the newest release tag of this app.
  local newest=""
  if fetch_release_tags; then
    newest=$(git -C "$REPO_ROOT" tag -l "${TAG_PREFIX}*" | newest_tag "$TAG_PREFIX")
  elif [[ -z "$version" ]]; then
    die "can't fetch ${TAG_PREFIX}* tags from origin to pick the next version; pass --version X.Y.Z"
  else
    echo "warning: can't fetch ${TAG_PREFIX}* tags from origin; using --version $version as given" >&2
  fi
  if [[ -z "$version" ]]; then
    version=$(next_patch "${newest#"$TAG_PREFIX"}") || exit "$EXIT_ERROR"
  fi
  local code
  code=$(version_code "$TAG_PREFIX" "$version") || exit "$EXIT_USAGE"
  local tag="$TAG_PREFIX$version"
  if [[ -n "$newest" ]]; then
    local newest_code
    newest_code=$(version_code "$TAG_PREFIX" "${newest#"$TAG_PREFIX"}")
    if ((code <= newest_code)); then
      echo "warning: $version (code $code) is not newer than the release $newest (code $newest_code);" \
        "Android won't install it over that release" >&2
    fi
  fi
  step "$RELEASE_TITLE_PREFIX $version (versionCode $code, $APP_ID)${newest:+, newest release: $newest}"

  if ((gh_release)); then github_release_preflight "$tag" "$dry" || exit "$EXIT_ERROR"; fi

  # Signing: the release key, so the APK installs over earlier release builds.
  local ks=${REEL_KEYSTORE_FILE:-"$HOME/.reel-android/reel-release.jks"} alias=${REEL_KEY_ALIAS:-reel}
  [[ -f "$ks" ]] || die "keystore not found: $ks (set REEL_KEYSTORE_FILE)"
  resolve_passwords "$alias" "$ks" || exit "$EXIT_ERROR"
  local java_home sdk bt
  java_home=$(find_java_home) || exit "$EXIT_ERROR"
  sdk=$(find_sdk) || exit "$EXIT_ERROR"
  bt=$(find_build_tools "$sdk") || exit "$EXIT_ERROR"
  export JAVA_HOME="$java_home" ANDROID_HOME="$sdk"
  export PATH="$JAVA_HOME/bin:$PATH"
  local cert
  cert=$(keystore_cert_sha256 "$ks" "$alias") || exit "$EXIT_ERROR"
  [[ -n "$cert" ]] || die "couldn't read the certificate digest of '$alias' in $ks"
  check_release_cert "$cert" "$gh_release" "$dry" || exit "$EXIT_ERROR"
  echo "Signing with $ks (alias $alias, password from $PW_SOURCE), certificate SHA-256 $cert"

  # Build: lint and unit tests first, then the signed release. --no-daemon so no Gradle process keeps the
  # passwords in its environment after the build; --no-configuration-cache so Gradle never serializes them.
  step "lint, unit tests, signed release build (clients/${APP_DIR##*/clients/})"
  rm -rf "$APP_DIR/app/build/outputs/apk/release"
  (
    cd "$APP_DIR"
    unset REEL_URL # a release has no baked-in server address
    REEL_VERSION_NAME="$version" REEL_VERSION_CODE="$code" REEL_KEYSTORE_FILE="$ks" REEL_KEY_ALIAS="$alias" \
      REEL_KEYSTORE_PASSWORD="$STORE_PW" REEL_KEY_PASSWORD="$KEY_PW" \
      ./gradlew --no-daemon --no-configuration-cache lint testDebugUnitTest assembleRelease
  ) || die "the Gradle build failed (see above); nothing was written to dist/"
  STORE_PW="" KEY_PW=""

  # Verify: signed with the release key, right package and version.
  step "verify"
  local apk="$APP_DIR/app/build/outputs/apk/release/app-release.apk"
  if [[ ! -f "$apk" ]]; then
    ls "$APP_DIR"/app/build/outputs/apk/release/ 2>/dev/null >&2 || true
    die "no signed APK (app-release.apk) was produced"
  fi
  local certs signed
  certs=$("$bt/apksigner" verify --print-certs "$apk" 2>&1) || { printf '%s\n' "$certs" >&2; die "apksigner verify failed"; }
  printf '%s\n' "$certs" | grep -E '^Signer #1 certificate (DN|SHA-256 digest)'
  signed=$(printf '%s\n' "$certs" | sed -n 's/^Signer #1 certificate SHA-256 digest: //p' | tr 'A-F' 'a-f')
  [[ "$signed" == "$cert" ]] || die "the APK is signed with $signed, not the release key $cert"
  local badging
  badging=$("$bt/aapt2" dump badging "$apk" 2>&1 | sed -n 1p)
  echo "$badging"
  case "$badging" in *"name='$APP_ID' "*) ;; *) die "package is not $APP_ID" ;; esac
  case "$badging" in *"versionCode='$code'"*) ;; *) die "versionCode is not $code" ;; esac
  case "$badging" in *"versionName='$version'"*) ;; *) die "versionName is not $version" ;; esac

  mkdir -p "$DIST_DIR"
  local name="$APK_BASENAME-$version.apk"
  local out="$DIST_DIR/$name"
  cp "$apk" "$out"
  (cd "$DIST_DIR" && shasum -a 256 "$name" >"$name.sha256")
  local sha
  sha=$(cut -d' ' -f1 <"$out.sha256")

  local rev dirty=""
  rev=$(git -C "$REPO_ROOT" rev-parse --short HEAD)
  [[ -z "$(git -C "$REPO_ROOT" status --porcelain --untracked-files=no)" ]] || dirty=" (uncommitted changes)"
  step "built $out"
  echo "  version   $version (versionCode $code), package $APP_ID"
  echo "  commit    $rev$dirty"
  echo "  SHA-256   $sha"
  echo "  signer    $cert"

  local status=0
  if ((taildrop)); then
    step "Taildrop → $target"
    taildrop_send "$out" "$target" "$dry" || status=$?
    ((status == 0 || status == EXIT_TAILDROP_UNDELIVERED)) || exit "$status"
  fi

  if ((gh_release)); then
    step "GitHub release $tag"
    local notes
    if [[ "$app" == web ]]; then
      notes="Reel Web $version (versionCode $code), package $APP_ID. Installs next to the Compose app."
    else
      notes="Reel for Android $version (versionCode $code), package $APP_ID. One APK for phones and Android TV."
    fi
    notes+="
Built locally (scripts/android-release.sh) from $rev and signed with the Reel release key, certificate SHA-256 $cert.
APK SHA-256: $sha ($name.sha256)."
    github_release "$tag" "$RELEASE_TITLE_PREFIX $version" "$notes" "$dry" "$out" "$out.sha256" || exit "$EXIT_ERROR"
  fi
  return "$status"
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  main "$@"
fi
