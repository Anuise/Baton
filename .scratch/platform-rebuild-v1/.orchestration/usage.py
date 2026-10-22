import json, os, glob, time, urllib.request
home = os.path.expanduser("~")
for d in sorted(glob.glob(os.path.join(home, ".claude-Work-*"))):
    wid = d.rsplit("-", 1)[1]
    exp = ""
    try:
        o = json.load(open(os.path.join(d, ".credentials.json"), encoding="utf-8"))["claudeAiOauth"]
        exp = "token expired" if o.get("expiresAt", 0) / 1000 < time.time() else ""
        req = urllib.request.Request("https://api.anthropic.com/api/oauth/usage", headers={
            "Authorization": "Bearer " + o["accessToken"], "anthropic-beta": "oauth-2025-04-20"})
        u = json.load(urllib.request.urlopen(req, timeout=15))
        fmt = lambda w: f"{w.get('utilization') or 0:.0f}% (reset {(w.get('resets_at') or '-')[5:16]}Z)" if w else "-"
        print(f"{wid} | {fmt(u.get('five_hour'))} | {fmt(u.get('seven_day'))}")
    except Exception as e:
        print(f"{wid} | err {type(e).__name__} {getattr(e, 'code', '')} {exp}")
