using System.Collections.Generic;
using System.IO;
using System.Text;
using static OfficeWorker.Json;

namespace OfficeWorker
{
    /// <summary>plugin.json for both plugins (protocol v2).</summary>
    internal static class Manifests
    {
        public static readonly string[] PluginIds = { "word-to-pdf", "pdf-to-word" };

        public static List<string> WriteAll(string dir)
        {
            var written = new List<string>();
            foreach (var id in PluginIds)
            {
                var folder = Path.Combine(dir, id);
                Directory.CreateDirectory(folder);
                var path = Path.Combine(folder, "plugin.json");
                File.WriteAllText(path, Pretty(Serialize(Build(id))), new UTF8Encoding(false));
                written.Add(path);
            }
            return written;
        }

        public static Dictionary<string, object> Build(string id)
        {
            return id == "word-to-pdf" ? WordToPdf() : PdfToWord();
        }

        private static Dictionary<string, object> CommonErrors()
        {
            return Obj(
                "engine_missing", T("Lipsește programul de conversie. Descarcă-l din aplicație și încearcă din nou.",
                                    "The conversion program is missing. Download it from the app and try again."),
                "office_not_ready", T("Word nu a putut porni. Deschide Word o dată, închide orice fereastră care apare (activare, licență), apoi încearcă din nou.",
                                      "Word couldn't start. Open Word once, close any window it shows (activation, license), then try again."),
                "password_required", T("Fișierul e protejat cu parolă. Scrie parola ca să continui.",
                                       "This file is password-protected. Enter the password to continue."),
                "wrong_password", T("Parola nu e corectă. Mai încearcă o dată.",
                                    "That password isn't right. Try again."),
                "corrupt_file", T("Fișierul pare deteriorat și nu poate fi deschis.",
                                  "This file looks damaged and can't be opened."),
                "unsupported_file", T("Acest tip de fișier nu e acceptat aici.",
                                      "This kind of file isn't supported here."),
                "file_not_found", T("Fișierul nu mai există în locul acela. Poate a fost mutat sau șters.",
                                    "The file isn't there anymore. It may have been moved or deleted."),
                "path_too_long", T("Calea fișierului e prea lungă pentru Office. Mută-l într-un dosar cu un nume mai scurt și încearcă din nou.",
                                   "The file's path is too long for Office. Move it to a folder with a shorter name and try again."),
                "file_locked", T("Fișierul e folosit de alt program. Închide-l acolo și încearcă din nou.",
                                 "Another program is using this file. Close it there and try again."),
                "permission_denied", T("Nu ai voie să citești fișierul sau să scrii în dosarul ales.",
                                       "You don't have permission to read this file or write to the chosen folder."),
                // Also the text for LibreOffice and pdf2docx, which run when Word isn't used.
                "engine_failed", T("Conversia nu a reușit să termine. Încearcă din nou; dacă tot nu merge, verifică dacă fișierul se deschide în programul lui.",
                                   "The conversion couldn't finish. Try again; if it still fails, check that the file opens in its own program."),
                "cancelled", T("Oprit.", "Stopped."));
        }

        private static Dictionary<string, object> Entry(string command)
        {
            return Obj("kind", "dotnet", "worker", "office-worker", "command", command);
        }

        private static Dictionary<string, object> WordWhen(string otherEngine)
        {
            // Word when it is installed, unless the user picked the other engine.
            return Obj("all", Arr(
                Obj("env", "office.word", "eq", "ok"),
                Obj("field", "engine", "ne", otherEngine)));
        }

        private static Dictionary<string, object> Option(string value, string ro, string en)
        {
            return Obj("value", value, "label", T(ro, en));
        }

        private static Dictionary<string, object> WordToPdf()
        {
            return Obj(
                "protocol", 2,
                "id", "word-to-pdf",
                "version", Program.PluginVersion,
                "name", T("Word în PDF", "Word to PDF"),
                "summary", T("Gata de trimis", "Ready to send"),
                "keywords", Obj(
                    "ro", Arr("docx", "doc", "document", "word", "pdf", "rtf", "odt", "transformă", "salvează ca pdf", "trimite"),
                    "en", Arr("docx", "doc", "document", "word", "pdf", "rtf", "odt", "convert", "save as pdf", "send")),
                "category", "pdf",
                "icon", "file-export",
                "entry", Arr(
                    Obj("id", "word", "when", WordWhen("libreoffice"), "kind", "dotnet", "worker", "office-worker", "command", "word-to-pdf"),
                    Obj("id", "libreoffice", "kind", "worker", "worker", "worker-core", "command", "word-to-pdf-lo")),
                // The analysis is worker-core's whatever the engine: it also checks a typed password.
                "analyzeEntry", Obj("kind", "worker", "worker", "worker-core", "command", "word-to-pdf-lo"),
                "requires", Arr(Obj("engine", "libreoffice", "when", Obj("env", "resolvedEngine", "eq", "libreoffice"))),
                "input", Obj(
                    "extensions", Arr("doc", "docx", "docm", "dot", "dotx", "rtf", "odt", "txt"),
                    "multiple", true,
                    "folders", true,
                    "mode", "perFile"),
                "output", Obj(
                    "kind", "perInput",
                    "extensions", Arr("pdf"),
                    "nameVariables", Arr(Obj("id", "pages", "label", T("număr de pagini", "page count"))),
                    "replaceOriginalAllowed", false),
                "analysis", Obj("fast", Arr("format", "hasMacros", "hasTrackedChanges", "hasComments", "cachedPageCount", "needsPassword")),
                "ui", Obj("preview", "none", "estimate", false),
                "resources", Obj("class", "office", "diskFactor", 1.5),
                "presets", Arr(),
                "groups", Arr(Obj("id", "more", "label", T("Detalii", "Details"), "order", 1)),
                "fields", Arr(
                    Obj("key", "quality", "type", "select",
                        "label", T("Pentru", "For"),
                        "default", "print",
                        "options", Arr(
                            Option("print", "Tipar (calitate maximă)", "Print (best quality)"),
                            Option("screen", "Ecran (fișier mai mic)", "Screen (smaller file)"))),
                    Obj("key", "markup", "type", "select",
                        "label", T("Comentarii și modificări", "Comments and changes"),
                        "default", "clean",
                        "showIf", Obj("any", Arr(
                            Obj("file", "hasComments", "eq", true),
                            Obj("file", "hasTrackedChanges", "eq", true))),
                        "options", Arr(
                            Option("clean", "Nu le arăta (doar textul final)", "Hide them (final text only)"),
                            Option("withComments", "Arată-le în PDF", "Show them in the PDF"))),
                    Obj("key", "bookmarks", "type", "select", "advanced", true, "group", "more",
                        "label", T("Cuprins clicabil în PDF", "Clickable outline in the PDF"),
                        "default", "headings",
                        "options", Arr(
                            Option("headings", "Din titluri", "From headings"),
                            Option("word", "Din semnele de carte Word", "From Word bookmarks"),
                            Option("none", "Fără", "None"))),
                    Obj("key", "pdfA", "type", "toggle", "advanced", true, "group", "more",
                        "default", false,
                        "label", T("PDF/A pentru arhivare", "PDF/A for archiving")),
                    Obj("key", "includeProperties", "type", "toggle", "advanced", true, "group", "more",
                        "default", true,
                        "label", T("Păstrează autorul și titlul", "Keep author and title")),
                    Obj("key", "engine", "type", "select", "advanced", true, "group", "more",
                        "label", T("Cu ce program", "With which program"),
                        "default", "auto",
                        "showIf", Obj("env", "office.word", "eq", "ok"),
                        "options", Arr(
                            Option("auto", "Microsoft Word (recomandat)", "Microsoft Word (recommended)"),
                            Option("libreoffice", "LibreOffice", "LibreOffice")))),
                "rules", Arr(
                    Obj("id", "tracked_changes",
                        "when", Obj("file", "hasTrackedChanges", "eq", true),
                        "severity", "info",
                        "message", T("Documentul are modificări urmărite. Alege la „Comentarii și modificări” dacă apar în PDF.",
                                     "This document has tracked changes. Use “Comments and changes” to choose whether they show in the PDF.")),
                    Obj("id", "comments",
                        "when", Obj("all", Arr(
                            Obj("file", "hasComments", "eq", true),
                            Obj("file", "hasTrackedChanges", "eq", false))),
                        "severity", "info",
                        "message", T("Documentul are comentarii. Alege la „Comentarii și modificări” dacă apar în PDF.",
                                     "This document has comments. Use “Comments and changes” to choose whether they show in the PDF.")),
                    Obj("id", "macros",
                        "when", Obj("file", "hasMacros", "eq", true),
                        "severity", "info",
                        "message", T("Documentul conține macrocomenzi. Ele nu vor rula; PDF-ul arată documentul așa cum e salvat.",
                                     "This document contains macros. They won't run; the PDF shows the document as it was saved.")),
                    Obj("id", "needs_password",
                        "when", Obj("file", "needsPassword", "eq", true),
                        "severity", "warning",
                        "message", T("Documentul e protejat cu parolă. Vei avea nevoie de parolă ca să-l transformi.",
                                     "This document is password-protected. You'll need the password to convert it."))),
                "outputBlock", Obj("defaults", Obj("nameTemplate", T("{name}", "{name}"))),
                "errors", CommonErrors());
        }

        private static Dictionary<string, object> PdfToWord()
        {
            return Obj(
                "protocol", 2,
                "id", "pdf-to-word",
                "version", Program.PluginVersion,
                "name", T("PDF în Word (beta)", "PDF to Word (beta)"),
                "summary", T("Ca să poți modifica textul", "So you can edit the text"),
                "keywords", Obj(
                    "ro", Arr("pdf", "word", "docx", "document", "editează", "modifică", "transformă", "text"),
                    "en", Arr("pdf", "word", "docx", "document", "edit", "change", "convert", "text")),
                "category", "pdf",
                "icon", "file-import",
                "entry", Arr(
                    Obj("id", "exact", "when", Obj("field", "layout", "eq", "exact"), "kind", "worker", "worker", "worker-core", "command", "pdf-to-word-exact"),
                    // Word can't open encrypted PDFs (it shows a dialog, even for restrictions only): those go to pdf2docx.
                    Obj("id", "word", "when", Obj("all", Arr(WordWhen("pdf2docx"), Obj("not", Obj("file", "isEncrypted", "eq", true)))),
                        "kind", "dotnet", "worker", "office-worker", "command", "pdf-to-word"),
                    Obj("id", "pdf2docx", "kind", "python", "script", "pdf2docx_worker.py")),
                // The analysis (page sizes, designs) is done by worker-core, whatever the engine.
                "analyzeEntry", Obj("kind", "worker", "worker", "worker-core", "command", "pdf-to-word-exact"),
                "requires", Arr(Obj("engine", "pdf2docx", "when", Obj("env", "resolvedEngine", "eq", "pdf2docx"))),
                "input", Obj(
                    "extensions", Arr("pdf"),
                    "multiple", true,
                    "folders", true,
                    "mode", "perFile"),
                "output", Obj(
                    "kind", "perInput",
                    "extensions", Arr("docx", "rtf", "odt"),
                    "nameVariables", Arr(Obj("id", "pages", "label", T("număr de pagini", "page count"))),
                    "replaceOriginalAllowed", false),
                "analysis", Obj("fast", Arr("format", "pageCount", "needsPassword", "designLike", "oversizePages")),
                "ui", Obj("preview", "none", "estimate", false),
                "resources", Obj("class", "office", "diskFactor", 3),
                "presets", Arr(),
                "groups", Arr(Obj("id", "more", "label", T("Detalii", "Details"), "order", 1)),
                "fields", Arr(
                    Obj("key", "layout", "type", "select",
                        "label", T("Cum arată în Word", "How it looks in Word"),
                        "display", "chips",
                        "default", Arr(
                            Obj("when", Obj("file", "designLike", "eq", true, "quant", "any"), "value", "exact"),
                            Obj("value", "editable")),
                        "options", Arr(
                            Option("editable", "Text editabil", "Editable text"),
                            Option("exact", "Aspect identic", "Exact look"))),
                    Obj("key", "imageQuality", "type", "select",
                        "label", T("Calitatea paginilor", "Page quality"),
                        "default", "high",
                        "showIf", Obj("field", "layout", "eq", "exact"),
                        "options", Arr(
                            Option("high", "Înaltă (fișier mai mare)", "High (larger file)"),
                            Option("smaller", "Mai mică (fișier mai mic)", "Lower (smaller file)"))),
                    Obj("key", "format", "type", "select",
                        "label", T("Salvează ca", "Save as"),
                        "default", "docx",
                        "showIf", Obj("env", "resolvedEngine", "eq", "word"),
                        "options", Arr(
                            Option("docx", "Word (.docx)", "Word (.docx)"),
                            Option("rtf", "RTF (se deschide aproape oriunde)", "RTF (opens almost anywhere)"),
                            Option("odt", "OpenDocument (.odt, pentru LibreOffice)", "OpenDocument (.odt, for LibreOffice)"))),
                    Obj("key", "pages", "type", "pages", "syntax", "set", "default", "all",
                        "label", T("Pagini", "Pages"), "placeholder", T("toate", "all"),
                        "showIf", Obj("env", "resolvedEngine", "in", Arr("pdf2docx", "exact"))),
                    Obj("key", "engine", "type", "select", "advanced", true, "group", "more",
                        "label", T("Cu ce program", "With which program"),
                        "default", "auto",
                        "showIf", Obj("all", Arr(Obj("env", "office.word", "eq", "ok"), Obj("field", "layout", "eq", "editable"))),
                        "options", Arr(
                            Option("auto", "Microsoft Word (recomandat)", "Microsoft Word (recommended)"),
                            Option("pdf2docx", "pdf2docx (fără Word)", "pdf2docx (without Word)")))),
                "rules", Arr(
                    Obj("id", "design_pdf",
                        "when", Obj("all", Arr(Obj("file", "designLike", "eq", true), Obj("field", "layout", "eq", "editable"))),
                        "severity", "warning",
                        "message", T("Acest PDF arată ca o prezentare sau un design (fundaluri colorate, pagini mari). Ca text editabil își pierde fundalurile și așezarea. „Aspect identic” păstrează totul, dar paginile devin imagini în Word.",
                                     "This PDF looks like a presentation or a design (coloured backgrounds, large pages). As editable text it loses its backgrounds and layout. “Exact look” keeps everything, but the pages become pictures in Word.")),
                    Obj("id", "exact_info",
                        "when", Obj("field", "layout", "eq", "exact"),
                        "severity", "info",
                        "message", T("Fiecare pagină devine o imagine în Word: arată exact ca PDF-ul, dar textul nu se poate modifica.",
                                     "Each page becomes a picture in Word: it looks exactly like the PDF, but the text can't be edited.")),
                    Obj("id", "layout_may_differ",
                        "when", Obj("all", Arr(Obj("file", "format", "eq", "pdf"), Obj("field", "layout", "eq", "editable"), Obj("file", "designLike", "eq", false, "quant", "all"))),
                        "severity", "info",
                        "message", T("Aspectul poate ieși puțin diferit față de PDF, mai ales la tabele și coloane. Verifică rezultatul. Paginile scanate rămân imagini.",
                                     "The layout may come out a little different from the PDF, especially tables and columns. Check the result. Scanned pages stay as pictures.")),
                    Obj("id", "needs_password",
                        "when", Obj("file", "needsPassword", "eq", true),
                        "severity", "warning",
                        "message", T("PDF-ul e protejat cu parolă. Vei avea nevoie de parolă ca să-l transformi.",
                                     "This PDF is password-protected. You'll need the password to convert it."))),
                "outputBlock", Obj("defaults", Obj("nameTemplate", T("{name}", "{name}"))),
                "errors", CommonErrors());
        }
    }
}
