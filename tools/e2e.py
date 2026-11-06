"""The whole path, measured, against a stand-in for the sources (tools/feed_standin.py): a new alert in the stand-in ->
the program reads it -> pictures on the real screen. Silent (--mute) and marked as a drill on screen.

    python -I -X utf8 tools/e2e.py <exe> <work dir> [part ...]     parts: latency barrage failures crash

Prints what it measured and writes screenshots of the real desktop into the work dir. Exit code 1 if a check failed.
"""
import json
import os
import statistics
import subprocess
import sys
import time
import urllib.parse
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
PORT = 18790
BASE = "http://127.0.0.1:%d" % PORT
FAR = ["קריית שמונה", "מטולה", "כפר גלעדי", "תל חי", "שדה נחמיה"]
HOME = ["רחובות", "נס ציונה", "אחיסמך", "בן שמן"]

fails = []


def check(name, ok, detail=""):
    print(("  ok   " if ok else "  FAIL ") + name + ((": " + str(detail)) if detail != "" else ""), flush=True)
    if not ok:
        fails.append(name)


def ctl(path, **q):
    url = BASE + path
    if q:
        url += "?" + urllib.parse.urlencode(q)
    with urllib.request.urlopen(url, timeout=5) as r:
        return json.loads(r.read().decode("utf-8"))


def shot(path):
    script = (
        "Add-Type -AssemblyName System.Windows.Forms,System.Drawing;"
        "$b=[System.Windows.Forms.SystemInformation]::VirtualScreen;"
        "$i=New-Object System.Drawing.Bitmap $b.Width,$b.Height;"
        "$g=[System.Drawing.Graphics]::FromImage($i);"
        "$g.CopyFromScreen($b.Left,$b.Top,0,0,$i.Size);"
        "$i.Save('%s');$g.Dispose();$i.Dispose()" % path.replace("'", "''")
    )
    subprocess.run(["powershell.exe", "-NoProfile", "-Command", script], check=False, timeout=30)


class Program:
    def __init__(self, exe, work, name, seconds, extra=()):
        ctl("/ctl/reset")
        self.data = os.path.join(work, name)
        os.makedirs(self.data, exist_ok=True)
        for f in ("alerts.jsonl", "status.json"):
            try:
                os.remove(os.path.join(self.data, f))
            except OSError:
                pass
        with open(os.path.join(self.data, "settings.json"), "w", encoding="utf-8") as f:
            json.dump({"autostart": False, "homes": ["רחובות"]}, f)
        args = [exe, "--base", BASE, "--data-dir", self.data, "--mute", "--background", "--exit-after", str(seconds)]
        args += list(extra) or ["--no-supervisor"]
        # started through WMI: as the real program runs, outside the CPU cap on this agent's own processes
        line = subprocess.list2cmdline(args)
        script = "$r=Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{CommandLine='%s'};$r.ProcessId" % line.replace("'", "''")
        self.pid = int(subprocess.run(["powershell.exe", "-NoProfile", "-Command", script], capture_output=True, text=True, timeout=40).stdout.strip())

    def status(self):
        try:
            with open(os.path.join(self.data, "status.json"), encoding="utf-8") as f:
                return json.load(f)
        except (OSError, ValueError):
            return {}

    def records(self):
        try:
            with open(os.path.join(self.data, "alerts.jsonl"), encoding="utf-8") as f:
                return [json.loads(x) for x in f if x.strip()]
        except OSError:
            return []

    def wait_status(self, test, secs, what=""):
        end = time.time() + secs
        while time.time() < end:
            s = self.status()
            if s and test(s):
                return s
            time.sleep(0.1)
        return self.status()

    def wait_record(self, test, secs):
        end = time.time() + secs
        while time.time() < end:
            for r in self.records():
                if test(r):
                    return r
            time.sleep(0.02)
        return None

    def alive(self):
        out = subprocess.run(["tasklist", "/FI", "PID eq %d" % self.pid, "/NH"], capture_output=True, text=True).stdout
        return str(self.pid) in out

    def done(self, secs=30):
        end = time.time() + secs
        while time.time() < end:
            if not self.alive():
                return 0
            time.sleep(0.3)
        subprocess.run(["taskkill", "/PID", str(self.pid), "/F", "/T"], capture_output=True, check=False)
        return None


def one_alert(exe, work, names, label, shot_to=None):
    """One alert from a screen with nothing on it. Returns (served->read ms, read->frame ms, posted->read ms)."""
    p = Program(exe, work, "run", 13)
    p.wait_status(lambda s: s.get("protection") == "Ok", 6.5)
    time.sleep(0.4)
    posted = ctl("/ctl/siren", names="|".join(names), threat=0)
    rec = p.wait_record(lambda r: "kind" in r, 4)
    frame = p.wait_record(lambda r: "first_frame_ms" in r, 4)
    served = ctl("/ctl/log").get(posted["ok"])
    if shot_to:
        time.sleep(1.2)
        shot(shot_to)
    ctl("/ctl/quiet")
    p.done()
    if not rec or not frame or not served:
        check(label + ": the alert reached the screen", False, (rec, frame, served))
        return None
    return ((rec["t"] / 1000.0 - served) * 1000.0, frame["first_frame_ms"], (rec["t"] / 1000.0 - posted["t"]) * 1000.0)


def latency(exe, work):
    print("latency, nothing on screen -> the alert on screen:", flush=True)
    rows = {"panel (far)": [], "full screen (home)": []}
    for i in range(4):
        r = one_alert(exe, work, FAR, "panel", os.path.join(work, "panel-desktop.png") if i == 0 else None)
        if r:
            rows["panel (far)"].append(r)
        r = one_alert(exe, work, HOME, "full", os.path.join(work, "full-desktop.png") if i == 0 else None)
        if r:
            rows["full screen (home)"].append(r)
    for name, v in rows.items():
        if v:
            print("  %s, %d runs:" % (name, len(v)))
            print("    answer served -> alert in the engine  median %.1f ms, worst %.1f ms" % (statistics.median(x[0] for x in v), max(x[0] for x in v)))
            print("    answer read -> first frame on screen  median %.1f ms, worst %.1f ms" % (statistics.median(x[1] for x in v), max(x[1] for x in v)))
            print("    alert posted -> read (the 1 s poll)   median %.0f ms, worst %.0f ms" % (statistics.median(x[2] for x in v), max(x[2] for x in v)), flush=True)
    check("every run reached the screen", all(len(v) == 4 for v in rows.values()), {k: len(v) for k, v in rows.items()})
    check("first frame under 250 ms each time", all(x[1] < 250 for v in rows.values() for x in v))


def barrage(exe, work):
    print("a barrage of 1200 places:", flush=True)
    cities = json.load(open(os.path.join(HERE, "..", "assets", "lists", "cities.json"), encoding="utf-8"))["cities"]
    names = list(cities)[:1200]
    if "רחובות" not in names:
        names.append("רחובות")
    p = Program(exe, work, "run", 22)
    p.wait_status(lambda s: s.get("protection") == "Ok", 6)
    ctl("/ctl/siren", names="|".join(names), threat=0)
    rec = p.wait_record(lambda r: "first_frame_ms" in r, 5)
    check("first frame", rec is not None, rec and "%.1f ms" % rec["first_frame_ms"])
    time.sleep(3)
    shot(os.path.join(work, "barrage-desktop.png"))
    time.sleep(3)
    s = p.wait_status(lambda s: s.get("frames", 0) > 0 and s.get("active", 0) > 1000, 6)
    print("  zones %s, frame draw avg %.2f ms, max %.1f ms, frames in the last report: %s" % (s.get("active"), s.get("frame_ms_avg", -1), s.get("frame_ms_max", -1), s.get("frames")))
    check("all the places are on the list", s.get("active", 0) >= 1200, s.get("active"))
    check("frames keep up: over 20 a second", s.get("frames", 0) > 20, s.get("frames"))
    ctl("/ctl/quiet")
    p.done()


def failures(exe, work):
    print("when the source fails:", flush=True)
    p = Program(exe, work, "run", 75)
    s = p.wait_status(lambda s: s.get("protection") == "Ok", 6)
    check("all well at the start", s.get("protection") == "Ok", s.get("protection"))
    # a page instead of the list: an answer, but not a good one
    ctl("/ctl/break", mode="html")
    s = p.wait_status(lambda s: s.get("protection") == "Weak", 14)
    check("HTML instead of the list counts as a bad answer", s.get("protection") == "Weak" and "not JSON" in s["sirens"]["err"], s.get("sirens", {}).get("err"))
    ctl("/ctl/break", mode="500")
    s = p.wait_status(lambda s: s.get("protection") == "Down", 26)
    check("no good answer for 20 s: says Down", s.get("protection") == "Down", s.get("trouble"))
    check("and says it in plain words", "אזעקה לא תוצג" in (s.get("trouble") or ""), s.get("trouble"))
    time.sleep(18)
    shot(os.path.join(work, "down-desktop.png"))
    ctl("/ctl/break", mode="ok")
    s = p.wait_status(lambda s: s.get("protection") == "Ok", 9)
    check("back to Ok when the answers return", s.get("protection") == "Ok", s.get("protection"))
    # a cache handing the same old answer: everything "works", nothing is fresh
    ctl("/ctl/age", seconds=600)
    s = p.wait_status(lambda s: s.get("protection") == "Weak", 9)
    check("an answer a cache held 10 minutes is not fresh", s.get("protection") == "Weak", s.get("trouble"))
    ctl("/ctl/age", seconds=1)
    p.done(40)


def crash(exe, work):
    print("when the program itself dies:", flush=True)
    p = Program(exe, work, "run", 16, extra=["--dummy"])
    s = p.wait_status(lambda s: s.get("protection") == "Ok", 8)
    first = s.get("pid")
    check("running under its keeper", bool(first) and first != p.pid, first)
    subprocess.run(["taskkill", "/PID", str(first), "/F"], capture_output=True, check=False)
    t0 = time.time()
    s = p.wait_status(lambda s: s.get("pid") != first and s.get("protection") == "Ok", 12)
    check("killed: a new copy reads again", s.get("pid") != first and s.get("protection") == "Ok", "%.1f s, pid %s -> %s" % (time.time() - t0, first, s.get("pid")))
    rc = p.done(30)
    check("a clean exit ends the keeper too", rc == 0, rc)


def main():
    exe, work = os.path.abspath(sys.argv[1]), os.path.abspath(sys.argv[2])
    parts = sys.argv[3:] or ["latency", "barrage", "failures", "crash"]
    os.makedirs(work, exist_ok=True)
    standin = subprocess.Popen([sys.executable, "-I", "-X", "utf8", os.path.join(HERE, "feed_standin.py"), str(PORT)])
    try:
        time.sleep(0.8)
        for part in parts:
            globals()[part](exe, work)
    finally:
        standin.kill()
    print("FAILED: %s" % fails if fails else "all checks passed", flush=True)
    sys.exit(1 if fails else 0)


if __name__ == "__main__":
    main()
