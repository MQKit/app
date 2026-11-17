"""Finds the tables of a PDF with PyMuPDF, for the pdf-to-excel tool (worker-core runs it).

    python.exe -I -X utf8 pdf_to_excel_tables.py      (CONVERTOR_PYPATH = PyMuPDF's site-packages)

stdin: one JSON line {"path", "password", "pages": [0-based], "find": "tables" | "text"}.
stdout: JSON lines only:
    {"ev": "page", "done": n, "of": total}
    {"ev": "page_tables", "page": p, "tables": [{"rows": [[str|null]], "header": [str] | null, "headerOutside": bool, "top": y, "bottom": y, "height": h}]}
    {"ev": "error", "code": "...", "message": "..."}
"""
import json
import os
import sys

sys.dont_write_bytecode = True
_pypath = os.environ.get("CONVERTOR_PYPATH")
if _pypath:
    for p in reversed(_pypath.split(os.pathsep)):
        if p and p not in sys.path:
            sys.path.insert(0, p)

_out = os.fdopen(os.dup(1), "wb", buffering=0)
os.dup2(2, 1)
sys.stdout = sys.stderr


def ev(**m):
    _out.write((json.dumps(m, ensure_ascii=False, separators=(",", ":")) + "\n").encode("utf-8"))


def clean(cell):
    if cell is None:
        return None
    # Lines inside a cell are the PDF's own wrapping: one line in Excel. A word cut with a
    # hyphen at the end of a line is joined back.
    parts = [p.strip() for p in str(cell).splitlines()]
    text = ""
    for p in parts:
        if not p:
            continue
        if text.endswith("-") and len(text) > 1 and text[-2].isalpha() and p[:1].islower():
            text = text[:-1] + p
        else:
            text = (text + " " + p) if text else p
    return text


def by_words(table, words, unruled):
    """The table's text with whole words: each word goes to the cell its middle is in
    (cutting at the cell edge splits words that overflow a narrow column).

    A table without lines has guessed column edges, often wrong for a header over
    right-aligned numbers: there a header word belongs to the last column whose content
    (in the rows below) ends before the word starts."""
    grid = [row.cells for row in table.rows]
    buckets = [[[] for _ in r] for r in grid]
    # Each word to the smallest cell around its middle. (Taking the first row whose span
    # held it lost the words of a row under a cell two rows tall: "Cycle Name" over a
    # second header row.)
    x0, y0, x1, y1 = table.bbox
    for w in words:
        cx, cy = (w[0] + w[2]) / 2, (w[1] + w[3]) / 2
        if not (x0 - 1 <= cx <= x1 + 1 and y0 - 1 <= cy <= y1 + 1):
            continue
        best = None
        for i, cells in enumerate(grid):
            for j, c in enumerate(cells):
                if c and c[0] <= cx <= c[2] and c[1] <= cy <= c[3]:
                    area = (c[2] - c[0]) * (c[3] - c[1])
                    if best is None or area < best[0]:
                        best = (area, i, j)
        if best:
            buckets[best[1]][best[2]].append(w)
    ncol = max((len(r) for r in grid), default=0)
    extra = split_wide_cells(grid, buckets, ncol)
    if unruled and len(grid) > 1 and ncol > 1:
        ends = [max((w[2] for i in range(1, len(grid)) if j < len(buckets[i]) for w in buckets[i][j]), default=None) for j in range(ncol)]
        lefts = [min((r[j][0] for r in grid if j < len(r) and r[j]), default=None) for j in range(ncol)]
        starts = [None] + [ends[j - 1] if ends[j - 1] is not None else lefts[j] for j in range(1, ncol)]
        for i in [0]:
            row_words = [w for cell in buckets[i] for w in cell]
            buckets[i] = [[] for _ in range(len(grid[i]))]
            for w in row_words:
                j = max(k for k in range(ncol) if k == 0 or (starts[k] is not None and w[0] >= starts[k] - 1))
                while j > 0 and (j >= len(grid[i]) or grid[i][j] is None):
                    j -= 1
                buckets[i][j].append(w)
    def text(ws):
        ws = sorted(ws, key=lambda w: (w[5], w[6], w[7]))
        lines, key, prev = [], None, None
        for w in ws:
            if (w[5], w[6]) != key:
                lines.append([])
                key = (w[5], w[6])
            elif prev is not None and CUT_NUMBER.match(prev[4]) and w[4].isdigit() and w[0] - prev[2] < (w[3] - w[1]) * 0.5:
                # "467. 9": a number the PDF spaced out after its decimal mark.
                lines[-1][-1] += w[4]
                prev = w
                continue
            lines[-1].append(w[4])
            prev = w
        return clean(chr(10).join(" ".join(l) for l in lines))

    wrows = []
    for i, cells in enumerate(grid):
        # Tables boxed with only column lines run their records together too.
        row = [extra.get((i, j)) if c is None else buckets[i][j] for j, c in enumerate(cells)]
        for part in records(cells, row):
            wrows.append([None if row[j] is None else part[j] for j in range(len(cells))])
    if unruled:
        wrows = rejoin_phrases(wrows)
    wrows = subcolumns(wrows)
    return [[None if c is None else text(c) for c in r] for r in wrows]


def rejoin_phrases(wrows):
    """Columns guessed from lined-up text can cut a phrase ("Payment ref 1000 | shop |
    no. 7" where the words happened to line up): two neighbour columns whose words meet at
    a plain word space on (nearly) every row are one column."""
    ncol = max((len(r) for r in wrows), default=0)
    cols = [[r[j] if j < len(r) else None for r in wrows] for j in range(ncol)]
    j = 0
    while j + 1 < len(cols):
        a, b = cols[j], cols[j + 1]
        both, close = 0, 0
        for ca, cb in zip(a, b):
            if not ca or not cb:
                continue
            both += 1
            h = sorted(w[3] - w[1] for w in ca + cb)[len(ca + cb) // 2]
            # On the same line: the end of a's words to the start of b's.
            line = [(wa, wb) for wa in ca for wb in cb if abs((wa[1] + wa[3]) / 2 - (wb[1] + wb[3]) / 2) < h * 0.5 and wb[0] >= wa[0]]
            if line:
                gap = min(wb[0] for _, wb in line) - max(wa[2] for wa, _ in line)
                if gap < h * 0.6:
                    close += 1
        if both >= 2 and close >= both * 0.8:
            cols[j] = [(ca or []) + (cb or []) if (ca is not None or cb is not None) else None for ca, cb in zip(a, b)]
            del cols[j + 1]
            continue
        j += 1
    return [[col[i] for col in cols] for i in range(len(wrows))]


def subcolumns(wrows):
    """A column holding several columns of values side by side, apart only by spacing
    ("Total | Under 18 | 18 and over" under one ruled heading): split where no word of any
    row crosses, when several rows have values on both sides of the gap."""
    ncol = max((len(r) for r in wrows), default=0)
    out_cols = []
    for j in range(ncol):
        cells = [r[j] if j < len(r) else None for r in wrows]
        # The values only: a heading over the sub-columns runs across their gaps (and its
        # "18" of "Under 18 years" is no value).
        words = [w for c in cells if c and all(NUMBER.match(w[4]) or w[4] in DASHES for w in c) for w in c if NUMBER.match(w[4])]
        cuts = []
        if len(words) >= 6:
            heights = sorted(w[3] - w[1] for w in words)
            h = heights[len(heights) // 2]
            ws = sorted(words, key=lambda w: w[0])
            spans, right = [[ws[0][0], ws[0][2]]], ws[0][2]
            for w in ws[1:]:
                if w[0] - spans[-1][1] >= h * 0.9:
                    spans.append([w[0], w[2]])
                else:
                    spans[-1][1] = max(spans[-1][1], w[2])
            cuts = [(spans[k][1] + spans[k + 1][0]) / 2 for k in range(len(spans) - 1)]
            if cuts:
                def side(w):
                    cx = (w[0] + w[2]) / 2
                    return sum(1 for c in cuts if cx > c)
                both = sum(1 for c in cells if c and len({side(w) for w in c if NUMBER.match(w[4])}) >= 2)
                if both < 3:
                    cuts = []
        if not cuts:
            out_cols.append(cells)
            continue
        parts = [[] for _ in range(len(cuts) + 1)]
        for c in cells:
            if c is None:
                for p in parts:
                    p.append(None)
                continue
            split = [[] for _ in parts]
            for w in c:
                cx = (w[0] + w[2]) / 2
                split[sum(1 for x in cuts if cx > x)].append(w)
            for p, ws in zip(parts, split):
                p.append(ws)
        out_cols.extend(parts)
    return [[col[i] for col in out_cols] for i in range(len(wrows))]


def split_wide_cells(grid, buckets, ncol):
    """A row without its own column lines (the white rows of a striped table) is found as
    one cell across the table: its words go back to the columns the other rows have, when
    they stand apart in clear gaps, each group within one column. A title over several
    columns (words at normal spacing, or across a column edge) stays as it is.
    Returns {(row, column): words} for the columns the wide cell covered."""
    lefts = []
    for j in range(ncol):
        xs = [r[j][0] for r in grid if j < len(r) and r[j]]
        lefts.append(min(xs) if xs else None)
    rights = max((c[2] for r in grid for c in r if c), default=0)
    edges = []
    for j in range(ncol):
        if lefts[j] is None:
            edges.append(None)
            continue
        nxt = next((lefts[k] for k in range(j + 1, ncol) if lefts[k] is not None), rights)
        edges.append((lefts[j], nxt))
    extra = {}
    for i, cells in enumerate(grid):
        for j, c in enumerate(cells):
            if not c or len(buckets[i][j]) < 2:
                continue
            cols = [k for k in range(j, ncol) if edges[k] and edges[k][0] < c[2] - 1 and edges[k][0] >= c[0] - 1]
            if len(cols) < 2:
                continue
            ws = sorted(buckets[i][j], key=lambda w: w[0])
            heights = sorted(w[3] - w[1] for w in ws)
            h = heights[len(heights) // 2]
            # Words side by side in clear gaps; the lines of a heading stacked over one
            # column overlap, so they stay together (as do the lines of wrapped text).
            groups, right = [[ws[0]]], ws[0][2]
            for w in ws[1:]:
                if w[0] - right >= h * 0.9:
                    groups.append([w])
                else:
                    groups[-1].append(w)
                right = max(right, w[2]) if len(groups[-1]) > 1 else w[2]
            if len(groups) < 2:
                continue
            placed = {}
            for g in groups:
                gx0, gx1 = g[0][0], max(w[2] for w in g)
                mid = (gx0 + gx1) / 2
                k = next((k for k in cols if edges[k][0] <= mid < edges[k][1]), None)
                if k is None or k in placed or gx0 < edges[k][0] - 2 or gx1 > edges[k][1] + 2:
                    placed = None
                    break
                placed[k] = g
            if not placed or len(placed) < 2:
                continue
            buckets[i][j] = placed.pop(j, [])
            for k, g in placed.items():
                if k < len(cells) and cells[k] is None:
                    extra[(i, k)] = g
                else:
                    buckets[i][k] = buckets[i][k] + g if k < len(cells) else g
    return extra


NUMBER = __import__("re").compile(r"^[(\-\u2212\u2013+$\u20ac\u00a3]?\d[\d.,'\u00a0 ]*%?\)?$")
DASHES = {"-", "\u2013", "\u2014", "\u2212", "."}
CUT_NUMBER = __import__("re").compile(r"^[(\-\u2212\u2013]?\d[\d,.]*[.,]$")


def records(cells, bucket):
    """A row of a table without lines can hold several records (rows the finder ran
    together): when two or more columns have numbers on two or more lines, the row is split
    at its lines. Text wrapped in one column (a long label) doesn't split it."""
    ws = [(j, w) for j, b in enumerate(bucket) for w in (b or [])]
    if len(ws) < 4:
        return [bucket]
    heights = sorted(w[3] - w[1] for _, w in ws)
    h = heights[len(heights) // 2]
    ys = sorted({round((w[1] + w[3]) / 2, 1) for _, w in ws})
    lines = [[ys[0]]]
    for y in ys[1:]:
        if y - lines[-1][-1] > h * 0.5:
            lines.append([y])
        else:
            lines[-1].append(y)
    if len(lines) < 2:
        return [bucket]

    def line_of(w):
        y = (w[1] + w[3]) / 2
        return min(range(len(lines)), key=lambda n: min(abs(y - v) for v in lines[n]))

    numeric_cols = 0
    for b in bucket:
        nums = {line_of(w) for w in (b or []) if NUMBER.match(w[4])}
        if len(nums) >= 2:
            numeric_cols += 1
    if numeric_cols < 2:
        return [bucket]
    parts = [[[] for _ in bucket] for _ in lines]
    for j, w in ws:
        parts[line_of(w)][j].append(w)
    return parts


def prune(rows):
    """Without the rows and columns that are empty all the way."""
    rows = [r for r in rows if any(c for c in r)]
    width = max((len(r) for r in rows), default=0)
    keep = [j for j in range(width) if any(j < len(r) and r[j] for r in rows)]
    return [[r[j] if j < len(r) else None for j in keep] for r in rows]


def useful(rows):
    """A real table: at least 2 columns and 2 rows with something in them."""
    filled = [r for r in rows if sum(1 for c in r if c) >= 1]
    cols = max((sum(1 for c in r if c) for r in rows), default=0)
    return len(filled) >= 2 and cols >= 2


def main():
    req = json.loads(sys.stdin.readline())
    try:
        import pymupdf as fitz
    except ImportError:
        import fitz
    try:
        doc = fitz.open(req["path"])
    except Exception as e:
        return ev(ev="error", code="corrupt_file", message=str(e))
    if doc.needs_pass:
        if not req.get("password"):
            return ev(ev="error", code="password_required", message="password required")
        if not doc.authenticate(req["password"]):
            return ev(ev="error", code="wrong_password", message="wrong password")
    pages = [p for p in req.get("pages") or range(doc.page_count) if 0 <= p < doc.page_count]
    find = req.get("find", "tables")
    for n, p in enumerate(pages):
        page = doc[p]
        found = []
        strategies = ["text"] if find == "text" else ["lines", "text"]
        words = page.get_text("words")
        for strategy in strategies:
            try:
                tf = page.find_tables(strategy=strategy)
            except Exception as e:
                print(f"page {p + 1}: {strategy}: {e}", file=sys.stderr)
                continue
            for t in tf.tables:
                try:
                    rows = by_words(t, words, strategy == "text")
                except Exception as e:
                    print(f"page {p + 1}: words: {e}", file=sys.stderr)
                    rows = [[clean(c) for c in r] for r in t.extract()]
                rows = prune(rows)
                if not useful(rows):
                    continue
                # Text-only tables guessed on a page that already had ruled tables, or
                # one column of prose split in two: not kept.
                if strategy == "text" and find != "text" and t.col_count < 3:
                    continue
                h = t.header
                header = [clean(c) or "" for c in h.names] if h and h.external else None
                found.append({"rows": rows, "header": header, "headerOutside": bool(h and h.external),
                              "top": t.bbox[1], "bottom": t.bbox[3], "height": page.rect.height})
            if found:
                break
        found.sort(key=lambda t: t["top"])
        ev(ev="page_tables", page=p, tables=found)
        ev(ev="page", done=n + 1, of=len(pages))


if __name__ == "__main__":
    try:
        main()
    except Exception as e:
        ev(ev="error", code="engine_failed", message=f"{type(e).__name__}: {e}")
