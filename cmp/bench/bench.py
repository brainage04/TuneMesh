#!/usr/bin/env python3
"""Profiles the GTK app and the Compose prototype in one isolated headless sway session.

Everything runs against a private D-Bus daemon, a headless sway (with Xwayland for the JVM, whose AWT has no
Wayland backend) and a PipeWire null sink. Both apps are driven the same way: the GTK app through its
`app.*` GActions on the private bus, the prototype through the same action names on a FIFO.

usage: bench.py --gtk BIN --cmp BIN --library DIR --out DIR [--runs 3] [--screenshots DIR]
"""

import argparse
import hashlib
import json
import os
import re
import shutil
import signal
import socket
import statistics
import struct
import subprocess
import threading
import time

TICK = os.sysconf("SC_CLK_TCK")
WIDTH, HEIGHT = 1280, 820


def log(*parts):
    print(time.strftime("%H:%M:%S"), *parts, flush=True)


# ---- sway IPC -------------------------------------------------------------

class SwayIpc:
    MAGIC = b"i3-ipc"

    def __init__(self, path):
        self.sock = socket.socket(socket.AF_UNIX)
        self.sock.connect(path)

    def send(self, kind, payload=""):
        data = payload.encode()
        self.sock.sendall(self.MAGIC + struct.pack("<II", len(data), kind) + data)
        return self.recv()[1]

    def recv(self):
        header = self._read(14)
        length, kind = struct.unpack("<II", header[6:])
        return kind, json.loads(self._read(length))

    def _read(self, n):
        buf = b""
        while len(buf) < n:
            chunk = self.sock.recv(n - len(buf))
            if not chunk:
                raise EOFError
            buf += chunk
        return buf

    def command(self, text):
        return self.send(0, text)


class VirtualPointer:
    """A persistent wlr virtual pointer (zwlr_virtual_pointer_v1), spoken over the raw Wayland wire protocol.

    The headless backend has no input devices, so the seat has no pointer capability and sway's own
    `seat cursor` commands reach no client; a virtual pointer that stays connected fixes both.
    """

    def __init__(self, wayland_display):
        runtime = os.environ["XDG_RUNTIME_DIR"]
        self.sock = socket.socket(socket.AF_UNIX)
        self.sock.connect(os.path.join(runtime, wayland_display))
        self.next_id = 2
        registry = self._new_id()
        self._send(1, 1, struct.pack("<I", registry))  # wl_display.get_registry
        globals_ = self._roundtrip(registry)
        manager_name = next(n for n, (iface, _) in globals_.items() if iface == "zwlr_virtual_pointer_manager_v1")
        seat_name = next(n for n, (iface, _) in globals_.items() if iface == "wl_seat")
        self.manager = self._bind(registry, manager_name, "zwlr_virtual_pointer_manager_v1", 1)
        self.seat = self._bind(registry, seat_name, "wl_seat", 1)
        self.pointer = self._new_id()
        self._send(self.manager, 0, struct.pack("<II", self.seat, self.pointer))  # create_virtual_pointer
        self._roundtrip(None)

    def _new_id(self):
        self.next_id += 1
        return self.next_id - 1

    @staticmethod
    def _string(text):
        data = text.encode() + b"\0"
        return struct.pack("<I", len(data)) + data + b"\0" * (-len(data) % 4)

    def _send(self, obj, opcode, args=b""):
        self.sock.sendall(struct.pack("<II", obj, ((8 + len(args)) << 16) | opcode) + args)

    def _bind(self, registry, name, interface, version):
        new = self._new_id()
        self._send(registry, 0, struct.pack("<I", name) + self._string(interface) + struct.pack("<II", version, new))
        return new

    def _roundtrip(self, registry):
        callback = self._new_id()
        self._send(1, 0, struct.pack("<I", callback))  # wl_display.sync
        globals_, buf = {}, b""
        while True:
            buf += self.sock.recv(65536)
            while len(buf) >= 8:
                obj, word = struct.unpack("<II", buf[:8])
                size, opcode = word >> 16, word & 0xFFFF
                if len(buf) < size:
                    break
                body, buf = buf[8:size], buf[size:]
                if obj == registry and opcode == 0:
                    name, length = struct.unpack("<II", body[:8])
                    globals_[name] = (body[8:8 + length - 1].decode(), body)
                elif obj == callback:
                    return globals_

    def _now(self):
        return int(time.monotonic() * 1000) & 0xFFFFFFFF

    def move_to(self, x, y):
        self._send(self.pointer, 1, struct.pack("<IIIII", self._now(), x, y, WIDTH, HEIGHT))
        self._send(self.pointer, 4)

    def wheel(self, clicks=1):
        # axis_source wheel, axis_discrete vertical (15 units per click, like a mouse wheel), frame
        self._send(self.pointer, 5, struct.pack("<I", 0))
        self._send(self.pointer, 7, struct.pack("<IIiI", self._now(), 0, 15 * clicks * 256, clicks))
        self._send(self.pointer, 4)

    def close(self):
        self.sock.close()


class WindowWatcher(threading.Thread):
    """Records the monotonic time of every sway `window` event."""

    def __init__(self, path):
        super().__init__(daemon=True)
        self.ipc = SwayIpc(path)
        self.ipc.send(2, json.dumps(["window"]))
        self.events = []

    def run(self):
        try:
            while True:
                _, event = self.ipc.recv()
                self.events.append((time.monotonic(), event.get("change")))
        except (EOFError, OSError):
            pass

    def first(self, change, after):
        return next((t for t, c in self.events if c == change and t >= after), None)


# ---- session --------------------------------------------------------------

class Session:
    def __init__(self, base, tools, gpu):
        self.base = base
        self.tools = tools
        self.gpu = gpu
        self.procs = []
        self.app_procs = []
        self.sink_module = None

    def start(self):
        os.makedirs(self.base, exist_ok=True)
        out = subprocess.run(["dbus-daemon", "--session", "--fork", "--print-address=1", "--print-pid=1"],
                             capture_output=True, text=True, check=True).stdout.split()
        self.dbus_address, self.dbus_pid = out[0], int(out[1])
        conf = os.path.join(self.base, "sway.conf")
        env_file = os.path.join(self.base, "sway-env")
        if os.path.exists(env_file):
            os.remove(env_file)
        with open(conf, "w") as f:
            f.write(f"""output HEADLESS-1 mode {WIDTH}x{HEIGHT}@60Hz position 0 0 scale 1
output * bg #000000 solid_color
default_border none
default_floating_border none
titlebar_padding 1
gaps inner 0
focus_follows_mouse no
xwayland force
exec env > {env_file}
""")
        env = {k: v for k, v in os.environ.items() if k not in ("WAYLAND_DISPLAY", "DISPLAY", "SWAYSOCK", "JAVA_TOOL_OPTIONS")}
        env.update(WLR_BACKENDS="headless", WLR_HEADLESS_OUTPUTS="1", WLR_LIBINPUT_NO_DEVICES="1",
                   XDG_SESSION_TYPE="wayland", DBUS_SESSION_BUS_ADDRESS=self.dbus_address)
        if self.gpu:
            env.update(WLR_RENDERER="gles2", WLR_RENDER_DRM_DEVICE="/dev/dri/renderD128")
        else:
            env.update(WLR_RENDERER="pixman")
        self.sway = subprocess.Popen([f"{self.tools}/sway", "-c", conf] + (["--unsupported-gpu"] if self.gpu else []),
                                     env=env, stdout=open(os.path.join(self.base, "sway.log"), "w"), stderr=subprocess.STDOUT)
        self.procs.append(self.sway)
        for _ in range(200):
            if os.path.exists(env_file) and os.path.getsize(env_file) > 0:
                break
            time.sleep(0.05)
        time.sleep(0.5)
        session_env = dict(line.split("=", 1) for line in open(env_file).read().splitlines() if "=" in line)
        self.wayland = session_env["WAYLAND_DISPLAY"]
        self.swaysock = session_env["SWAYSOCK"]
        self.display = session_env.get("DISPLAY")
        for _ in range(100):
            if self.display:
                break
            time.sleep(0.1)
        self.ipc = SwayIpc(self.swaysock)
        self.pointer = VirtualPointer(self.wayland)
        self.pointer.move_to(WIDTH // 2, 2)
        self.sink = "tunemesh_bench_null"
        self.sink_module = subprocess.run(
            ["pactl", "load-module", "module-null-sink", f"sink_name={self.sink}",
             "sink_properties=device.description=TuneMeshBenchNull"],
            capture_output=True, text=True, check=True).stdout.strip()
        log("session", self.wayland, self.display, "sink module", self.sink_module, "gpu" if self.gpu else "pixman")

    def app_env(self, home, extra=None):
        env = {k: v for k, v in os.environ.items()
               if k not in ("WAYLAND_DISPLAY", "DISPLAY", "SWAYSOCK", "JAVA_TOOL_OPTIONS", "DBUS_SESSION_BUS_ADDRESS",
                            "XDG_CONFIG_HOME", "XDG_CACHE_HOME", "XDG_STATE_HOME", "XDG_DATA_HOME", "GDK_BACKEND")}
        env.update(HOME=home, XDG_CONFIG_HOME=f"{home}/.config", XDG_CACHE_HOME=f"{home}/.cache",
                   XDG_STATE_HOME=f"{home}/.local/state", XDG_DATA_HOME=f"{home}/.local/share",
                   WAYLAND_DISPLAY=self.wayland, DISPLAY=self.display or "", SWAYSOCK=self.swaysock,
                   DBUS_SESSION_BUS_ADDRESS=self.dbus_address, PIPEWIRE_NODE=self.sink, PULSE_SINK=self.sink,
                   XDG_SESSION_TYPE="wayland")
        env.update(extra or {})
        return env

    def screenshot(self, path=None):
        data = subprocess.run([f"{self.tools}/grim", "-t", "ppm", "-"], capture_output=True,
                              env={**os.environ, "WAYLAND_DISPLAY": self.wayland}).stdout
        if path:
            subprocess.run([f"{self.tools}/grim", "-t", "png", path], env={**os.environ, "WAYLAND_DISPLAY": self.wayland})
        return data

    def stop(self):
        for p in self.app_procs:
            if p.poll() is None:
                try:
                    os.killpg(p.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
        if self.sink_module:
            subprocess.run(["pactl", "unload-module", self.sink_module])
        for p in self.procs:
            if p.poll() is None:
                p.terminate()
                try:
                    p.wait(5)
                except subprocess.TimeoutExpired:
                    p.kill()
        try:
            os.kill(self.dbus_pid, signal.SIGTERM)
        except (ProcessLookupError, AttributeError):
            pass


# ---- process measurements ----------------------------------------------------

def tree(pid):
    pids = [pid]
    for p in pids:
        try:
            children = open(f"/proc/{p}/task/{p}/children").read().split()
        except OSError:
            continue
        pids += [int(c) for c in children]
    return pids


def memory(pid):
    rss = pss = 0
    for p in tree(pid):
        try:
            for line in open(f"/proc/{p}/smaps_rollup"):
                if line.startswith("Rss:"):
                    rss += int(line.split()[1])
                elif line.startswith("Pss:"):
                    pss += int(line.split()[1])
        except OSError:
            pass
    return {"rss_mib": round(rss / 1024, 1), "pss_mib": round(pss / 1024, 1)}


def cpu_ticks(pid):
    total = 0
    for p in tree(pid):
        try:
            fields = open(f"/proc/{p}/stat").read().rsplit(")", 1)[1].split()
            total += int(fields[11]) + int(fields[12])
        except (OSError, IndexError):
            pass
    return total


def threads(pid):
    try:
        return len(os.listdir(f"/proc/{pid}/task"))
    except OSError:
        return 0


def cpu_percent(pid, seconds):
    start_ticks, start = cpu_ticks(pid), time.monotonic()
    time.sleep(seconds)
    return round((cpu_ticks(pid) - start_ticks) / TICK / (time.monotonic() - start) * 100, 2)


def bright_fraction(ppm):
    # P6 header: "P6\n<w> <h>\n255\n"
    body = ppm.split(b"\n", 3)[3] if ppm.startswith(b"P6") else ppm
    if not body:
        return 0.0
    dark = body.translate(None, bytes(range(70, 256)))
    return (len(body) - len(dark)) / len(body)


# ---- apps -------------------------------------------------------------------

class App:
    def __init__(self, name, session, home, cmd, extra_env=None):
        self.name = name
        self.session = session
        self.home = home
        self.cmd = cmd
        self.extra_env = extra_env or {}
        self.proc = None

    def launch(self, log_path, extra_env=None):
        env = self.session.app_env(self.home, {**self.extra_env, **(extra_env or {})})
        self.log_file = open(log_path, "w")
        self.proc = subprocess.Popen(self.cmd, env=env, stdout=self.log_file, stderr=subprocess.STDOUT, start_new_session=True)
        self.session.app_procs.append(self.proc)
        return self.proc

    @property
    def pid(self):
        return self.proc.pid

    def wait_exit(self, timeout=15):
        start = time.monotonic()
        try:
            self.proc.wait(timeout)
            log(self.name, f"exited by itself after {time.monotonic() - start:.1f} s")
        except subprocess.TimeoutExpired:
            log(self.name, f"still running {timeout} s after quit, terminating")
            os.killpg(self.proc.pid, signal.SIGTERM)
            try:
                self.proc.wait(5)
            except subprocess.TimeoutExpired:
                os.killpg(self.proc.pid, signal.SIGKILL)
                self.proc.wait()
        self.log_file.close()


class GtkApp(App):
    def action(self, name, arg=None):
        params = f"[<'{arg}'>]" if isinstance(arg, str) else ("[<%s>]" % arg if arg is not None else "[]")
        subprocess.run([f"{self.session.gdbus}", "call", "--session", "--dest", "io.github.brainage04.TuneMesh",
                        "--object-path", "/io/github/brainage04/TuneMesh", "--method", "org.gtk.Actions.Activate",
                        name, params, "{}"],
                       env={**os.environ, "DBUS_SESSION_BUS_ADDRESS": self.session.dbus_address},
                       capture_output=True, timeout=10)

    def quit(self):
        self.action("quit")


class CmpApp(App):
    def launch(self, log_path, extra_env=None):
        self.fifo = os.path.join(self.home, "control.fifo")
        if os.path.exists(self.fifo):
            os.remove(self.fifo)
        os.mkfifo(self.fifo)
        proc = super().launch(log_path, {"TUNEMESH_CONTROL_FIFO": self.fifo, **(extra_env or {})})
        self.writer = None
        return proc

    def action(self, name, arg=None):
        if self.writer is None:
            # Opening a FIFO for writing blocks until the app opens it; poll so a dead app cannot hang the run.
            deadline = time.monotonic() + 30
            while True:
                try:
                    fd = os.open(self.fifo, os.O_WRONLY | os.O_NONBLOCK)
                    break
                except OSError:
                    if self.proc.poll() is not None or time.monotonic() > deadline:
                        raise RuntimeError(f"{self.name} is not reading its control FIFO")
                    time.sleep(0.05)
            os.set_blocking(fd, True)
            self.writer = os.fdopen(fd, "w")
        self.writer.write(name + (f" {arg}" if arg is not None else "") + "\n")
        self.writer.flush()

    def quit(self):
        try:
            self.action("quit")
            self.writer.close()
        except (BrokenPipeError, OSError):
            pass


# ---- benchmark steps ------------------------------------------------------

def measure_startup(session, app, log_path, watcher, timeout=60, done_file=None):
    """Launch to window map (sway event), first painted content and a screen unchanged for 1.5 s.

    With [done_file], also the time until the app writes it (the tag cache, written when a full scan ends)."""
    t0 = time.monotonic()
    app.launch(log_path)
    first_paint = stable_at = done_at = None
    last_hash, last_change = None, None
    samples = 0
    while time.monotonic() - t0 < timeout:
        if app.proc.poll() is not None:
            raise RuntimeError(f"{app.name} exited during startup, see {log_path}")
        shot = session.screenshot()
        now = time.monotonic()
        samples += 1
        digest = hashlib.sha1(shot).digest()
        if first_paint is None and bright_fraction(shot) > 0.002:
            first_paint = now
        if done_file and done_at is None and os.path.exists(done_file):
            done_at = now
        if digest != last_hash:
            last_hash, last_change = digest, now
        elif first_paint is not None and now - last_change >= 1.5 and (done_file is None or done_at is not None):
            stable_at = last_change
            break
        time.sleep(0.02)
    mapped = watcher.first("new", t0)
    return {
        "window_mapped_s": round(mapped - t0, 3) if mapped else None,
        "first_paint_s": round(first_paint - t0, 3) if first_paint else None,
        "stable_s": round(stable_at - t0, 3) if stable_at else None,
        "screenshot_samples": samples,
        **({"scan_done_s": round(done_at - t0, 3) if done_at else None} if done_file else {}),
    }


def sink_inputs(session):
    out = subprocess.run(["pactl", "-f", "json", "list", "sink-inputs"], capture_output=True, text=True).stdout
    try:
        items = json.loads(out)
    except json.JSONDecodeError:
        return []
    sinks = {s["index"]: s["name"] for s in json.loads(subprocess.run(["pactl", "-f", "json", "list", "sinks"], capture_output=True, text=True).stdout)}
    return [{"app": i["properties"].get("application.name"), "binary": i["properties"].get("application.process.binary"),
             "pid": i["properties"].get("application.process.id"), "corked": i.get("corked")}
            for i in items if sinks.get(i["sink"]) == session.sink]


def scroll(session, wheel_clicks, interval):
    """Scrolls the page under the content area with wheel events from the virtual pointer."""
    session.pointer.move_to(760, 450)
    time.sleep(0.3)
    start = time.monotonic()
    for i in range(wheel_clicks):
        session.pointer.wheel()
        target = start + (i + 1) * interval
        delay = target - time.monotonic()
        if delay > 0:
            time.sleep(delay)
    return time.monotonic() - start


def frames_run(session, app, run_dir, watcher, wheel_clicks=150, interval=1 / 30):
    """Scrolls the 1000-song playlist and records the app's frames: Wayland surface commits for GTK
    (WAYLAND_DEBUG), Skiko's FPS counter for the JVM app (it draws through Xwayland)."""
    os.makedirs(run_dir, exist_ok=True)
    log_path = os.path.join(run_dir, f"{app.name}-frames.log")
    extra = {"WAYLAND_DEBUG": "client", "GSK_DEBUG": "renderer"} if app.name == "gtk" else {
        "JAVA_TOOL_OPTIONS": "-Dskiko.fps.enabled=true -Dskiko.fps.periodSeconds=1 -Dskiko.fps.longFrames.show=true"}
    app.extra_env = {**app.extra_env, **extra}
    try:
        measure_startup(session, app, log_path, watcher)
    finally:
        for k in extra:
            app.extra_env.pop(k, None)
    app.action("navigate", "playlist:Road Trip")
    time.sleep(4)
    marker = os.path.getsize(log_path)
    ticks = cpu_ticks(app.pid)
    start = time.monotonic()
    elapsed = scroll(session, wheel_clicks, interval)
    time.sleep(0.5)
    cpu = (cpu_ticks(app.pid) - ticks) / TICK
    app.quit()
    app.wait_exit()
    text = open(log_path, errors="replace").read()
    window = text[marker:]
    result = {"app": app.name, "scroll_s": round(elapsed, 2), "cpu_s": round(cpu, 2)}
    if app.name == "gtk":
        commits = {}
        for m in re.finditer(r"\[\s*(\d+\.\d+)\]\s+(?:->\s+)?wl_surface#?@?(\d+)\.commit\(\)", window):
            commits.setdefault(m.group(2), []).append(float(m.group(1)))
        times = max(commits.values(), key=len) if commits else []
        times = [t for t in times if t - times[0] <= elapsed * 1000] if times else []
        gaps = [b - a for a, b in zip(times, times[1:])]
        result.update(frames=len(times), fps=round(len(times) / elapsed, 1),
                      frame_interval_p50_ms=round(statistics.median(gaps), 2) if gaps else None,
                      frame_interval_p95_ms=round(sorted(gaps)[int(len(gaps) * 0.95)], 2) if gaps else None,
                      frame_interval_max_ms=round(max(gaps), 2) if gaps else None,
                      renderer=(re.findall(r"[Uu]sing renderer[^\n]*", text) or [None])[0])
    else:
        fps = [float(x) for x in re.findall(r"FPS[^\d]*(\d+(?:\.\d+)?)", window)]
        result.update(fps_samples=fps, fps=round(statistics.median(fps), 1) if fps else None,
                      long_frames=len(re.findall(r"[Ll]ong frame", window)),
                      renderer=(re.findall(r"render API \S+", text) or [None])[0])
    log(app.name, "frames", {k: v for k, v in result.items() if k != "fps_samples"})
    return result


def train_aot(session, app, out_dir, watcher, routes):
    """JDK 25 AOT cache (JEP 483/514/515): record a browse-and-play session, then assemble the cache."""
    os.makedirs(out_dir, exist_ok=True)
    conf, cache = os.path.join(out_dir, "cmp.aotconf"), os.path.join(out_dir, "cmp.aot")
    base = app.extra_env.get("JAVA_TOOL_OPTIONS", "")
    try:
        app.extra_env["JAVA_TOOL_OPTIONS"] = f"{base} -XX:AOTMode=record -XX:AOTConfiguration={conf}".strip()
        measure_startup(session, app, os.path.join(out_dir, "record.log"), watcher)
        for route in BROWSE + CMP_ONLY:
            app.action("navigate", route.format(**routes))
            time.sleep(1.5)
        app.action("play-route", "playlist:Road Trip")
        time.sleep(3)
        app.quit()
        app.wait_exit(60)
        app.extra_env["JAVA_TOOL_OPTIONS"] = f"{base} -XX:AOTMode=create -XX:AOTConfiguration={conf} -XX:AOTCache={cache}".strip()
        app.launch(os.path.join(out_dir, "create.log"))
        app.proc.wait(600)
        app.log_file.close()
    finally:
        app.extra_env["JAVA_TOOL_OPTIONS"] = base
    log("aot cache", cache, os.path.getsize(cache) if os.path.exists(cache) else "missing")
    return cache


BROWSE = ["home", "search:{search}", "albums", "artists", "album:{album}", "artist:{artist}", "playlist:Road Trip", "liked", "playlists"]
CMP_ONLY = ["songs", "player"]


def run_once(session, app, run_dir, watcher, routes, shots_dir, idle_seconds, play_seconds):
    os.makedirs(run_dir, exist_ok=True)
    result = {"app": app.name}
    log(app.name, "launch")
    result["startup"] = measure_startup(session, app, os.path.join(run_dir, f"{app.name}.log"), watcher)
    log(app.name, "startup", result["startup"])
    time.sleep(10)
    result["idle_memory"] = memory(app.pid)
    result["threads_idle"] = threads(app.pid)
    result["idle_cpu_pct"] = cpu_percent(app.pid, idle_seconds)
    log(app.name, "idle", result["idle_memory"], result["idle_cpu_pct"])

    names = BROWSE + (CMP_ONLY if app.name == "cmp" else [])
    for route in names:
        target = route.format(**routes)
        app.action("navigate", target)
        time.sleep(2.5)
        if shots_dir:
            session.screenshot(os.path.join(shots_dir, f"{route.split(':')[0]}.png"))
    app.action("navigate", "home")
    time.sleep(5)
    result["browsed_memory"] = memory(app.pid)
    log(app.name, "browsed", result["browsed_memory"])

    app.action("play-route", "playlist:Road Trip")
    time.sleep(3)
    app.action("navigate", "playlist:Road Trip")
    app.action("toggle-queue")
    time.sleep(2)
    if shots_dir:
        session.screenshot(os.path.join(shots_dir, "queue.png"))
    app.action("toggle-queue")
    time.sleep(2)
    if shots_dir and app.name == "cmp":
        app.action("navigate", "player")
        time.sleep(2)
        session.screenshot(os.path.join(shots_dir, "player.png"))
        app.action("navigate", "playlist:Road Trip")
        time.sleep(1)
    result["sink_inputs"] = sink_inputs(session)
    if not result["sink_inputs"]:
        raise RuntimeError(f"{app.name} is not playing into the null sink, see {run_dir}/{app.name}.log")
    result["playback_cpu_pct"] = cpu_percent(app.pid, play_seconds)
    result["playback_memory"] = memory(app.pid)
    log(app.name, "playback", result["playback_cpu_pct"], result["playback_memory"], result["sink_inputs"])

    app.action("play-pause")
    time.sleep(1)
    ticks = cpu_ticks(app.pid)
    elapsed = scroll(session, 150, 1 / 30)
    time.sleep(1)
    result["scroll_cpu_s"] = round((cpu_ticks(app.pid) - ticks) / TICK, 2)
    result["scroll_wall_s"] = round(elapsed + 1, 2)
    result["after_scroll_memory"] = memory(app.pid)
    log(app.name, "scroll", result["scroll_cpu_s"], "s CPU over", result["scroll_wall_s"])
    app.quit()
    app.wait_exit()
    return result


def first_launch(session, app, run_dir, watcher, cache_file):
    """Start with no tag cache: the full scan of the library, until the app writes its cache file."""
    shutil.rmtree(os.path.dirname(cache_file), ignore_errors=True)
    os.makedirs(run_dir, exist_ok=True)
    startup = measure_startup(session, app, os.path.join(run_dir, f"{app.name}-first.log"), watcher, timeout=180, done_file=cache_file)
    log(app.name, "first launch", startup)
    app.quit()
    app.wait_exit()
    return startup


def median(values):
    values = [v for v in values if v is not None]
    return round(statistics.median(values), 3) if values else None


def summarize(runs):
    keys = {
        "window_mapped_s": lambda r: r["startup"]["window_mapped_s"],
        "first_paint_s": lambda r: r["startup"]["first_paint_s"],
        "stable_s": lambda r: r["startup"]["stable_s"],
        "idle_rss_mib": lambda r: r["idle_memory"]["rss_mib"],
        "idle_pss_mib": lambda r: r["idle_memory"]["pss_mib"],
        "browsed_rss_mib": lambda r: r["browsed_memory"]["rss_mib"],
        "browsed_pss_mib": lambda r: r["browsed_memory"]["pss_mib"],
        "playback_rss_mib": lambda r: r["playback_memory"]["rss_mib"],
        "playback_pss_mib": lambda r: r["playback_memory"]["pss_mib"],
        "idle_cpu_pct": lambda r: r["idle_cpu_pct"],
        "playback_cpu_pct": lambda r: r["playback_cpu_pct"],
        "scroll_cpu_s": lambda r: r["scroll_cpu_s"],
        "threads_idle": lambda r: r["threads_idle"],
    }
    out = {}
    for app in sorted({r["app"] for r in runs}):
        mine = [r for r in runs if r["app"] == app]
        out[app] = {k: median([f(r) for r in mine]) for k, f in keys.items()}
        out[app]["runs"] = len(mine)
    return out


def main():
    # A timeout or heavy stopping the job sends SIGTERM: unwind so the session, apps and null sink are cleaned up.
    signal.signal(signal.SIGTERM, lambda *_: (_ for _ in ()).throw(SystemExit(143)))
    parser = argparse.ArgumentParser()
    parser.add_argument("--gtk", required=True)
    parser.add_argument("--cmp", required=True)
    parser.add_argument("--cmp-variant", action="append", default=[], help="NAME=JAVA_TOOL_OPTIONS for extra CMP runs")
    parser.add_argument("--library", required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--tools", required=True, help="directory with sway, swaymsg, grim")
    parser.add_argument("--gdbus", required=True)
    parser.add_argument("--runs", type=int, default=3)
    parser.add_argument("--first-launch-runs", type=int, default=3)
    parser.add_argument("--idle-seconds", type=int, default=60)
    parser.add_argument("--play-seconds", type=int, default=60)
    parser.add_argument("--screenshots")
    parser.add_argument("--album", required=True)
    parser.add_argument("--artist", required=True)
    parser.add_argument("--search", default="lantern")
    parser.add_argument("--gpu", action="store_true")
    parser.add_argument("--cmp-ld-path", default="", help="LD_LIBRARY_PATH for the JVM app (libGL on NixOS)")
    parser.add_argument("--frames-runs", type=int, default=3)
    args = parser.parse_args()

    base = os.path.join(args.out, "session")
    session = Session(base, args.tools, args.gpu)
    session.gdbus = args.gdbus
    results = {"runs": [], "first_launch": [], "variants": {}}
    routes = {"album": args.album, "artist": args.artist, "search": args.search}
    homes = {name: os.path.join(args.out, f"home-{name}") for name in ["gtk", "cmp"]}
    for h in homes.values():
        os.makedirs(h, exist_ok=True)
    try:
        session.start()
        watcher = WindowWatcher(session.swaysock)
        watcher.start()
        gtk = GtkApp("gtk", session, homes["gtk"], [args.gtk, "--library", args.library], {"GDK_BACKEND": "wayland"})
        cmp_env = {"LD_LIBRARY_PATH": args.cmp_ld_path} if args.cmp_ld_path else {}
        cmp = CmpApp("cmp", session, homes["cmp"], [args.cmp, "--library", args.library], cmp_env)

        # First launch (empty tag caches), then a warm-up launch so later runs find their caches.
        for i in range(args.first_launch_runs):
            for app, cache in ((gtk, f"{homes['gtk']}/.cache/tunemesh/library.json"), (cmp, f"{homes['cmp']}/.cache/tunemesh-cmp/tags.json")):
                results["first_launch"].append({"app": app.name, **first_launch(session, app, os.path.join(args.out, f"first-{i}"), watcher, cache)})

        for i in range(args.runs):
            for app in (gtk, cmp):
                shots = None
                if args.screenshots and i == 0:
                    shots = os.path.join(args.screenshots, app.name)
                    os.makedirs(shots, exist_ok=True)
                results["runs"].append(run_once(session, app, os.path.join(args.out, f"run-{i}"), watcher, routes, shots,
                                                args.idle_seconds, args.play_seconds))
                json.dump(results, open(os.path.join(args.out, "results.json"), "w"), indent=1)

        results["frames"] = []
        for i in range(args.frames_runs):
            for app in (gtk, cmp):
                results["frames"].append(frames_run(session, app, os.path.join(args.out, f"frames-{i}"), watcher))
                json.dump(results, open(os.path.join(args.out, "results.json"), "w"), indent=1)

        aot_cache = None
        for variant in args.cmp_variant:
            name, options = variant.split("=", 1)
            if "{aot}" in options:
                if aot_cache is None:
                    # Train with the variant's own flags (the cache records the GC and heap settings it was made with).
                    trainer = CmpApp("cmp", session, homes["cmp"], [args.cmp, "--library", args.library],
                                     {**cmp_env, "JAVA_TOOL_OPTIONS": options.replace("-XX:AOTCache={aot}", "").strip()})
                    aot_cache = train_aot(session, trainer, os.path.join(args.out, "aot"), watcher, routes)
                options = options.replace("{aot}", aot_cache)
            app = CmpApp("cmp", session, homes["cmp"], [args.cmp, "--library", args.library], {**cmp_env, "JAVA_TOOL_OPTIONS": options})
            runs = [run_once(session, app, os.path.join(args.out, f"variant-{name}-{i}"), watcher, routes, None,
                             args.idle_seconds, args.play_seconds) for i in range(args.runs)]
            results["variants"][name] = {"options": options, "runs": runs, "summary": summarize(runs)}
            json.dump(results, open(os.path.join(args.out, "results.json"), "w"), indent=1)

        results["summary"] = summarize(results["runs"])
        firsts = {}
        for app in ("gtk", "cmp"):
            mine = [r for r in results["first_launch"] if r["app"] == app]
            firsts[app] = {k: median([r.get(k) for r in mine]) for k in ("window_mapped_s", "first_paint_s", "stable_s", "scan_done_s")}
        results["first_launch_summary"] = firsts
        frames = {}
        for app in ("gtk", "cmp"):
            mine = [r for r in results["frames"] if r["app"] == app]
            frames[app] = {k: median([r.get(k) for r in mine]) for k in ("fps", "cpu_s", "frame_interval_p50_ms", "frame_interval_p95_ms", "frame_interval_max_ms", "long_frames")}
        results["frames_summary"] = frames
        json.dump(results, open(os.path.join(args.out, "results.json"), "w"), indent=1)
        log("summary", json.dumps(results["summary"], indent=1))
        log("first launch", json.dumps(firsts))
    finally:
        session.stop()


if __name__ == "__main__":
    main()
