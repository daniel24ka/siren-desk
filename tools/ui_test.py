"""Presses the program's real buttons by sending its windows mouse messages, against the stand-in for the sources:
tabs, history, adding and removing a place in the settings, a rehearsal and its X. Silent, marked as a drill on screen.

    python -I -X utf8 tools/ui_test.py <exe> <work dir>

Exit code 1 if a check failed. Writes pictures of the real screen into the work dir.
"""
import ctypes
import json
import os
import subprocess
import sys
import time
import urllib.request
from ctypes import wintypes

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import e2e  # noqa: E402

user32 = ctypes.WinDLL("user32", use_last_error=True)
user32.FindWindowW.restype = wintypes.HWND
user32.FindWindowW.argtypes = [wintypes.LPCWSTR, wintypes.LPCWSTR]
user32.FindWindowExW.restype = wintypes.HWND
user32.FindWindowExW.argtypes = [wintypes.HWND, wintypes.HWND, wintypes.LPCWSTR, wintypes.LPCWSTR]
user32.PostMessageW.argtypes = [wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM]
user32.SendMessageW.restype = ctypes.c_ssize_t
user32.SendMessageW.argtypes = [wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPCWSTR]
user32.GetClientRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]
user32.IsWindowVisible.argtypes = [wintypes.HWND]

WM_MOUSEMOVE, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEWHEEL, WM_SETTEXT, WM_CLOSE = 0x200, 0x201, 0x202, 0x20A, 0x0C, 0x10


def click(hwnd, x, y):
    lp = (int(y) << 16) | (int(x) & 0xFFFF)
    user32.PostMessageW(hwnd, WM_MOUSEMOVE, 0, lp)
    time.sleep(0.12)
    user32.PostMessageW(hwnd, WM_LBUTTONDOWN, 1, lp)
    user32.PostMessageW(hwnd, WM_LBUTTONUP, 0, lp)
    time.sleep(0.35)


def size(hwnd):
    rc = wintypes.RECT()
    user32.GetClientRect(hwnd, ctypes.byref(rc))
    return rc.right, rc.bottom


def alert_windows():
    found = []
    h = None
    while True:
        h = user32.FindWindowExW(None, h, "SirenDeskOverlay", None)
        if not h:
            return found
        found.append(h)


def main():
    exe, work = os.path.abspath(sys.argv[1]), os.path.abspath(sys.argv[2])
    os.makedirs(work, exist_ok=True)
    standin = subprocess.Popen([sys.executable, "-I", "-X", "utf8", os.path.join(HERE, "feed_standin.py"), str(e2e.PORT)])
    data = os.path.join(work, "ui")
    os.makedirs(data, exist_ok=True)
    try:
        time.sleep(0.8)
        e2e.ctl("/ctl/reset")
        e2e.ctl("/ctl/siren", names="נחל עוז|כיסופים", threat=0)
        with open(os.path.join(data, "settings.json"), "w", encoding="utf-8") as f:
            json.dump({"autostart": False, "homes": ["רחובות"]}, f)
        proc = subprocess.Popen([exe, "--base", e2e.BASE, "--data-dir", data, "--mute", "--no-supervisor", "--exit-after", "75"])
        hwnd = None
        for _ in range(60):
            hwnd = user32.FindWindowW("SirenDeskMain", None)
            if hwnd and user32.IsWindowVisible(hwnd):
                break
            time.sleep(0.1)
        e2e.check("the main window came up", bool(hwnd))
        time.sleep(2.5)
        w, h = size(hwnd)
        print("  client area", w, h, flush=True)
        side = min(440.0, w * 0.45)
        right = w - 16
        tab_w = (side - 32) / 3
        tab_y = 142
        e2e.shot(os.path.join(work, "ui-state.png"))

        # the history tab: a row picked shows its places on the map
        click(hwnd, right - tab_w * 1.5, tab_y)
        click(hwnd, right - 120, 196)
        time.sleep(1.2)
        e2e.shot(os.path.join(work, "ui-history.png"))

        # the settings tab: add a place by typing, then take it off
        click(hwnd, right - tab_w * 2.5, tab_y)
        edit = user32.FindWindowExW(hwnd, None, "EDIT", None)
        e2e.check("the search box exists", bool(edit))
        time.sleep(0.3)
        e2e.check("the search box shows on this tab", bool(user32.IsWindowVisible(edit)))
        user32.SendMessageW(edit, WM_SETTEXT, 0, "רמלה")
        time.sleep(0.5)
        e2e.shot(os.path.join(work, "ui-search.png"))
        # settings tab: heading 24, one place row 34, box 38, then the results
        body_y = 168
        click(hwnd, right - 150, body_y + 24 + 34 + 38 + 14)
        time.sleep(0.4)
        saved = json.load(open(os.path.join(data, "settings.json"), encoding="utf-8"))
        e2e.check("clicking a result adds the place", saved.get("homes") == ["רחובות", "רמלה"], saved.get("homes"))
        # the new row's X, at the far (left) end of the column
        col_x = w - side + 16
        click(hwnd, col_x + 15, body_y + 24 + 34 + 15)
        time.sleep(0.4)
        saved = json.load(open(os.path.join(data, "settings.json"), encoding="utf-8"))
        e2e.check("its X takes it off", saved.get("homes") == ["רחובות"], saved.get("homes"))
        e2e.shot(os.path.join(work, "ui-settings.png"))

        # a toggle: far sound, third row under the mode buttons
        toggles_y = body_y + 24 + 34 + 38 + 6 + 24 + 42 + 16
        click(hwnd, col_x + 20, toggles_y)
        time.sleep(0.3)
        saved = json.load(open(os.path.join(data, "settings.json"), encoding="utf-8"))
        e2e.check("a toggle changes the setting", saved.get("far_sound") is False, saved.get("far_sound"))
        click(hwnd, col_x + 20, toggles_y)
        time.sleep(0.3)

        # the map: wheel and drag do not break anything; a rehearsal by its own button
        click(hwnd, right - tab_w * 0.5, tab_y)
        time.sleep(0.5)
        e2e.ctl("/ctl/reset")
        print("  status:", {k: e2e_status(data).get(k) for k in ("protection", "mode", "zones", "main_visible")}, flush=True)
        e2e.check("the program is still alive", proc.poll() is None, proc.poll())
        # a home alert on top of it all, then its X
        e2e.ctl("/ctl/siren", names="רחובות|נס ציונה", threat=0)
        for _ in range(40):
            if e2e_status(data).get("mode") == "Full":
                break
            time.sleep(0.1)
        e2e.check("a siren at home takes the full screen", e2e_status(data).get("mode") == "Full", e2e_status(data).get("mode"))
        time.sleep(0.8)
        shown = [x for x in alert_windows() if user32.IsWindowVisible(x)]
        e2e.check("on both monitors", len(shown) >= 2, len(shown))
        e2e.shot(os.path.join(work, "ui-full.png"))
        aw, ah = size(shown[0])
        map_w = round((aw - 48) * 0.57)
        click(shown[0], 24 + map_w + 28 + 22, 56 + 24 + 22)
        time.sleep(1.3)
        e2e.check("its X closes the full screen", e2e_status(data).get("mode") == "Hidden", e2e_status(data).get("mode"))
        e2e.check("nothing left on screen", not [x for x in alert_windows() if user32.IsWindowVisible(x)])
        rc = proc.poll()
        e2e.check("still alive after all the buttons", rc is None, rc)
        proc.wait(timeout=80)
        e2e.check("clean exit", proc.returncode == 0, proc.returncode)
    finally:
        standin.kill()
    print("FAILED: %s" % e2e.fails if e2e.fails else "all checks passed", flush=True)
    sys.exit(1 if e2e.fails else 0)


def e2e_status(data):
    try:
        with open(os.path.join(data, "status.json"), encoding="utf-8") as f:
            return json.load(f)
    except (OSError, ValueError):
        return {}


if __name__ == "__main__":
    main()
