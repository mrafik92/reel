#!/usr/bin/env python3
"""Add the Xtream source from /root/.reel-xtream to Reel, refresh, and smoke-test a channel.
Never prints credentials. Runs on the PVE host."""
import json, time, urllib.request, urllib.error

REEL = "http://10.10.10.207:8420/api/v1"
creds = dict(l.split("=", 1) for l in open("/root/.reel-admin").read().split())
x = dict(l.rstrip("\n").split("=", 1) for l in open("/root/.reel-xtream") if "=" in l)


def call(method, path, body=None, token=None, raw=False, timeout=120):
    url = path if path.startswith("http") else REEL + path
    req = urllib.request.Request(url, method=method, data=None if body is None else json.dumps(body).encode(),
                                 headers={"Content-Type": "application/json", **({"Authorization": f"Bearer {token}"} if token else {})})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            data = r.read()
            return r.status, (data if raw else (json.loads(data) if data else None))
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode(errors="replace")[:300]


st, r = call("POST", "/auth/login", {"username": creds["username"], "password": creds["password"], "device_id": "bootstrap-pve"})
T = r["token"]
body = {"name": "Xtream", "kind": "xtream", "url": x["url"].rstrip("/"), "username": x["username"], "password": x["password"]}

st, test = call("POST", "/admin/iptv/sources/test", body, T)
safe = {k: v for k, v in (test.items() if isinstance(test, dict) else []) if k not in ("password", "username", "url")}
print("test:", st, json.dumps(safe)[:600] if safe else test)
if st >= 300:
    raise SystemExit(1)
maxc = None
for k in ("max_connections", "account_max_connections"):
    if isinstance(test, dict) and test.get(k):
        maxc = int(test[k])
info = test.get("account") if isinstance(test, dict) else None
if not maxc and isinstance(info, dict) and info.get("max_connections"):
    maxc = int(info["max_connections"])
if maxc:
    body["max_connections"] = maxc

st, sources = call("GET", "/admin/iptv/sources", token=T)
existing = next((s for s in sources if s.get("name") == "Xtream"), None) if isinstance(sources, list) else None
if existing:
    sid = existing["id"]
    st, r = call("PATCH", f"/admin/iptv/sources/{sid}", body, T)
else:
    st, r = call("POST", "/admin/iptv/sources", body, T)
    sid = r["id"] if isinstance(r, dict) else None
print("source:", st, "id", sid, "max_connections", maxc)

st, r = call("POST", f"/admin/iptv/sources/{sid}/refresh", {}, T)
print("refresh:", st)
for _ in range(60):
    time.sleep(5)
    st, s = call("GET", f"/admin/iptv/sources/{sid}", token=T)
    if isinstance(s, dict) and (s.get("last_refresh") or s.get("last_refreshed_at")) and not s.get("refreshing"):
        break
print("source state:", json.dumps({k: v for k, v in s.items() if k not in ("password", "username", "url")})[:500] if isinstance(s, dict) else s)

st, groups = call("GET", "/iptv/groups", token=T)
st, chans = call("GET", "/iptv/channels", token=T)
print("groups:", len(groups) if isinstance(groups, list) else groups, "channels:", len(chans) if isinstance(chans, list) else chans)
st, now = call("GET", "/iptv/now", token=T)
with_epg = sum(1 for n in now if n.get("now")) if isinstance(now, list) else now
print("channels with current EPG programme:", with_epg)

# Smoke-test: tune a few channels (passthrough) and measure time to first TS bytes through Reel.
tested = 0
for c in (chans if isinstance(chans, list) else [])[:40]:
    if tested >= 3:
        break
    t0 = time.time()
    st, tune = call("POST", f"/iptv/channels/{c['id']}/tune", {"mode": "passthrough"}, T)
    if st >= 300:
        print(" tune", c["name"], st, tune)
        continue
    try:
        req = urllib.request.Request("http://10.10.10.207:8420" + tune["url"])
        with urllib.request.urlopen(req, timeout=20) as r:
            first = r.read(188 * 64)
        print(f" {c['name']}: first {len(first)} bytes in {time.time() - t0:.2f}s, sync byte ok={first[:1] == b'G'}")
        tested += 1
    except Exception as e:
        print(" ", c["name"], "stream error:", type(e).__name__, str(e)[:120])
    call("DELETE", f"/iptv/sessions/{tune['session_id']}", token=T)
