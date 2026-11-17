"""Pins the portable Python, PyMuPDF (PDF to Excel) and the pdf2docx wheels (PDF to Word without Word).

  python scripts/gen-python-engines.py            writes src-tauri/engines.python.toml and prints it

- Python: the newest 3.13 patch that has an embeddable zip on python.org (downloaded
  once here to compute its size and SHA-256).
- PyMuPDF gets an engine of its own (PDF to Excel needs only it); pdf2docx requires it.
- pdf2docx: the newest release plus its full dependency closure for CPython 3.13,
  win_amd64, resolved from PyPI's JSON API (`requires_dist`, environment markers
  evaluated for Windows / CPython 3.13). Only wheels: cp313-cp313-win_amd64, abi3 or
  py3-none-any. Size and SHA-256 come from PyPI. No pip needed anywhere.
"""
import hashlib, json, os, re, sys, urllib.error, urllib.request

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, "src-tauri", "engines.python.toml")
PY_MINOR = (3, 13)
TOP = "pdf2docx"
UA = {"User-Agent": "engines-pinning-script"}
OWN = "pymupdf"  # a dependency of pdf2docx that is an engine of its own

# Extra constraints on top of what the packages declare (normalized names).
CONSTRAINTS = {
    # PyMuPDF 2.x is 63 MB and pulls onnxruntime, networkx, pymupdf4llm, pymupdf-layout,
    # psutil, pyyaml, tabulate: none of it is used by pdf2docx. 1.x has no dependencies.
    "pymupdf": "<2",
}
# Declared dependencies that are not needed (normalized names).
SKIP = {
    # Imported only by the pdf2docx command line (pdf2docx/main.py, inside `if __name__`).
    "fire": "only the pdf2docx command line uses it",
}
# Files of a wheel not extracted (exact paths inside the wheel), by normalized name.
def skip_files(name, version):
    if name == "opencv-python-headless":
        # FFmpeg plugin of cv2.VideoCapture, loaded only for video files: 31 MB never used.
        return ["cv2/opencv_videoio_ffmpeg%s_64.dll" % "".join(version.split(".")[:3])]
    return []


# Same module, smaller build without GUI libraries.
SUBSTITUTE = {"opencv-python": "opencv-python-headless", "opencv-contrib-python": "opencv-contrib-python-headless"}

ENV = {
    "os_name": "nt", "sys_platform": "win32", "platform_system": "Windows", "platform_machine": "AMD64",
    "platform_python_implementation": "CPython", "implementation_name": "cpython",
    "python_version": "%d.%d" % PY_MINOR, "python_full_version": "%d.%d.0" % PY_MINOR,
    "implementation_version": "%d.%d.0" % PY_MINOR, "platform_release": "", "platform_version": "",
}


def get(url, binary=False):
    with urllib.request.urlopen(urllib.request.Request(url, headers=UA), timeout=60) as r:
        data = r.read()
    return data if binary else data.decode("utf-8")


def norm(name):
    return re.sub(r"[-_.]+", "-", name).lower()


# ---- versions (PEP 440 subset) -------------------------------------------------------

VER_RE = re.compile(r"^v?(\d+(?:\.\d+)*)(?:[-_.]?(a|b|c|rc|alpha|beta|pre|preview)[-_.]?(\d*))?"
                    r"(?:-(\d+)|[-_.]?(?:post|rev|r)[-_.]?(\d*))?(?:[-_.]?dev[-_.]?(\d*))?(?:\+.*)?$", re.I)


def parse_ver(s):
    m = VER_RE.match(s.strip())
    if not m:
        return None
    rel = tuple(int(x) for x in m.group(1).split("."))
    while len(rel) > 1 and rel[-1] == 0:
        rel = rel[:-1]
    big = 10 ** 9
    pre = None
    if m.group(2):
        pre = ({"a": 1, "alpha": 1, "b": 2, "beta": 2}.get(m.group(2).lower(), 3), int(m.group(3) or 0))
    post = None
    if m.group(4) is not None:
        post = int(m.group(4))
    elif m.group(5) is not None:
        post = int(m.group(5) or 0)
    dev = int(m.group(6) or 0) if m.group(6) is not None else None
    if pre is None:
        pre = (-big, 0) if post is None and dev is not None else (big, 0)  # 1.0.dev1 < 1.0a1 < 1.0
    key = (rel, pre, -big if post is None else post, big if dev is None else dev)
    return {"rel": rel, "key": key, "pre": bool(m.group(2)) or dev is not None}


def cmp_rel(a, b):
    n = max(len(a), len(b))
    a, b = a + (0,) * (n - len(a)), b + (0,) * (n - len(b))
    return (a > b) - (a < b)


def cmp_key(a, b):
    c = cmp_rel(a["rel"], b["rel"])
    if c:
        return c
    return (a["key"][1:] > b["key"][1:]) - (a["key"][1:] < b["key"][1:])


def spec_ok(version, spec):
    """`version` (string) satisfies `spec` like '>=1.2,<2,!=1.5.*'."""
    v = parse_ver(version)
    if v is None:
        return False
    for part in [p.strip() for p in spec.split(",") if p.strip()]:
        m = re.match(r"^(===|==|!=|<=|>=|~=|<|>)\s*(.+)$", part)
        if not m:
            return False
        op, want = m.group(1), m.group(2).strip()
        if op == "===":
            if version != want:
                return False
            continue
        if want.endswith(".*"):
            prefix = tuple(int(x) for x in want[:-2].split("."))
            match = (v["rel"] + (0,) * len(prefix))[:len(prefix)] == prefix
            if (op == "==") != match:
                return False
            continue
        w = parse_ver(want)
        if w is None:
            return False
        c = cmp_key(v, w)
        ok = {"==": c == 0, "!=": c != 0, "<=": c <= 0, ">=": c >= 0, "<": c < 0, ">": c > 0}.get(op)
        if op == "~=":
            parts = tuple(int(x) for x in want.split("+")[0].split(".") if x.isdigit())
            ok = c >= 0 and cmp_rel(v["rel"][:len(parts) - 1], parts[:-1]) == 0
        if not ok:
            return False
    return True


# ---- requirements and markers (PEP 508 subset) ---------------------------------------

def parse_req(s):
    s, _, marker = s.partition(";")
    m = re.match(r"^\s*([A-Za-z0-9][A-Za-z0-9._-]*)\s*(\[[^\]]*\])?\s*\(?([^)]*)\)?\s*$", s)
    if not m:
        raise ValueError(f"cannot parse requirement {s!r}")
    extras = [e.strip() for e in (m.group(2) or "[]")[1:-1].split(",") if e.strip()]
    return m.group(1), extras, m.group(3).strip(), marker.strip()


TOKEN = re.compile(r"\s*(\(|\)|===|==|!=|<=|>=|~=|<|>|not\s+in\b|in\b|and\b|or\b|'[^']*'|\"[^\"]*\"|[A-Za-z_][A-Za-z0-9_.]*)")


def eval_marker(marker, extras):
    if not marker:
        return True
    toks, pos = [], 0
    while pos < len(marker):
        m = TOKEN.match(marker, pos)
        if not m:
            if marker[pos:].strip():
                raise ValueError(f"bad marker {marker!r}")
            break
        toks.append(re.sub(r"\s+", " ", m.group(1)))
        pos = m.end()
    i = 0

    def value(t):
        if t[0] in "'\"":
            return t[1:-1]
        return ENV.get(t, "")

    def compare(lhs_t, op, rhs_t):
        if "extra" in (lhs_t, rhs_t):
            other = value(rhs_t if lhs_t == "extra" else lhs_t)
            hit = norm(other) in {norm(e) for e in extras}
            return hit if op == "==" else (not hit if op == "!=" else False)
        l, r = value(lhs_t), value(rhs_t)
        if op in ("in", "not in"):
            return (l in r) == (op == "in")
        version_vars = {"python_version", "python_full_version", "implementation_version"}
        if (lhs_t in version_vars or rhs_t in version_vars) and op not in ("===",):
            if lhs_t in version_vars:
                return spec_ok(l, op + r)
            flip = {"<": ">", ">": "<", "<=": ">=", ">=": "<="}.get(op, op)
            return spec_ok(r, flip + l)
        return {"==": l == r, "!=": l != r, "<": l < r, ">": l > r, "<=": l <= r, ">=": l >= r}.get(op, False)

    def atom():
        nonlocal i
        if toks[i] == "(":
            i += 1
            v = expr()
            i += 1  # ')'
            return v
        l, op, r = toks[i], toks[i + 1], toks[i + 2]
        i += 3
        return compare(l, op, r)

    def conj():
        nonlocal i
        v = atom()
        while i < len(toks) and toks[i] == "and":
            i += 1
            v = atom() and v
        return v

    def expr():
        nonlocal i
        v = conj()
        while i < len(toks) and toks[i] == "or":
            i += 1
            v = conj() or v
        return v

    return expr()


# ---- wheels ----------------------------------------------------------------------------

def wheel_rank(filename):
    """Lower is better; None if not usable on CPython 3.13 win_amd64."""
    m = re.match(r"^(.+?)-(.+?)(?:-\d[^-]*)?-([^-]+)-([^-]+)-([^-]+)\.whl$", filename)
    if not m:
        return None
    pys, abis, plats = m.group(3).split("."), m.group(4).split("."), m.group(5).split(".")
    tag = "cp%d%d" % PY_MINOR
    best = None
    for py in pys:
        for abi in abis:
            for plat in plats:
                r = None
                if plat == "win_amd64":
                    if py == tag and abi == tag:
                        r = 0
                    elif abi == "abi3" and re.match(r"^cp3\d+$", py) and int(py[3:]) <= PY_MINOR[1]:
                        r = 1
                    elif py in ("py3", tag) and abi == "none":
                        r = 2
                elif plat == "any" and abi == "none" and py in ("py3", "py2.py3", "py%d%d" % PY_MINOR, tag):
                    r = 3
                if r is not None and (best is None or r < best):
                    best = r
    return best


def python_ok(requires_python):
    return not requires_python or spec_ok("%d.%d.0" % PY_MINOR, requires_python)


_project_cache = {}


def project(name):
    n = norm(name)
    if n not in _project_cache:
        _project_cache[n] = json.loads(get(f"https://pypi.org/pypi/{n}/json"))
    return _project_cache[n]


def pick(name, specs):
    """Newest final release meeting all `specs` that has a usable wheel → (version, file)."""
    data = project(name)
    candidates = []
    for ver, files in data["releases"].items():
        pv = parse_ver(ver)
        if pv is None or pv["pre"] or not all(spec_ok(ver, s) for s in specs if s):
            continue
        usable = [(wheel_rank(f["filename"]), f) for f in files
                  if f["packagetype"] == "bdist_wheel" and not f.get("yanked") and python_ok(f.get("requires_python"))]
        usable = [(r, f) for r, f in usable if r is not None]
        if usable:
            candidates.append((pv, ver, min(usable, key=lambda x: x[0])[1]))
    if not candidates:
        raise SystemExit(f"no usable win_amd64 / py3 wheel for {name} {specs}")
    import functools
    candidates.sort(key=functools.cmp_to_key(lambda a, b: cmp_key(a[0], b[0])))
    return candidates[-1][1], candidates[-1][2]


def wheel_requires(name, version, file):
    """Requires-Dist of that exact wheel (PEP 658 metadata), else the release JSON."""
    try:
        meta = get(file["url"] + ".metadata")
        return [l.split(":", 1)[1].strip() for l in meta.splitlines() if l.startswith("Requires-Dist:")]
    except urllib.error.URLError:
        return json.loads(get(f"https://pypi.org/pypi/{norm(name)}/{version}/json"))["info"].get("requires_dist") or []


def license_of(name, version):
    info = json.loads(get(f"https://pypi.org/pypi/{norm(name)}/{version}/json"))["info"]
    lic = info.get("license_expression") or ""
    if not lic and info.get("license") and len(info["license"]) < 80 and "\n" not in info["license"]:
        lic = info["license"]
    if not lic:
        cls = [c.split("::")[-1].strip() for c in info.get("classifiers", []) if c.startswith("License ::")]
        lic = ", ".join(cls)
    fixes = {"Apache 2.0": "Apache-2.0", "Apache Software License": "Apache-2.0",
             "Dual Licensed - GNU AFFERO GPL 3.0 or Artifex Commercial License": "AGPL-3.0-or-later OR Artifex commercial"}
    return fixes.get(lic, lic) or "unknown"


def resolve():
    specs = {norm(TOP): [""]}
    for k, v in CONSTRAINTS.items():
        specs.setdefault(k, []).append(v)
    wanted = {norm(TOP)}
    chosen = {}  # name → (display name, version, file, requires)
    changed = True
    while changed:
        changed = False
        for n in sorted(wanted):
            ver, f = pick(n, specs.get(n, []))
            if n in chosen and chosen[n][1] == ver:
                continue
            reqs = wheel_requires(n, ver, f)
            chosen[n] = (project(n)["info"]["name"], ver, f, reqs)
            changed = True
            for r in reqs:
                dep, extras, spec, marker = parse_req(r)
                if not eval_marker(marker, []):
                    continue
                d = norm(SUBSTITUTE.get(norm(dep), dep))
                if d in SKIP:
                    print(f"skip {d}: {SKIP[d]}", file=sys.stderr)
                    continue
                if extras:
                    print(f"note: {n} wants {dep}[{','.join(extras)}]; extras are not resolved", file=sys.stderr)
                specs.setdefault(d, [])
                if spec and spec not in specs[d]:
                    specs[d].append(spec)
                if d not in wanted:
                    wanted.add(d)
            break  # restart: specs may have changed
    return chosen


# ---- python.org ----------------------------------------------------------------------

def embeddable_python():
    listing = get("https://www.python.org/ftp/python/")
    patches = sorted({int(m) for m in re.findall(r'href="%d\.%d\.(\d+)/"' % PY_MINOR, listing)}, reverse=True)
    for p in patches:
        ver = "%d.%d.%d" % (PY_MINOR + (p,))
        url = f"https://www.python.org/ftp/python/{ver}/python-{ver}-embed-amd64.zip"
        try:
            urllib.request.urlopen(urllib.request.Request(url, method="HEAD", headers=UA), timeout=30).close()
        except urllib.error.HTTPError:
            continue
        print(f"downloading {url}", file=sys.stderr)
        data = get(url, binary=True)
        return ver, url, len(data), hashlib.sha256(data).hexdigest()
    raise SystemExit("no embeddable zip found for Python %d.%d" % PY_MINOR)


def q(s):
    return json.dumps(s, ensure_ascii=False)


def main():
    chosen = resolve()
    py_ver, py_url, py_size, py_sha = embeddable_python()
    top = chosen[norm(TOP)]
    licenses = {n: license_of(c[0], c[1]) for n, c in chosen.items()}
    own = chosen[OWN]
    rest = {n: c for n, c in chosen.items() if n != OWN}
    total = sum(c[2]["size"] for c in rest.values())
    lines = [
        "# Generated by scripts/gen-python-engines.py. Do not edit by hand; run the script again.",
        "# Portable Python for the Python plugins, PyMuPDF (PDF to Excel) and the wheels of pdf2docx (PDF to Word without Word).",
        "",
        "[python]",
        f"version = {q(py_ver)}",
        f"url = {q(py_url)}",
        f"size = {py_size}",
        f"sha256 = {q(py_sha)}",
        'license = "PSF-2.0"',
        'source = "https://www.python.org/"',
        'extract = "all"',
        'files = ["python.exe"]',
        "",
        "# PDF reading for the Python plugins. pdf2docx needs it too.",
        "[pymupdf]",
        f"version = {q(own[1])}",
        'requires = ["python"]',
        f"license = {q(licenses[OWN])}",
        f"source = {q('https://pypi.org/project/PyMuPDF/')}",
        'extract = "wheels"',
        'files = ["site-packages/pymupdf/__init__.py"]',
        "",
        "[[pymupdf.wheels]]",
        f"name = {q(own[2]['filename'])}",
        f"url = {q(own[2]['url'])}",
        f"size = {own[2]['size']}",
        f"sha256 = {q(own[2]['digests']['sha256'])}",
        f"license = {q(licenses[OWN])}",
        "",
        f"# {len(rest)} wheels, {total} bytes in all (PyMuPDF is its own engine). Skipped: "
        + ", ".join(f"{k} ({v})" for k, v in SKIP.items()) + ". Constraints: "
        + ", ".join(f"{k}{v}" for k, v in CONSTRAINTS.items()) + ".",
        "[pdf2docx]",
        f"version = {q(top[1])}",
        'requires = ["python", "pymupdf"]',
        f"license = {q(summary_license(licenses))}",
        f"source = {q('https://pypi.org/project/pdf2docx/')}",
        'extract = "wheels"',
        'files = ["site-packages/pdf2docx/__init__.py"]',
    ]
    for n in [norm(TOP)] + sorted(k for k in rest if k != norm(TOP)):
        disp, ver, f, _ = chosen[n]
        lines += [
            "",
            "[[pdf2docx.wheels]]",
            f"name = {q(f['filename'])}",
            f"url = {q(f['url'])}",
            f"size = {f['size']}",
            f"sha256 = {q(f['digests']['sha256'])}",
            f"license = {q(licenses[n])}",
        ]
        if skip_files(n, ver):
            lines.append(f"skip = {json.dumps(skip_files(n, ver))}  # not extracted")
    text = "\n".join(lines) + "\n"
    with open(OUT, "w", encoding="utf-8", newline="\n") as fh:
        fh.write(text)
    sys.stdout.write(text)


def summary_license(licenses):
    return f"{licenses[norm(TOP)]} (pdf2docx), others BSD/MIT/Apache/PSF"


if __name__ == "__main__":
    main()
