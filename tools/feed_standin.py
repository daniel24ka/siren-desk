"""A stand-in for the alert sources, on this PC only, to rehearse the whole path: the program reads it with
--base http://127.0.0.1:18790 and cannot tell it from the real thing. Nothing here reaches the real sources.

    python -I -X utf8 tools/feed_standin.py [port]

Control (GET): /ctl/siren?names=a|b&threat=0   a new alert on /notifications
               /ctl/pre?names=a|b              a new early warning on /system-messages and /ios/feed
               /ctl/end?names=a|b              a new "event ended"
               /ctl/quiet                      /notifications goes back to []
               /ctl/break?mode=html|500|hang|ok   how /notifications answers from now on
               /ctl/age?seconds=N              the Age header it gives (a cache holding an old answer)
               /ctl/log                        when each alert was first served (JSON)
"""
import json
import sys
import threading
import time
import urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

CITIES = json.load(open(__file__.rsplit("tools", 1)[0] + "assets/lists/cities.json", encoding="utf-8"))["cities"]

lock = threading.Lock()
state = {"alerts": [], "messages": [], "history": [], "mode": "ok", "age": 1, "seq": 1000, "served": {}}
BASE_ID = 5000


def names_of(q):
    return [n for n in urllib.parse.parse_qs(q).get("names", [""])[0].split("|") if n]


class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *a):
        pass

    def send_json(self, obj, status=200):
        body = json.dumps(obj, ensure_ascii=False).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Age", str(state["age"]))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        path, _, query = self.path.partition("?")
        now = time.time()
        with lock:
            if path == "/notifications":
                mode = state["mode"]
                if mode == "500":
                    return self.send_json({"error": 1}, 500)
                if mode == "hang":
                    time.sleep(30)
                    return self.send_json([])
                if mode == "html":
                    body = b"<html>not json</html>"
                    self.send_response(200)
                    self.send_header("Content-Length", str(len(body)))
                    self.end_headers()
                    self.wfile.write(body)
                    return None
                for a in state["alerts"]:
                    state["served"].setdefault(a["notificationId"], now)
                return self.send_json(state["alerts"])
            if path.startswith("/system-messages/id/"):
                mid = int(path.rsplit("/", 1)[1])
                for m in state["messages"]:
                    if m["id"] == mid:
                        state["served"].setdefault("m%d" % mid, now)
                        return self.send_json(m)
                if mid == BASE_ID:
                    return self.send_json({"id": BASE_ID, "time": int(now) - 100000, "titleHe": "ישן", "bodyHe": "", "instructionType": 1, "citiesIds": []})
                self.send_response(404)
                self.send_header("Content-Length", "0")
                self.end_headers()
                return None
            if path == "/ios/feed":
                base = {"id": BASE_ID, "time": int(now) - 100000, "titleHe": "ישן", "bodyHe": "", "instructionType": 1, "citiesIds": []}
                return self.send_json({"alertsHistory": state["history"], "systemMessages": [], "instructions": list(reversed(state["messages"])) + [base], "citiesVersion": 10, "polygonsVersion": 5})
            if path == "/warningMessages/alert/Alerts.json":
                self.send_response(403)
                self.send_header("Content-Length", "0")
                self.end_headers()
                return None
            if path == "/ctl/siren":
                state["seq"] += 1
                threat = int(urllib.parse.parse_qs(query).get("threat", ["0"])[0])
                a = {"notificationId": "s%d" % state["seq"], "time": int(now), "threat": threat, "isDrill": False, "cities": names_of(query)}
                state["alerts"].append(a)
                state["history"].insert(0, {"id": state["seq"], "description": None, "alerts": [{"time": a["time"], "cities": a["cities"], "threat": threat, "isDrill": False}]})
                return self.send_json({"ok": a["notificationId"], "t": now})
            if path in ("/ctl/pre", "/ctl/end"):
                kind = 0 if path == "/ctl/pre" else 1
                ids = [CITIES[n]["id"] for n in names_of(query) if n in CITIES]
                mid = BASE_ID + len(state["messages"]) + 1
                title = "התרעה מקדימה" if kind == 0 else "סיום אירוע"
                body = "בעקבות זיהוי שיגורים, *בדקות הקרובות* צפויות להתקבל התרעות" if kind == 0 else "האירוע הסתיים"
                state["messages"].append({"id": mid, "time": int(now), "titleHe": title, "bodyHe": body, "instructionType": kind, "citiesIds": ids, "areasIds": []})
                return self.send_json({"ok": mid, "t": now})
            if path == "/ctl/reset":
                state.update({"alerts": [], "messages": [], "history": [], "mode": "ok", "age": 1, "served": {}})
                return self.send_json({"ok": True})
            if path == "/ctl/quiet":
                state["alerts"] = []
                return self.send_json({"ok": True})
            if path == "/ctl/break":
                state["mode"] = urllib.parse.parse_qs(query).get("mode", ["ok"])[0]
                return self.send_json({"mode": state["mode"]})
            if path == "/ctl/age":
                state["age"] = int(urllib.parse.parse_qs(query).get("seconds", ["1"])[0])
                return self.send_json({"age": state["age"]})
            if path == "/ctl/log":
                return self.send_json(state["served"])
        return self.send_json({"unknown": path}, 404)


def main():
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 18790
    class Quiet(ThreadingHTTPServer):
        def handle_error(self, request, client_address):
            pass

    Quiet(("127.0.0.1", port), H).serve_forever()


if __name__ == "__main__":
    main()
