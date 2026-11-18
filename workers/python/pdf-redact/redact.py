"""Redact a PDF for real with PyMuPDF: what is under each black box (text, pictures,
drawings) is taken out of the file, not just covered.

    python -I redact.py <site-packages;…> <spec.json> <in.pdf> <out.pdf>

spec.json: {"terms": ["Ana Pop", …], "patterns": ["email", "phone", "iban", "card", "idnum"],
            "boxes": [{"page": 1, "x": .., "y": .., "w": .., "h": ..}],   # fractions of the page as shown
            "color": [0, 0, 0], "scrub": true, "password": null}
stdout: "progress <0..1>" lines, then "done <count>" or "error <code> <message>".
"""
import json
import re
import sys

for p in reversed(sys.argv[1].split(";")):
    if p and p not in sys.path:
        sys.path.insert(0, p)
sys.dont_write_bytecode = True

PATTERNS = {
    "email": r"[\w.+-]+@[\w-]+(?:\.[\w-]+)+",
    # +40 721 234 567, (021) 123-4567, 0721234567: at least 9 digits with the usual separators.
    "phone": r"(?<!\w)\+?\(?\d{1,4}\)?(?:[ .\-/]?\(?\d{2,4}\)?){2,5}(?!\w)",
    "iban": r"\b[A-Z]{2}\d{2}(?:[ ]?[A-Z0-9]{4}){2,7}(?:[ ]?[A-Z0-9]{1,4})?\b",
    "card": r"(?<!\d)(?:\d[ -]?){12,18}\d(?!\d)",
    # National ID and tax numbers: long runs of digits (9–13).
    "idnum": r"(?<!\d)\d{9,13}(?!\d)",
}


def luhn(s):
    d = [int(c) for c in s if c.isdigit()]
    total = 0
    for i, n in enumerate(reversed(d)):
        if i % 2:
            n *= 2
            if n > 9:
                n -= 9
        total += n
    return total % 10 == 0


def keep(kind, text):
    digits = sum(c.isdigit() for c in text)
    if kind == "phone":
        return 9 <= digits <= 15
    if kind == "card":
        return 13 <= digits <= 19 and luhn(text)
    return True


def say(line):
    sys.stdout.write(line + "\n")
    sys.stdout.flush()


def lines_of(page):
    """Each line of text with the box of every character, for matching patterns."""
    out = []
    raw = page.get_text("rawdict")
    for block in raw.get("blocks", []):
        for line in block.get("lines", []):
            chars = [c for span in line.get("spans", []) for c in span.get("chars", [])]
            if chars:
                out.append(("".join(c["c"] for c in chars), [c["bbox"] for c in chars]))
    return out


def main():
    import pymupdf

    spec = json.load(open(sys.argv[2], encoding="utf-8"))
    doc = pymupdf.open(sys.argv[3])
    if doc.needs_pass and not doc.authenticate(spec.get("password") or ""):
        say("error password_required")
        return
    color = tuple(c / 255 for c in spec.get("color", [0, 0, 0]))
    terms = [t for t in (spec.get("terms") or []) if t.strip()]
    patterns = [PATTERNS[p] and p for p in (spec.get("patterns") or []) if p in PATTERNS]
    boxes = spec.get("boxes") or []
    count = 0
    n = len(doc)
    for i, page in enumerate(doc):
        say(f"progress {i / max(n, 1):.3f}")
        rects = []
        for t in terms:
            rects += page.search_for(t.strip())
        if patterns:
            for text, bboxes in lines_of(page):
                for kind in patterns:
                    for m in re.finditer(PATTERNS[kind], text):
                        if not keep(kind, m.group(0)):
                            continue
                        r = pymupdf.Rect()
                        for b in bboxes[m.start():m.end()]:
                            r |= pymupdf.Rect(b)
                        if not r.is_empty:
                            rects.append(r)
        # Boxes drawn in the app: fractions of the page as shown → the page's own space.
        shown = page.rect
        for b in boxes:
            if int(b.get("page", 0)) != i + 1:
                continue
            r = pymupdf.Rect(b["x"] * shown.width, b["y"] * shown.height,
                             (b["x"] + b["w"]) * shown.width, (b["y"] + b["h"]) * shown.height)
            rects.append(r * page.derotation_matrix)
        for r in rects:
            page.add_redact_annot(r, fill=color)
        if rects:
            # Text and drawings under a box go; pictures lose only the covered pixels.
            page.apply_redactions(images=pymupdf.PDF_REDACT_IMAGE_PIXELS,
                                  graphics=pymupdf.PDF_REDACT_LINE_ART_REMOVE_IF_COVERED)
            count += len(rects)
    if spec.get("scrub", True):
        # Metadata, attached files, comments, hidden text, scripts: what a PDF can carry unseen.
        doc.scrub(attached_files=True, clean_pages=True, embedded_files=True, hidden_text=True,
                  javascript=True, metadata=True, redactions=False, redact_images=0, remove_links=False,
                  reset_fields=False, reset_responses=True, thumbnails=True, xml_metadata=True)
    doc.save(sys.argv[4], garbage=4, deflate=True)
    say(f"done {count}")


if __name__ == "__main__":
    try:
        main()
    except Exception as e:  # reported to the app as one line
        say(f"error failed {type(e).__name__}: {e}")
