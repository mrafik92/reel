"""Mock Reel API for the Android player (#190). Serves test media; logs every call."""
import json, os, re, sys, threading, time
from http.server import ThreadingHTTPServer, BaseHTTPRequestHandler

MEDIA = sys.argv[1]
PORT = int(sys.argv[2]) if len(sys.argv) > 2 else 8421
LOG = open(os.devnull, "w")
REAP = {"on": False}
SID = {"n": 0}

def epi(id, no, file, title, progress=None):
    return {"id": id, "kind": "episode", "title": title, "played": False, "added_at": 1, "parent_id": 201,
            "season_no": 1, "episode_no": no, "progress": progress,
            "files": [{"id": file, "container": "ts", "duration_s": 90.0, "size": 1, "url": "", "audio": [], "subtitles": []}]}

ITEMS = {
    101: {"id": 101, "kind": "movie", "title": "H.264 MP4 test", "played": False, "added_at": 1, "progress": 0.5,
          "files": [{"id": 1001, "container": "mp4", "duration_s": 90.0, "size": 1, "url": "",
                     "audio": [{"index": 1, "codec": "aac", "channels": 2, "default": True, "language": "eng"}], "subtitles": []}]},
    102: {"id": 102, "kind": "movie", "title": "HEVC 10-bit MKV, AC3 + subs", "played": False, "added_at": 1,
          "files": [{"id": 1002, "container": "mkv", "duration_s": 90.0, "size": 1, "url": "",
                     "audio": [{"index": 1, "codec": "ac3", "channels": 6, "default": True, "language": "eng"},
                               {"index": 2, "codec": "aac", "channels": 2, "default": False, "language": "fre"}],
                     "subtitles": [{"index": 3, "codec": "subrip", "forced": False, "external": False, "language": "eng"}]}]},
    104: {"id": 104, "kind": "movie", "title": "H.264 MKV, AC3 + AAC + SRT", "played": False, "added_at": 1,
          "files": [{"id": 1004, "container": "mkv", "duration_s": 90.0, "size": 1, "url": "",
                     "audio": [{"index": 1, "codec": "ac3", "channels": 6, "default": True, "language": "eng"},
                               {"index": 2, "codec": "aac", "channels": 2, "default": False, "language": "fre"}],
                     "subtitles": [{"index": 3, "codec": "subrip", "forced": False, "external": False, "language": "eng"}]}]},
    200: {"id": 200, "kind": "show", "title": "Mock Show", "played": False, "added_at": 1, "files": []},
    201: {"id": 201, "kind": "season", "title": "Season 1", "parent_id": 200, "season_no": 1, "played": False, "added_at": 1, "files": []},
    202: epi(202, 1, 1003, "The HLS one"),
    203: epi(203, 2, 1001, "The direct one"),
}
FILES = {1001: "a.mp4", 1002: "b.mkv", 1004: "c.mkv"}

def summary(i):
    return {k: v for k, v in i.items() if k not in ("files",)}

def decide(file_id, body):
    SID["n"] += 1
    sid = f"s{SID['n']}"
    qopts = [{"id": "original", "label": "Original", "bitrate_kbps": 2600, "available": True},
             {"id": "720p", "label": "720p", "bitrate_kbps": 2000, "available": True},
             {"id": "480p", "label": "480p", "bitrate_kbps": 1000, "available": True}]
    base = {"session_id": sid, "selected": {"reason": "mock: your preferences"}, "bitrate_kbps": 2600,
            "quality_options": qopts, "variants": [], "audio": [], "subtitles": []}
    if file_id == 1003 or body.get("quality") in ("720p", "480p"):
        return {**base, "mode": "transcode", "url": f"/api/v1/stream/{sid}/master.m3u8?sig=mock",
                "quality": body.get("quality") if body.get("quality") in ("720p", "480p") else "720p",
                "decision": {"mode": "transcode", "video": {"type": "encode"}, "audio": {"type": "encode"}, "subtitle": {"type": "none"},
                             "reasons": ["mock: video codec not supported (forced HLS)"]},
                "audio": [{"index": 1, "codec": "aac", "channels": 2, "default": True, "language": "eng"}]}
    f = ITEMS[{1001: 101, 1002: 102, 1004: 104}[file_id]]["files"][0]
    subs = [{"index": s["index"], "stream_id": 30 + s["index"], "forced": False, "external": False,
             "language": s.get("language"), "url": f"/api/v1/subs/{sid}/{s['index']}.vtt?sig=mock"} for s in f["subtitles"]]
    sub_sel = body.get("subtitle_index")
    return {**base, "mode": "direct_play", "url": f"/api/v1/direct/{file_id}?sig=mock", "quality": "original",
            "decision": {"mode": "direct_play", "video": {"type": "copy"}, "audio": {"type": "copy"},
                         "subtitle": {"type": "native" if subs else "none"}, "reasons": ["mock: everything plays as is"]},
            "audio": f["audio"], "subtitles": subs,
            "selected": {"reason": "mock: your preferences", "audio_index": 1,
                         "subtitle_index": (3 if subs else None) if sub_sel is None else (None if sub_sel < 0 else sub_sel)}}


CHANNELS = [
    {"id": i, "name": name, "number": num, "group": group, "favorite": i in (1, 2), "has_epg": True}
    for i, num, name, group in [
        (1, 101, "Arena One", "Sport"), (2, 102, "News 24", "News"),
        (3, 103, "Cine HD", "Movies"), (4, 104, "Documentary", "Movies"),
        (5, 105, "World Sport", "Sport"),
    ]
]
LIVE = {"id": None, "next": 0, "vod_busy": True, "takeover": True}
LOCK = threading.Lock()

def guide(i):
    start = (int(time.time()) // 3600) * 3600
    name = CHANNELS[i - 1]["name"]
    return [
        {"start": start + hour * 3600, "stop": start + (hour + 1) * 3600, "title": f"{name}: {title}"}
        for hour, title in enumerate(("Now Playing", "Coming Up", "Later Tonight"))
    ]

class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def log_message(self, *a): pass

    def log(self, extra=""):
        pass

    def json(self, obj, code=200):
        b = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)

    def empty(self, code=204):
        self.send_response(code)
        self.send_header("Content-Length", "0")
        self.end_headers()

    def body(self):
        n = int(self.headers.get("Content-Length") or 0)
        return json.loads(self.rfile.read(n) or b"{}") if n else {}

    def file(self, path, ctype):
        size = os.path.getsize(path)
        rng = self.headers.get("Range")
        start, end = 0, size - 1
        if rng:
            m = re.match(r"bytes=(\d*)-(\d*)", rng)
            if m.group(1): start = int(m.group(1))
            if m.group(2): end = min(int(m.group(2)), size - 1)
            self.send_response(206)
            self.send_header("Content-Range", f"bytes {start}-{end}/{size}")
        else:
            self.send_response(200)
        self.send_header("Content-Type", ctype)
        self.send_header("Accept-Ranges", "bytes")
        self.send_header("Content-Length", str(end - start + 1))
        self.end_headers()
        with open(path, "rb") as f:
            f.seek(start)
            left = end - start + 1
            try:
                while left > 0:
                    chunk = f.read(min(65536, left))
                    if not chunk: break
                    self.wfile.write(chunk)
                    left -= len(chunk)
            except (BrokenPipeError, ConnectionResetError):
                pass

    def do_GET(self):
        p = self.path.split("?")[0]
        if p.startswith("/api/v1/direct/") or p.startswith("/api/v1/stream/") or p.startswith("/api/v1/subs/"):
            self.log()
        else:
            self.log()
        if p == "/mock/reap":
            REAP["on"] = True; return self.json({"reap": True})

        if p == "/api/v1/iptv/channels": return self.json(CHANNELS)
        if p == "/api/v1/iptv/groups":
            return self.json([{"name": group, "count": sum(c["group"] == group for c in CHANNELS)}
                              for group in ("Movies", "News", "Sport")])
        if p in ("/api/v1/iptv/now", "/api/v1/iptv/epg"):
            from urllib.parse import parse_qs, urlsplit
            param = parse_qs(urlsplit(self.path).query).get("channels", ["1,2,3,4,5"])[0]
            ids = [int(i) for i in param.split(",")]
            if p.endswith("/now"):
                return self.json([{"channel_id": i, "now": guide(i)[0], "next": guide(i)[1]} for i in ids])
            return self.json({"from": guide(ids[0])[0]["start"], "to": guide(ids[0])[-1]["stop"],
                              "channels": [{"channel_id": i, "programs": guide(i)} for i in ids]})
        if re.fullmatch(r"/api/v1/iptv/live/[^/]+/stream.ts", p):
            if self.headers.get("Authorization"): return self.json({"error": "Unexpected bearer"}, 400)
            if p.split("/")[-2] != LIVE["id"]: return self.json({"error": "Session gone"}, 404)
            return self.file(os.path.join(MEDIA, "live.ts"), "video/mp2t")
        if p == "/api/v1/health": return self.json({"status": "ok", "version": "mock"})
        if p == "/api/v1/setup": return self.json({"needs_setup": False})
        if p == "/api/v1/me": return self.json(ME)
        if p == "/api/v1/home":
            return self.json({"continue_watching": [summary(ITEMS[101]), summary(ITEMS[202])],
                              "recently_added": [summary(ITEMS[102]), summary(ITEMS[104]), summary(ITEMS[203])], "rows": []})
        qs = self.path.split("?", 1)[1] if "?" in self.path else ""
        if p == "/api/v1/libraries": return self.json([{"id": 1, "name": "Movies", "kind": "movies", "item_count": 3}])
        if p == "/api/v1/libraries/1/items": return self.json({"items": [summary(ITEMS[i]) for i in (101, 102, 104)], "next_cursor": None})
        if p == "/api/v1/genres": return self.json([])
        if p == "/api/v1/vod/lanes":
            if "type=series" in qs:
                return self.json([{"key": "new-s", "title": "New series", "kind": "new", "items": SERIES}])
            return self.json([{"key": "new", "title": "New on IPTV", "kind": "new", "subtitle": "Added this week", "items": MOVIES[:8],
                               "see_all": {"type": "movie", "query": "sort=added"}},
                              {"key": "top", "title": "Top rated", "kind": "top_unwatched", "items": MOVIES[8:16]}])
        if p == "/api/v1/vod/movies": return self.json(vod_page(MOVIES, qs))
        if p == "/api/v1/vod/series": return self.json(vod_page(SERIES, qs))
        m = re.match(r"/api/v1/vod/movies/(\d+)$", p)
        if m:
            c = next((x for x in MOVIES if x["id"] == int(m.group(1))), None)
            if not c: return self.json({"error": "Not found"}, 404)
            return self.json({**c, "overview": "A mock IPTV movie served by the Mac.", "runtime_s": 90, "cast": [{"name": "Ann Example"}], "director": "Mock Director", "genres": ["Drama"]})
        m = re.match(r"/api/v1/vod/series/(\d+)$", p)
        if m:
            return self.json({**SERIES[0], "overview": "Two seasons of test pattern.", "cast": [],
                "seasons": [{"season_no": 1, "episodes": [{"id": 6011, "episode_no": 1, "title": "Episode 1", "runtime_s": 90, "played": True},
                                                          {"id": 6012, "episode_no": 2, "title": "The second one", "runtime_s": 90, "progress": 0.5}]},
                            {"season_no": 2, "episodes": [{"id": 6021, "episode_no": 1, "title": "New season", "runtime_s": 90}]}]})
        m = re.match(r"/api/v1/vod/direct/[^/]+$", p)
        if m: return self.file(os.path.join(MEDIA, "a.mp4"), "video/mp4")
        m = re.match(r"/api/v1/items/(\d+)/children$", p)
        if m:
            kids = [summary(i) for i in ITEMS.values() if i.get("parent_id") == int(m.group(1))]
            return self.json(kids)
        m = re.match(r"/api/v1/items/(\d+)$", p)
        if m:
            i = ITEMS.get(int(m.group(1)))
            return self.json({"genres": [], "cast": [], **i}) if i else self.json({"error": "Not found"}, 404)
        m = re.match(r"/api/v1/direct/(\d+)$", p)
        if m:
            name = FILES[int(m.group(1))]
            return self.file(os.path.join(MEDIA, name), "video/mp4" if name.endswith("mp4") else "video/x-matroska")
        m = re.match(r"/api/v1/stream/[^/]+/(.+)$", p)
        if m:
            name = m.group(1)
            return self.file(os.path.join(MEDIA, "hls", name), "application/vnd.apple.mpegurl" if name.endswith("m3u8") else "video/mp2t")
        if p.startswith("/api/v1/subs/"):
            return self.file(os.path.join(MEDIA, "side.vtt"), "text/vtt")
        self.json({"error": "Not found"}, 404)

    def do_POST(self):
        p = self.path.split("?")[0]
        b = self.body()
        pass

        if m := re.fullmatch(r"/api/v1/iptv/channels/(\d+)/favorite", p):
            CHANNELS[int(m[1]) - 1]["favorite"] = True
            return self.empty()
        if m := re.fullmatch(r"/api/v1/iptv/channels/(\d+)/tune", p):
            ch = int(m[1])
            if b.get("mode") != "passthrough" or b.get("preset") != "android_exoplayer":
                return self.json({"error": "Wrong mode or preset"}, 400)
            with LOCK:
                if ch == 3 and LIVE["vod_busy"]:
                    return self.json({"error":"busy","holder":{"mine":True,"kind":"vod","title":"Sample Movie",
                                      "session_id":"live-test-other-vod","device":"Other screen"}}, 429)
                if ch == 4 and LIVE["takeover"]:
                    if not b.get("takeover"):
                        return self.json({"error":"busy","takeover":True,"holders":[{"user_id":8,
                                          "username":"Viewer","session_id":"other-live","channel_name":"Arena One"}]}, 409)
                    LIVE["takeover"] = False
                if LIVE["id"] is not None:
                    return self.json({"error": "Previous live session was not deleted"}, 429)
                LIVE["next"] += 1
                sid = f"live{LIVE['next']}"
                LIVE["id"] = sid
            return self.json({"session_id":sid,"channel_id":ch,"channel_name":CHANNELS[ch-1]["name"],
                              "mode":"passthrough","url":f"/api/v1/iptv/live/{sid}/stream.ts?sig=mock"})
        if p == "/api/v1/auth/login":
            return self.json({"token": "mock-token", "expires_at": 4102444800, "user": ME})
        if p == "/api/v1/auth/logout": return self.empty()
        if p == "/api/v1/vod/play":
            if b.get("id") == 501 and VBUSY["mine"]:
                b2 = json.dumps({"error": "busy", "holder": {"mine": True, "session_id": "other1", "kind": "vod", "title": "Some film", "device": "Bedroom TV"}}).encode()
                self.send_response(429); self.send_header("Content-Type", "application/json"); self.send_header("Retry-After", "60")
                self.send_header("Content-Length", str(len(b2))); self.end_headers(); self.wfile.write(b2); return
            if b.get("id") == 502 and not b.get("takeover") and VBUSY["takeover"]:
                return self.json({"error": "busy", "takeover": True, "holders": [{"user_id": 9, "username": "sam", "session_id": "l7", "channel_name": "News 24"}]}, 409)
            if b.get("id") == 502: VBUSY["takeover"] = False
            SID["n"] += 1; sid = f"v{SID['n']}"
            ep = b.get("kind") == "episode"
            return self.json({"session_id": sid, "mode": "remux" if ep else "direct_play",
                "url": f"/api/v1/stream/{sid}/master.m3u8?sig=mock" if ep else f"/api/v1/vod/direct/{sid}?sig=mock",
                "decision": {"mode": "remux" if ep else "direct_play", "video": {"type": "copy"}, "audio": {"type": "copy"}, "subtitle": {"type": "none"}, "reasons": ["mock vod"]},
                "subtitles": [], "audio": [], "selected": {"reason": "mock"}, "bitrate_kbps": 2600, "quality": "original", "quality_options": [], "variants": []})
        m = re.match(r"/api/v1/playback/(\d+)/decide$", p)
        if m: return self.json(decide(int(m.group(1)), b))
        m = re.match(r"/api/v1/playback/([^/]+)/progress$", p)
        if m:
            if REAP["on"]:
                REAP["on"] = False
                LOG.write("   -> 404 (reaped)\n")
                return self.json({"error": "no such session"}, 404)
            return self.empty()
        self.json({"error": "Not found"}, 404)

    def do_DELETE(self):
        self.log()

        if m := re.fullmatch(r"/api/v1/iptv/channels/(\d+)/favorite", self.path):
            CHANNELS[int(m[1]) - 1]["favorite"] = False
            return self.empty()
        if self.path == "/api/v1/playback/live-test-other-vod":
            LIVE["vod_busy"] = False
            return self.empty()
        if self.path.startswith("/api/v1/iptv/sessions/"):
            with LOCK:
                if self.path.split("/")[-1] != LIVE["id"]: return self.json({"error": "Session gone"}, 404)
                LIVE["id"] = None
            return self.empty()
        if self.path.startswith("/api/v1/playback/other1"): VBUSY["mine"] = False
        self.empty()

def vcard(i, kind="movie", title=None, progress=None):
    return {"id": i, "kind": kind, "title": title or f"IPTV movie {i}", "year": 2000 + i % 25, "poster_url": None, "progress": progress}
MOVIES = [vcard(501, title="Mine-busy movie (429)"), vcard(502, title="Takeover movie (409)"), vcard(503, title="Free movie", progress=0.3)] + [vcard(i) for i in range(504, 570)]
SERIES = [vcard(601, "series", "IPTV Mock Series")]
VBUSY = {"mine": True, "takeover": True}

def vod_page(items, qs):
    import urllib.parse
    q = urllib.parse.parse_qs(qs)
    cur = int(q.get("cursor", ["0"])[0] or 0); lim = int(q.get("limit", ["40"])[0])
    nxt = cur + lim
    return {"items": items[cur:nxt], "next_cursor": str(nxt) if nxt < len(items) else None}

ME = {"id": 1, "username": "tester", "role": "admin", "permissions": {"iptv_access": True, "vod_access": True}}

ThreadingHTTPServer(("127.0.0.1", PORT), H).serve_forever()
