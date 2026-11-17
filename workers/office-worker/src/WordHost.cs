using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Linq;
using System.Runtime.InteropServices;
using System.Threading;
using Microsoft.Win32;

namespace OfficeWorker
{
    /// <summary>An Office program (WINWORD, EXCEL, POWERPNT) this worker started, identified by PID + start time.</summary>
    internal sealed class OwnedProcess
    {
        public int Pid;
        public DateTime StartUtc;
        public string Name = "WINWORD";
    }

    /// <summary>A running Office program the job drives: Word, Excel or PowerPoint.</summary>
    internal interface IOfficeHost
    {
        OwnedProcess Proc { get; }
        bool IsAlive();
        void Shutdown();
    }

    /// <summary>
    /// Thread-safe registry of the WINWORD.EXE processes this worker started. Only these are ever killed;
    /// every kill re-verifies the PID's start time so a recycled PID (or the user's own Word) is never touched.
    /// </summary>
    internal static class ProcessRegistry
    {
        private static readonly object Lock = new object();
        private static readonly List<OwnedProcess> Owned = new List<OwnedProcess>();
        private static Dictionary<int, DateTime> _pendingBefore;
        private static DateTime _pendingSinceUtc;
        private static string _pendingName = "WINWORD";
        private static IntPtr _job = IntPtr.Zero;

        public static Dictionary<int, DateTime> SnapshotWinword()
        {
            return Snapshot("WINWORD");
        }

        public static Dictionary<int, DateTime> Snapshot(string name)
        {
            var d = new Dictionary<int, DateTime>();
            Process[] procs;
            try { procs = Process.GetProcessesByName(name); }
            catch { return d; }
            foreach (var p in procs)
            {
                try { d[p.Id] = p.StartTime.ToUniversalTime(); }
                catch { d[p.Id] = DateTime.MinValue; }
                finally { p.Dispose(); }
            }
            return d;
        }

        /// <summary>Marks that an Office application is being created (its PID is not known yet).</summary>
        public static void BeginCreate(Dictionary<int, DateTime> before, DateTime sinceUtc, string name = "WINWORD")
        {
            lock (Lock) { _pendingBefore = before; _pendingSinceUtc = sinceUtc; _pendingName = name; }
        }

        public static void EndCreate()
        {
            lock (Lock) { _pendingBefore = null; }
        }

        /// <summary>PowerPoint stays out of the kill-on-close job: the user's own presentations may open in it.</summary>
        public static void Add(OwnedProcess p)
        {
            lock (Lock) Owned.Add(p);
            if (p.Name != "POWERPNT") AttachToJob(p);
        }

        public static void Remove(OwnedProcess p)
        {
            lock (Lock) Owned.Remove(p);
        }

        public static bool IsAlive(OwnedProcess p)
        {
            return OpenVerified(p, out var proc) && Dispose(proc, true);
        }

        private static bool Dispose(Process p, bool result)
        {
            p.Dispose();
            return result;
        }

        /// <summary>Opens the process only if it is still the same program (same name, PID and start time).</summary>
        private static bool OpenVerified(OwnedProcess p, out Process proc)
        {
            proc = null;
            try
            {
                proc = Process.GetProcessById(p.Pid);
                if (proc.HasExited) { proc.Dispose(); proc = null; return false; }
                if (!string.Equals(proc.ProcessName, p.Name, StringComparison.OrdinalIgnoreCase)) { proc.Dispose(); proc = null; return false; }
                var st = proc.StartTime.ToUniversalTime();
                if (Math.Abs((st - p.StartUtc).TotalSeconds) > 1.0) { proc.Dispose(); proc = null; return false; }
                return true;
            }
            catch
            {
                proc?.Dispose();
                proc = null;
                return false;
            }
        }

        /// <summary>Waits for the process to exit; returns true if it is gone.</summary>
        public static bool WaitExit(OwnedProcess p, int ms)
        {
            if (!OpenVerified(p, out var proc)) return true;
            using (proc)
            {
                try { return proc.WaitForExit(ms); } catch { return true; }
            }
        }

        public static bool Kill(OwnedProcess p, string reason)
        {
            if (!OpenVerified(p, out var proc)) return false;
            using (proc)
            {
                try
                {
                    Log.Info("killing " + p.Name + ".EXE pid " + p.Pid + " (" + reason + ")");
                    proc.Kill();
                    proc.WaitForExit(3000);
                    return true;
                }
                catch (Exception ex)
                {
                    Log.Info("kill pid " + p.Pid + " failed: " + ex.Message);
                    return false;
                }
            }
        }

        /// <summary>
        /// Kills every Word this worker started, including one whose creation is still in flight
        /// (a WINWORD.EXE that did not exist before creation began and started after it).
        /// </summary>
        public static void KillAll(string reason)
        {
            List<OwnedProcess> list;
            Dictionary<int, DateTime> before;
            DateTime since;
            string name;
            lock (Lock)
            {
                list = Owned.ToList();
                before = _pendingBefore;
                since = _pendingSinceUtc;
                name = _pendingName;
            }
            foreach (var p in list) Kill(p, reason);
            if (before != null) KillNewSince(before, since, reason, name);
        }

        public static void KillNewSince(Dictionary<int, DateTime> before, DateTime sinceUtc, string reason, string name = "WINWORD")
        {
            foreach (var kv in Snapshot(name))
            {
                bool existed = before.TryGetValue(kv.Key, out var st) && st == kv.Value;
                if (existed) continue;
                if (kv.Value == DateTime.MinValue || kv.Value < sinceUtc.AddSeconds(-1)) continue;
                Kill(new OwnedProcess { Pid = kv.Key, StartUtc = kv.Value, Name = name }, reason + " (pending creation)");
            }
        }

        /// <summary>Puts the Word process in a kill-on-close job so it dies with this worker even if we are killed.</summary>
        private static void AttachToJob(OwnedProcess p)
        {
            try
            {
                lock (Lock)
                {
                    if (_job == IntPtr.Zero)
                    {
                        var job = Native.CreateJobObject(IntPtr.Zero, null);
                        if (job == IntPtr.Zero) return;
                        var info = new Native.JOBOBJECT_EXTENDED_LIMIT_INFORMATION();
                        info.BasicLimitInformation.LimitFlags = Native.JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                        if (!Native.SetInformationJobObject(job, Native.JobObjectExtendedLimitInformation, ref info,
                                (uint)Marshal.SizeOf(typeof(Native.JOBOBJECT_EXTENDED_LIMIT_INFORMATION))))
                        {
                            Native.CloseHandle(job);
                            return;
                        }
                        _job = job;
                    }
                }
                var h = Native.OpenProcess(Native.PROCESS_SET_QUOTA | Native.PROCESS_TERMINATE, false, p.Pid);
                if (h == IntPtr.Zero) return;
                try
                {
                    if (!Native.AssignProcessToJobObject(_job, h))
                        Log.Info("could not attach " + p.Name + " pid " + p.Pid + " to job object (error " + Marshal.GetLastWin32Error() + ")");
                }
                finally { Native.CloseHandle(h); }
            }
            catch (Exception ex)
            {
                Log.Info("job object: " + ex.Message);
            }
        }
    }

    /// <summary>One automated Word instance (late-bound COM).</summary>
    internal sealed class WordHost : IOfficeHost
    {
        /// <summary>Wrong dummy password: makes Word fail on protected documents instead of showing a password prompt.</summary>
        public const string DummyPassword = "\u0001";

        public dynamic App { get; private set; }
        public OwnedProcess Proc { get; private set; }
        private bool _mayQuit;
        private object _documents;

        public static bool IsRegistered()
        {
            return Type.GetTypeFromProgID("Word.Application", false) != null;
        }

        /// <summary>"ok" | "missing" | "unknown" based on the registry only (Word is not started).</summary>
        public static string ProbeRegistry()
        {
            try
            {
                foreach (var view in new[] { RegistryView.Registry64, RegistryView.Registry32 })
                {
                    using (var root = RegistryKey.OpenBaseKey(RegistryHive.ClassesRoot, view))
                    using (var k = root.OpenSubKey(@"Word.Application\CLSID"))
                    {
                        var clsid = k?.GetValue(null) as string;
                        if (string.IsNullOrEmpty(clsid)) continue;
                        using (var ls = root.OpenSubKey(@"CLSID\" + clsid + @"\LocalServer32"))
                        {
                            var cmd = ls?.GetValue(null) as string;
                            var exe = ExtractExe(cmd);
                            if (exe == null || System.IO.File.Exists(exe)) return "ok";
                        }
                    }
                }
                // Not registered, or registered but the server binary is gone (leftover registration).
                return "missing";
            }
            catch (Exception ex)
            {
                Log.Info("probe: " + ex.Message);
                return "unknown";
            }
        }

        private static string ExtractExe(string cmd)
        {
            if (string.IsNullOrWhiteSpace(cmd)) return null;
            cmd = cmd.Trim();
            if (cmd.StartsWith("\""))
            {
                int end = cmd.IndexOf('"', 1);
                return end > 1 ? cmd.Substring(1, end - 1) : null;
            }
            int idx = cmd.IndexOf(".exe", StringComparison.OrdinalIgnoreCase);
            return idx > 0 ? cmd.Substring(0, idx + 4) : null;
        }

        public static WordHost Start(RunJob job)
        {
            Type t = Type.GetTypeFromProgID("Word.Application", false);
            if (t == null)
                throw new WorkerError("engine_missing", "Microsoft Word is not installed", Json.Obj("component", "word"));

            var before = ProcessRegistry.SnapshotWinword();
            var since = DateTime.UtcNow;
            ProcessRegistry.BeginCreate(before, since);
            object raw;
            try
            {
                Log.Info("starting Word.Application");
                raw = Activator.CreateInstance(t);
            }
            catch (Exception ex)
            {
                // A WINWORD.EXE we launched may be stuck (activation / first-run dialog): kill only those new ones.
                ProcessRegistry.KillNewSince(before, since, "creation failed");
                ProcessRegistry.EndCreate();
                if (job.HardCancel) throw new WorkerError("cancelled", "cancelled");
                int hr = Com.HResult(ex);
                if (hr == unchecked((int)0x80040154) || hr == unchecked((int)0x800401F3))
                    throw new WorkerError("engine_missing", "Microsoft Word is not registered", Json.Obj("component", "word", "hresult", Com.Hex(hr)), ex);
                throw new WorkerError("office_not_ready", "Word could not be started: " + ex.Message,
                    Json.Obj("component", "word", "hresult", Com.Hex(hr), "message", ex.Message), ex);
            }

            var host = new WordHost { App = raw };
            try
            {
                host.Identify(before, job);
            }
            finally
            {
                ProcessRegistry.EndCreate();
            }

            if (job.HardCancel)
            {
                host.Shutdown();
                throw new WorkerError("cancelled", "cancelled");
            }

            try
            {
                dynamic app = host.App;
                app.Visible = false;
                app.DisplayAlerts = 0; // wdAlertsNone
            }
            catch (Exception ex)
            {
                host.Shutdown();
                if (job.HardCancel) throw new WorkerError("cancelled", "cancelled");
                throw new WorkerError("office_not_ready", "Word is not ready for automation: " + ex.Message,
                    Json.Obj("component", "word", "hresult", Com.Hex(Com.HResult(ex)), "message", ex.Message), ex);
            }
            try { host.App.ScreenUpdating = false; }
            catch (Exception ex) { Log.Info("ScreenUpdating: " + ex.Message); }
            try
            {
                host.App.AutomationSecurity = 3; // msoAutomationSecurityForceDisable: macros never run
                host._documents = host.App.Documents;
                int count = ((dynamic)host._documents).Count;
                Log.Info("Word ready (version " + SafeVersion(host.App) + ", open documents: " + count + ")");
            }
            catch (Exception ex)
            {
                host.Shutdown();
                if (job.HardCancel) throw new WorkerError("cancelled", "cancelled");
                throw new WorkerError("office_not_ready", "Word is not ready for automation: " + ex.Message,
                    Json.Obj("component", "word", "hresult", Com.Hex(Com.HResult(ex)), "message", ex.Message), ex);
            }
            return host;
        }

        private static string SafeVersion(dynamic app)
        {
            try { return (string)app.Version; } catch { return "?"; }
        }

        /// <summary>Find the PID of the WINWORD.EXE behind this Application object.</summary>
        private void Identify(Dictionary<int, DateTime> before, RunJob job)
        {
            dynamic app = App;
            int pidByCaption = 0;
            bool initiallyVisible = false;
            int initialDocs = 0;
            try { initiallyVisible = app.Visible; } catch { }
            try { dynamic d = app.Documents; initialDocs = d.Count; Marshal.FinalReleaseComObject((object)d); } catch { }
            try
            {
                // Unique caption -> find the hidden main window ("OpusApp") -> its PID.
                string tag = "mqkit-office-worker-" + Guid.NewGuid().ToString("N");
                app.Caption = tag;
                for (int i = 0; i < 30 && pidByCaption == 0; i++)
                {
                    var h = Native.FindWindow("OpusApp", tag);
                    if (h != IntPtr.Zero)
                    {
                        Native.GetWindowThreadProcessId(h, out uint p);
                        pidByCaption = (int)p;
                    }
                    else Thread.Sleep(100);
                }
            }
            catch (Exception ex) { Log.Info("caption lookup failed: " + ex.Message); }

            var after = ProcessRegistry.SnapshotWinword();
            var fresh = after.Where(kv => !(before.TryGetValue(kv.Key, out var st) && st == kv.Value)).Select(kv => kv.Key).ToList();

            int pid = 0;
            bool attachedToExisting = false;
            if (pidByCaption != 0)
            {
                if (fresh.Contains(pidByCaption)) pid = pidByCaption;
                else attachedToExisting = true;
            }
            else if (fresh.Count == 1)
            {
                pid = fresh[0];
            }
            else
            {
                Log.Info("could not identify Word PID (new WINWORD.EXE: " + fresh.Count + ")");
            }

            if (pid != 0 && after.TryGetValue(pid, out var startUtc) && startUtc != DateTime.MinValue)
            {
                Proc = new OwnedProcess { Pid = pid, StartUtc = startUtc };
                ProcessRegistry.Add(Proc);
                _mayQuit = true;
                Log.Info("Word started as pid " + pid);
                job.EmitChildProcess(Proc);
            }
            else
            {
                // Never quit a Word the user is using. Only quit if it looks like a fresh, invisible automation instance.
                _mayQuit = !attachedToExisting && !initiallyVisible && initialDocs == 0;
                Log.Info("Word instance not owned by PID (attachedToExisting=" + attachedToExisting + ", mayQuit=" + _mayQuit + ")");
            }
        }

        public bool IsAlive()
        {
            if (App == null) return false;
            if (Proc != null) return ProcessRegistry.IsAlive(Proc);
            try { var _ = App.Version; return true; } catch { return false; }
        }

        /// <summary>Documents.Open with positional arguments (late binding).</summary>
        public dynamic Open(string path, string password, bool confirmConversions)
        {
            dynamic docs = _documents ?? (_documents = App.Documents);
            object pwd = string.IsNullOrEmpty(password) ? DummyPassword : password;
            object m = Type.Missing;
            return docs.Open(
                path,                 // FileName
                confirmConversions,   // ConfirmConversions
                true,                 // ReadOnly
                false,                // AddToRecentFiles
                pwd,                  // PasswordDocument
                m,                    // PasswordTemplate
                false,                // Revert
                m,                    // WritePasswordDocument
                m,                    // WritePasswordTemplate
                0,                    // Format: wdOpenFormatAuto
                m,                    // Encoding
                false,                // Visible
                false,                // OpenAndRepair
                m,                    // DocumentDirection
                true);                // NoEncodingDialog
        }

        public static void CloseDoc(object doc)
        {
            if (doc == null) return;
            try { ((dynamic)doc).Close(0); } // wdDoNotSaveChanges
            catch (Exception ex) { Log.Info("doc.Close: " + ex.Message); }
            Com.Release(doc);
        }

        public void Shutdown()
        {
            if (App != null)
            {
                if (_mayQuit)
                {
                    try { App.Quit(0); } // wdDoNotSaveChanges
                    catch (Exception ex) { Log.Info("Word.Quit: " + ex.Message); }
                }
                Com.Release(_documents);
                _documents = null;
                Com.Release((object)App);
                App = null;
            }
            GC.Collect();
            GC.WaitForPendingFinalizers();
            GC.Collect();
            GC.WaitForPendingFinalizers();

            if (Proc != null)
            {
                if (!ProcessRegistry.WaitExit(Proc, 5000))
                    ProcessRegistry.Kill(Proc, "still running 5 s after Quit");
                ProcessRegistry.Remove(Proc);
                Proc = null;
            }
        }
    }

    internal static class Com
    {
        public static int HResult(Exception ex)
        {
            if (ex is COMException c) return c.ErrorCode;
            return ex?.HResult ?? 0;
        }

        public static string Hex(int hr)
        {
            return "0x" + hr.ToString("X8");
        }

        /// <summary>Word errors come back as 0x800A&lt;number&gt;.</summary>
        public static int WordErrorNumber(int hr)
        {
            return ((uint)hr & 0xFFFF0000u) == 0x800A0000u ? hr & 0xFFFF : 0;
        }

        public static bool IsServerGone(Exception ex)
        {
            if (ex is InvalidComObjectException) return true;
            uint hr = (uint)HResult(ex);
            switch (hr)
            {
                case 0x800706BA: // RPC_S_SERVER_UNAVAILABLE
                case 0x800706BE: // RPC_S_CALL_FAILED
                case 0x800706BF: // RPC_S_CALL_FAILED_DNE
                case 0x80010108: // RPC_E_DISCONNECTED
                case 0x80010007: // RPC_E_SERVER_DIED
                case 0x80010012: // RPC_E_SERVER_DIED_DNE
                case 0x80010114: // RPC_E_INVALID_OBJECT? (object disconnected)
                case 0x800706B5: // RPC_S_UNKNOWN_IF
                    return true;
                default:
                    return false;
            }
        }

        public static void Release(object o)
        {
            if (o == null) return;
            try { if (Marshal.IsComObject(o)) Marshal.FinalReleaseComObject(o); }
            catch { /* ignore */ }
        }
    }
}
