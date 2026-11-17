"""Generates test documents for office-worker.

Writes into  testdata/in/documente ăîșț/ :
  contract închiriere.docx   heading, table, comment, tracked change, Romanian diacritics (raw OOXML, no Word needed)
  mare ștampilă.docx          long document (for the cancel test)
  protejat cu parolă.docx     password-protected copy (made by Word through COM, see word_protect.ps1)

Usage: python make_testdata.py
"""
import os
import subprocess
import sys
import zipfile
from xml.sax.saxutils import escape

HERE = os.path.dirname(os.path.abspath(__file__))
APP = os.path.abspath(os.path.join(HERE, "..", "..", ".."))
IN_DIR = os.path.join(APP, "testdata", "in", "documente ăîșț")

PASSWORD = "parolă123"  # test password, only for this generated fixture

W = 'xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" ' \
    'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"'

CONTENT_TYPES = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
<Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/>
{comments_ct}
<Override PartName="/docProps/app.xml" ContentType="application/vnd.openxmlformats-officedocument.extended-properties+xml"/>
<Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/>
</Types>"""

ROOT_RELS = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties" Target="docProps/app.xml"/>
<Relationship Id="rId3" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/>
</Relationships>"""

DOC_RELS = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>
{comments_rel}
</Relationships>"""

STYLES = f"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles {W}>
<w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Calibri" w:hAnsi="Calibri" w:cs="Calibri"/><w:sz w:val="22"/><w:lang w:val="ro-RO"/></w:rPr></w:rPrDefault>
<w:pPrDefault><w:pPr><w:spacing w:after="160" w:line="259" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults>
<w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/><w:qFormat/></w:style>
<w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/>
<w:pPr><w:keepNext/><w:spacing w:before="240" w:after="120"/><w:outlineLvl w:val="0"/></w:pPr><w:rPr><w:b/><w:color w:val="1F3864"/><w:sz w:val="36"/></w:rPr></w:style>
<w:style w:type="paragraph" w:styleId="Heading2"><w:name w:val="heading 2"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/>
<w:pPr><w:keepNext/><w:spacing w:before="200" w:after="80"/><w:outlineLvl w:val="1"/></w:pPr><w:rPr><w:b/><w:sz w:val="28"/></w:rPr></w:style>
<w:style w:type="table" w:default="1" w:styleId="TableNormal"><w:name w:val="Normal Table"/><w:tblPr><w:tblInd w:w="0" w:type="dxa"/>
<w:tblCellMar><w:top w:w="0" w:type="dxa"/><w:left w:w="108" w:type="dxa"/><w:bottom w:w="0" w:type="dxa"/><w:right w:w="108" w:type="dxa"/></w:tblCellMar></w:tblPr></w:style>
<w:style w:type="table" w:styleId="TableGrid"><w:name w:val="Table Grid"/><w:basedOn w:val="TableNormal"/><w:tblPr><w:tblBorders>
<w:top w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:left w:val="single" w:sz="4" w:space="0" w:color="auto"/>
<w:bottom w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:right w:val="single" w:sz="4" w:space="0" w:color="auto"/>
<w:insideH w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:insideV w:val="single" w:sz="4" w:space="0" w:color="auto"/>
</w:tblBorders></w:tblPr></w:style>
</w:styles>"""

COMMENTS = f"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:comments {W}>
<w:comment w:id="0" w:author="Ștefan Țurcanu" w:date="2026-11-16T10:00:00Z" w:initials="ȘȚ">
<w:p><w:r><w:t>Verifică suma chiriei înainte de semnare (comentariu de test).</w:t></w:r></w:p>
</w:comment>
</w:comments>"""

CORE = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
<dc:title>{title}</dc:title><dc:creator>Ștefan Țurcanu</dc:creator>
<dcterms:created xsi:type="dcterms:W3CDTF">2026-11-16T10:00:00Z</dcterms:created>
</cp:coreProperties>"""

APP_XML = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties"><Application>MQKit test generator</Application><Pages>{pages}</Pages></Properties>"""


def p(text, style=None):
    ppr = f'<w:pPr><w:pStyle w:val="{style}"/></w:pPr>' if style else ""
    return f'<w:p>{ppr}<w:r><w:t xml:space="preserve">{escape(text)}</w:t></w:r></w:p>'


def table(rows):
    out = ['<w:tbl><w:tblPr><w:tblStyle w:val="TableGrid"/><w:tblW w:w="0" w:type="auto"/></w:tblPr><w:tblGrid>']
    out += ['<w:gridCol w:w="3000"/>' for _ in rows[0]]
    out.append("</w:tblGrid>")
    for i, row in enumerate(rows):
        out.append("<w:tr>")
        for cell in row:
            run = f'<w:r>{"<w:rPr><w:b/></w:rPr>" if i == 0 else ""}<w:t xml:space="preserve">{escape(cell)}</w:t></w:r>'
            out.append(f'<w:tc><w:tcPr><w:tcW w:w="3000" w:type="dxa"/></w:tcPr><w:p>{run}</w:p></w:tc>')
        out.append("</w:tr>")
    out.append("</w:tbl>")
    return "".join(out)


def write_docx(path, body_xml, title, with_comments, pages):
    doc = f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document {W}><w:body>{body_xml}' \
          '<w:sectPr><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1417" w:right="1417" w:bottom="1417" w:left="1417" w:header="708" w:footer="708" w:gutter="0"/></w:sectPr>' \
          '</w:body></w:document>'
    comments_ct = '<Override PartName="/word/comments.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.comments+xml"/>' if with_comments else ""
    comments_rel = '<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/comments" Target="comments.xml"/>' if with_comments else ""
    tmp = path + ".tmp"
    with zipfile.ZipFile(tmp, "w", zipfile.ZIP_DEFLATED) as z:
        z.writestr("[Content_Types].xml", CONTENT_TYPES.format(comments_ct=comments_ct))
        z.writestr("_rels/.rels", ROOT_RELS)
        z.writestr("word/_rels/document.xml.rels", DOC_RELS.format(comments_rel=comments_rel))
        z.writestr("word/document.xml", doc)
        z.writestr("word/styles.xml", STYLES)
        if with_comments:
            z.writestr("word/comments.xml", COMMENTS)
        z.writestr("docProps/core.xml", CORE.format(title=escape(title)))
        z.writestr("docProps/app.xml", APP_XML.format(pages=pages))
    os.replace(tmp, path)


def contract_body():
    parts = [
        p("Contract de închiriere", "Heading1"),
        p("Încheiat astăzi între Ștefan Țurcanu (proprietar) și Ioana Mureșan (chiriaș), "
          "pentru apartamentul din strada Mărășești nr. 7, Iași."),
        p("1. Obiectul contractului", "Heading2"),
        # paragraph with a comment range and a tracked insertion
        '<w:p><w:commentRangeStart w:id="0"/><w:r><w:t xml:space="preserve">Chiria lunară este de 2.500 lei, plătibilă până în data de 5 a fiecărei luni</w:t></w:r>'
        '<w:commentRangeEnd w:id="0"/><w:r><w:commentReference w:id="0"/></w:r>'
        '<w:ins w:id="1" w:author="Ioana Mureșan" w:date="2026-11-16T10:05:00Z"><w:r><w:t xml:space="preserve">, prin transfer bancar</w:t></w:r></w:ins>'
        '<w:r><w:t>.</w:t></w:r></w:p>',
        p("2. Inventar", "Heading2"),
        table([
            ["Obiect", "Cantitate", "Stare"],
            ["Mașină de spălat", "1", "bună"],
            ["Frigider", "1", "foarte bună"],
            ["Scaune de bucătărie", "4", "uzate ușor"],
        ]),
        p("3. Obligații", "Heading2"),
        p("Chiriașul păstrează locuința curată și anunță orice defecțiune în cel mult trei zile. "
          "Diacritice de verificare: ă â î ș ț Ă Â Î Ș Ț."),
    ]
    return "".join(parts)


def big_body(chapters=60, paras=25):
    lorem = ("Aceasta este o propoziție lungă de umplutură, cu diacritice românești: ăâîșț ĂÂÎȘȚ. "
             "Scopul ei este să facă documentul mare, ca exportul în PDF să dureze câteva secunde. ") * 3
    out = [p("Document mare pentru testul de anulare", "Heading1")]
    for c in range(1, chapters + 1):
        out.append(p(f"Capitolul {c}", "Heading2"))
        for i in range(paras):
            out.append(p(f"{c}.{i + 1} " + lorem))
        out.append(table([["Coloana A", "Coloana B", "Coloana C"]] + [[f"r{r} a", f"r{r} ș", f"r{r} ț"] for r in range(8)]))
    return "".join(out)


def main():
    os.makedirs(IN_DIR, exist_ok=True)
    contract = os.path.join(IN_DIR, "contract închiriere.docx")
    write_docx(contract, contract_body(), "Contract de închiriere", True, 1)
    print("wrote", contract)

    big = os.path.join(IN_DIR, "mare ștampilă.docx")
    write_docx(big, big_body(), "Document mare", False, 300)
    print("wrote", big)

    protected = os.path.join(IN_DIR, "protejat cu parolă.docx")
    if not os.path.exists(protected) or "--force" in sys.argv:
        ps1 = os.path.join(HERE, "word_protect.ps1")
        env = dict(os.environ, OW_TEST_PASSWORD=PASSWORD)
        r = subprocess.run(["powershell", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", ps1, contract, protected],
                           env=env, capture_output=True, text=True, encoding="utf-8", timeout=180)
        print(r.stdout.strip())
        if r.returncode != 0:
            print(r.stderr, file=sys.stderr)
            sys.exit("word_protect.ps1 failed")
    print("wrote", protected)


if __name__ == "__main__":
    main()
