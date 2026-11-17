"""pdf-to-word without Microsoft Word: pdf2docx on the portable CPython (contract v2).

The parent app starts it as

    <engines>\\python\\<ver>\\python.exe -I -X utf8 pdf2docx_worker.py <analyze|run|probe|serve>

with CONVERTOR_PYPATH = the site-packages of pdf2docx and of pymupdf, ";"-separated
(PYTHONPATH is ignored under -I).
The request comes on stdin as one JSON line; stdout carries only JSON lines; logs go to stderr.

`run` converts each file in a child process of the same interpreter
(`python -I -X utf8 pdf2docx_worker.py _convert`, parameters on its stdin), because pdf2docx
cannot be interrupted: a cancel kills the child. The child reports its progress as JSON lines,
which the parent turns into `progress` messages.
"""
import json
import logging
import os
import subprocess
import sys
import threading
import time

PLUGIN = "pdf-to-word"
VERSION = "0.1.0"
PROTOCOL = 2
HEARTBEAT_SEC = 4.0
STAGES = 3  # analyze (read pages), convert (lay out pages), save (write the docx)

# The engines folder stays read-only for us: no __pycache__ written next to the packages.
sys.dont_write_bytecode = True
# One site-packages folder per engine (pdf2docx, pymupdf…), separated by ";".
for _pypath in reversed((os.environ.get("CONVERTOR_PYPATH") or "").split(os.pathsep)):
    if _pypath and _pypath not in sys.path:
        sys.path.insert(0, _pypath)


def _json_stdout():
    """A writer on the original stdout, for JSON only. Everything else (print, engine
    messages, C-level writes to fd 1) is sent to stderr."""
    try:
        sys.stdout.flush()
    except Exception:
        pass
    try:
        fd = os.dup(1)
        os.dup2(2, 1)
        w = os.fdopen(fd, "wb", buffering=0)
    except OSError:
        w = sys.__stdout__.buffer
    sys.stdout = sys.stderr
    return w


class Emitter:
    def __init__(self):
        self._w = _json_stdout()
        self._lock = threading.Lock()
        self.last_emit = time.monotonic()
        self.last_progress = None

    def emit(self, msg):
        line = (json.dumps(msg, ensure_ascii=False, separators=(",", ":")) + "\n").encode("utf-8")
        with self._lock:
            self.last_emit = time.monotonic()
            t = msg.get("type")
            if t == "progress":
                self.last_progress = msg
            elif t in ("fileStart", "fileDone"):
                self.last_progress = None
            try:
                self._w.write(line)
                self._w.flush()
            except OSError:
                pass

    def start_heartbeat(self):
        """Re-sends the last progress while nothing was said for HEARTBEAT_SEC (contract 12.4)."""
        def loop():
            while True:
                time.sleep(HEARTBEAT_SEC / 4)
                with self._lock:
                    quiet = time.monotonic() - self.last_emit >= HEARTBEAT_SEC
                    m = self.last_progress
                if quiet and m is not None:
                    self.emit(dict(m, heartbeat=True))
        threading.Thread(target=loop, daemon=True).start()


def setup_logging(progress_handler=None):
    root = logging.getLogger()
    for h in list(root.handlers):
        root.removeHandler(h)
    err = logging.StreamHandler(sys.stderr)
    err.setLevel(logging.WARNING)
    err.setFormatter(logging.Formatter("[%(levelname)s] %(name)s: %(message)s"))
    root.addHandler(err)
    if progress_handler:
        root.addHandler(progress_handler)
    root.setLevel(logging.INFO)


def import_fitz():
    try:
        import pymupdf as fitz
    except ImportError:
        import fitz
    return fitz


def is_pdf(path):
    with open(path, "rb") as fh:
        return b"%PDF-" in fh.read(1024)


# ---- page ranges (same syntax as crates/contract/src/pages.rs) ----------------------------

def _page(s, count):
    s = s.strip().lower()
    if s == "z":
        n = count
    elif s.startswith("r"):
        k = int(s[1:])
        if k == 0 or k > count:
            raise ValueError(s)
        n = count + 1 - k
    else:
        n = int(s)
    if n == 0 or n > count:
        raise ValueError(s)
    return n


def parse_pages(spec, count):
    """1-based, sorted, unique page numbers. Raises ValueError(part) when invalid."""
    spec = (spec or "").strip()
    if not spec or spec.lower() == "all":
        return list(range(1, count + 1))
    out = set()
    for raw in spec.split(","):
        part = raw.strip()
        if not part:
            continue
        rng, _, parity = part.partition(":")
        rng, parity = rng.strip(), parity.strip().lower()
        try:
            if "-" in rng:
                a, b = rng.split("-", 1)
                a, b = _page(a, count), _page(b, count)
            elif not rng:
                a, b = 1, count
            else:
                a = b = _page(rng, count)
        except ValueError:
            raise ValueError(part)
        seq = list(range(a, b + 1)) if a <= b else list(range(a, b - 1, -1))
        for idx, p in enumerate(seq, 1):
            if parity == "odd" and idx % 2 == 0 or parity == "even" and idx % 2 == 1:
                continue
            if parity not in ("", "odd", "even"):
                raise ValueError(part)
            out.add(p)
    return sorted(out)


# ---- analyze ----------------------------------------------------------------------------

def analyze_one(fitz, path, password):
    """→ (status, props, code)"""
    if not os.path.isfile(path):
        return "failed", {}, "file_not_found"
    try:
        if not is_pdf(path):
            return "unsupported", {}, "unsupported_file"
    except PermissionError:
        return "failed", {}, "file_locked"
    props = {"format": "pdf"}
    try:
        doc = fitz.open(path, filetype="pdf")
    except Exception as e:
        print(f"analyze: open failed: {type(e).__name__}: {e}", file=sys.stderr)
        return "corrupt", props, "corrupt_file"
    try:
        needs = bool(doc.needs_pass)
        if needs:
            if not password:
                props.update(needsPassword=True, isEncrypted=True)
                return "locked", props, "password_required"
            if not doc.authenticate(password):
                props.update(needsPassword=True, isEncrypted=True)
                return "locked", props, "wrong_password"
        enc = needs or bool(doc.is_encrypted) or bool((doc.metadata or {}).get("encryption"))
        props.update(pageCount=doc.page_count, needsPassword=False, isEncrypted=enc)
        if doc.page_count == 0:
            return "corrupt", props, "corrupt_file"
        return "ok", props, None
    finally:
        doc.close()


def cmd_analyze(req, out):
    fitz = import_fitz()
    secrets = (req.get("secrets") or {}).get("files") or {}
    for f in req.get("files") or []:
        path = f.get("safePath") or f.get("path") or ""
        try:
            status, props, code = analyze_one(fitz, path, secrets.get(f.get("id")))
        except Exception as e:
            print(f"analyze: {type(e).__name__}: {e}", file=sys.stderr)
            status, props, code = "failed", {}, "engine_failed"
        m = {"type": "analysis", "id": req.get("id"), "file": f.get("id"), "status": status, "props": props, "warnings": []}
        if code:
            m["code"] = code
        out.emit(m)
    out.emit({"type": "done", "id": req.get("id")})


# ---- run (parent side) ------------------------------------------------------------------

class Cancel:
    def __init__(self):
        self.now = False
        self.after_current = False
        self.child = None
        self.lock = threading.Lock()

    def kill_child(self):
        with self.lock:
            c = self.child
        if c is not None and c.poll() is None:
            try:
                c.kill()
            except OSError:
                pass

    def watch_stdin(self):
        def loop():
            for raw in sys.stdin.buffer:
                try:
                    m = json.loads(raw.decode("utf-8"))
                except ValueError:
                    continue
                if isinstance(m, dict) and m.get("cmd") == "cancel":
                    if m.get("after") == "currentFile":
                        self.after_current = True
                    else:
                        self.now = True
                        self.kill_child()
        threading.Thread(target=loop, daemon=True).start()


# Fractions per stage: reading pages, laying them out, writing the docx.
# Measured: reading pages (fonts, text, images) takes most of the time, both for text and scans.
SPAN = {"analyze": (0.02, 0.70), "convert": (0.70, 0.80), "save": (0.80, 0.97)}
STAGE_NO = {"analyze": 1, "convert": 2, "save": 3}


class Progress:
    def __init__(self, out, file_id):
        self.out, self.file = out, file_id
        self.fraction, self.stage, self.last = 0.0, "analyze", 0.0

    def set(self, stage, fraction, detail=None, force=False):
        fraction = max(self.fraction, min(1.0, fraction))
        now = time.monotonic()
        # at most 10 per second, except stage changes and the end
        if not force and stage == self.stage and now - self.last < 0.1 and fraction < 1.0:
            self.fraction = fraction
            return
        self.fraction, self.stage, self.last = fraction, stage, now
        m = {"type": "progress", "file": self.file, "fraction": round(fraction, 4), "stageCode": stage,
             "stage": STAGE_NO[stage], "stages": STAGES}
        if detail:
            m["detail"] = detail
        self.out.emit(m)

    def child_event(self, ev):
        kind = ev.get("ev")
        if kind == "stage":
            lo, _ = SPAN[ev["stage"]]
            self.set(ev["stage"], lo, force=True)
        elif kind == "page":
            lo, hi = SPAN[ev["stage"]]
            i, n = ev["i"], max(1, ev["n"])
            self.set(ev["stage"], lo + (hi - lo) * i / n, {"page": i, "pages": n})


def convert_file(f, req, out, cancel, out_dir, work_dir):
    """→ (status, code, details, output_path, pages, warnings)"""
    fid = f["id"]
    src = f.get("safePath") or f["path"]
    password = ((req.get("secrets") or {}).get("files") or {}).get(fid)
    opts = req.get("options") or {}
    fwork = os.path.join(work_dir, fid)
    tmp = os.path.join(fwork, "tmp")
    os.makedirs(tmp, exist_ok=True)
    part = os.path.join(fwork, "result.docx")
    env = dict(os.environ, TEMP=tmp, TMP=tmp, PYTHONUTF8="1")
    args = [sys.executable, "-I", "-X", "utf8", os.path.abspath(__file__), "_convert"]
    prog = Progress(out, fid)
    prog.set("analyze", 0.0, force=True)
    flags = 0x08000000 if os.name == "nt" else 0  # CREATE_NO_WINDOW
    try:
        child = subprocess.Popen(args, stdin=subprocess.PIPE, stdout=subprocess.PIPE, env=env, cwd=fwork, creationflags=flags)
    except OSError as e:
        return "failed", "engine_failed", {"message": f"cannot start the converter: {e}"}, None, 0, []
    with cancel.lock:
        cancel.child = child
    if cancel.now:
        cancel.kill_child()
    result = None
    warnings = []
    try:
        child.stdin.write((json.dumps({"path": src, "out": part, "password": password,
                                       "pages": opts.get("pages", "all")}) + "\n").encode("utf-8"))
        child.stdin.close()
    except OSError:
        pass
    for raw in child.stdout:
        try:
            ev = json.loads(raw.decode("utf-8"))
        except ValueError:
            print(f"converter: {raw[:200]!r}", file=sys.stderr)
            continue
        if ev.get("ev") in ("done", "error"):
            result = ev
        elif ev.get("ev") == "warning":
            warnings.append(ev)
        elif not cancel.now:
            prog.child_event(ev)
    child.wait()
    with cancel.lock:
        cancel.child = None
    if cancel.now:
        return "failed", "cancelled", None, None, 0, warnings
    if result is None:
        return "failed", "engine_failed", {"message": f"the converter stopped (exit code {child.returncode})"}, None, 0, warnings
    if result["ev"] == "error":
        d = {"message": result.get("message", "")}
        d.update(result.get("details") or {})
        return "failed", result.get("code", "engine_failed"), d, None, 0, warnings
    # move into outDir: nothing half-written ever lands there
    prog.set("save", 0.98, force=True)
    dest_dir = os.path.join(out_dir, fid)
    os.makedirs(dest_dir, exist_ok=True)
    dest = os.path.join(dest_dir, "result.docx")
    try:
        os.replace(part, dest)
    except OSError:
        import shutil
        shutil.move(part, dest)
    prog.set("save", 1.0, force=True)
    return "ok", None, None, dest, result.get("pages", 0), warnings


def cmd_run(req, out, cancel):
    out_dir, work_dir = req.get("outDir"), req.get("workDir")
    if not out_dir or not work_dir:
        out.emit({"type": "error", "code": "invalid_options", "details": {"message": "run needs outDir and workDir"}})
        return 2
    os.makedirs(out_dir, exist_ok=True)
    os.makedirs(work_dir, exist_ok=True)
    out.start_heartbeat()
    summary = {"ok": 0, "skipped": 0, "failed": 0}
    for f in req.get("files") or []:
        if cancel.now or cancel.after_current:
            break
        fid = f["id"]
        out.emit({"type": "fileStart", "file": fid})
        try:
            status, code, details, path, pages, warnings = convert_file(f, req, out, cancel, out_dir, work_dir)
        except Exception as e:
            print(f"run {fid}: {type(e).__name__}: {e}", file=sys.stderr)
            status, code, details, path, pages, warnings = "failed", "engine_failed", {"message": str(e)}, None, 0, []
        for w in warnings:
            m = {"type": "warning", "file": fid, "code": w.get("code")}
            if w.get("details"):
                m["details"] = w["details"]
            out.emit(m)
        if status == "ok":
            src = f.get("safePath") or f["path"]
            out.emit({"type": "output", "file": fid, "path": path, "ext": "docx", "vars": {"pages": pages}, "role": "main",
                      "stats": {"inputBytes": os.path.getsize(src), "outputBytes": os.path.getsize(path)}})
            out.emit({"type": "fileDone", "file": fid, "status": "ok"})
            summary["ok"] += 1
        else:
            m = {"type": "fileDone", "file": fid, "status": "failed", "code": code}
            if details:
                m["details"] = details
            out.emit(m)
            summary["failed"] += 1
    done = {"type": "done", "summary": summary}
    if cancel.now:
        done["cancelled"] = True
    out.emit(done)
    return 0


# ---- _convert (child side) --------------------------------------------------------------

def cmd_convert():
    """Child: converts one file. stdin: {"path","out","password","pages"}. stdout: JSON events."""
    w = _json_stdout()

    def ev(**m):
        try:
            w.write((json.dumps(m, ensure_ascii=False) + "\n").encode("utf-8"))
            w.flush()
        except OSError:
            pass

    p = json.loads(sys.stdin.buffer.readline().decode("utf-8"))
    state = {"stage": "analyze", "n": 0, "read": 0, "skipped": []}

    class ProgressLog(logging.Handler):
        def emit(self, record):
            try:
                msg = record.getMessage()
                if "[3/4]" in msg:
                    state["stage"] = "convert"
                    ev(ev="stage", stage="convert")
                elif "[4/4]" in msg:
                    state["stage"] = "save"
                    ev(ev="stage", stage="save")
                elif msg.startswith("(") and "/" in msg and ") Page" in msg and state["stage"] in ("convert", "save"):
                    i, n = msg[1:msg.index(")")].split("/")
                    # the line comes before the page is done
                    ev(ev="page", stage=state["stage"], i=int(i) - 1, n=int(n))
                elif msg.startswith("Ignore page"):
                    state["skipped"].append(msg)
            except Exception:
                pass

    setup_logging(ProgressLog(level=logging.INFO))
    path, out_path, password = p["path"], p["out"], p.get("password")
    try:
        fitz = import_fitz()
        if not os.path.isfile(path):
            return ev(ev="error", code="file_not_found", message="input file not found")
        try:
            if not is_pdf(path):
                return ev(ev="error", code="unsupported_file", message="not a PDF")
        except PermissionError:
            return ev(ev="error", code="file_locked", message="cannot read the input file")
        try:
            doc = fitz.open(path, filetype="pdf")
        except Exception as e:
            return ev(ev="error", code="corrupt_file", message=f"{type(e).__name__}: {e}")
        if doc.needs_pass:
            if not password:
                return ev(ev="error", code="password_required", message="password required")
            if not doc.authenticate(password):
                return ev(ev="error", code="wrong_password", message="wrong password")
        count = doc.page_count
        doc.close()
        if count == 0:
            return ev(ev="error", code="corrupt_file", message="no pages")
        try:
            pages = parse_pages(str(p.get("pages") or "all"), count)
        except ValueError as e:
            return ev(ev="error", code="range_out_of_bounds", message=f"invalid page range: {e}",
                      details={"part": str(e), "pageCount": count})
        if not pages:
            return ev(ev="error", code="range_out_of_bounds", message="no pages selected", details={"pageCount": count})

        from pdf2docx import Converter
        from pdf2docx.page import RawPageFactory as rpf

        orig = rpf.RawPageFactory.create.__func__
        n = len(pages)

        def create(cls, *a, **k):
            # called once per selected page while the document is read
            ev(ev="page", stage="analyze", i=state["read"], n=n)
            state["read"] += 1
            return orig(cls, *a, **k)

        rpf.RawPageFactory.create = classmethod(create)
        keep_spaces()
        fast_tables()
        tolerate_empty_images()
        ev(ev="stage", stage="analyze")
        cv = Converter(path, password=password)
        try:
            all_pages = len(pages) == count
            cv.convert(out_path, pages=None if all_pages else [x - 1 for x in pages])
        finally:
            cv.close()
        if not os.path.isfile(out_path):
            return ev(ev="error", code="engine_failed", message="no docx was written")
        try:
            shrink_photos(out_path)
        except Exception as e:  # the docx as pdf2docx wrote it is still good
            print(f"shrink_photos: {type(e).__name__}: {e}", file=sys.stderr)
        if state["skipped"]:
            ev(ev="warning", code="pages_skipped", details={"count": len(state["skipped"])})
        ev(ev="done", pages=n)
    except MemoryError:
        ev(ev="error", code="out_of_memory", message="out of memory")
    except Exception as e:
        import traceback
        traceback.print_exc(file=sys.stderr)
        ev(ev="error", code="engine_failed", message=f"{type(e).__name__}: {e}")


def merge_space_spans(blocks):
    """MuPDF gives a space it infers from a gap (PDFs that place words instead of writing
    spaces, e.g. from LaTeX) as a span of its own; pdf2docx drops whitespace-only spans,
    so the words ran together in Word. Each such space joins the span before it."""
    for block in blocks:
        for line in block.get("lines", []):
            merged = []
            for span in line.get("spans", []):
                chars = span.get("chars") or []
                if chars and merged and merged[-1].get("chars") and all(not c.get("c", "x").strip() for c in chars):
                    prev = merged[-1]
                    prev["chars"].extend(chars)
                    a, b = prev["bbox"], span["bbox"]
                    prev["bbox"] = (min(a[0], b[0]), min(a[1], b[1]), max(a[2], b[2]), max(a[3], b[3]))
                    continue
                merged.append(span)
            line["spans"] = merged
    return blocks


def keep_spaces():
    from pdf2docx.page import RawPageFitz as raw_fitz
    original = raw_fitz.RawPageFitz._preprocess_text
    if getattr(original, "_keeps_spaces", False):
        return

    def preprocess(self, **settings):
        return merge_space_spans(original(self, **settings))

    preprocess._keeps_spaces = True
    raw_fitz.RawPageFitz._preprocess_text = preprocess


def fast_tables():
    """python-docx rebuilds a table's whole cell grid on every `table.cell(i, j)`, and
    pdf2docx asks for each cell in turn: a large table (or a chart taken for one) took
    minutes for one page. The grid is kept until the table's structure changes (a merge,
    an added row or column); the cells themselves are live, so content stays current."""
    import docx.table as T
    if getattr(T.Table, "_convertor_fast", False):
        return
    state = {"version": 0, "cache": {}}
    grid = T.Table._cells.fget

    def cells(self):
        hit = state["cache"].get(id(self))
        if hit and hit[0] is self and hit[1] == state["version"]:
            return hit[2]
        found = grid(self)
        state["cache"][id(self)] = (self, state["version"], found)
        return found

    def changes_structure(method):
        def wrapper(*a, **k):
            state["version"] += 1
            try:
                return method(*a, **k)
            finally:
                state["version"] += 1
        return wrapper

    T.Table._cells = property(cells)
    T._Cell.merge = changes_structure(T._Cell.merge)
    T.Table.add_row = changes_structure(T.Table.add_row)
    T.Table.add_column = changes_structure(T.Table.add_column)
    T.Table._convertor_fast = True


def tolerate_empty_images():
    """An image whose visible area is empty (clipped away on a rotated or cropped page)
    gave a 0-pixel picture that MuPDF refuses to write, failing the whole file. It becomes
    one transparent pixel instead."""
    from pdf2docx.image import ImagesExtractor as images
    original = images.ImagesExtractor._to_raw_dict
    if getattr(original, "_tolerant", False):
        return

    def to_raw_dict(image, bbox):
        if image is None or image.width < 1 or image.height < 1:
            import pymupdf
            image = pymupdf.Pixmap(pymupdf.csRGB, pymupdf.IRect(0, 0, 1, 1), 1)
            image.clear_with(0)
        return original(image, bbox)

    to_raw_dict._tolerant = True
    images.ImagesExtractor._to_raw_dict = staticmethod(to_raw_dict)


def shrink_photos(docx_path):
    """pdf2docx stores every picture as PNG: a photo then takes several times its size in the
    PDF. Photos (where JPEG is a fraction of the PNG, no transparency) become JPEG;
    screenshots and drawings stay PNG. The docx is rewritten only when something changed."""
    import zipfile
    import pymupdf
    import re as _re
    with zipfile.ZipFile(docx_path) as z:
        items = [(i, z.read(i.filename)) for i in z.infolist()]
    renamed = {}
    new_items = []
    for info, data in items:
        name = info.filename
        if name.startswith("word/media/") and name.lower().endswith(".png") and len(data) > 150_000:
            try:
                pix = pymupdf.Pixmap(data)
                if not pix.alpha and pix.n in (1, 3):
                    jpg = pix.tobytes("jpg", jpg_quality=88)
                    # A photo or a gradient: JPEG is a fraction of the PNG. Text and drawings
                    # keep PNG close to JPEG, and JPEG would blur them.
                    if len(jpg) < len(data) * 0.35:
                        new = name[:-4] + ".jpeg"
                        renamed[name[len("word/"):]] = new[len("word/"):]
                        new_items.append((new, jpg))
                        continue
            except Exception as e:
                print(f"shrink_photos {name}: {e}", file=sys.stderr)
        new_items.append((name, data))
    if not renamed:
        return
    out = []
    for name, data in new_items:
        if name.endswith(".rels"):
            text = data.decode("utf-8")
            for old, new in renamed.items():
                text = text.replace(f'Target="{old}"', f'Target="{new}"').replace(f'Target="/word/{old}"', f'Target="/word/{new}"')
            data = text.encode("utf-8")
        elif name == "[Content_Types].xml":
            text = data.decode("utf-8")
            if not _re.search(r'Extension="jpeg"', text, _re.I):
                text = text.replace("<Default ", '<Default Extension="jpeg" ContentType="image/jpeg"/><Default ', 1)
            data = text.encode("utf-8")
        out.append((name, data))
    tmp = docx_path + ".tmp"
    with zipfile.ZipFile(tmp, "w", zipfile.ZIP_DEFLATED) as z:
        for name, data in out:
            z.writestr(name, data, compress_type=zipfile.ZIP_STORED if name.endswith((".jpeg", ".png")) else zipfile.ZIP_DEFLATED)
    os.replace(tmp, docx_path)


# ---- entry point --------------------------------------------------------------------------

def read_request(argv):
    if "--request" in argv:
        with open(argv[argv.index("--request") + 1], encoding="utf-8") as fh:
            return json.loads(fh.read())
    line = sys.stdin.buffer.readline().decode("utf-8")
    return json.loads(line)


def cmd_serve(out):
    for raw in sys.stdin.buffer:
        if not raw.strip():
            continue
        try:
            req = json.loads(raw.decode("utf-8"))
        except ValueError as e:
            out.emit({"type": "error", "code": "invalid_options", "details": {"message": str(e)}})
            continue
        c = req.get("cmd")
        if c == "analyze":
            cmd_analyze(req, out)
        elif c == "probe":
            out.emit({"type": "env", "id": req.get("id"), "props": {}})
            out.emit({"type": "done", "id": req.get("id")})
        elif c == "quit":
            break
        elif c != "cancel":
            out.emit({"type": "error", "code": "invalid_options", "details": {"message": "unsupported in serve"}})
    return 0


def main(argv):
    if len(argv) >= 1 and argv[0] == "_convert":
        cmd_convert()
        return 0
    out = Emitter()
    setup_logging()
    out.emit({"type": "hello", "protocol": PROTOCOL, "plugin": PLUGIN, "pluginVersion": VERSION})
    if not argv:
        out.emit({"type": "error", "code": "invalid_options", "details": {"message": "missing command"}})
        return 2
    cmd = argv[0]
    if cmd == "serve":
        return cmd_serve(out)
    try:
        req = read_request(argv)
    except (ValueError, OSError) as e:
        out.emit({"type": "error", "code": "invalid_options", "details": {"message": f"invalid request: {e}"}})
        return 2
    if cmd == "analyze":
        cmd_analyze(req, out)
        return 0
    if cmd == "probe":
        out.emit({"type": "env", "id": req.get("id"), "props": {}})
        out.emit({"type": "done", "id": req.get("id")})
        return 0
    if cmd == "run":
        cancel = Cancel()
        cancel.watch_stdin()
        return cmd_run(req, out, cancel)
    out.emit({"type": "error", "code": "invalid_options", "details": {"message": f"unknown command {cmd}"}})
    return 2


if __name__ == "__main__":
    code = main(sys.argv[1:])
    sys.stderr.flush()
    # daemon threads may still block on stdin; leave without waiting for them
    os._exit(code)
