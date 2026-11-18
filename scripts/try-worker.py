"""Dev helper: runs a worker-core plugin command and prints the JSON lines.

  python scripts/try-worker.py <plugin> <analyze|run|preview|probe> <file>... [--opt key=value]... [--item key=value]...
                               (a value "@path" is read from that file)
                               [--worker path] [--out dir] [--password p] [--base folder] [--engine name=folder]
"""
import json, os, subprocess, sys, time

root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
args = sys.argv[1:]
plugin, cmd = args[0], args[1]
files, opts, items, password, base, extra_engines = [], {}, {}, None, None, {}
worker = [os.path.join(root, "target", "debug", "worker-core.exe"), plugin]
out = os.path.join(root, "testdata", "out", plugin)
i = 2
while i < len(args):
    a = args[i]
    if a in ("--opt", "--item"):
        k, v = args[i + 1].split("=", 1)
        if v.startswith("@"):
            # @file: the value is the file's text (a long data: URL, a JSON list…).
            v = open(v[1:], encoding="utf-8").read().strip()
        try:
            v = json.loads(v)
        except ValueError:
            pass
        (opts if a == "--opt" else items)[k] = v
        i += 2
    elif a == "--engine":
        k, v = args[i + 1].split("=", 1)
        extra_engines[k] = {"path": os.path.abspath(v)}
        i += 2
    elif a == "--base":
        base = os.path.abspath(args[i + 1])
        i += 2
    elif a == "--password":
        password = args[i + 1]
        i += 2
    elif a == "--worker":
        worker = args[i + 1].split(" ")
        i += 2
    elif a == "--out":
        out = os.path.abspath(args[i + 1])
        i += 2
    else:
        files.append(os.path.abspath(a))
        i += 1

req = {"cmd": cmd, "id": "t1", "level": "fast", "locale": "ro-RO",
       "files": [{"id": f"f{n + 1}", "path": p, "item": items, **({"base": base} if base else {})} for n, p in enumerate(files)],
       "options": opts, "workDir": os.path.join(out, "work"), "outDir": os.path.join(out, "out"),
       "previewDir": os.path.join(out, "preview"), "target": {"kind": os.environ.get("TRY_TARGET", "estimate"), "maxPx": 900},
       "engines": {"ffmpeg": {"path": os.path.dirname(subprocess.run(["where", "ffmpeg"], capture_output=True, text=True).stdout.splitlines()[0]) if subprocess.run(["where", "ffmpeg"], capture_output=True).returncode == 0 else ""}},
       "limits": {"threads": 4, "memoryMb": 2048, "maxMegapixels": 250, "idleTimeoutSec": 120}}
# The LibreOffice the app downloaded, if any.
lo_root = os.path.join(os.environ.get("LOCALAPPDATA", ""), "com.mqkit.app", "engines", "libreoffice")
if os.path.isdir(lo_root):
    versions = sorted(os.listdir(lo_root))
    if versions:
        req["engines"]["libreoffice"] = {"path": os.path.join(lo_root, versions[-1])}
# The portable Python and PyMuPDF (pdf2docx's copy will do when PyMuPDF has no folder of its own).
eng_root = os.path.join(os.environ.get("LOCALAPPDATA", ""), "com.mqkit.app", "engines")
for name, fallback in (("python", None), ("pymupdf", "pdf2docx")):
    for cand in (name, fallback):
        d = os.path.join(eng_root, cand or "")
        if cand and os.path.isdir(d) and os.listdir(d):
            req["engines"][name] = {"path": os.path.join(d, sorted(os.listdir(d))[-1])}
            break
req["engines"].update(extra_engines)
if password is not None:
    req["secrets"] = {"files": {f"f{n + 1}": password for n in range(len(files))}}
t0 = time.time()
p = subprocess.Popen(worker + [cmd], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
p.stdin.write((json.dumps(req) + "\n").encode("utf-8"))
p.stdin.flush()
for line in p.stdout:
    m = json.loads(line)
    if m.get("type") == "progress":
        continue
    print(json.dumps(m, ensure_ascii=False))
p.stdin.close()
p.wait()
err = p.stderr.read().decode("utf-8", "replace").strip()
if err:
    print("stderr:", err[-2000:])
print(f"exit {p.returncode} in {time.time() - t0:.2f}s")
