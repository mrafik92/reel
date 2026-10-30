#!/usr/bin/env python3
"""Bootstrap Reel on CT 113: admin, libraries, Arr servers, Arr webhooks. Idempotent. Runs on the PVE host."""
import json, re, secrets, subprocess, urllib.request, urllib.error, os, sys

REEL = "http://10.10.10.207:8420/api/v1"
CREDS = "/root/.reel-admin"
ARRS = [  # (name, kind, url, config.xml in CT 105)
    ("Radarr", "radarr", "http://10.10.10.122:7878", "/mnt/media/config/radarr/config.xml"),
    ("Sonarr", "sonarr", "http://10.10.10.122:8989", "/mnt/media/config/sonarr/config.xml"),
    ("Sonarr Anime", "sonarr", "http://10.10.10.122:8990", "/mnt/media/config/sonarr-anime/config.xml"),
]
LIBS = [("Movies", "movies", ["/data/media/movies"]), ("TV Shows", "shows", ["/data/media/tv"]), ("Anime", "shows", ["/data/media/anime"])]


def call(method, url, body=None, headers=None):
    req = urllib.request.Request(url, method=method, data=None if body is None else json.dumps(body).encode(),
                                 headers={"Content-Type": "application/json", **(headers or {})})
    try:
        with urllib.request.urlopen(req, timeout=60) as r:
            raw = r.read()
            return r.status, (json.loads(raw) if raw else None)
    except urllib.error.HTTPError as e:
        raw = e.read()
        try:
            return e.code, json.loads(raw)
        except Exception:
            return e.code, raw.decode(errors="replace")


def sh(*cmd):
    return subprocess.run(cmd, check=True, capture_output=True, text=True).stdout


# ── admin account ────────────────────────────────────────────────────────────────────────────
dev = {"device_id": "bootstrap-pve", "device_name": "bootstrap", "client": "web"}
st, setup = call("GET", f"{REEL}/setup")
if setup.get("needs_setup"):
    pw = secrets.token_urlsafe(12)
    st, r = call("POST", f"{REEL}/setup", {"username": "rafik", "password": pw, **dev})
    assert st == 201, (st, r)
    old = os.umask(0o077)
    open(CREDS, "w").write(f"username=rafik\npassword={pw}\n")
    os.umask(old)
    print("admin created (credentials in", CREDS + ")")
creds = dict(l.split("=", 1) for l in open(CREDS).read().split())
st, r = call("POST", f"{REEL}/auth/login", {"username": creds["username"], "password": creds["password"], **dev})
assert st == 200, (st, r)
H = {"Authorization": f"Bearer {r['token']}"}

# ── libraries ────────────────────────────────────────────────────────────────────────────────
st, libs = call("GET", f"{REEL}/libraries", headers=H)
have = {l["name"] for l in libs}
for name, kind, paths in LIBS:
    if name not in have:
        st, r = call("POST", f"{REEL}/libraries", {"name": name, "kind": kind, "paths": paths}, H)
        print("library", name, st, r if st >= 300 else "")

# ── Arr servers + webhooks ───────────────────────────────────────────────────────────────────
env = dict(l.split("=", 1) for l in sh("pct", "exec", "113", "--", "cat", "/root/reel/.env").split() if "=" in l)
secret = env["REEL_WEBHOOK_SECRET"]
st, existing = call("GET", f"{REEL}/admin/arr-servers", headers=H)
by_name = {a["name"]: a for a in existing}
for name, kind, url, cfg in ARRS:
    key = re.search(r"<ApiKey>([^<]+)", sh("pct", "exec", "105", "--", "cat", cfg)).group(1)
    AH = {"X-Api-Key": key}
    _, roots = call("GET", f"{url}/api/v3/rootfolder", headers=AH)
    items = call("GET", f"{url}/api/v3/{'movie' if kind == 'radarr' else 'series'}", headers=AH)[1]
    counts = {}
    for it in items:
        counts[it["qualityProfileId"]] = counts.get(it["qualityProfileId"], 0) + 1
    profile = max(counts, key=counts.get) if counts else call("GET", f"{url}/api/v3/qualityprofile", headers=AH)[1][0]["id"]
    root = roots[0]["path"] if roots else None
    if name not in by_name:
        st, a = call("POST", f"{REEL}/admin/arr-servers", {"kind": kind, "name": name, "url": url, "api_key": key,
                                                          "default_profile": profile, "default_root": root, "is_4k": False}, H)
        assert st in (200, 201), (name, st, a)
        by_name[name] = a
    sid = by_name[name]["id"]
    print(f"arr {name}: id={sid} profile={profile} root={root}")

    # Webhook connection in the Arr pointing at Reel
    _, notes = call("GET", f"{url}/api/v3/notification", headers=AH)
    if any(n.get("name") == "Reel" for n in notes):
        continue
    _, schema = call("GET", f"{url}/api/v3/notification/schema", headers=AH)
    wh = next(s for s in schema if s["implementation"] == "Webhook")
    wh["name"] = "Reel"
    for f in wh["fields"]:
        if f["name"] == "url":
            f["value"] = f"{REEL}/webhooks/arr?server={sid}&token={secret}"
        elif f["name"] == "method":
            f["value"] = 1  # POST
    for k in list(wh):
        if k.startswith("on") and isinstance(wh[k], bool):
            wh[k] = k in ("onGrab", "onDownload", "onUpgrade", "onImportComplete", "onMovieFileDelete",
                          "onMovieFileDeleteForUpgrade", "onEpisodeFileDelete", "onEpisodeFileDeleteForUpgrade",
                          "onManualInteractionRequired")
    st, r = call("POST", f"{url}/api/v3/notification", wh, AH)
    print(f"  webhook in {name}:", st, "" if st < 300 else r)

st, libs = call("GET", f"{REEL}/libraries", headers=H)
print(json.dumps([{k: l[k] for k in ("id", "name", "item_count")} for l in libs]))
