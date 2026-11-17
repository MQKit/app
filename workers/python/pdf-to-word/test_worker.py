"""End-to-end test of pdf2docx_worker.py on the pinned portable Python.

  python workers/python/pdf-to-word/test_worker.py [--in <testdata\\in>]

1. Downloads the embeddable CPython and the wheels pinned in src-tauri/engines.python.toml
   into testdata/out/pyengine/ (cached), checking size and SHA-256 like the app does.
2. Extracts Python whole (its python3XX._pth kept as shipped) and every wheel into
   pdf2docx/<version>/site-packages (minus each wheel's `skip` entries). With --compile, also
   precompiles the packages (python -m compileall), as a possible install step.
3. Runs the worker with `python.exe -I -X utf8 pdf2docx_worker.py <cmd>` and
   CONVERTOR_PYPATH, and checks the protocol (hello first, JSON only, progress, outputs,
   inputs unchanged, passwords, cancel within 3 s).
"""
import hashlib, json, os, re, shutil, subprocess, sys, threading, time, tomllib, urllib.request, zipfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(os.path.dirname(HERE)))
WORKER = os.path.join(HERE, "pdf2docx_worker.py")
TOML = os.path.join(ROOT, "src-tauri", "engines.python.toml")
BASE = os.path.join(ROOT, "testdata", "out", "pyengine")
IN = os.path.join(ROOT, "testdata", "in")
if "--in" in sys.argv:
    IN = os.path.abspath(sys.argv[sys.argv.index("--in") + 1])
elif not os.path.isdir(IN):
    IN = os.path.join(os.path.dirname(ROOT), "app", "testdata", "in")
COMPILE = "--compile" in sys.argv
CONTRACT = os.path.join(IN, "documente ăîșț", "contract închiriere.pdf")
POZE = os.path.join(IN, "poze.pdf")

failures = []


def check(cond, what):
    print(("  ok   " if cond else "  FAIL ") + what)
    if not cond:
        failures.append(what)
    return cond


def fetch(url, size, sha256, dest):
    """Downloads `url` to `dest` unless it is already there; checks size and SHA-256."""
    def good():
        if not os.path.isfile(dest) or os.path.getsize(dest) != size:
            return False
        h = hashlib.sha256()
        with open(dest, "rb") as fh:
            for chunk in iter(lambda: fh.read(1 << 20), b""):
                h.update(chunk)
        return h.hexdigest().lower() == sha256.lower()
    if good():
        return
    if not url.startswith("https://"):
        raise SystemExit(f"refusing non-HTTPS url {url}")
    os.makedirs(os.path.dirname(dest), exist_ok=True)
    tmp = dest + ".part"
    h, n = hashlib.sha256(), 0
    req = urllib.request.Request(url, headers={"User-Agent": "Convertor-engine-downloader"})
    with urllib.request.urlopen(req, timeout=60) as r, open(tmp, "wb") as fh:
        while True:
            chunk = r.read(256 * 1024)
            if not chunk:
                break
            n += len(chunk)
            if n > size:
                break
            h.update(chunk)
            fh.write(chunk)
    if n != size or h.hexdigest().lower() != sha256.lower():
        os.remove(tmp)
        raise SystemExit(f"hash_mismatch for {url}: {n} bytes, sha256 {h.hexdigest()}")
    os.replace(tmp, dest)


def safe_extract(zip_path, dest, skip=()):
    """Extracts everything except the `skip` entries (each must exist in the archive)."""
    with zipfile.ZipFile(zip_path) as z:
        names = [i.filename for i in z.infolist()]
        for name in names:
            n = name.replace("\\", "/")
            if n.startswith("/") or ".." in n.split("/") or ":" in n:
                raise SystemExit(f"unsafe path in {zip_path}: {name}")
        for s in skip:
            check(s in names, f"skip entry {s} is in {os.path.basename(zip_path)}")
        z.extractall(dest, [n for n in names if n not in skip])


def install():
    cat = tomllib.load(open(TOML, "rb"))
    py, p2d = cat["python"], cat["pdf2docx"]
    dl = os.path.join(BASE, "downloads")
    t0 = time.time()
    fetch(py["url"], py["size"], py["sha256"], os.path.join(dl, os.path.basename(py["url"])))
    for w in p2d["wheels"]:
        fetch(w["url"], w["size"], w["sha256"], os.path.join(dl, w["name"]))
    print(f"downloads verified in {time.time() - t0:.1f}s "
          f"({(py['size'] + sum(w['size'] for w in p2d['wheels'])) / 1e6:.1f} MB)")
    pydir = os.path.join(BASE, "python", py["version"])
    sp = os.path.join(BASE, "pdf2docx", p2d["version"], "site-packages")
    stamp = os.path.join(BASE, "installed.json")
    want = json.dumps([py["sha256"]] + [[w["sha256"], w.get("skip", [])] for w in p2d["wheels"]] + [COMPILE])
    if not (os.path.isfile(stamp) and open(stamp).read() == want):
        shutil.rmtree(os.path.join(BASE, "python"), ignore_errors=True)
        shutil.rmtree(os.path.join(BASE, "pdf2docx"), ignore_errors=True)
        safe_extract(os.path.join(dl, os.path.basename(py["url"])), pydir)
        for w in p2d["wheels"]:
            safe_extract(os.path.join(dl, w["name"]), sp, w.get("skip", []))
        if COMPILE:
            # optional install step: precompiled bytecode, ~0.6 s faster per start
            subprocess.run([os.path.join(pydir, "python.exe"), "-I", "-m", "compileall", "-q", "-j0", sp], check=True,
                           stdout=subprocess.DEVNULL)
        open(stamp, "w").write(want)
    for f in py["files"]:
        check(os.path.isfile(os.path.join(pydir, f)), f"python has {f}")
    for f in p2d["files"]:
        check(os.path.isfile(os.path.join(BASE, "pdf2docx", p2d["version"], f)), f"pdf2docx has {f}")
    pth = [f for f in os.listdir(pydir) if f.endswith("._pth")]
    check(len(pth) == 1, f"embeddable ._pth kept as shipped: {pth}")
    if pth:
        print("       " + " | ".join(l for l in open(os.path.join(pydir, pth[0])).read().splitlines() if l.strip()))
    return os.path.join(pydir, "python.exe"), sp, dir_size(pydir), dir_size(sp)


def dir_size(d):
    return sum(os.path.getsize(os.path.join(r, f)) for r, _, fs in os.walk(d) for f in fs)


def fingerprint(p):
    st = os.stat(p)
    return hashlib.sha256(open(p, "rb").read()).hexdigest(), st.st_size, st.st_mtime_ns


def run_worker(python, sp, cmd, req, cancel_after=None, cancel_msg=b'{"cmd":"cancel"}\n'):
    env = {k: v for k, v in os.environ.items() if not k.upper().startswith("PYTHON")}
    env["CONVERTOR_PYPATH"] = sp
    env["PYTHONUTF8"] = "1"
    if req.get("workDir"):
        os.makedirs(req["workDir"], exist_ok=True)
        env["TEMP"] = env["TMP"] = req["workDir"]
    t0 = time.time()
    p = subprocess.Popen([python, "-I", "-X", "utf8", WORKER, cmd], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                         stderr=subprocess.PIPE, env=env, cwd=BASE)
    p.stdin.write((json.dumps(req, ensure_ascii=False) + "\n").encode("utf-8"))
    p.stdin.flush()
    cancel_at = [None]
    if cancel_after is not None:
        def cancel():
            time.sleep(cancel_after)
            cancel_at[0] = time.time()
            try:
                p.stdin.write(cancel_msg)
                p.stdin.flush()
            except OSError:
                pass
        threading.Thread(target=cancel, daemon=True).start()
    err = []
    threading.Thread(target=lambda: err.append(p.stderr.read()), daemon=True).start()
    msgs, bad = [], []
    for raw in p.stdout:
        try:
            msgs.append((time.time() - t0, json.loads(raw.decode("utf-8"))))
        except ValueError:
            bad.append(raw)
    try:
        p.wait(timeout=600)
    except subprocess.TimeoutExpired:
        p.kill()
    end = time.time()
    try:
        p.stdin.close()
    except OSError:
        pass
    time.sleep(0.1)
    stderr = (err[0] if err else b"").decode("utf-8", "replace")
    return {"msgs": [m for _, m in msgs], "times": [t for t, _ in msgs], "bad": bad, "code": p.returncode,
            "elapsed": end - t0, "stderr": stderr, "cancel_lag": (end - cancel_at[0]) if cancel_at[0] else None}


def base_checks(r, label):
    m = r["msgs"]
    check(not r["bad"], f"{label}: only JSON on stdout" + (f" (got {r['bad'][:2]})" if r["bad"] else ""))
    check(bool(m) and m[0] == {"type": "hello", "protocol": 2, "plugin": "pdf-to-word", "pluginVersion": "0.1.0"},
          f"{label}: hello first")
    check(bool(m) and m[-1].get("type") == "done", f"{label}: done last")
    check(r["code"] == 0, f"{label}: exit code {r['code']}")


def docx_text(path):
    with zipfile.ZipFile(path) as z:
        xml = z.read("word/document.xml").decode("utf-8")
    return "".join(re.findall(r"<w:t(?:\s[^>]*)?>([^<]*)</w:t>", xml))


def job(name, files, options=None, secrets=None):
    d = os.path.join(BASE, "jobs", name)
    shutil.rmtree(d, ignore_errors=True)
    return {"cmd": "run", "protocol": 2, "jobId": name, "locale": "ro-RO",
            "files": [{"id": f"f{i + 1}", "path": p, "analysis": {}, "item": {}} for i, p in enumerate(files)],
            "options": options or {"pages": "all"}, "outDir": os.path.join(d, "out"), "workDir": os.path.join(d, "work"),
            "engines": {}, "limits": {"threads": 2, "memoryMb": 2048, "maxMegapixels": 250, "idleTimeoutSec": 120},
            "secrets": {"files": secrets or {}, "fields": {}}}


def run_checks(r, req, label):
    units = {}
    for m in r["msgs"]:
        t, f = m.get("type"), m.get("file")
        if t == "fileStart":
            units[f] = {"progress": [], "outputs": [], "done": None}
        elif t == "progress" and f in units:
            units[f]["progress"].append(m["fraction"])
        elif t == "output" and f in units:
            units[f]["outputs"].append(m)
        elif t == "fileDone" and f in units:
            units[f]["done"] = m
    for f, u in units.items():
        pr = u["progress"]
        check(u["done"] is not None and u["done"]["status"] == "ok", f"{label} {f}: fileDone ok {u['done']}")
        check(bool(pr) and pr[0] <= 0.05, f"{label} {f}: progress starts at 0")
        check(all(b >= a for a, b in zip(pr, pr[1:])), f"{label} {f}: progress never goes back")
        check(bool(pr) and pr[-1] == 1.0, f"{label} {f}: progress ends at 1")
        mid = sorted({x for x in pr if 0 < x < 1})
        check(len(mid) >= 3, f"{label} {f}: {len(mid)} intermediate values {mid[:12]}")
        for o in u["outputs"]:
            p = o["path"]
            check(os.path.abspath(p).startswith(os.path.abspath(req["outDir"]) + os.sep), f"{label} {f}: output inside outDir")
            check(os.path.isfile(p) and o["stats"]["outputBytes"] == os.path.getsize(p), f"{label} {f}: output exists, size matches")
            with zipfile.ZipFile(p) as z:
                check("word/document.xml" in z.namelist() and z.testzip() is None, f"{label} {f}: docx is a valid zip with word/document.xml")
    return units


def main():
    if not os.path.isfile(CONTRACT) or not os.path.isfile(POZE):
        raise SystemExit(f"test files missing under {IN}")
    print("install")
    python, sp, py_bytes, sp_bytes = install()
    print(f"  installed: python {py_bytes / 1e6:.1f} MB + site-packages {sp_bytes / 1e6:.1f} MB = {(py_bytes + sp_bytes) / 1e6:.1f} MB")

    print("imports on the embeddable Python (-I, ._pth as shipped)")
    env = {"CONVERTOR_PYPATH": sp, "SYSTEMROOT": os.environ.get("SYSTEMROOT", "")}
    t0 = time.time()
    r = subprocess.run([python, "-I", "-B", "-X", "utf8", "-c",
                        "import os,sys;sys.path.insert(0,os.environ['CONVERTOR_PYPATH']);"
                        "import pdf2docx,pymupdf,cv2,numpy,docx,lxml;"
                        "print(sys.version.split()[0],pymupdf.VersionBind,cv2.__version__,numpy.__version__)"],
                       capture_output=True, text=True, env=env)
    check(r.returncode == 0, f"import pdf2docx, pymupdf, cv2, numpy, docx, lxml: {r.stdout.strip()} {r.stderr.strip()[-300:]} "
                             f"({time.time() - t0:.2f}s)")

    inputs = [CONTRACT, POZE]
    before = {p: fingerprint(p) for p in inputs}
    listing = {os.path.dirname(p): sorted(os.listdir(os.path.dirname(p))) for p in inputs}

    print("analyze")
    r = run_worker(python, sp, "analyze", {"cmd": "analyze", "id": "a1", "level": "fast", "secrets": {"files": {}},
                                           "files": [{"id": "f1", "path": CONTRACT}, {"id": "f2", "path": POZE},
                                                     {"id": "f3", "path": os.path.join(IN, "alfa.png")}]})
    base_checks(r, "analyze")
    an = {m["file"]: m for m in r["msgs"] if m.get("type") == "analysis"}
    check(an.get("f1", {}).get("status") == "ok" and an["f1"]["props"].get("pageCount", 0) >= 1, f"analyze contract: {an.get('f1')}")
    check(an.get("f2", {}).get("status") == "ok", f"analyze poze: {an.get('f2')}")
    check(an.get("f3", {}).get("status") == "unsupported", f"analyze png: {an.get('f3')}")
    check(r["msgs"][-1] == {"type": "done", "id": "a1"}, "analyze ends with done + id")
    print(f"  analyze of 3 files: {r['elapsed']:.2f}s")

    print("run: contract (1 file, diacritics in path)")
    req = job("contract", [CONTRACT])
    r = run_worker(python, sp, "run", req)
    base_checks(r, "run contract")
    units = run_checks(r, req, "run contract")
    out = next((m for m in r["msgs"] if m.get("type") == "output"), None)
    if out:
        txt = docx_text(out["path"])
        check("închiriere" in txt, "docx contains 'închiriere'")
    pages = out["vars"]["pages"] if out else "?"
    t_contract = r["elapsed"]
    print(f"  contract: {pages} page(s), {t_contract:.2f}s including Python startup")
    sample = r["msgs"]
    done = r["msgs"][-1]
    check(done.get("summary") == {"ok": 1, "skipped": 0, "failed": 0}, f"summary {done}")

    print("run: 1 page of poze.pdf (timing)")
    req = job("poze-p1", [POZE], {"pages": "1"})
    r = run_worker(python, sp, "run", req)
    base_checks(r, "run poze p1")
    run_checks(r, req, "run poze p1")
    out = next((m for m in r["msgs"] if m.get("type") == "output"), None)
    check(bool(out) and out["vars"]["pages"] == 1, f"pages=1 → vars.pages {out and out['vars']}")
    t_one = r["elapsed"]
    print(f"  1-page conversion incl. Python startup: {t_one:.2f}s")

    print("run: poze.pdf, all pages")
    req = job("poze", [POZE])
    r = run_worker(python, sp, "run", req)
    base_checks(r, "run poze")
    run_checks(r, req, "run poze")
    t_poze = r["elapsed"]
    print(f"  poze.pdf all pages: {t_poze:.2f}s, {sum(1 for m in r['msgs'] if m.get('type') == 'progress')} progress messages")
    poze_msgs = r["msgs"]

    print("corrupt PDF (a PDF header, then garbage)")
    bad = os.path.join(BASE, "jobs", "stricat ș.pdf")
    os.makedirs(os.path.dirname(bad), exist_ok=True)
    open(bad, "wb").write(b"%PDF-1.7\n" + bytes((i * 7) % 251 for i in range(20000)))
    r = run_worker(python, sp, "analyze", {"cmd": "analyze", "id": "a4", "files": [{"id": "f1", "path": bad}], "secrets": {"files": {}}})
    base_checks(r, "analyze corrupt")
    a = next((m for m in r["msgs"] if m.get("type") == "analysis"), {})
    check(a.get("status") == "corrupt" and a.get("code") == "corrupt_file", f"analyze corrupt: {a}")
    req = job("corrupt", [bad])
    r = run_worker(python, sp, "run", req)
    base_checks(r, "run corrupt")
    fd = next((m for m in r["msgs"] if m.get("type") == "fileDone"), {})
    check(fd.get("status") == "failed" and fd.get("code") == "corrupt_file", f"run corrupt: {fd}")

    print("run: bad page range")
    req = job("range", [CONTRACT], {"pages": "99"})
    r = run_worker(python, sp, "run", req)
    base_checks(r, "run range")
    fd = next((m for m in r["msgs"] if m.get("type") == "fileDone"), {})
    check(fd.get("status") == "failed" and fd.get("code") == "range_out_of_bounds", f"range: {fd}")

    print("passwords (an encrypted copy made in testdata/out)")
    enc = os.path.join(BASE, "jobs", "protejat ăș.pdf")
    os.makedirs(os.path.dirname(enc), exist_ok=True)
    subprocess.run([python, "-I", "-B", "-X", "utf8", "-c",
                    "import os,sys;sys.path.insert(0,os.environ['CONVERTOR_PYPATH']);import pymupdf;"
                    "d=pymupdf.open(sys.argv[1]);d.save(sys.argv[2],encryption=pymupdf.PDF_ENCRYPT_AES_256,user_pw='parolă',owner_pw='o')",
                    CONTRACT, enc], env=env, check=True)
    r = run_worker(python, sp, "analyze", {"cmd": "analyze", "id": "a2", "files": [{"id": "f1", "path": enc}],
                                           "secrets": {"files": {}}})
    a = next((m for m in r["msgs"] if m.get("type") == "analysis"), {})
    check(a.get("status") == "locked" and a["props"].get("needsPassword") is True, f"analyze locked: {a}")
    r = run_worker(python, sp, "analyze", {"cmd": "analyze", "id": "a3", "files": [{"id": "f1", "path": enc}],
                                           "secrets": {"files": {"f1": "parolă"}}})
    a = next((m for m in r["msgs"] if m.get("type") == "analysis"), {})
    check(a.get("status") == "ok" and a["props"].get("isEncrypted") is True and a["props"].get("pageCount", 0) >= 1,
          f"analyze with password: {a}")
    for pw, code in ((None, "password_required"), ("greșit", "wrong_password")):
        req = job("pw", [enc], secrets={"f1": pw} if pw else {})
        r = run_worker(python, sp, "run", req)
        fd = next((m for m in r["msgs"] if m.get("type") == "fileDone"), {})
        check(fd.get("code") == code, f"run with password {pw!r}: {fd}")
    req = job("pw-ok", [enc], secrets={"f1": "parolă"})
    r = run_worker(python, sp, "run", req)
    base_checks(r, "run with password")
    run_checks(r, req, "run with password")
    leaked = "parolă" in r["stderr"] or any("parolă" in json.dumps(m, ensure_ascii=False) for m in r["msgs"])
    check(not leaked, "password not in stdout or stderr")
    pyc = [d for d, _, _ in os.walk(sp) if d.endswith("__pycache__")]
    check(COMPILE or not pyc, f"no __pycache__ written into the engine folder ({len(pyc)})")

    print("cancel: poze.pdf x3, cancel after 1 s")
    req = job("cancel", [POZE] * 3)
    r = run_worker(python, sp, "run", req, cancel_after=1.0)
    base_checks(r, "cancel")
    check(r["cancel_lag"] is not None and r["cancel_lag"] < 3.0, f"exited {r['cancel_lag']:.2f}s after the cancel")
    check(r["msgs"][-1].get("cancelled") is True, f"done has cancelled:true: {r['msgs'][-1]}")
    fds = [m for m in r["msgs"] if m.get("type") == "fileDone"]
    check(any(m.get("code") == "cancelled" for m in fds), f"current file reported cancelled: {fds}")
    out_files = [os.path.join(dp, f) for dp, _, fs in os.walk(req["outDir"]) for f in fs] if os.path.isdir(req["outDir"]) else []
    check(not out_files, f"nothing in outDir after cancel: {out_files}")

    print("cancel after the current file: poze.pdf x2")
    req = job("cancel-after", [POZE] * 2)
    r = run_worker(python, sp, "run", req, cancel_after=1.0, cancel_msg=b'{"cmd":"cancel","after":"currentFile"}\n')
    base_checks(r, "cancel after")
    fds = [m for m in r["msgs"] if m.get("type") == "fileDone"]
    check(len(fds) == 1 and fds[0]["status"] == "ok", f"only the current file, finished: {fds}")
    check("cancelled" not in r["msgs"][-1], f"done: {r['msgs'][-1]}")

    for p, fp in before.items():
        check(fingerprint(p) == fp, f"input unchanged: {os.path.basename(p)}")
    for d, l in listing.items():
        check(sorted(os.listdir(d)) == l, f"nothing written next to the inputs in {os.path.basename(d)}")

    print("\nsample run (contract):")
    for m in sample:
        print("  " + json.dumps(m, ensure_ascii=False))
    print("\nsample progress (poze.pdf):")
    pr = [m for m in poze_msgs if m.get("type") == "progress"]
    for m in pr[:3] + pr[len(pr) // 2:len(pr) // 2 + 2] + pr[-2:]:
        print("  " + json.dumps(m, ensure_ascii=False))
    print(f"\ninstalled size {(py_bytes + sp_bytes) / 1e6:.1f} MB; contract {t_contract:.2f}s; 1 page {t_one:.2f}s; poze {t_poze:.2f}s")
    print("FAILED: " + "; ".join(failures) if failures else "ALL OK")
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
