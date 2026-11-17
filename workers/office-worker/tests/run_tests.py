"""End-to-end tests for office-worker.exe (pipes protocol-v2 requests on stdin, checks stdout JSON lines).

Usage:
  python make_testdata.py            (once)
  python run_tests.py [test ...]     (default: all)  --verbose prints every JSON line

Outputs go to testdata/out/office-worker/ (git-ignored).
"""
import ctypes
import json
import os
import shutil
import subprocess
import sys
import threading
import time
import zipfile

HERE = os.path.dirname(os.path.abspath(__file__))
WORKER_DIR = os.path.abspath(os.path.join(HERE, ".."))
APP = os.path.abspath(os.path.join(WORKER_DIR, "..", ".."))
EXE = os.path.join(WORKER_DIR, "bin", "Release", "net48", "office-worker.exe")
IN_DIR = os.path.join(APP, "testdata", "in", "documente ăîșț")
OUT_ROOT = os.path.join(APP, "testdata", "out", "office-worker")

CONTRACT = os.path.join(IN_DIR, "contract închiriere.docx")
BIG = os.path.join(IN_DIR, "mare ștampilă.docx")
PROTECTED = os.path.join(IN_DIR, "protejat cu parolă.docx")
CONTRACT_PDF = os.path.join(IN_DIR, "contract închiriere.pdf")  # produced by test_word_to_pdf
PASSWORD = "parolă123"

VERBOSE = "--verbose" in sys.argv
RESULTS = []
SAMPLES = {}


# ---------------------------------------------------------------- helpers

def winword_pids():
    out = subprocess.run(["tasklist", "/FI", "IMAGENAME eq WINWORD.EXE", "/FO", "CSV", "/NH"],
                         capture_output=True, text=True).stdout
    pids = set()
    for line in out.splitlines():
        parts = [x.strip('"') for x in line.split('","')]
        if len(parts) > 1 and parts[0].lower().startswith("winword"):
            pids.add(int(parts[1]))
    return pids


def pid_alive(pid):
    k32 = ctypes.windll.kernel32
    h = k32.OpenProcess(0x1000, False, pid)  # PROCESS_QUERY_LIMITED_INFORMATION
    if not h:
        return False
    code = ctypes.c_ulong()
    k32.GetExitCodeProcess(h, ctypes.byref(code))
    k32.CloseHandle(h)
    return code.value == 259  # STILL_ACTIVE


class Worker:
    """Runs the worker, collects JSON lines from stdout, lets a callback react (e.g. send cancel)."""

    def __init__(self, args, request, on_message=None, close_stdin=False):
        self.lines, self.raw = [], []
        self.proc = subprocess.Popen([EXE] + args, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                     stderr=subprocess.PIPE)
        self.stderr_chunks = []
        self.t_err = threading.Thread(target=self._read_err, daemon=True)
        self.t_err.start()
        self.times = []
        self.start = time.time()
        if request is not None:
            self.send(request)
        if close_stdin:
            self.proc.stdin.close()
        self.on_message = on_message

    def _read_err(self):
        for chunk in iter(lambda: self.proc.stderr.readline(), b""):
            self.stderr_chunks.append(chunk.decode("utf-8", "replace"))

    def send(self, obj):
        data = (json.dumps(obj, ensure_ascii=False) + "\n").encode("utf-8")
        self.proc.stdin.write(data)
        self.proc.stdin.flush()

    def run(self, timeout=300):
        deadline = time.time() + timeout
        for raw in iter(self.proc.stdout.readline, b""):
            self.raw.append(raw)
            assert not raw.startswith(b"\xef\xbb\xbf"), "stdout line starts with a BOM"
            text = raw.decode("utf-8")
            msg = json.loads(text)
            assert isinstance(msg, dict) and "type" in msg, f"not a message: {text!r}"
            self.lines.append(msg)
            self.times.append(time.time() - self.start)
            if VERBOSE:
                print("    <", text.rstrip())
            if self.on_message:
                self.on_message(self, msg)
            if time.time() > deadline:
                self.proc.kill()
                raise AssertionError("timeout")
        self.code = self.proc.wait(timeout=max(1, deadline - time.time()))
        self.t_err.join(timeout=2)
        self.stderr = "".join(self.stderr_chunks)
        return self

    def of(self, type_):
        return [m for m in self.lines if m["type"] == type_]

    def jsonl(self):
        return "\n".join(r.decode("utf-8").rstrip() for r in self.raw)


def run_request(plugin, request, **kw):
    timeout = kw.pop("timeout", 300)
    return Worker([plugin, request["cmd"]], request, **kw).run(timeout)


def job_dirs(name):
    base = os.path.join(OUT_ROOT, name)
    if os.path.isdir(base):
        shutil.rmtree(base)
    work, out = os.path.join(base, "work"), os.path.join(base, "out")
    return work, out


def run_req(job, files, options=None, secrets=None):
    work, out = job_dirs(job)
    return {
        "cmd": "run", "protocol": 2, "jobId": job, "locale": "ro-RO",
        "files": [{"id": fid, "path": path, "analysis": {}} for fid, path in files],
        "options": options or {},
        "workDir": work, "outDir": out,
        "limits": {"threads": 2, "memoryMb": 2048, "maxMegapixels": 250, "idleTimeoutSec": 300},
        "secrets": {"files": secrets or {}, "fields": {}},
    }


def check_hello(w, plugin):
    assert w.lines, "no output"
    h = w.lines[0]
    assert h == {"type": "hello", "protocol": 2, "plugin": plugin, "pluginVersion": "0.1.0"}, h


def check_children_gone(w, within=0.0):
    for c in w.of("childProcess"):
        assert "startedAt" in c and c["startedAt"].endswith("Z"), c
        deadline = time.time() + within
        while pid_alive(c["pid"]) and time.time() < deadline:
            time.sleep(0.1)
        assert not pid_alive(c["pid"]), f"WINWORD.EXE {c['pid']} started by the worker is still running"


def check_heartbeats(w):
    """progress (or other) messages must never be more than ~10 s apart while a file is running."""
    gaps = [b - a for a, b in zip(w.times, w.times[1:])]
    worst = max(gaps) if gaps else 0
    assert worst < 10.5, f"gap of {worst:.1f}s between messages"
    return worst


def snapshot_inputs():
    return {n: (os.path.getsize(os.path.join(IN_DIR, n)), os.path.getmtime(os.path.join(IN_DIR, n)))
            for n in os.listdir(IN_DIR)}


def test(fn):
    RESULTS.append(fn)
    return fn


# ---------------------------------------------------------------- tests

@test
def test_manifests():
    target = os.path.join(OUT_ROOT, "manifests")
    shutil.rmtree(target, ignore_errors=True)
    w = Worker(["manifests", target], None, close_stdin=True).run(30)
    assert w.code == 0, w.stderr
    for pid in ("word-to-pdf", "pdf-to-word"):
        path = os.path.join(target, pid, "plugin.json")
        with open(path, "rb") as f:
            raw = f.read()
        assert not raw.startswith(b"\xef\xbb\xbf")
        m = json.loads(raw.decode("utf-8"))
        assert m["id"] == pid and m["protocol"] == 2 and m["version"] == "0.1.0"
        assert m["entry"][0] == {"kind": "dotnet", "worker": "office-worker", "command": pid}
        visible = [f for f in m["fields"] if not f.get("advanced")]
        assert len(visible) <= 5
        for fld in m["fields"]:
            assert fld["type"] in ("select", "toggle", "range")
            assert set(fld["label"]) == {"ro", "en"}
    return f"wrote {target}\\<id>\\plugin.json"


@test
def test_probe():
    w = run_request("word-to-pdf", {"cmd": "probe", "id": "p1"}, timeout=30)
    check_hello(w, "word-to-pdf")
    assert w.code == 0
    assert w.lines[1] == {"type": "env", "id": "p1", "props": {"office.word": "ok"}}, w.lines[1]
    assert w.lines[2] == {"type": "done", "id": "p1"}
    assert not w.of("childProcess")
    SAMPLES["probe"] = w.jsonl()


@test
def test_analyze_word():
    t0 = time.time()
    before = winword_pids()
    w = run_request("word-to-pdf", {"cmd": "analyze", "id": "a1", "level": "fast", "files": [
        {"id": "f1", "path": CONTRACT}, {"id": "f2", "path": PROTECTED}, {"id": "f3", "path": BIG},
        {"id": "f4", "path": os.path.join(IN_DIR, "nu exista.docx")},
        {"id": "f5", "path": os.path.join(APP, "testdata", "in", "tema.jpg")}]}, timeout=30)
    elapsed = time.time() - t0
    assert winword_pids() == before, "analyze must not start Word"
    check_hello(w, "word-to-pdf")
    a = {m["file"]: m for m in w.of("analysis")}
    p1 = a["f1"]["props"]
    assert a["f1"]["status"] == "ok", a["f1"]
    assert p1["app"] == "word" and p1["format"] == "docx"
    assert p1["hasComments"] is True and p1["hasTrackedChanges"] is True and p1["hasMacros"] is False
    assert p1["cachedPageCount"] == 1 and p1["needsPassword"] is False
    assert a["f2"]["status"] == "locked" and a["f2"]["props"]["needsPassword"] is True, a["f2"]
    assert a["f3"]["status"] == "ok" and a["f3"]["props"]["hasComments"] is False
    assert a["f4"]["status"] == "failed", a["f4"]
    assert a["f5"]["status"] == "unsupported", a["f5"]
    assert w.lines[-1] == {"type": "done", "id": "a1"}
    SAMPLES["analyze-word"] = w.jsonl()
    return f"{elapsed:.2f}s for 5 files"


@test
def test_word_to_pdf():
    inputs_before = snapshot_inputs()
    req = run_req("w2p-basic", [("f1", CONTRACT)])
    w = run_request("word-to-pdf", req)
    check_hello(w, "word-to-pdf")
    assert w.code == 0, w.stderr
    assert w.of("childProcess"), "no childProcess message"
    outs = w.of("output")
    assert len(outs) == 1, w.lines
    o = outs[0]
    expected = os.path.join(req["outDir"], "f1", "result.pdf")
    assert o["path"] == expected and o["ext"] == "pdf" and o["role"] == "main"
    assert o["vars"]["pages"] >= 1 and o["stats"]["outputBytes"] == os.path.getsize(expected)
    with open(expected, "rb") as f:
        data = f.read()
    assert data.startswith(b"%PDF-") and b"%%EOF" in data[-1024:]
    assert w.of("fileDone")[-1] == {"type": "fileDone", "file": "f1", "status": "ok"}
    assert w.lines[-1] == {"type": "done", "summary": {"ok": 1, "skipped": 0, "failed": 0}}
    check_children_gone(w, within=1.0)
    assert snapshot_inputs() == inputs_before, "input folder changed"
    shutil.copyfile(expected, CONTRACT_PDF)  # input for pdf-to-word
    SAMPLES["word-to-pdf"] = w.jsonl()
    return f"{len(data)} bytes, {o['vars']['pages']} page(s), {w.times[-1]:.1f}s"


@test
def test_word_to_pdf_options():
    req = run_req("w2p-options", [("f1", CONTRACT)],
                  {"quality": "screen", "pdfA": True, "bookmarks": "none", "markup": "withComments",
                   "includeProperties": False})
    w = run_request("word-to-pdf", req)
    assert w.code == 0, w.stderr
    assert w.of("fileDone")[-1]["status"] == "ok", w.lines
    path = w.of("output")[0]["path"]
    with open(path, "rb") as f:
        data = f.read()
    assert data.startswith(b"%PDF-")
    assert b"pdfaid" in data, "PDF/A metadata missing"
    check_children_gone(w, within=1.0)
    return f"{len(data)} bytes (PDF/A, with markup)"


@test
def test_pdf_to_word():
    assert os.path.exists(CONTRACT_PDF), "run test_word_to_pdf first"
    before = winword_pids()
    t0 = time.time()
    a = run_request("pdf-to-word", {"cmd": "analyze", "id": "a2", "level": "fast",
                                    "files": [{"id": "f1", "path": CONTRACT_PDF}]}, timeout=30)
    assert winword_pids() == before
    an = a.of("analysis")[0]
    assert an["status"] == "ok" and an["props"]["format"] == "pdf" and an["props"]["pageCount"] >= 1, an
    assert an["props"]["needsPassword"] is False
    SAMPLES["analyze-pdf"] = a.jsonl()

    inputs_before = snapshot_inputs()
    req = run_req("p2w-basic", [("f1", CONTRACT_PDF)])
    w = run_request("pdf-to-word", req, timeout=180)
    check_hello(w, "pdf-to-word")
    assert w.code == 0, w.stderr
    o = w.of("output")
    assert o and o[0]["ext"] == "docx", w.lines
    path = o[0]["path"]
    assert path == os.path.join(req["outDir"], "f1", "result.docx")
    with zipfile.ZipFile(path) as z:
        xml = z.read("word/document.xml").decode("utf-8")
    for word in ("închiriere", "Mărășești", "Mașină", "ă â î ș ț"):
        assert word in xml, f"'{word}' missing from converted docx"
    assert w.lines[-1] == {"type": "done", "summary": {"ok": 1, "skipped": 0, "failed": 0}}
    assert "visible Word window" not in w.stderr, "a Word window/dialog became visible:\n" + w.stderr
    check_children_gone(w, within=1.0)
    assert snapshot_inputs() == inputs_before, "input folder changed"
    SAMPLES["pdf-to-word"] = w.jsonl()
    return f"{os.path.getsize(path)} bytes docx, {time.time() - t0:.1f}s, no dialog"


@test
def test_markup_clean_vs_comments():
    """Reads the PDFs back through pdf-to-word: 'clean' has no comment text, 'withComments' has it."""
    texts = {}
    for markup in ("clean", "withComments"):
        w = run_request("word-to-pdf", run_req(f"w2p-markup-{markup}", [("f1", CONTRACT)], {"markup": markup}))
        pdf = w.of("output")[0]["path"]
        w2 = run_request("pdf-to-word", run_req(f"p2w-markup-{markup}", [("f1", pdf)]), timeout=180)
        with zipfile.ZipFile(w2.of("output")[0]["path"]) as z:
            texts[markup] = z.read("word/document.xml").decode("utf-8")
        check_children_gone(w, 1.0)
        check_children_gone(w2, 1.0)
    assert "Verifică suma" not in texts["clean"], "comment text leaked into the clean PDF"
    assert "prin transfer bancar" in texts["clean"], "tracked insertion should appear as final text"
    assert "Verifică suma" in texts["withComments"], "comment text missing from the withComments PDF"
    return "clean: no comments; withComments: comment balloon present"


@test
def test_pdf_to_word_formats():
    req = run_req("p2w-formats", [("f1", CONTRACT_PDF)], {"format": "odt"})
    w = run_request("pdf-to-word", req, timeout=180)
    assert w.code == 0 and w.of("fileDone")[-1]["status"] == "ok", w.lines
    odt = w.of("output")[0]["path"]
    assert odt.endswith("result.odt")
    with zipfile.ZipFile(odt) as z:
        assert z.read("mimetype") == b"application/vnd.oasis.opendocument.text"
    req = run_req("p2w-formats-rtf", [("f1", CONTRACT_PDF)], {"format": "rtf"})
    w2 = run_request("pdf-to-word", req, timeout=180)
    assert w2.code == 0 and w2.of("fileDone")[-1]["status"] == "ok", w2.lines
    rtf = w2.of("output")[0]["path"]
    with open(rtf, "rb") as f:
        assert f.read(5) == b"{\\rtf"
    check_children_gone(w, within=1.0)
    check_children_gone(w2, within=1.0)
    return "odt + rtf ok"


@test
def test_password_required_precheck():
    before = winword_pids()
    t0 = time.time()
    w = run_request("word-to-pdf", run_req("w2p-pw-none", [("f1", PROTECTED)]), timeout=60)
    assert w.code == 0
    fd = w.of("fileDone")[-1]
    assert fd["status"] == "failed" and fd["code"] == "password_required", fd
    assert w.lines[-1]["summary"] == {"ok": 0, "skipped": 0, "failed": 1}
    assert not w.of("childProcess"), "Word should not be started for a known-locked file"
    assert winword_pids() == before
    SAMPLES["password_required"] = w.jsonl()
    return f"{time.time() - t0:.2f}s (detected without Word)"


@test
def test_password_required_in_word():
    """Bypass the pre-check so Word itself opens the protected file with the dummy password: must fail, not prompt."""
    t0 = time.time()
    env_before = os.environ.get("OFFICE_WORKER_NO_PRECHECK")
    os.environ["OFFICE_WORKER_NO_PRECHECK"] = "1"
    try:
        w = run_request("word-to-pdf", run_req("w2p-pw-word", [("f1", PROTECTED)]), timeout=90)
    finally:
        if env_before is None:
            del os.environ["OFFICE_WORKER_NO_PRECHECK"]
    assert w.code == 0
    assert w.of("childProcess"), "Word should have been started"
    fd = w.of("fileDone")[-1]
    assert fd["status"] == "failed" and fd["code"] == "password_required", fd
    assert "visible Word window" not in w.stderr, w.stderr
    check_children_gone(w, within=1.0)
    SAMPLES["password_required_word"] = w.jsonl()
    return f"{time.time() - t0:.1f}s, Word error {fd['details'].get('wordError')} {fd['details'].get('hresult')}"


@test
def test_wrong_and_right_password():
    w = run_request("word-to-pdf", run_req("w2p-pw-wrong", [("f1", PROTECTED)], secrets={"f1": "greșită"}), timeout=90)
    fd = w.of("fileDone")[-1]
    assert fd["status"] == "failed" and fd["code"] == "wrong_password", fd
    check_children_gone(w, within=1.0)
    w2 = run_request("word-to-pdf", run_req("w2p-pw-ok", [("f1", PROTECTED)], secrets={"f1": PASSWORD}), timeout=90)
    fd2 = w2.of("fileDone")[-1]
    assert fd2["status"] == "ok", w2.lines
    with open(w2.of("output")[0]["path"], "rb") as f:
        assert f.read(5) == b"%PDF-"
    check_children_gone(w2, within=1.0)
    return "wrong -> wrong_password, right -> ok"


@test
def test_cancel_during_run():
    state = {"sent": None}

    def on_msg(wk, m):
        if state["sent"] is None and m["type"] == "progress" and m.get("stageCode") == "export":
            time.sleep(1.0)  # Word is now busy exporting the big document
            state["sent"] = time.time()
            wk.send({"cmd": "cancel"})

    req = run_req("w2p-cancel", [("f1", BIG), ("f2", CONTRACT)])
    w = run_request("word-to-pdf", req, on_message=on_msg, timeout=300)
    assert state["sent"], "never reached the export stage: " + json.dumps(w.lines)
    exit_after = time.time() - state["sent"]
    assert w.code == 0, w.stderr
    fd = w.of("fileDone")
    assert fd and fd[-1]["file"] == "f1" and fd[-1]["status"] == "failed" and fd[-1]["code"] == "cancelled", w.lines
    last = w.lines[-1]
    assert last["type"] == "done" and last.get("cancelled") is True, last
    assert last["summary"] == {"ok": 0, "skipped": 1, "failed": 1}, last
    assert not os.path.exists(os.path.join(req["outDir"], "f1", "result.pdf")), "partial output left behind"
    check_children_gone(w, within=0.0)
    assert exit_after < 3.5, f"took {exit_after:.1f}s to stop"
    SAMPLES["cancel"] = w.jsonl()
    return f"worker exited {exit_after:.2f}s after cancel; Word pid(s) {[c['pid'] for c in w.of('childProcess')]} gone"


@test
def test_cancel_after_current_file():
    state = {"sent": False}

    def on_msg(wk, m):
        if not state["sent"] and m["type"] == "fileStart" and m["file"] == "f1":
            state["sent"] = True
            wk.send({"cmd": "cancel", "after": "currentFile"})

    w = run_request("word-to-pdf", run_req("w2p-softcancel", [("f1", CONTRACT), ("f2", CONTRACT)]), on_message=on_msg)
    assert w.code == 0
    fds = w.of("fileDone")
    assert [f["file"] for f in fds] == ["f1"] and fds[0]["status"] == "ok", w.lines
    assert w.lines[-1] == {"type": "done", "cancelled": True, "summary": {"ok": 1, "skipped": 1, "failed": 0}}, w.lines[-1]
    check_children_gone(w, within=1.0)
    return "f1 ok, f2 skipped"


@test
def test_big_heartbeat():
    """Full run of the big document: heartbeats keep message gaps below 10 s."""
    w = run_request("word-to-pdf", run_req("w2p-big", [("f1", BIG)]), timeout=600)
    assert w.code == 0 and w.of("fileDone")[-1]["status"] == "ok", w.lines
    worst = check_heartbeats(w)
    check_children_gone(w, within=1.0)
    return f"{w.of('output')[0]['vars'].get('pages')} pages in {w.times[-1]:.1f}s, max gap {worst:.1f}s"


@test
def test_invalid_request():
    w = Worker(["word-to-pdf", "run"], None)
    w.proc.stdin.write(b"{not json\n")
    w.proc.stdin.flush()
    w.run(30)
    assert w.code == 2, w.code
    assert w.lines[0]["type"] == "hello" and w.lines[-1]["type"] == "error"
    w2 = run_request("word-to-pdf", {"cmd": "run", "files": [{"id": "..", "path": CONTRACT}], "outDir": OUT_ROOT}, timeout=30)
    assert w2.code == 2
    w3 = Worker(["bogus-plugin", "run"], None, close_stdin=True).run(30)
    assert w3.code == 2
    return "exit code 2"


@test
def test_unsupported_and_missing():
    w = run_request("word-to-pdf", run_req("w2p-bad", [
        ("f1", os.path.join(APP, "testdata", "in", "tema.jpg")),
        ("f2", os.path.join(IN_DIR, "nu există.docx"))]), timeout=60)
    fds = {f["file"]: f for f in w.of("fileDone")}
    assert fds["f1"]["code"] == "unsupported_file", fds
    assert fds["f2"]["code"] == "file_not_found", fds
    assert not w.of("childProcess")
    return "unsupported_file / file_not_found"


# ---------------------------------------------------------------- main

def main():
    names = [a for a in sys.argv[1:] if not a.startswith("--")]
    selected = [t for t in RESULTS if not names or t.__name__ in names or t.__name__[5:] in names]
    assert os.path.exists(EXE), f"build first: {EXE}"
    os.makedirs(OUT_ROOT, exist_ok=True)
    word_before = winword_pids()
    print(f"WINWORD.EXE before tests: {sorted(word_before) or 'none'}")
    failed = 0
    for t in selected:
        t0 = time.time()
        try:
            info = t()
            print(f"PASS {t.__name__:36s} {time.time() - t0:6.1f}s  {info or ''}")
        except Exception as e:  # noqa
            failed += 1
            print(f"FAIL {t.__name__:36s} {time.time() - t0:6.1f}s  {type(e).__name__}: {e}")
    time.sleep(1)
    word_after = winword_pids()
    print(f"WINWORD.EXE after tests: {sorted(word_after) or 'none'}")
    leaked = word_after - word_before
    if leaked:
        failed += 1
        print(f"FAIL leaked WINWORD.EXE: {sorted(leaked)}")
    missing = word_before - word_after
    if missing:
        print(f"NOTE pre-existing WINWORD.EXE gone (not killed by us?): {sorted(missing)}")
    samples = os.path.join(OUT_ROOT, "samples.jsonl.txt")
    with open(samples, "w", encoding="utf-8") as f:
        for k, v in SAMPLES.items():
            f.write(f"### {k}\n{v}\n\n")
    print(f"sample JSON lines: {samples}")
    print("ALL PASSED" if not failed else f"{failed} FAILED")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
