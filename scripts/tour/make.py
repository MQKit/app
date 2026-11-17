"""Records the voiced tour for the website from the running debug app (npm run dev with
tauri.debug.conf.json), light and dark, with the narration in narration.json:

  python scripts/tour/make.py [--voice kokoro:af_heart|MarkM|ZiraM|DavidM|folder] [--reencode] [light] [dark]

The voice: Kokoro (a neural voice, from an existing install: KOKORO_DIR), a Windows voice
through the app's own Text to speech tool, or recordings: a folder with 01.wav, 02.wav…
(one per line, in order). Writes
website/public/tour/tour-<theme>.mp4/.webm/.webp (no captions: the voice says it all). Needs ffmpeg.
The app's saved options and history are restored afterwards."""
import base64, json, os, shutil, subprocess, sys, time

import frames

here = os.path.dirname(os.path.abspath(__file__))
app = os.path.dirname(os.path.dirname(here))
site = os.path.join(os.path.dirname(app), "website", "public", "tour")
work = os.path.join(app, "testdata", "out", "tour")
files_dir = os.path.join(work, "files")
dev = os.path.join(app, "testdata", "out", "dev")
VOICES = "HKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Speech_OneCore\\Voices\\Tokens\\MSTTS_V110_enUS_"


def js(code, timeout=15):
    out = os.path.join(dev, "out.json")
    if os.path.exists(out):
        os.remove(out)
    with open(os.path.join(dev, "in.js"), "w", encoding="utf-8") as f:
        f.write(code)
    end = time.time() + timeout
    while time.time() < end:
        time.sleep(0.1)
        if os.path.exists(out):
            return json.loads(open(out, encoding="utf-8").read())
    raise SystemExit("the app didn't answer (is the debug app running?)")


def duration(path):
    r = subprocess.run(["ffprobe", "-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", path], capture_output=True, text=True)
    return float(r.stdout.strip())


# The Kokoro folder (kokoro_onnx, its model and voices, a venv): set KOKORO_DIR.
KOKORO = os.environ.get("KOKORO_DIR", "")


def voice_lines(lines, voice):
    """One WAV per line: recordings from a folder, a Kokoro voice (kokoro:af_heart) or a
    Windows voice (MarkM)."""
    out = []
    os.makedirs(os.path.join(work, "voice"), exist_ok=True)
    if voice.startswith("kokoro:"):
        name = voice.split(":", 1)[1]
        if not KOKORO:
            raise SystemExit("Set KOKORO_DIR to the Kokoro folder (or use a Windows voice, or a folder of WAVs).")
        py =os.path.join(KOKORO, ".venv", "Scripts", "python.exe")
        made = os.path.join(work, "voice")
        subprocess.run([py, os.path.join(here, "kokoro_voice.py"), KOKORO, name, "0.95", made, os.path.join(here, "narration.json")],
                       check=True, cwd=work)
        return [(os.path.join(made, f"{i:02}.wav"), duration(os.path.join(made, f"{i:02}.wav"))) for i in range(1, len(lines) + 1)]
    for i, line in enumerate(lines, 1):
        wav = os.path.join(work, "voice", f"{i:02}.wav")
        if os.path.isdir(voice):
            shutil.copy(os.path.join(voice, f"{i:02}.wav"), wav)
        else:
            txt = os.path.join(work, "voice", f"{i:02}.txt")
            open(txt, "w", encoding="utf-8").write(line.get("say", line["text"]))
            req = {"cmd": "run", "id": "v", "level": "fast", "locale": "en-US",
                   "files": [{"id": "f1", "path": txt, "item": {}, "analysis": {}}],
                   "options": {"voice": VOICES + voice, "format": "wav", "rate": 1.0},
                   "workDir": os.path.join(work, "tts", "work"), "outDir": os.path.join(work, "tts", "out"),
                   "previewDir": os.path.join(work, "tts", "preview"), "target": {"kind": "estimate"}, "engines": {},
                   "limits": {"threads": 2, "memoryMb": 1024, "maxMegapixels": 100, "idleTimeoutSec": 60}}
            p = subprocess.run([os.path.join(app, "target", "debug", "worker-core.exe"), "tts", "run"], input=json.dumps(req) + "\n",
                               capture_output=True, text=True, encoding="utf-8")
            made = [json.loads(l) for l in p.stdout.splitlines() if l.startswith("{")]
            path = next((m["path"] for m in made if m.get("type") == "output"), None)
            if not path:
                raise SystemExit(f"no voice for line {i}: {p.stdout[-400:]} {p.stderr[-400:]}")
            shutil.copy(path, wav)
        out.append((wav, duration(wav)))
    return out


def prepare():
    shutil.rmtree(files_dir, ignore_errors=True)
    os.makedirs(files_dir)
    src = os.path.join(app, "testdata", "in")
    copies = {
        "pdf": [("poze.pdf", "Q3 report.pdf"), (os.path.join("documente ăîșț", "contract închiriere.pdf"), "lease 2026.pdf")],
        "heic": [("iphone.heic", "IMG_2041.heic"), ("drept.heic", "IMG_2042.heic")],
        "video": [("clip.mp4", "Team call.mp4")],
    }
    files = {}
    for kind, pairs in copies.items():
        files[kind] = []
        for a, b in pairs:
            shutil.copy(os.path.join(src, a), os.path.join(files_dir, b))
            files[kind].append(os.path.join(files_dir, b))
    return files


def record(theme, plan):
    js(f"const d=window.__dev; d.app.settings.theme={json.dumps(theme)}; d.applySettings(); return 1")
    time.sleep(0.5)
    raw = os.path.join(work, f"raw-{theme}.mkv")
    stop = os.path.join(work, "stop")
    if os.path.exists(stop):
        os.remove(stop)
    rec = subprocess.Popen(["powershell", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", os.path.join(app, "scripts", "demo", "record.ps1"), raw, stop, "30"],
                           stdout=subprocess.PIPE, text=True)
    time.sleep(1.0)
    scene = open(os.path.join(here, "scene.js"), encoding="utf-8").read().replace("__PLAN__", json.dumps({**plan, "theme": theme}))
    result = js(scene, timeout=240)
    time.sleep(0.3)
    open(stop, "w").close()
    log = rec.communicate()[0].strip()
    print(theme, log)
    start = int(log.rsplit("start ", 1)[1].split()[0])
    # Never write a broken film over the site's: the window must have been captured whole.
    size = log.split(" frames ")[0].split()[-1]
    w, h = (int(x) for x in size.split("x"))
    if w < 900 or h < 600:
        raise SystemExit(f"the window was captured at {size} (minimized or hidden?): nothing written")
    return raw, start, result


def encode(theme, raw, start, result, voices):
    os.makedirs(site, exist_ok=True)
    # The recording starts a moment before the tour: cut to its first scene, less a breath.
    cut = max(0.0, (result["marks"][0]["at"] - start) / 1000 - 1.0)
    length = (result["end"] - start) / 1000 - cut
    at = [(m["at"] - start) / 1000 - cut + 0.15 for m in result["marks"]]
    inputs, filters = [], []
    for i, ((wav, _), t) in enumerate(zip(voices, at)):
        inputs += ["-i", wav]
        filters.append(f"[{i + 1}:a]aresample=48000,adelay={int(t * 1000)}:all=1[v{i}]")
    mix = "".join(f"[v{i}]" for i in range(len(voices))) + f"amix=inputs={len(voices)}:normalize=0,loudnorm=I=-16:TP=-1.5,apad[a]"
    # Torn captures (the window caught half redrawn) are left out: the frame before them
    # stays on screen in their place, and the timing doesn't move.
    bad = frames.torn(raw)
    print(theme, "torn frames left out:", len(bad))
    keep = "select='not(" + "+".join(f"eq(n\\,{i})" for i in bad) + ")'," if bad else ""
    video = (f"[0:v]{keep}fps=30,trim=start={cut:.3f}:duration={length:.3f},setpts=PTS-STARTPTS,"
             "crop=trunc(iw/2)*2:trunc(ih/2)*2,format=yuv420p[vid]")
    graph = ";".join([video] + filters + [mix])
    base = ["ffmpeg", "-loglevel", "error", "-y", "-i", raw] + inputs + [
        "-filter_complex", graph, "-map", "[vid]", "-map", "[a]", "-t", f"{length:.3f}"]
    webm = os.path.join(site, f"tour-{theme}.webm")
    mp4 = os.path.join(site, f"tour-{theme}.mp4")
    poster = os.path.join(site, f"tour-{theme}.webp")
    subprocess.run(base + ["-c:v", "libvpx-vp9", "-b:v", "0", "-crf", "36", "-row-mt", "1", "-deadline", "good", "-cpu-used", "2",
                           "-c:a", "libopus", "-b:a", "64k", webm], check=True)
    subprocess.run(base + ["-c:v", "libx264", "-crf", "24", "-preset", "slow", "-c:a", "aac", "-b:a", "96k", "-movflags", "+faststart", mp4], check=True)
    # The still shown before it plays: from the cleaned picture too (taken straight from the
    # recording it was once a torn capture).
    still = (f"[0:v]{keep}fps=30,trim=start={cut + 0.8:.3f},setpts=PTS-STARTPTS,crop=trunc(iw/2)*2:trunc(ih/2)*2[p]")
    subprocess.run(["ffmpeg", "-loglevel", "error", "-y", "-i", raw, "-filter_complex", still, "-map", "[p]", "-frames:v", "1",
                    "-c:v", "libwebp", "-quality", "90", poster], check=True)
    for p in (webm, mp4, poster):
        print(os.path.basename(p), os.path.getsize(p) // 1024, "KB")
    left = frames.torn(mp4)
    print(os.path.basename(mp4), "torn frames still in it:", len(left), left[:10])
    print("length", round(length, 1), "s")


def main():
    args = sys.argv[1:]
    voice = "kokoro:af_heart"
    if "--voice" in args:
        i = args.index("--voice")
        voice = args[i + 1]
        del args[i:i + 2]
    again = "--reencode" in args  # from the last recordings, without recording again
    args = [a for a in args if a != "--reencode"]
    themes = args or ["light", "dark"]
    lines = json.load(open(os.path.join(here, "narration.json"), encoding="utf-8"))
    os.makedirs(work, exist_ok=True)
    voices = voice_lines(lines, voice)
    if again:
        for theme in themes:
            take = json.load(open(os.path.join(work, f"take-{theme}.json"), encoding="utf-8"))
            encode(theme, os.path.join(work, f"raw-{theme}.mkv"), take["start"], take["result"], voices)
        return
    logo = "data:image/svg+xml;base64," + base64.b64encode(open(os.path.join(os.path.dirname(app), "website", "public", "icon.svg"), "rb").read()).decode()
    plan = {"scenes": {l["scene"]: int(dur * 1000) for l, (_, dur) in zip(lines, voices)}, "logo": logo}
    saved = js("const d=window.__dev; return {lastValues: await d.api.stateGet('lastValues'), history: await d.api.stateGet('history'), theme: (await d.api.stateGet('settings'))?.theme ?? 'system'}")
    try:
        for theme in themes:
            plan["files"] = prepare()
            raw, start, result = record(theme, plan)
            # What a later --reencode needs.
            json.dump({"start": start, "result": result}, open(os.path.join(work, f"take-{theme}.json"), "w", encoding="utf-8"))
            encode(theme, raw, start, result, voices)
    finally:
        js("const d=window.__dev; const s=%s; await d.api.stateSet('lastValues', s.lastValues ?? {}); await d.api.stateSet('history', s.history ?? []);"
           " d.app.lastValues = s.lastValues ?? {}; d.app.settings.theme = s.theme; d.applySettings(); d.app.pending = []; d.go({name:'home'}); return 1" % json.dumps(saved))
        shutil.rmtree(files_dir, ignore_errors=True)


main()
