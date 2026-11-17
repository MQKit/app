using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Threading;

namespace OfficeWorker
{
    internal sealed class FileItem
    {
        public string Id;
        public string Path;
        public string Password;
    }

    internal sealed class FileResult
    {
        public string OutputPath;
        public string Ext;
        public Dictionary<string, object> Vars = new Dictionary<string, object>();
        public long InputBytes;
        public long OutputBytes;
    }

    /// <summary>Executes one "run" request: files, progress, heartbeats, cancellation, Word lifecycle.</summary>
    internal sealed class RunJob
    {
        public readonly string Plugin;
        public readonly string OutDir;
        public readonly string WorkDir;
        public readonly Dictionary<string, object> Options;
        private readonly List<FileItem> _files;
        private readonly StreamReader _stdin;
        private readonly int _opTimeoutSec;

        public volatile bool HardCancel;
        public volatile bool SoftCancel;

        private readonly object _lock = new object();
        private bool _terminated;
        private string _currentFile;
        private bool _currentReported;
        private int _ok, _failed, _processed;

        // progress / busy state (read by the heartbeat timer)
        private double _fraction;
        private string _stageCode = "open";
        private int _stage = 1, _stages = 1;
        private DateTime _lastProgressUtc = DateTime.MinValue;
        private volatile bool _busy;
        private DateTime _busySinceUtc;
        private string _busyOp;
        private int _dialogHits;
        private int _tickGuard;

        /// <summary>Set by the watchdog when it had to kill Word ("dialog", "timeout").</summary>
        public volatile string AbortReason;
        public string AbortDetail;

        private readonly ManualResetEvent _finished = new ManualResetEvent(false);
        private WordHost _word;
        private bool _wordEverStarted;
        private readonly Dictionary<string, AppHost> _apps = new Dictionary<string, AppHost>();
        private readonly HashSet<string> _appsEverStarted = new HashSet<string>();
        /// <summary>The program the current call runs in (watched for dialogs and timeouts).</summary>
        private IOfficeHost _active;
        private string _activeName = "Word";
        private Timer _heartbeat;

        public RunJob(string plugin, Dictionary<string, object> req, List<FileItem> files, StreamReader stdin)
        {
            Plugin = plugin;
            _files = files;
            _stdin = stdin;
            OutDir = Json.Str(req, "outDir");
            WorkDir = Json.Str(req, "workDir");
            Options = Json.O(req, "options") ?? new Dictionary<string, object>();
            var limits = Json.O(req, "limits");
            long idle = Json.Long(limits, "idleTimeoutSec") ?? 300;
            // Hard ceiling for one Word call (Open / Export / SaveAs). Hidden dialogs are caught much sooner.
            _opTimeoutSec = (int)Math.Max(900, idle * 3);
        }

        public int Run()
        {
            OleMessageFilter.GiveUp = () => HardCancel || AbortReason != null;
            var reader = new Thread(ReadCommands) { IsBackground = true, Name = "stdin" };
            reader.Start();
            _heartbeat = new Timer(OnTick, null, 2000, 2000);

            try
            {
                Directory.CreateDirectory(OutDir);
                if (!string.IsNullOrEmpty(WorkDir)) Directory.CreateDirectory(WorkDir);
            }
            catch (Exception ex)
            {
                Out.Error("permission_denied", Json.Obj("message", "cannot create output folder: " + ex.Message, "path", OutDir));
                Finish();
                return 1;
            }

            if (Plugin != "office-to-pdf" && !WordHost.IsRegistered())
            {
                Out.Error("engine_missing", Json.Obj("component", "word", "message", "Microsoft Word is not installed"));
                Finish();
                return 1;
            }

            try
            {
                foreach (var f in _files)
                {
                    if (HardCancel || SoftCancel) break;
                    try
                    {
                        ProcessFile(f);
                    }
                    catch (JobAbort ja)
                    {
                        Out.Error(ja.Error.Code, ja.Error.Details);
                        ShutdownAll();
                        Finish();
                        return 1;
                    }
                }
            }
            finally
            {
                ShutdownAll();
            }

            EmitDone();
            Finish();
            return 0;
        }

        private void Finish()
        {
            try { _heartbeat?.Dispose(); } catch { }
            _finished.Set();
        }

        private sealed class JobAbort : Exception
        {
            public readonly WorkerError Error;
            public JobAbort(WorkerError e) : base(e.Message, e) { Error = e; }
        }

        // ------------------------------------------------------------ per file

        private void ProcessFile(FileItem f)
        {
            lock (_lock)
            {
                _currentFile = f.Id;
                _currentReported = false;
                _fraction = 0;
                _stageCode = Plugin == "pdf-to-word" ? "reflow" : "open";
                _stage = 1;
                _stages = 2;
                AbortReason = null;
                AbortDetail = null;
                _dialogHits = 0;
                if (_terminated) return;
                Out.Emit(Json.Obj("type", "fileStart", "file", f.Id));
            }

            FileResult result = null;
            WorkerError error = null;
            try
            {
                result = Plugin == "word-to-pdf" ? Converters.WordToPdf(this, f)
                    : Plugin == "office-to-pdf" ? Converters.OfficeToPdf(this, f)
                    : Converters.PdfToWord(this, f);
            }
            catch (JobAbort)
            {
                throw;
            }
            catch (Exception ex)
            {
                error = Classify(ex, f);
                if (Com.IsServerGone(ex) || AbortReason != null || (_active != null && !_active.IsAlive()))
                    ShutdownActive();
                Log.Info("file " + f.Id + " failed: " + error.Code + " - " + ex.GetType().Name + ": " + ex.Message);
            }
            finally
            {
                EndOp();
            }

            if (error != null)
            {
                Converters.DeletePartialOutputs(this, f);
                FileDone(f.Id, "failed", error.Code, error.Details);
            }
            else
            {
                lock (_lock)
                {
                    if (_terminated) return;
                    Out.Emit(Json.Obj(
                        "type", "output",
                        "file", f.Id,
                        "path", result.OutputPath,
                        "ext", result.Ext,
                        "vars", result.Vars,
                        "role", "main",
                        "stats", Json.Obj("inputBytes", result.InputBytes, "outputBytes", result.OutputBytes)));
                }
                FileDone(f.Id, "ok", null, null);
            }
        }

        private WorkerError Classify(Exception ex, FileItem f)
        {
            if (HardCancel) return new WorkerError("cancelled", "cancelled by user");
            if (ex is WorkerError we) return we;
            string abort = AbortReason;
            if (abort == "dialog")
                return new WorkerError("engine_failed", _activeName + " showed a dialog and was stopped",
                    Json.Obj("reason", "blocking_dialog", "dialog", AbortDetail ?? "", "message", _activeName + " showed a dialog: " + (AbortDetail ?? "")));
            if (abort == "timeout")
                return new WorkerError("engine_failed", _activeName + " did not finish in time",
                    Json.Obj("reason", "timeout", "message", _activeName + " did not respond for " + _opTimeoutSec + " s"));
            int hr = Com.HResult(ex);
            if (Com.IsServerGone(ex))
                return new WorkerError("engine_failed", _activeName + " stopped unexpectedly",
                    Json.Obj("reason", "word_crashed", "hresult", Com.Hex(hr), "message", ex.Message));
            if (ex is UnauthorizedAccessException)
                return new WorkerError("permission_denied", ex.Message);
            if (ex is IOException io && ((io.HResult & 0xFFFF) == 32 || (io.HResult & 0xFFFF) == 33))
                return new WorkerError("file_locked", ex.Message);
            return new WorkerError("engine_failed", ex.Message, Json.Obj("hresult", Com.Hex(hr), "message", ex.Message));
        }

        private void FileDone(string fileId, string status, string code, Dictionary<string, object> details)
        {
            lock (_lock)
            {
                if (_terminated || _currentReported) return;
                var msg = Json.Obj("type", "fileDone", "file", fileId, "status", status);
                if (code != null)
                {
                    msg["code"] = code;
                    msg["details"] = details ?? new Dictionary<string, object>();
                }
                Out.Emit(msg);
                _currentReported = true;
                _processed++;
                if (status == "ok") _ok++; else _failed++;
                _currentFile = null;
            }
        }

        private void EmitDone()
        {
            lock (_lock)
            {
                if (_terminated) return;
                _terminated = true;
                var msg = Json.Obj("type", "done");
                if (HardCancel || SoftCancel) msg["cancelled"] = true;
                msg["summary"] = Json.Obj("ok", _ok, "skipped", _files.Count - _processed, "failed", _failed);
                Out.Emit(msg);
            }
        }

        // ------------------------------------------------------------ Word

        public WordHost EnsureWord()
        {
            CheckCancel();
            if (_word != null && !_word.IsAlive())
            {
                Shutdown(_word);
                _word = null;
            }
            if (_word == null)
            {
                SetProgress(_fraction, _stageCode, _stage, _stages);
                BeginOp("start");
                try
                {
                    _word = WordHost.Start(this);
                }
                catch (WorkerError e) when (e.Code != "cancelled" && !_wordEverStarted)
                {
                    // Word cannot run at all: this is a whole-job error.
                    if (HardCancel) throw new WorkerError("cancelled", "cancelled");
                    throw new JobAbort(e);
                }
                finally
                {
                    EndOp();
                }
                _wordEverStarted = true;
            }
            _active = _word;
            _activeName = "Word";
            return _word;
        }

        /// <summary>Excel or PowerPoint, started once per job and reused for its files.</summary>
        public AppHost EnsureApp(string kind)
        {
            CheckCancel();
            _apps.TryGetValue(kind, out var host);
            if (host != null && !host.IsAlive())
            {
                Shutdown(host);
                _apps.Remove(kind);
                host = null;
            }
            if (host == null)
            {
                SetProgress(_fraction, _stageCode, _stage, _stages);
                BeginOp("start");
                try
                {
                    host = AppHost.Start(this, kind);
                }
                catch (WorkerError e) when (e.Code != "cancelled" && !_appsEverStarted.Contains(kind))
                {
                    if (HardCancel) throw new WorkerError("cancelled", "cancelled");
                    // Files of the other kind may still convert: only these fail.
                    if (e.Code == "engine_missing") throw;
                    throw new JobAbort(e);
                }
                finally
                {
                    EndOp();
                }
                _apps[kind] = host;
                _appsEverStarted.Add(kind);
            }
            _active = host;
            _activeName = AppHost.Title(kind);
            return host;
        }

        private static void Shutdown(IOfficeHost h)
        {
            if (h == null) return;
            try { h.Shutdown(); }
            catch (Exception ex) { Log.Info("shutdown: " + ex.Message); }
        }

        private void ShutdownActive()
        {
            var a = _active;
            _active = null;
            if (a == null) return;
            if (a == _word) _word = null;
            foreach (var k in _apps.Where(kv => kv.Value == a).Select(kv => kv.Key).ToList()) _apps.Remove(k);
            Shutdown(a);
        }

        private void ShutdownAll()
        {
            _active = null;
            var w = _word;
            _word = null;
            Shutdown(w);
            foreach (var h in _apps.Values.ToList()) Shutdown(h);
            _apps.Clear();
        }

        public void EmitChildProcess(OwnedProcess p)
        {
            lock (_lock)
            {
                if (_terminated) return;
                Out.Emit(Json.Obj("type", "childProcess", "pid", p.Pid, "startedAt", Json.IsoUtc(p.StartUtc)));
            }
        }

        // ------------------------------------------------------------ progress & watchdog

        public void SetProgress(double fraction, string stageCode, int stage, int stages)
        {
            lock (_lock)
            {
                _fraction = Math.Round(Math.Max(0, Math.Min(1, fraction)), 3);
                _stageCode = stageCode;
                _stage = stage;
                _stages = stages;
                EmitProgressLocked();
            }
        }

        private void EmitProgressLocked()
        {
            if (_terminated || _currentFile == null) return;
            Out.Emit(Json.Obj("type", "progress", "file", _currentFile, "fraction", _fraction,
                "stageCode", _stageCode, "stage", _stage, "stages", _stages));
            _lastProgressUtc = DateTime.UtcNow;
        }

        /// <summary>Marks the start of a Word call that may block for a long time.</summary>
        public void BeginOp(string op)
        {
            _busyOp = op;
            _busySinceUtc = DateTime.UtcNow;
            _dialogHits = 0;
            _busy = true;
        }

        public void EndOp()
        {
            _busy = false;
        }

        public void CheckCancel()
        {
            if (HardCancel) throw new WorkerError("cancelled", "cancelled by user");
        }

        private void OnTick(object state)
        {
            if (Interlocked.Exchange(ref _tickGuard, 1) == 1) return;
            try
            {
                if (!_busy) return;
                var now = DateTime.UtcNow;
                lock (_lock)
                {
                    if ((now - _lastProgressUtc).TotalSeconds >= 5) EmitProgressLocked(); // heartbeat
                }

                var proc = _active?.Proc;
                if (proc != null && AbortReason == null && !HardCancel)
                {
                    var windows = Native.VisibleWindowsOf(proc.Pid);
                    bool blocking = false;
                    string title = null;
                    foreach (var w in windows)
                    {
                        Log.Info("visible " + _activeName + " window during '" + _busyOp + "': class=" + w.ClassName + " title=" + w.Title);
                        if (w.ClassName == "#32770" || w.ClassName.StartsWith("bosa_sdm", StringComparison.Ordinal) || w.ClassName == "NUIDialog")
                        {
                            blocking = true;
                            title = string.IsNullOrEmpty(w.Title) ? w.ClassName : w.Title;
                        }
                    }
                    _dialogHits = blocking ? _dialogHits + 1 : 0;
                    if (_dialogHits >= 3) // ~6 s with a modal dialog up: Word will not come back on its own
                    {
                        AbortDetail = title;
                        AbortReason = "dialog";
                        ProcessRegistry.Kill(proc, "blocking dialog: " + title);
                    }
                }

                if (AbortReason == null && !HardCancel && (now - _busySinceUtc).TotalSeconds > _opTimeoutSec)
                {
                    AbortReason = "timeout";
                    if (proc != null) ProcessRegistry.Kill(proc, "operation timeout");
                }
            }
            catch (Exception ex)
            {
                Log.Info("tick: " + ex.Message);
            }
            finally
            {
                Interlocked.Exchange(ref _tickGuard, 0);
            }
        }

        // ------------------------------------------------------------ stdin commands

        private void ReadCommands()
        {
            while (true)
            {
                string line;
                try { line = _stdin.ReadLine(); }
                catch { return; }
                if (line == null) return; // stdin closed: keep working
                line = line.Trim();
                if (line.Length == 0) continue;
                Dictionary<string, object> cmd;
                try { cmd = Json.ParseObject(line); }
                catch { Log.Info("ignoring malformed stdin line"); continue; }
                if (Json.Str(cmd, "cmd") != "cancel") continue;
                if (Json.Str(cmd, "after") == "currentFile")
                {
                    Log.Info("cancel requested after current file");
                    SoftCancel = true;
                }
                else
                {
                    OnHardCancel();
                }
            }
        }

        private void OnHardCancel()
        {
            if (HardCancel) return;
            Log.Info("cancel requested");
            HardCancel = true;
            // Word cannot be interrupted mid-call: kill the instance we started; the blocked COM call then fails fast.
            ProcessRegistry.KillAll("cancel");
            var watchdog = new Thread(() =>
            {
                if (_finished.WaitOne(2500)) return;
                // Main thread is still stuck (e.g. in a COM call that did not notice the kill). Finish the protocol ourselves.
                ProcessRegistry.KillAll("cancel watchdog");
                lock (_lock)
                {
                    if (!_terminated)
                    {
                        if (_currentFile != null && !_currentReported)
                        {
                            Out.Emit(Json.Obj("type", "fileDone", "file", _currentFile, "status", "failed", "code", "cancelled",
                                "details", Json.Obj("message", "cancelled by user")));
                            _processed++;
                            _failed++;
                        }
                        Out.Emit(Json.Obj("type", "done", "cancelled", true,
                            "summary", Json.Obj("ok", _ok, "skipped", _files.Count - _processed, "failed", _failed)));
                        _terminated = true;
                    }
                }
                Log.Info("exiting from cancel watchdog");
                Environment.Exit(0);
            }) { IsBackground = true, Name = "cancel-watchdog" };
            watchdog.Start();
        }
    }
}
