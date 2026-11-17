using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Text;
using System.Text.RegularExpressions;

namespace OfficeWorker
{
    internal static class Program
    {
        public const string PluginVersion = "0.1.0";
        private static readonly string[] Commands = { "probe", "analyze", "run" };
        private static readonly Regex SafeId = new Regex("^[A-Za-z0-9_\\-][A-Za-z0-9_.\\-]{0,63}$");

        [STAThread]
        private static int Main(string[] args)
        {
            Out.Init();
            AppDomain.CurrentDomain.UnhandledException += (s, e) =>
            {
                ProcessRegistry.KillAll("unhandled exception");
                Log.Info("unhandled: " + e.ExceptionObject);
            };
            Console.CancelKeyPress += (s, e) => ProcessRegistry.KillAll("console break");

            try
            {
                return Dispatch(args);
            }
            catch (Exception ex)
            {
                ProcessRegistry.KillAll("fatal error");
                Log.Info("fatal: " + ex);
                Out.Error("engine_failed", Json.Obj("message", ex.Message, "hresult", Com.Hex(Com.HResult(ex))));
                return 1;
            }
        }

        private static int Usage(string why)
        {
            Log.Info(why);
            Log.Info("usage: office-worker.exe <word-to-pdf|pdf-to-word|office-to-pdf> <probe|analyze|run>   (request JSON on stdin)");
            Log.Info("       office-worker.exe manifests <dir>");
            Out.Error("invalid_request", Json.Obj("message", why));
            return 2;
        }

        private static int Dispatch(string[] args)
        {
            if (args.Length >= 1 && args[0] == "manifests")
            {
                if (args.Length < 2) return Usage("manifests needs a target directory");
                var written = Manifests.WriteAll(Path.GetFullPath(args[1]));
                foreach (var p in written) Log.Info("wrote " + p);
                Out.Emit(Json.Obj("type", "done", "written", written.ToArray()));
                return 0;
            }

            if (args.Length < 2) return Usage("missing arguments");
            string plugin = args[0], command = args[1];
            // office-to-pdf's manifest and analysis are worker-core's; only its "run" for Office comes here.
            bool runOnly = plugin == "office-to-pdf";
            if (!Manifests.PluginIds.Contains(plugin) && !runOnly) return Usage("unknown plugin id: " + plugin);
            if (runOnly && command == "analyze") return Usage("office-to-pdf is analyzed by worker-core");
            if (!Commands.Contains(command)) return Usage("unknown command: " + command);

            Out.Hello(plugin);

            var stdin = new StreamReader(Console.OpenStandardInput(), new UTF8Encoding(false), true);
            string line;
            do { line = stdin.ReadLine(); } while (line != null && line.Trim().Length == 0);
            if (line == null) return Invalid("no request on stdin");

            Dictionary<string, object> req;
            try { req = Json.ParseObject(line.Trim()); }
            catch (Exception ex) { return Invalid("request is not valid JSON: " + ex.Message); }
            if (req == null) return Invalid("request must be a JSON object");
            if (Json.Str(req, "cmd") != command) return Invalid("request cmd '" + Json.Str(req, "cmd") + "' does not match command '" + command + "'");

            switch (command)
            {
                case "probe": return Probe(req);
                case "analyze": return Analyze(plugin, req);
                default: return Run(plugin, req, stdin);
            }
        }

        private static int Invalid(string why)
        {
            Log.Info("invalid request: " + why);
            Out.Error("invalid_request", Json.Obj("message", why));
            return 2;
        }

        // ------------------------------------------------------------ probe

        private static int Probe(Dictionary<string, object> req)
        {
            var id = Json.Get(req, "id");
            Out.Emit(Json.Obj("type", "env", "id", id, "props", Json.Obj("office.word", WordHost.ProbeRegistry(),
                "office.excel", AppHost.IsRegistered("excel") ? "ok" : "missing",
                "office.powerpoint", AppHost.IsRegistered("powerpoint") ? "ok" : "missing")));
            Out.Emit(Json.Obj("type", "done", "id", id));
            return 0;
        }

        // ------------------------------------------------------------ analyze

        private static int Analyze(string plugin, Dictionary<string, object> req)
        {
            var id = Json.Get(req, "id");
            var files = Json.A(req, "files");
            if (files == null) return Invalid("analyze needs a files array");
            foreach (var o in files)
            {
                var f = o as Dictionary<string, object>;
                var fid = Json.Get(f, "id");
                var path = Json.Str(f, "path");
                var warnings = new List<object>();
                Dictionary<string, object> props;
                string status;
                Dictionary<string, object> error = null;
                try
                {
                    if (string.IsNullOrEmpty(path)) throw new FileNotFoundException("missing path");
                    props = plugin == "word-to-pdf"
                        ? Analyzer.AnalyzeWord(path, out status, warnings)
                        : Analyzer.AnalyzePdf(path, out status, warnings);
                }
                catch (Exception ex)
                {
                    props = new Dictionary<string, object>();
                    status = "failed";
                    string code = ex is FileNotFoundException || ex is DirectoryNotFoundException ? "file_not_found"
                        : ex is UnauthorizedAccessException ? "permission_denied"
                        : ex is IOException ? "file_locked"
                        : "engine_failed";
                    error = Json.Obj("code", code, "details", Json.Obj("message", ex.Message));
                    Log.Info("analyze " + path + ": " + ex.Message);
                }
                var msg = Json.Obj("type", "analysis", "id", id, "file", fid, "status", status, "props", props, "warnings", warnings);
                if (error != null) msg["error"] = error;
                Out.Emit(msg);
            }
            Out.Emit(Json.Obj("type", "done", "id", id));
            return 0;
        }

        // ------------------------------------------------------------ run

        private static int Run(string plugin, Dictionary<string, object> req, StreamReader stdin)
        {
            var protocol = Json.Long(req, "protocol");
            if (protocol != null && protocol != 2) return Invalid("unsupported protocol " + protocol);
            var outDir = Json.Str(req, "outDir");
            if (string.IsNullOrEmpty(outDir) || !Path.IsPathRooted(outDir)) return Invalid("outDir must be an absolute path");
            var filesRaw = Json.A(req, "files");
            if (filesRaw == null) return Invalid("run needs a files array");

            var secrets = Json.O(Json.O(req, "secrets"), "files");
            var files = new List<FileItem>();
            var seen = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
            foreach (var o in filesRaw)
            {
                var f = o as Dictionary<string, object>;
                var fid = Json.Str(f, "id");
                var path = Json.Str(f, "path");
                if (fid == null || !SafeId.IsMatch(fid) || fid == "." || fid == "..") return Invalid("bad file id: " + fid);
                if (!seen.Add(fid)) return Invalid("duplicate file id: " + fid);
                if (string.IsNullOrEmpty(path)) return Invalid("file " + fid + " has no path");
                files.Add(new FileItem { Id = fid, Path = path, Password = Json.Str(secrets, fid) });
            }

            OleMessageFilter.Register();
            try
            {
                var job = new RunJob(plugin, req, files, stdin);
                return job.Run();
            }
            finally
            {
                OleMessageFilter.Revoke();
                ProcessRegistry.KillAll("exit");
            }
        }
    }
}
