"""Conformance test (contract v2, section 13.3) for every tool.

  python scripts/conformance.py            (needs: cargo build -p worker-core, office-worker built)

Checks for every tool:
  - stdout carries only JSON lines; `hello` first, `done` last;
  - every file gets fileStart → progress… → fileDone;
  - progress: starts at 0, has intermediate steps, never goes back, ends at 1 on success;
  - inputs are unchanged (hash, size, date) and outputs are only inside outDir;
  - a cancel stops the process within 3 seconds.
"""
import hashlib, json, os, shutil, subprocess, sys, threading, time

root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
IN = os.path.join(root, "testdata", "in")
OUT = os.path.join(root, "testdata", "out", "conformance")
CORE = [os.path.join(root, "target", "debug", "worker-core.exe")]
OFFICE = [os.path.join(root, "workers", "office-worker", "bin", "Release", "net48", "office-worker.exe")]
DOCS = os.path.join(IN, "documente ăîșț")

# Engines the app downloaded on this PC (LibreOffice, Python, the AI models…), newest version each.
ENGINES = {}
_lo = os.path.join(os.environ.get("LOCALAPPDATA", ""), "com.mqkit.app", "engines", "libreoffice")
_root = os.path.dirname(_lo)
for _name in (os.listdir(_root) if os.path.isdir(_root) else []):
    _vers = [v for v in os.listdir(os.path.join(_root, _name)) if not v.startswith(".")] if os.path.isdir(os.path.join(_root, _name)) else []
    if _vers:
        ENGINES[_name] = {"path": os.path.join(_root, _name, sorted(_vers)[-1])}
# The portable Python and PyMuPDF (pdf2docx's copy will do when PyMuPDF has no folder of its own).
for _name, _fallback in (("python", None), ("pymupdf", "pdf2docx")):
    for _cand in filter(None, (_name, _fallback)):
        _d = os.path.join(os.path.dirname(_lo), _cand)
        if os.path.isdir(_d) and os.listdir(_d):
            ENGINES[_name] = {"path": os.path.join(_d, sorted(os.listdir(_d))[-1])}
            break

# More engines from elsewhere (not installed by the app): CONFORMANCE_ENGINES="isnet=D:/x/isnet;onnxruntime=D:/x/ort".
for _pair in filter(None, os.environ.get("CONFORMANCE_ENGINES", "").split(";")):
    _k, _v = _pair.split("=", 1)
    ENGINES[_k] = {"path": _v}

# Skips that are the tool working as designed (a subtitle used for its video).
EXPECTED_SKIPS = {"used_with_video"}

CASES = [
    ("pdf-compress", CORE, ["poze.pdf"], {"mode": "preset", "level": "recommended"}),
    ("pdf-compress", CORE, ["poze.pdf"], {"mode": "targetSize", "targetSize": 300000}),
    ("pdf-to-word", OFFICE, [os.path.join(DOCS, "contract închiriere.pdf")], {"format": "docx"}),
    ("pdf-merge", CORE, ["poze.pdf", os.path.join(DOCS, "contract închiriere.pdf")], {"bookmarks": "perFile"}),
    ("images-to-pdf", CORE, ["peisaj ăîș.jpg", "alfa.png", "iphone.heic"], {"pageSize": "a4", "margin": 10}),
    ("word-to-pdf", OFFICE, [os.path.join(DOCS, "contract închiriere.docx")], {"quality": "print"}),
    ("pdf-split", CORE, ["poze.pdf"], {"mode": "everyN", "pagesPerFile": 2}),
    ("pdf-split", CORE, ["poze.pdf"], {"mode": "bySize", "maxSize": 1500000}),
    ("pdf-password", CORE, ["poze.pdf"], {"action": "protect", "openPassword": "Parolă#2026"}),
    ("pdf-password", CORE, ["poze.pdf"], {"action": "protect", "restrict": True, "allowPrint": "low", "encryption": "aes128"}),
    ("pdf-pages", CORE, ["poze.pdf"], {"rotation": 90, "applyTo": "odd", "deletePages": "z", "order": "reverse"}),
    ("pdf-to-images", CORE, ["poze.pdf"], {"format": "jpg", "dpi": 150, "pages": "all", "quality": 85}),
    ("image-compress", CORE, ["peisaj ăîș.jpg", "alfa.png"], {"mode": "quality", "quality": 75, "pngLossy": True}),
    ("image-compress", CORE, ["peisaj ăîș.jpg"], {"mode": "targetSize", "targetSize": 150000}),
    ("image-convert", CORE, ["alfa.png", "iphone.heic"], {"format": "webp", "quality": 80}),
    ("image-resize", CORE, ["peisaj ăîș.jpg", "alfa.png", "iphone.heic"], {"mode": "fill", "width": 500, "height": 500}),
    ("image-resize", CORE, ["anim.gif"], {"mode": "longEdge", "longEdge": 120, "format": "original"}),
    ("icons", CORE, ["alfa.png"], {"kind": "favicon"}),
    ("icons", CORE, ["peisaj ăîș.jpg"], {"kind": "ico", "sizes": "16, 32, 48, 256", "fit": "crop"}),
    ("heic-to-jpg", CORE, ["iphone.heic", "drept.heic"], {"format": "jpg", "quality": 90, "keepMetadata": True}),
    ("video-compress", CORE, ["clip.mp4"], {"mode": "quality", "quality": 5, "maxResolution": 240}),
    ("video-compress", CORE, ["clip.mp4"], {"mode": "targetSize", "targetSize": 400000}),
    ("video-convert", CORE, ["clip.mp4"], {"format": "mkv", "mode": "remux"}),
    ("video-convert", CORE, ["clip.mp4"], {"format": "webm", "mode": "auto", "quality": 4}),
    ("audio-convert", CORE, ["clip.mp4"], {"format": "flac"}),
    ("audio-convert", CORE, ["clip.mp4"], {"format": "opus", "quality": 48, "channels": "mono", "fadeOut": 2}),
    ("media-trim", CORE, ["clip.mp4"], {"start": "1", "end": "4"}),
    ("media-trim", CORE, ["clip.mp4"], {"action": "remove", "start": "2", "end": "5"}),
    ("video-gif", CORE, ["clip.mp4"], {"format": "gif", "width": 320, "fps": 10, "end": "3"}),
    ("video-gif", CORE, ["anim.gif"], {"format": "mp4"}),
    ("media-merge", CORE, ["clip.mp4", "clip.mp4"], {"method": "auto"}),
    ("media-merge", CORE, ["clip.mp4", "anim.gif"], {"method": "reencode"}),
    ("meta-strip", CORE, ["peisaj ăîș.jpg", "alfa.png", "poze.pdf", "clip.mp4", os.path.join(DOCS, "contract închiriere.docx")], {}),
    ("subtitles", CORE, ["subtitrare.srt"], {"operation": "repair"}),
    ("subtitles", CORE, ["subtitrare.srt"], {"operation": "convert", "format": "vtt"}),
    ("subtitles", CORE, ["clip.mp4", "subtitrare.srt"], {"operation": "attach", "attachMode": "track"}),
    ("html-to-pdf", CORE, ["pagina.html"], {"pageSize": "a4", "margin": 15}),
    ("markdown", CORE, ["notite.md"], {"format": "pdf", "toc": True}),
    ("markdown", CORE, ["notite.md"], {"format": "html", "theme": "compact"}),
    ("office-to-pdf", CORE, ["buget.xlsx", "poze.pptx"], {"quality": "screen"}),
    ("tts", CORE, ["citeste.txt"], {"format": "wav"}),
    ("ocr", CORE, ["scan.pdf", "scan-pagina.png"], {"output": "pdf"}),
    ("ocr", CORE, ["scan.pdf"], {"output": "txt"}),
    ("pdf-stamp", CORE, ["poze.pdf"], {"kind": "numbers", "numberFormat": "pageOf", "position": "bottomRight"}),
    ("pdf-stamp", CORE, ["poze.pdf"], {"kind": "watermark", "markType": "text", "text": "CONFIDENȚIAL", "layout": "tiled"}),
    ("tabular", CORE, ["buget.xlsx", "vechi.csv"], {"format": "json"}),
    ("tabular", CORE, ["vechi.csv"], {"format": "xlsx"}),
    ("avif", CORE, ["peisaj ăîș.jpg", "alfa.png"], {"format": "avif", "quality": 60, "speed": 8}),
    ("avif", CORE, ["alfa.avif"], {"format": "png"}),
    ("pdf-to-pptx", CORE, ["poze.pdf", "scan.pdf"], {"slideSize": "16:9", "fit": "fit"}),
    ("svg", CORE, ["desen.svg", "alfa.png"], {"format": "png", "size": 2, "detail": "simple"}),
    ("svg", CORE, ["desen.svg"], {"format": "pdf"}),
    ("svg", CORE, ["desen.emf", "desen.ai"], {"format": "png", "size": 1}),
    ("svg", CORE, ["desen.emf", "desen.ai"], {"format": "pdf"}),
    ("archives", CORE, ["proiect-v2.zip", "scrieri.tar.gz"], {"operation": "extract"}),
    ("archives", CORE, ["peisaj ăîș.jpg", "alfa.png", "citeste.txt"], {"operation": "create", "format": "7z"}),
    ("archives", CORE, ["proiect-v2.zip"], {"operation": "convert", "format": "tar.gz"}),
    ("fonts", CORE, ["AdobeClean-Regular.otf", "FiraCode-Regular.woff2", "cambria.ttc"], {"format": "woff2", "keep": "scripts", "scripts": ["Latn"]}),
    ("fonts", CORE, ["CascadiaCode.ttf", "FiraCode-Regular.woff2"], {"format": "webkit", "keep": "text", "text": "Salut 2026"}),
    ("fonts", CORE, ["FiraCode-Regular.woff2"], {"format": "desktop"}),
    ("pdf-to-excel", CORE, ["extras.pdf", "buget-tabel.pdf"], {"sheets": "table", "join": True}),
    ("pdf-to-excel", CORE, ["extras.pdf"], {"sheets": "page", "format": "csv"}),
    ("remove-background", CORE, ["alfa.png", "logo.png"], {"background": "transparent", "crop": True}),
    ("remove-background", CORE, ["tema.jpg"], {"background": "blur", "format": "jpg"}),
    ("ai-upscale", CORE, ["logo.png", "alfa.png"], {"scale": 2}),
    ("transcribe", CORE, ["vorbire.wav"], {"model": "fast", "format": "srt"}),
    ("mail", CORE, ["scrisoare.eml", "msg-unicode.msg", "posta.pst", "cutie.mbox"], {"format": "pdf"}),
    ("mail", CORE, ["scrisoare.eml", "scurt.eml"], {"format": "mbox"}),
    ("mail", CORE, ["posta.pst"], {"format": "eml"}),
    ("raw", CORE, ["raw/compressed_14_bit.NEF", "raw/canon-r10.CR3"], {"source": "camera", "format": "jpg"}),
    ("raw", CORE, ["raw/phone.dng"], {"source": "develop", "format": "tif"}),
    ("design-files", CORE, ["psd/layer_comps.psd", "psd/16bit5x5.psb", "psd/cmyk-gray-ramp.psd", "desen.ai", "desen.eps"], {"format": "png"}),
    ("design-files", CORE, ["psd/gray1.psd"], {"format": "jpg"}),
    ("ebooks", CORE, ["carti/alice.azw3", "carti/amintiri.fb2", "notite.md"], {"format": "epub"}),
    ("ebooks", CORE, ["carti/alice.epub"], {"format": "pdf", "page": "a5", "fontSize": "large"}),
    ("cad-3d", CORE, ["3d/teapot.stl", "3d/suzanne.fbx", "3d/pins.glb", "3d/featuretype.3MF", "3d/sphere.ply"], {"model": "glb"}),
    ("cad-3d", CORE, ["3d/sliding-base.dxf", "3d/spline.DXF"], {"drawing": "pdf"}),
    ("video-to-mp3", CORE, ["clip.mp4"], {"format": "mp3", "bitrate": 192}),
]


def fingerprint(p):
    st = os.stat(p)
    return hashlib.sha256(open(p, "rb").read()).hexdigest(), st.st_size, st.st_mtime_ns


def snapshot_dir(d):
    return sorted(os.listdir(d))


def run_case(n, tool, exe, files, options, cancel_after=None):
    paths = [f if os.path.isabs(f) else os.path.join(IN, f) for f in files]
    work = os.path.join(OUT, f"{n:02d}-{tool}")
    shutil.rmtree(work, ignore_errors=True)
    req = {
        "cmd": "run", "protocol": 2, "jobId": f"t{n}", "locale": "ro-RO",
        "files": [{"id": f"f{i + 1}", "path": p, "analysis": {}, "item": {}} for i, p in enumerate(paths)],
        "options": options, "workDir": os.path.join(work, "work"), "outDir": os.path.join(work, "out"),
        "engines": ENGINES, "limits": {"threads": 4, "memoryMb": 2048, "maxMegapixels": 250, "idleTimeoutSec": 120},
        "secrets": {"files": {}, "fields": {}},
    }
    # analysis first, like the parent does (combined tools and estimates rely on it)
    a = subprocess.run(exe + [tool, "analyze"], input=(json.dumps({"cmd": "analyze", "id": "a", "level": "fast", "files": req["files"]}) + "\n").encode(), capture_output=True)
    for line in a.stdout.decode("utf-8").splitlines():
        m = json.loads(line)
        if m.get("type") == "analysis":
            for f in req["files"]:
                if f["id"] == m["file"]:
                    f["analysis"] = m.get("props", {})
    before = {p: fingerprint(p) for p in paths}
    dirs_before = {os.path.dirname(p): snapshot_dir(os.path.dirname(p)) for p in paths}
    problems = []
    t0 = time.time()
    proc = subprocess.Popen(exe + [tool, "run"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    proc.stdin.write((json.dumps(req, ensure_ascii=False) + "\n").encode("utf-8"))
    proc.stdin.flush()
    if cancel_after is not None:
        def cancel():
            time.sleep(cancel_after)
            try:
                proc.stdin.write(b'{"cmd":"cancel"}\n')
                proc.stdin.flush()
            except OSError:
                pass
        threading.Thread(target=cancel, daemon=True).start()
    msgs = []
    for raw in proc.stdout:
        try:
            msgs.append(json.loads(raw))
        except ValueError:
            problems.append(f"non-JSON on stdout: {raw[:80]!r}")
    try:
        proc.wait(timeout=5 if cancel_after is not None else 600)
    except subprocess.TimeoutExpired:
        proc.kill()
        problems.append("process did not exit")
    elapsed = time.time() - t0
    try:
        proc.stdin.close()
    except OSError:
        pass

    if not msgs or msgs[0].get("type") != "hello":
        problems.append("first message is not hello")
    if not msgs or msgs[-1].get("type") not in ("done", "error"):
        problems.append("last message is not done")
    units = {}
    for m in msgs:
        f = m.get("file")
        t = m.get("type")
        if t == "fileStart":
            units[f] = {"progress": [], "done": None, "outputs": []}
        elif t == "progress" and f in units:
            units[f]["progress"].append(m["fraction"])
        elif t == "output" and f in units:
            units[f]["outputs"].append(m["path"])
            if not os.path.abspath(m["path"]).startswith(os.path.abspath(req["outDir"])):
                problems.append(f"output outside outDir: {m['path']}")
        elif t == "fileDone" and f in units:
            units[f]["done"] = m
    if not units:
        problems.append("no fileStart")
    for f, u in units.items():
        pr = u["progress"]
        st = (u["done"] or {}).get("status")
        if u["done"] is None:
            problems.append(f"{f}: no fileDone")
            continue
        if not pr:
            problems.append(f"{f}: no progress at all")
            continue
        if pr[0] > 0.05:
            problems.append(f"{f}: first progress is {pr[0]}, not ~0")
        if any(b < a - 1e-9 for a, b in zip(pr, pr[1:])):
            problems.append(f"{f}: progress goes back {pr}")
        if st == "ok":
            if pr[-1] < 0.999:
                problems.append(f"{f}: last progress {pr[-1]}, not 1")
            if len({round(x, 3) for x in pr if 0 < x < 1}) < 1:
                problems.append(f"{f}: no intermediate progress {pr}")
            if not u["outputs"]:
                problems.append(f"{f}: ok without output")
        elif st == "skipped" and u["done"].get("code") in EXPECTED_SKIPS:
            pass
        elif cancel_after is None:
            problems.append(f"{f}: status {st} {u['done'].get('code')}")
    for p, fp in before.items():
        if fingerprint(p) != fp:
            problems.append(f"input changed: {p}")
    for d, listing in dirs_before.items():
        if snapshot_dir(d) != listing:
            problems.append(f"something was written next to the inputs in {d}")
    if cancel_after is not None:
        if elapsed - cancel_after > 3.5:
            problems.append(f"cancel took {elapsed - cancel_after:.1f}s")
        if msgs and not msgs[-1].get("cancelled") and msgs[-1].get("summary", {}).get("ok", 0) == len(paths):
            problems.append("finished before the cancel arrived (test too short)")
    counts = {f: len(u["progress"]) for f, u in units.items()}
    return problems, elapsed, counts


def main():
    failed = 0
    for n, (tool, exe, files, options) in enumerate(CASES, 1):
        if not os.path.exists(exe[0]):
            print(f"SKIP {tool}: {exe[0]} missing")
            continue
        problems, elapsed, counts = run_case(n, tool, exe, files, options)
        status = "ok  " if not problems else "FAIL"
        failed += bool(problems)
        print(f"{status} {tool:15} {elapsed:5.1f}s  progress messages per file: {counts}")
        for p in problems:
            print("      -", p)
    # cancel: a slow job (300 dpi pages), cancelled after 0.5 s
    problems, elapsed, _ = run_case(99, "pdf-to-images", CORE, ["poze.pdf"] * 3, {"format": "png", "dpi": 300}, cancel_after=0.5)
    print(("ok   " if not problems else "FAIL ") + f"cancel          {elapsed:5.1f}s")
    for p in problems:
        print("      -", p)
    failed += bool(problems)
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
