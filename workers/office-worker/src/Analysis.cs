using System;
using System.Collections.Generic;
using System.IO;
using System.IO.Compression;
using System.Linq;
using System.Text;
using System.Text.RegularExpressions;

namespace OfficeWorker
{
    internal enum FileKind { Missing, Empty, Zip, Ole, Pdf, Rtf, Other }

    /// <summary>Fast, Word-free inspection of input files.</summary>
    internal static class Analyzer
    {
        public static readonly string[] WordExtensions = { "doc", "docx", "docm", "dot", "dotx", "rtf", "odt", "txt" };
        private static readonly string[] OoxmlExtensions = { "docx", "docm", "dotx" };

        public static string Ext(string path)
        {
            var e = Path.GetExtension(path ?? "");
            return string.IsNullOrEmpty(e) ? "" : e.Substring(1).ToLowerInvariant();
        }

        public static FileStream OpenShared(string path)
        {
            return new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.ReadWrite | FileShare.Delete, 65536);
        }

        public static FileKind Sniff(string path)
        {
            if (!File.Exists(path)) return FileKind.Missing;
            using (var fs = OpenShared(path))
            {
                var head = new byte[1024];
                int n = fs.Read(head, 0, head.Length);
                if (n == 0) return FileKind.Empty;
                if (n >= 4 && head[0] == 0x50 && head[1] == 0x4B && (head[2] == 3 || head[2] == 5 || head[2] == 7)) return FileKind.Zip;
                if (n >= 8 && head[0] == 0xD0 && head[1] == 0xCF && head[2] == 0x11 && head[3] == 0xE0 &&
                    head[4] == 0xA1 && head[5] == 0xB1 && head[6] == 0x1A && head[7] == 0xE1) return FileKind.Ole;
                if (n >= 5 && head[0] == (byte)'{' && head[1] == (byte)'\\' && head[2] == (byte)'r' && head[3] == (byte)'t' && head[4] == (byte)'f') return FileKind.Rtf;
                var latin = Encoding.GetEncoding(28591).GetString(head, 0, n);
                if (latin.Contains("%PDF-")) return FileKind.Pdf;
                return FileKind.Other;
            }
        }

        /// <summary>
        /// Result of a cheap check that decides whether Word is worth starting:
        /// "ok", "locked" (password protected), "corrupt".
        /// </summary>
        public static string QuickWordCheck(string path, string ext)
        {
            var kind = Sniff(path);
            if (kind == FileKind.Empty) return ext == "txt" ? "ok" : "corrupt";
            if (OoxmlExtensions.Contains(ext) || ext == "odt")
            {
                if (kind == FileKind.Zip) return OdtEncrypted(path, ext) ? "locked" : "ok";
                if (kind == FileKind.Ole) return OleInfo.Read(path).Encrypted != false ? "locked" : "ok";
                return "corrupt";
            }
            if (ext == "doc" || ext == "dot")
            {
                if (kind == FileKind.Ole) return OleInfo.Read(path).Encrypted == true ? "locked" : "ok";
                return "ok"; // could be RTF/HTML/docx renamed; let Word decide
            }
            return "ok";
        }

        private static bool OdtEncrypted(string path, string ext)
        {
            if (ext != "odt") return false;
            try
            {
                using (var fs = OpenShared(path))
                using (var zip = new ZipArchive(fs, ZipArchiveMode.Read))
                {
                    var m = zip.GetEntry("META-INF/manifest.xml");
                    return m != null && ReadEntry(m, 4 << 20).Contains("encryption-data");
                }
            }
            catch { return false; }
        }

        public static Dictionary<string, object> AnalyzeWord(string path, out string status, List<object> warnings)
        {
            string ext = Ext(path);
            var props = Json.Obj("app", "word", "format", ext);
            if (!WordExtensions.Contains(ext))
            {
                status = "unsupported";
                return props;
            }
            var kind = Sniff(path);
            if (kind == FileKind.Missing) throw new FileNotFoundException("file not found", path);

            props["hasMacros"] = ext == "docm";
            props["hasTrackedChanges"] = false;
            props["hasComments"] = false;
            props["cachedPageCount"] = null;
            props["isEncrypted"] = false;
            props["needsPassword"] = false;
            status = "ok";

            if (kind == FileKind.Empty)
            {
                if (ext != "txt") status = "corrupt";
                return props;
            }

            if (OoxmlExtensions.Contains(ext) || ext == "odt")
            {
                if (kind == FileKind.Ole)
                {
                    var ole = OleInfo.Read(path);
                    if (ole.Encrypted != false || ole.IsEncryptedPackage)
                    {
                        props["isEncrypted"] = true;
                        props["needsPassword"] = true;
                        status = "locked";
                    }
                    else
                    {
                        // A binary .doc with a .docx name: Word opens it fine.
                        props["hasMacros"] = (bool)props["hasMacros"] || ole.HasMacros;
                        warnings.Add(Json.Obj("code", "extension_mismatch"));
                    }
                    return props;
                }
                if (kind != FileKind.Zip)
                {
                    status = "corrupt";
                    return props;
                }
                try
                {
                    if (ext == "odt") AnalyzeOdt(path, props, ref status);
                    else AnalyzeOoxml(path, props);
                }
                catch (InvalidDataException)
                {
                    status = "corrupt";
                }
                return props;
            }

            if (ext == "doc" || ext == "dot")
            {
                if (kind == FileKind.Ole)
                {
                    var ole = OleInfo.Read(path);
                    props["hasMacros"] = ole.HasMacros;
                    if (ole.Encrypted == true || ole.IsEncryptedPackage)
                    {
                        props["isEncrypted"] = true;
                        props["needsPassword"] = true;
                        status = "locked";
                    }
                    if (ole.Corrupt) status = "corrupt";
                }
                else if (kind == FileKind.Zip)
                {
                    warnings.Add(Json.Obj("code", "extension_mismatch"));
                    try { AnalyzeOoxml(path, props); } catch (InvalidDataException) { status = "corrupt"; }
                }
                else if (kind != FileKind.Rtf)
                {
                    warnings.Add(Json.Obj("code", "extension_mismatch"));
                }
                return props;
            }

            if (ext == "rtf" && kind != FileKind.Rtf)
                warnings.Add(Json.Obj("code", "extension_mismatch"));
            return props;
        }

        private static void AnalyzeOoxml(string path, Dictionary<string, object> props)
        {
            using (var fs = OpenShared(path))
            using (var zip = new ZipArchive(fs, ZipArchiveMode.Read))
            {
                var entries = new Dictionary<string, ZipArchiveEntry>(StringComparer.OrdinalIgnoreCase);
                foreach (var e in zip.Entries) entries[e.FullName.Replace('\\', '/')] = e;

                if (entries.Keys.Any(k => k.EndsWith("vbaProject.bin", StringComparison.OrdinalIgnoreCase)))
                    props["hasMacros"] = true;

                string mainPath = "word/document.xml";
                if (entries.TryGetValue("_rels/.rels", out var rels))
                {
                    var m = Regex.Match(ReadEntry(rels, 1 << 20),
                        "<Relationship[^>]*Type=\"[^\"]*/officeDocument\"[^>]*Target=\"/?([^\"]+)\"|<Relationship[^>]*Target=\"/?([^\"]+)\"[^>]*Type=\"[^\"]*/officeDocument\"");
                    if (m.Success) mainPath = m.Groups[1].Success && m.Groups[1].Value.Length > 0 ? m.Groups[1].Value : m.Groups[2].Value;
                }
                if (!entries.TryGetValue(mainPath, out var main) && !entries.TryGetValue("word/document.xml", out main))
                    throw new InvalidDataException("main document part not found");

                string xml = ReadEntry(main, int.MaxValue);
                props["hasTrackedChanges"] = xml.Contains("<w:ins ") || xml.Contains("<w:del ") ||
                                             xml.Contains("<w:moveFrom ") || xml.Contains("<w:moveTo ") ||
                                             xml.Contains("<w:rPrChange ") || xml.Contains("<w:pPrChange ");

                string dir = mainPath.Contains("/") ? mainPath.Substring(0, mainPath.LastIndexOf('/') + 1) : "";
                if (entries.TryGetValue(dir + "comments.xml", out var comments))
                    props["hasComments"] = ReadEntry(comments, int.MaxValue).Contains("<w:comment ");

                if (entries.TryGetValue("docProps/app.xml", out var app))
                {
                    var m = Regex.Match(ReadEntry(app, 1 << 20), "<Pages>\\s*(\\d+)\\s*</Pages>");
                    if (m.Success && int.TryParse(m.Groups[1].Value, out int pages)) props["cachedPageCount"] = pages;
                }
            }
        }

        private static void AnalyzeOdt(string path, Dictionary<string, object> props, ref string status)
        {
            using (var fs = OpenShared(path))
            using (var zip = new ZipArchive(fs, ZipArchiveMode.Read))
            {
                var manifest = zip.GetEntry("META-INF/manifest.xml");
                if (manifest != null && ReadEntry(manifest, 4 << 20).Contains("encryption-data"))
                {
                    props["isEncrypted"] = true;
                    props["needsPassword"] = true;
                    status = "locked";
                    return;
                }
                if (zip.Entries.Any(e => e.FullName.StartsWith("Basic/", StringComparison.OrdinalIgnoreCase) ||
                                         e.FullName.StartsWith("Scripts/", StringComparison.OrdinalIgnoreCase)))
                    props["hasMacros"] = true;
                var content = zip.GetEntry("content.xml");
                if (content == null)
                {
                    status = "corrupt";
                    return;
                }
                string xml = ReadEntry(content, int.MaxValue);
                props["hasComments"] = xml.Contains("<office:annotation");
                props["hasTrackedChanges"] = xml.Contains("<text:changed-region");
                var meta = zip.GetEntry("meta.xml");
                if (meta != null)
                {
                    var m = Regex.Match(ReadEntry(meta, 1 << 20), "meta:page-count=\"(\\d+)\"");
                    if (m.Success && int.TryParse(m.Groups[1].Value, out int pages)) props["cachedPageCount"] = pages;
                }
            }
        }

        private static string ReadEntry(ZipArchiveEntry e, int maxBytes)
        {
            using (var s = e.Open())
            using (var r = new StreamReader(s, Encoding.UTF8, true))
            {
                if (maxBytes == int.MaxValue) return r.ReadToEnd();
                var buf = new char[Math.Min(maxBytes, (int)Math.Min(int.MaxValue, Math.Max(1, e.Length)))];
                int n = r.ReadBlock(buf, 0, buf.Length);
                return new string(buf, 0, n);
            }
        }

        // ---------------------------------------------------------------- PDF

        public sealed class PdfInfo
        {
            public bool IsPdf;
            public int? PageCount;
            public bool Encrypted;
            public string Version;
        }

        public static PdfInfo InspectPdf(string path)
        {
            var info = new PdfInfo();
            string s;
            using (var fs = OpenShared(path))
            {
                long len = fs.Length;
                byte[] bytes;
                const long whole = 256L << 20;
                const int part = 32 << 20;
                if (len <= whole)
                {
                    bytes = new byte[len];
                    ReadFully(fs, bytes, 0, bytes.Length);
                }
                else
                {
                    // Huge file: look at the head and the tail only (page count becomes a rough estimate).
                    bytes = new byte[part * 2];
                    ReadFully(fs, bytes, 0, part);
                    fs.Seek(-part, SeekOrigin.End);
                    ReadFully(fs, bytes, part, part);
                }
                s = Encoding.GetEncoding(28591).GetString(bytes);
            }

            int headEnd = Math.Min(s.Length, 1024);
            var vm = Regex.Match(s.Substring(0, headEnd), "%PDF-(\\d\\.\\d)");
            info.IsPdf = vm.Success;
            if (!info.IsPdf) return info;
            info.Version = vm.Groups[1].Value;

            int pageObjects = Regex.Matches(s, "/Type\\s*/Page(?![A-Za-z])").Count;
            int maxCount = 0;
            foreach (Match m in Regex.Matches(s, "/Type\\s*/Pages(?![A-Za-z])"))
            {
                int start = Math.Max(0, m.Index - 600);
                int end = Math.Min(s.Length, m.Index + 600);
                foreach (Match c in Regex.Matches(s.Substring(start, end - start), "/Count\\s+(\\d+)"))
                    if (int.TryParse(c.Groups[1].Value, out int n) && n > maxCount && n < 1000000) maxCount = n;
            }
            int pages = maxCount > 0 ? maxCount : pageObjects;
            info.PageCount = pages > 0 ? pages : (int?)null;
            info.Encrypted = Regex.IsMatch(s, "/Encrypt\\s*(\\d+\\s+\\d+\\s+R|<<)");
            return info;
        }

        private static void ReadFully(Stream s, byte[] buf, int offset, int count)
        {
            while (count > 0)
            {
                int n = s.Read(buf, offset, count);
                if (n <= 0) break;
                offset += n;
                count -= n;
            }
        }

        public static Dictionary<string, object> AnalyzePdf(string path, out string status, List<object> warnings)
        {
            string ext = Ext(path);
            var props = Json.Obj("format", ext);
            if (ext != "pdf")
            {
                status = "unsupported";
                return props;
            }
            if (!File.Exists(path)) throw new FileNotFoundException("file not found", path);
            var info = InspectPdf(path);
            props["format"] = "pdf";
            props["pageCount"] = info.PageCount;
            props["needsPassword"] = info.Encrypted;
            if (!info.IsPdf)
            {
                status = "corrupt";
                return props;
            }
            props["pdfVersion"] = info.Version;
            if (info.PageCount == null) warnings.Add(Json.Obj("code", "page_count_unknown"));
            status = info.Encrypted ? "locked" : "ok";
            return props;
        }
    }

    /// <summary>
    /// Minimal OLE compound-file reader: enough to see whether a .doc is encrypted (FIB flag),
    /// whether it carries macros, or whether it is an encrypted OOXML package.
    /// </summary>
    internal sealed class OleInfo
    {
        /// <summary>true / false, or null when it could not be determined.</summary>
        public bool? Encrypted;
        public bool IsEncryptedPackage;
        public bool HasMacros;
        public bool Corrupt;

        private const uint EndOfChain = 0xFFFFFFFE;
        private const uint FreeSect = 0xFFFFFFFF;

        public static OleInfo Read(string path)
        {
            var info = new OleInfo();
            try
            {
                using (var fs = Analyzer.OpenShared(path))
                    info.Parse(fs);
            }
            catch (Exception ex)
            {
                Log.Info("OLE parse: " + ex.Message);
                info.Encrypted = null;
            }
            return info;
        }

        private void Parse(FileStream fs)
        {
            var hdr = new byte[512];
            if (fs.Read(hdr, 0, 512) < 512) { Corrupt = true; Encrypted = null; return; }
            int sectorShift = BitConverter.ToUInt16(hdr, 0x1E);
            if (sectorShift < 7 || sectorShift > 16) { Corrupt = true; Encrypted = null; return; }
            int sectorSize = 1 << sectorShift;
            uint numFat = BitConverter.ToUInt32(hdr, 0x2C);
            uint firstDir = BitConverter.ToUInt32(hdr, 0x30);
            uint miniCutoff = BitConverter.ToUInt32(hdr, 0x38);
            uint firstDifat = BitConverter.ToUInt32(hdr, 0x44);
            uint numDifat = BitConverter.ToUInt32(hdr, 0x48);
            long totalSectors = Math.Max(1, (fs.Length - sectorSize) / sectorSize + 1);
            if (numFat > totalSectors) { Corrupt = true; Encrypted = null; return; }

            // FAT sector list from the header DIFAT (+ DIFAT chain).
            var fatSectors = new List<uint>();
            for (int i = 0; i < 109 && fatSectors.Count < numFat; i++)
            {
                uint sid = BitConverter.ToUInt32(hdr, 0x4C + i * 4);
                if (sid == FreeSect) break;
                fatSectors.Add(sid);
            }
            uint difat = firstDifat;
            int guard = 0;
            var buf = new byte[sectorSize];
            while (difat != EndOfChain && difat != FreeSect && numDifat > 0 && fatSectors.Count < numFat && guard++ < 10000)
            {
                ReadSector(fs, difat, sectorSize, buf);
                int per = sectorSize / 4 - 1;
                for (int i = 0; i < per && fatSectors.Count < numFat; i++)
                {
                    uint sid = BitConverter.ToUInt32(buf, i * 4);
                    if (sid != FreeSect) fatSectors.Add(sid);
                }
                difat = BitConverter.ToUInt32(buf, per * 4);
            }

            var fat = new uint[fatSectors.Count * (sectorSize / 4)];
            for (int i = 0; i < fatSectors.Count; i++)
            {
                ReadSector(fs, fatSectors[i], sectorSize, buf);
                Buffer.BlockCopy(buf, 0, fat, i * sectorSize, sectorSize);
            }

            // Directory entries.
            uint wordDocStart = FreeSect;
            ulong wordDocSize = 0;
            uint sect = firstDir;
            guard = 0;
            while (sect != EndOfChain && sect < fat.Length && guard++ < 100000)
            {
                ReadSector(fs, sect, sectorSize, buf);
                for (int off = 0; off + 128 <= sectorSize; off += 128)
                {
                    int nameLen = BitConverter.ToUInt16(buf, off + 64);
                    byte type = buf[off + 66];
                    if (type == 0 || nameLen < 2 || nameLen > 64) continue;
                    string name = Encoding.Unicode.GetString(buf, off, nameLen - 2);
                    if (name == "EncryptedPackage" || name == "EncryptionInfo") IsEncryptedPackage = true;
                    if (name == "Macros" || name == "_VBA_PROJECT_CUR" || name == "VBA") HasMacros = true;
                    if (name == "WordDocument" && type == 2)
                    {
                        wordDocStart = BitConverter.ToUInt32(buf, off + 116);
                        wordDocSize = BitConverter.ToUInt64(buf, off + 120);
                    }
                }
                sect = fat[sect];
            }

            if (IsEncryptedPackage) { Encrypted = true; return; }
            if (wordDocStart == FreeSect || wordDocStart == EndOfChain) { Encrypted = null; return; }
            if (wordDocSize < miniCutoff) { Encrypted = null; return; } // stream lives in the mini stream; not handled
            ReadSector(fs, wordDocStart, sectorSize, buf);
            ushort wIdent = BitConverter.ToUInt16(buf, 0);
            if (wIdent != 0xA5EC && wIdent != 0xA5DC) { Encrypted = null; return; }
            ushort flags = BitConverter.ToUInt16(buf, 0x0A);
            Encrypted = (flags & 0x0100) != 0; // FIB.fEncrypted
        }

        private static void ReadSector(FileStream fs, uint sid, int sectorSize, byte[] buf)
        {
            long pos = ((long)sid + 1) * sectorSize;
            if (pos + sectorSize > fs.Length) throw new InvalidDataException("sector out of range");
            fs.Seek(pos, SeekOrigin.Begin);
            int read = 0;
            while (read < sectorSize)
            {
                int n = fs.Read(buf, read, sectorSize - read);
                if (n <= 0) throw new InvalidDataException("short read");
                read += n;
            }
        }
    }
}
