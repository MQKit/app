using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Runtime.InteropServices;

namespace OfficeWorker
{
    internal static class Converters
    {
        // Word constants
        private const int wdExportFormatPDF = 17;
        private const int wdExportOptimizeForPrint = 0;
        private const int wdExportOptimizeForOnScreen = 1;
        private const int wdExportAllDocument = 0;
        private const int wdExportDocumentContent = 0;
        private const int wdExportDocumentWithMarkup = 7;
        private const int wdExportCreateNoBookmarks = 0;
        private const int wdExportCreateHeadingBookmarks = 1;
        private const int wdExportCreateWordBookmarks = 2;
        private const int wdStatisticPages = 2;
        private const int wdFormatDocumentDefault = 16;
        private const int wdFormatRTF = 6;
        private const int wdFormatOpenDocumentText = 23;

        public static string OutputFolder(RunJob job, FileItem f)
        {
            return Path.Combine(job.OutDir, f.Id);
        }

        public static void DeletePartialOutputs(RunJob job, FileItem f)
        {
            try
            {
                var dir = OutputFolder(job, f);
                if (!Directory.Exists(dir)) return;
                foreach (var p in Directory.GetFiles(dir, "result.*"))
                {
                    try { File.Delete(p); } catch { }
                }
            }
            catch { }
        }

        /// <summary>Checks the input is there and readable, without modifying it.</summary>
        private static long Preflight(string path)
        {
            if (string.IsNullOrEmpty(path) || !File.Exists(path))
            {
                if (!string.IsNullOrEmpty(path) && Directory.Exists(path))
                    throw new WorkerError("unsupported_file", "path is a folder", Json.Obj("reason", "is_directory"));
                throw new WorkerError("file_not_found", "file not found", Json.Obj("path", path ?? ""));
            }
            try
            {
                using (var fs = Analyzer.OpenShared(path))
                {
                    if (fs.Length > 0) fs.ReadByte();
                    return fs.Length;
                }
            }
            catch (UnauthorizedAccessException ex)
            {
                throw new WorkerError("permission_denied", ex.Message);
            }
            catch (IOException ex)
            {
                throw IoError(ex, path);
            }
        }

        /// <summary>
        /// A file system error as a protocol error: only a sharing or lock violation means
        /// another program has the file open; a path that is too long, or a drive or network
        /// share that went away, are said as such.
        /// </summary>
        public static WorkerError IoError(Exception ex, string path)
        {
            var details = Json.Obj("message", ex.Message, "hresult", Com.Hex(ex.HResult), "path", path ?? "");
            if (ex is UnauthorizedAccessException) return new WorkerError("permission_denied", ex.Message, details);
            if (ex is PathTooLongException) return new WorkerError("path_too_long", ex.Message, details);
            if (ex is FileNotFoundException || ex is DirectoryNotFoundException || ex is DriveNotFoundException)
                return new WorkerError("file_not_found", ex.Message, details);
            switch (ex.HResult & 0xFFFF)
            {
                case 32:   // ERROR_SHARING_VIOLATION
                case 33:   // ERROR_LOCK_VIOLATION
                    return new WorkerError("file_locked", ex.Message, details);
                case 206:  // ERROR_FILENAME_EXCED_RANGE
                    return new WorkerError("path_too_long", ex.Message, details);
                case 39:   // ERROR_HANDLE_DISK_FULL
                case 112:  // ERROR_DISK_FULL
                    return new WorkerError("disk_full", ex.Message, details);
                case 2: case 3: case 15:      // not found, path not found, invalid drive
                case 21:                      // ERROR_NOT_READY (card or disc removed)
                case 53: case 64: case 67:    // network path or share gone
                case 55: case 1167:           // device gone, device not connected
                    details["reason"] = "device_gone";
                    return new WorkerError("file_not_found", ex.Message, details);
                default:
                    return new WorkerError("engine_failed", ex.Message, details);
            }
        }

        private static string PrepareOutput(RunJob job, FileItem f, string ext)
        {
            var dir = OutputFolder(job, f);
            try
            {
                Directory.CreateDirectory(dir);
                var path = Path.Combine(dir, "result." + ext);
                if (File.Exists(path)) File.Delete(path);
                return path;
            }
            catch (UnauthorizedAccessException ex)
            {
                throw new WorkerError("permission_denied", "cannot write output: " + ex.Message, Json.Obj("path", dir));
            }
            catch (IOException ex)
            {
                // The output folder's drive gone is "can't write there", not "the file is gone".
                var e = IoError(ex, dir);
                if (e.Code == "file_not_found" || e.Code == "engine_failed")
                    throw new WorkerError("permission_denied", "cannot write output: " + ex.Message, e.Details, ex);
                throw e;
            }
        }

        private static string Opt(RunJob job, string key, string def)
        {
            return Json.Str(job.Options, key) ?? def;
        }

        private static bool OptBool(RunJob job, string key, bool def)
        {
            return Json.Bool(job.Options, key) ?? def;
        }

        /// <summary>Maps a Documents.Open failure to a protocol error.</summary>
        private static WorkerError MapOpenError(RunJob job, Exception ex, FileItem f, bool likelyEncrypted)
        {
            if (job.HardCancel) return new WorkerError("cancelled", "cancelled by user");
            if (ex is WorkerError we) return we;
            if (job.AbortReason != null || Com.IsServerGone(ex)) return null; // let the job classify it
            int hr = Com.HResult(ex);
            int num = Com.WordErrorNumber(hr);
            string msg = ex.Message ?? "";
            string lower = msg.ToLowerInvariant();
            var details = Json.Obj("message", msg, "hresult", Com.Hex(hr));
            if (num != 0) details["wordError"] = num;

            bool passwordError = num == 5408 || lower.Contains("password") || lower.Contains("parol") || lower.Contains("kennwort") || lower.Contains("mot de passe");
            if (passwordError || (likelyEncrypted && num != 0))
                return new WorkerError(string.IsNullOrEmpty(f.Password) ? "password_required" : "wrong_password", msg, details);
            if (num == 5174 || num == 5273)
                return new WorkerError("file_not_found", msg, details);
            if (lower.Contains("corrupt") || lower.Contains("deteriorat") || lower.Contains("unreadable") || lower.Contains("problems with its contents") || num == 5792
                || lower.Contains("file format or file extension is not valid"))
                return new WorkerError("corrupt_file", msg, details);
            if (lower.Contains("locked") || lower.Contains("in use") || lower.Contains("blocat") || lower.Contains("în uz"))
                return new WorkerError("file_locked", msg, details);
            if (lower.Contains("access") && lower.Contains("denied"))
                return new WorkerError("permission_denied", msg, details);
            return new WorkerError("engine_failed", msg, details);
        }

        private static long FileSize(string path)
        {
            try { return new FileInfo(path).Length; } catch { return 0; }
        }

        // ------------------------------------------------------------ word-to-pdf

        public static FileResult WordToPdf(RunJob job, FileItem f)
        {
            string ext = Analyzer.Ext(f.Path);
            if (!Analyzer.WordExtensions.Contains(ext))
                throw new WorkerError("unsupported_file", "unsupported file type: ." + ext, Json.Obj("ext", ext));
            long inputBytes = Preflight(f.Path);

            // OFFICE_WORKER_NO_PRECHECK=1 (tests only) lets Word itself face protected/damaged files.
            bool precheck = Environment.GetEnvironmentVariable("OFFICE_WORKER_NO_PRECHECK") != "1";
            string check = precheck ? Analyzer.QuickWordCheck(f.Path, ext) : "ok";
            bool likelyEncrypted = check == "locked";
            if (check == "corrupt") throw new WorkerError("corrupt_file", "the file is damaged or not a " + ext + " document");
            if (likelyEncrypted && string.IsNullOrEmpty(f.Password))
                throw new WorkerError("password_required", "the document is password protected");

            int optimizeFor = Opt(job, "quality", "print") == "screen" ? wdExportOptimizeForOnScreen : wdExportOptimizeForPrint;
            bool pdfA = OptBool(job, "pdfA", false);
            int bookmarks;
            switch (Opt(job, "bookmarks", "headings"))
            {
                case "word": bookmarks = wdExportCreateWordBookmarks; break;
                case "none": bookmarks = wdExportCreateNoBookmarks; break;
                default: bookmarks = wdExportCreateHeadingBookmarks; break;
            }
            bool withMarkup = Opt(job, "markup", "clean") == "withComments";
            int item = withMarkup ? wdExportDocumentWithMarkup : wdExportDocumentContent;
            bool includeProps = OptBool(job, "includeProperties", true);

            string outPath = PrepareOutput(job, f, "pdf");
            var word = job.EnsureWord();
            object doc = null;
            try
            {
                job.SetProgress(0.05, "open", 1, 2);
                job.BeginOp("open");
                try
                {
                    doc = word.Open(f.Path, f.Password, false);
                }
                catch (Exception ex)
                {
                    var mapped = MapOpenError(job, ex, f, likelyEncrypted);
                    if (mapped != null) throw mapped;
                    throw;
                }
                finally { job.EndOp(); }
                job.CheckCancel();

                dynamic d = doc;
                job.SetProgress(0.25, "open", 1, 2);
                int pages = 0;
                job.BeginOp("paginate");
                try { pages = d.ComputeStatistics(wdStatisticPages); }
                catch (Exception ex) when (!Com.IsServerGone(ex) && job.AbortReason == null && !job.HardCancel) { Log.Info("ComputeStatistics: " + ex.Message); }
                finally { job.EndOp(); }
                job.CheckCancel();

                job.SetProgress(0.35, "export", 2, 2);
                job.BeginOp("export");
                try
                {
                    d.ExportAsFixedFormat(
                        outPath,            // OutputFileName
                        wdExportFormatPDF,  // ExportFormat
                        false,              // OpenAfterExport
                        optimizeFor,        // OptimizeFor
                        wdExportAllDocument,// Range
                        1,                  // From
                        1,                  // To
                        item,               // Item
                        includeProps,       // IncludeDocProps
                        true,               // KeepIRM
                        bookmarks,          // CreateBookmarks
                        true,               // DocStructureTags
                        true,               // BitmapMissingFonts
                        pdfA);              // UseISO19005_1
                }
                finally { job.EndOp(); }
                job.CheckCancel();

                long outBytes = FileSize(outPath);
                if (outBytes <= 0 || !LooksLikePdf(outPath))
                    throw new WorkerError("engine_failed", "Word did not produce a PDF");
                job.SetProgress(1.0, "export", 2, 2);

                var r = new FileResult { OutputPath = outPath, Ext = "pdf", InputBytes = inputBytes, OutputBytes = outBytes };
                if (pages > 0) r.Vars["pages"] = pages;
                return r;
            }
            finally
            {
                if (doc != null && !job.HardCancel && job.AbortReason == null) WordHost.CloseDoc(doc);
                else Com.Release(doc);
            }
        }

        private static bool LooksLikePdf(string path)
        {
            try
            {
                using (var fs = Analyzer.OpenShared(path))
                {
                    var b = new byte[5];
                    return fs.Read(b, 0, 5) == 5 && b[0] == '%' && b[1] == 'P' && b[2] == 'D' && b[3] == 'F' && b[4] == '-';
                }
            }
            catch { return false; }
        }

        // ------------------------------------------------------------ office-to-pdf

        public static readonly string[] SheetExtensions = { "xlsx", "xlsm", "xls", "xlsb", "ods" };
        public static readonly string[] SlideExtensions = { "pptx", "pptm", "ppt", "pps", "ppsx", "odp" };

        /// <summary>A modern Office file inside an OLE container is an encrypted one.</summary>
        private static bool OoxmlEncrypted(string path, string ext)
        {
            if (!(ext == "xlsx" || ext == "xlsm" || ext == "xlsb" || ext == "pptx" || ext == "pptm" || ext == "ppsx")) return false;
            try
            {
                using (var fs = Analyzer.OpenShared(path))
                {
                    var b = new byte[4];
                    return fs.Read(b, 0, 4) == 4 && b[0] == 0xD0 && b[1] == 0xCF && b[2] == 0x11 && b[3] == 0xE0;
                }
            }
            catch { return false; }
        }

        /// <summary>Excel and PowerPoint files to PDF with the Office program that made them.</summary>
        public static FileResult OfficeToPdf(RunJob job, FileItem f)
        {
            string ext = Analyzer.Ext(f.Path);
            bool sheets = SheetExtensions.Contains(ext);
            if (!sheets && !SlideExtensions.Contains(ext))
                throw new WorkerError("unsupported_file", "unsupported file type: ." + ext, Json.Obj("ext", ext));
            long inputBytes = Preflight(f.Path);
            bool locked = OoxmlEncrypted(f.Path, ext);
            if (locked && string.IsNullOrEmpty(f.Password))
                throw new WorkerError("password_required", "the file is password protected");

            bool screen = Opt(job, "quality", "print") == "screen";
            string outPath = PrepareOutput(job, f, "pdf");
            if (sheets) ExcelToPdf(job, f, outPath, screen, OptBool(job, "sheetPerPage", false), locked);
            else PowerPointToPdf(job, f, outPath, screen, OptBool(job, "notes", false), OptBool(job, "pdfA", false), locked);
            job.CheckCancel();

            long outBytes = FileSize(outPath);
            if (outBytes <= 0 || !LooksLikePdf(outPath))
                throw new WorkerError("engine_failed", "Office did not produce a PDF");
            job.SetProgress(1.0, "export", 2, 2);
            var r = new FileResult { OutputPath = outPath, Ext = "pdf", InputBytes = inputBytes, OutputBytes = outBytes };
            int pages = PdfPageCount(outPath);
            if (pages > 0) r.Vars["pages"] = pages;
            return r;
        }

        private static void ExcelToPdf(RunJob job, FileItem f, string outPath, bool screen, bool onePage, bool locked)
        {
            var xl = job.EnsureApp("excel");
            dynamic app = xl.App;
            object books = null, wb = null;
            try
            {
                job.SetProgress(0.05, "open", 1, 2);
                job.BeginOp("open");
                try
                {
                    object m = Type.Missing;
                    object pwd = string.IsNullOrEmpty(f.Password) ? WordHost.DummyPassword : f.Password;
                    books = app.Workbooks;
                    wb = ((dynamic)books).Open(
                        f.Path,  // FileName
                        0,       // UpdateLinks: never
                        true,    // ReadOnly
                        m,       // Format
                        pwd,     // Password
                        m,       // WriteResPassword
                        true,    // IgnoreReadOnlyRecommended
                        m, m,    // Origin, Delimiter
                        false,   // Editable
                        false,   // Notify
                        m,       // Converter
                        false);  // AddToMru
                }
                catch (Exception ex)
                {
                    throw MapOfficeOpenError(job, ex, f, locked);
                }
                finally { job.EndOp(); }
                job.CheckCancel();
                dynamic book = wb;

                if (onePage)
                {
                    // The printer isn't asked about each change: much faster with many sheets.
                    try { app.PrintCommunication = false; } catch { }
                    job.BeginOp("pagesetup");
                    try
                    {
                        foreach (dynamic ws in book.Worksheets)
                        {
                            try
                            {
                                dynamic ps = ws.PageSetup;
                                ps.Zoom = false;
                                ps.FitToPagesWide = 1;
                                ps.FitToPagesTall = 1;
                                Com.Release((object)ps);
                            }
                            catch (Exception ex) when (!Com.IsServerGone(ex)) { Log.Info("PageSetup: " + ex.Message); }
                            Com.Release((object)ws);
                        }
                    }
                    finally { job.EndOp(); }
                    try { app.PrintCommunication = true; } catch { }
                    job.CheckCancel();
                }

                job.SetProgress(0.35, "export", 2, 2);
                job.BeginOp("export");
                try
                {
                    object m = Type.Missing;
                    book.ExportAsFixedFormat(
                        0,                // xlTypePDF
                        outPath,          // Filename
                        screen ? 1 : 0,   // xlQualityMinimum / xlQualityStandard
                        true,             // IncludeDocProperties
                        false,            // IgnorePrintAreas
                        m, m,             // From, To
                        false);           // OpenAfterPublish
                }
                catch (Exception ex) when (!Com.IsServerGone(ex) && job.AbortReason == null && !job.HardCancel)
                {
                    // Excel refuses a workbook with nothing in it.
                    if (NothingToPrint(app, book))
                        throw new WorkerError("nothing_to_print", "the sheets are empty");
                    throw;
                }
                finally { job.EndOp(); }
            }
            finally
            {
                if (wb != null && (xl.Proc == null || xl.IsAlive()))
                {
                    try { ((dynamic)wb).Close(false); } catch (Exception ex) { Log.Info("workbook.Close: " + ex.Message); }
                }
                Com.Release(wb);
                Com.Release(books);
            }
        }

        /// <summary>
        /// An Open that failed: a file that exists and is readable but that Excel or PowerPoint
        /// can't open is a damaged one (their errors say no more, e.g. 1004 or 0x80CB4000).
        /// PowerPoint answers a wrong password with a message box, which the watchdog closes.
        /// </summary>
        private static Exception MapOfficeOpenError(RunJob job, Exception ex, FileItem f, bool locked)
        {
            var mapped = MapOpenError(job, ex, f, locked);
            if (mapped == null)
            {
                if (locked && job.AbortReason == "dialog")
                    return new WorkerError(string.IsNullOrEmpty(f.Password) ? "password_required" : "wrong_password", "the password was not accepted");
                return ex;
            }
            if (mapped.Code == "engine_failed")
                return new WorkerError("corrupt_file", mapped.Message, mapped.Details, ex);
            return mapped;
        }

        /// <summary>No visible sheet holds a value, a picture, a chart or a shape.</summary>
        private static bool NothingToPrint(dynamic app, dynamic book)
        {
            try
            {
                foreach (dynamic sh in book.Sheets)
                {
                    try
                    {
                        if ((int)sh.Visible != -1) continue; // xlSheetVisible
                        dynamic used = sh.UsedRange; // a chart sheet has none: it has something to print
                        if ((double)app.WorksheetFunction.CountA(used) > 0) return false;
                        if ((int)sh.Shapes.Count > 0) return false;
                    }
                    catch { return false; }
                }
                return true;
            }
            catch { return false; }
        }

        private static void PowerPointToPdf(RunJob job, FileItem f, string outPath, bool screen, bool notes, bool pdfA, bool locked)
        {
            var pp = job.EnsureApp("powerpoint");
            dynamic app = pp.App;
            object list = null, pres = null;
            try
            {
                job.SetProgress(0.05, "open", 1, 2);
                job.BeginOp("open");
                try
                {
                    // "file::password::" opens a protected presentation; a wrong one makes it fail
                    // rather than ask, and files without a password ignore it.
                    string pwd = string.IsNullOrEmpty(f.Password) ? WordHost.DummyPassword : f.Password;
                    list = app.Presentations;
                    pres = ((dynamic)list).Open(f.Path + "::" + pwd + "::", -1, 0, 0); // ReadOnly, not Untitled, no window
                }
                catch (Exception ex)
                {
                    throw MapOfficeOpenError(job, ex, f, locked);
                }
                finally { job.EndOp(); }
                job.CheckCancel();

                dynamic p = pres;
                job.SetProgress(0.35, "export", 2, 2);
                job.BeginOp("export");
                try
                {
                    p.ExportAsFixedFormat(
                        outPath,
                        2,                 // ppFixedFormatTypePDF
                        screen ? 1 : 2,    // ppFixedFormatIntentScreen / Print
                        0,                 // FrameSlides
                        1,                 // HandoutOrder
                        notes ? 5 : 1,     // ppPrintOutputNotesPages / ppPrintOutputSlides
                        0,                 // PrintHiddenSlides
                        new DispatchWrapper(null), // PrintRange: none (a missing or empty value is a type mismatch)
                        1,                 // ppPrintAll
                        "",                // SlideShowName
                        true,              // IncludeDocProperties
                        true,              // KeepIRMSettings
                        true,              // DocStructureTags
                        true,              // BitmapMissingFonts
                        pdfA);             // UseISO19005_1
                }
                finally { job.EndOp(); }
            }
            finally
            {
                // Closed even after a cancel or a stop: in the user's own PowerPoint (not ours to
                // quit) it would otherwise stay open there, hidden. Skipped only when the
                // PowerPoint we started was stopped, as there is nothing left to close.
                if (pres != null && (pp.Proc == null || pp.IsAlive()))
                {
                    try { ((dynamic)pres).Close(); } catch (Exception ex) { Log.Info("presentation.Close: " + ex.Message); }
                }
                Com.Release(pres);
                Com.Release(list);
            }
        }

        /// <summary>The page count from the PDF's page tree (the largest /Count next to a /Pages node).</summary>
        private static int PdfPageCount(string path)
        {
            try
            {
                string text = System.Text.Encoding.GetEncoding(28591).GetString(File.ReadAllBytes(path));
                int best = 0;
                foreach (System.Text.RegularExpressions.Match m in System.Text.RegularExpressions.Regex.Matches(text, @"/Type\s*/Pages\b"))
                {
                    int from = Math.Max(0, m.Index - 400);
                    string around = text.Substring(from, Math.Min(text.Length - from, 800));
                    foreach (System.Text.RegularExpressions.Match c in System.Text.RegularExpressions.Regex.Matches(around, @"/Count\s+(\d+)"))
                        best = Math.Max(best, int.Parse(c.Groups[1].Value));
                }
                return best;
            }
            catch { return 0; }
        }

        // ------------------------------------------------------------ pdf-to-word

        /// <summary>Word reads PDF 2.0 files as plain text, yet their content reads fine as 1.7:
        /// such a file is opened from a copy whose header says 1.7. Other files open as they are.</summary>
        private static string Pdf17Copy(RunJob job, FileItem f)
        {
            try
            {
                var head = new byte[16];
                int n;
                using (var s = File.OpenRead(f.Path)) n = s.Read(head, 0, head.Length);
                string h = System.Text.Encoding.ASCII.GetString(head, 0, n);
                int at = h.IndexOf("%PDF-2.", StringComparison.Ordinal);
                if (at < 0 || string.IsNullOrEmpty(job.WorkDir)) return f.Path;
                string dir = Path.Combine(job.WorkDir, f.Id);
                Directory.CreateDirectory(dir);
                string copy = Path.Combine(dir, "pdf17.pdf");
                File.Copy(f.Path, copy, true);
                using (var s = File.OpenWrite(copy))
                {
                    s.Position = at + 5;
                    s.Write(new byte[] { (byte)'1', (byte)'.', (byte)'7' }, 0, 3);
                }
                return copy;
            }
            catch (Exception ex)
            {
                Log.Info("pdf 1.7 copy: " + ex.Message);
                return f.Path;
            }
        }

        public static FileResult PdfToWord(RunJob job, FileItem f)
        {
            string ext = Analyzer.Ext(f.Path);
            if (ext != "pdf") throw new WorkerError("unsupported_file", "unsupported file type: ." + ext, Json.Obj("ext", ext));
            long inputBytes = Preflight(f.Path);

            var info = Analyzer.InspectPdf(f.Path);
            if (!info.IsPdf) throw new WorkerError("corrupt_file", "the file is not a valid PDF");

            string fmt = Opt(job, "format", "docx");
            int fileFormat;
            string outExt;
            switch (fmt)
            {
                case "rtf": fileFormat = wdFormatRTF; outExt = "rtf"; break;
                case "odt": fileFormat = wdFormatOpenDocumentText; outExt = "odt"; break;
                default: fileFormat = wdFormatDocumentDefault; outExt = "docx"; break;
            }

            string outPath = PrepareOutput(job, f, outExt);
            var word = job.EnsureWord();
            object doc = null;
            try
            {
                job.SetProgress(0.05, "reflow", 1, 2);
                string openPath = Pdf17Copy(job, f);
                job.BeginOp("open");
                try
                {
                    doc = word.Open(openPath, f.Password, false);
                }
                catch (Exception ex)
                {
                    var mapped = MapOpenError(job, ex, f, info.Encrypted);
                    if (mapped != null) throw mapped;
                    throw;
                }
                finally { job.EndOp(); }
                job.CheckCancel();

                dynamic d = doc;
                // A PDF Word can't read is opened as plain text: never hand that back as the result.
                string start = "";
                try { start = (string)d.Range(0, 16).Text ?? ""; } catch (Exception) { }
                if (start.TrimStart().StartsWith("%PDF", StringComparison.Ordinal))
                    throw new WorkerError("unsupported_file", "Word opened the PDF as text", Json.Obj("reason", "word_cannot_read_pdf"));
                int pages = info.PageCount ?? 0;
                if (pages <= 0)
                {
                    job.BeginOp("paginate");
                    try { pages = d.ComputeStatistics(wdStatisticPages); }
                    catch (Exception ex) when (!Com.IsServerGone(ex) && job.AbortReason == null && !job.HardCancel) { Log.Info("ComputeStatistics: " + ex.Message); }
                    finally { job.EndOp(); }
                    job.CheckCancel();
                }

                job.SetProgress(0.7, "save", 2, 2);
                job.BeginOp("save");
                try
                {
                    d.SaveAs2(
                        outPath,     // FileName
                        fileFormat,  // FileFormat
                        false,       // LockComments
                        "",          // Password
                        false);      // AddToRecentFiles
                }
                finally { job.EndOp(); }
                job.CheckCancel();

                long outBytes = FileSize(outPath);
                if (outBytes <= 0) throw new WorkerError("engine_failed", "Word did not produce the document");
                job.SetProgress(1.0, "save", 2, 2);

                var r = new FileResult { OutputPath = outPath, Ext = outExt, InputBytes = inputBytes, OutputBytes = outBytes };
                if (pages > 0) r.Vars["pages"] = pages;
                return r;
            }
            finally
            {
                if (doc != null && !job.HardCancel && job.AbortReason == null) WordHost.CloseDoc(doc);
                else Com.Release(doc);
            }
        }
    }
}
