using System;
using System.Collections.Generic;
using System.Linq;
using System.Runtime.InteropServices;

namespace OfficeWorker
{
    /// <summary>
    /// One automated Excel or PowerPoint (late-bound COM). Excel starts a new hidden EXCEL.EXE
    /// for each automation request. PowerPoint runs as one process per user: when the user
    /// already has it open, the presentation is opened there without a window. That instance
    /// is never quit or killed, and its settings are restored afterwards.
    /// </summary>
    internal sealed class AppHost : IOfficeHost
    {
        public readonly string Kind; // "excel" | "powerpoint"
        public dynamic App { get; private set; }
        public OwnedProcess Proc { get; private set; }
        private bool _mayQuit;
        private object _savedAlerts, _savedSecurity;

        private AppHost(string kind) { Kind = kind; }

        public static string ProgId(string kind) => kind == "excel" ? "Excel.Application" : "PowerPoint.Application";
        private static string ProcessName(string kind) => kind == "excel" ? "EXCEL" : "POWERPNT";
        public static string Title(string kind) => kind == "excel" ? "Excel" : "PowerPoint";

        public static bool IsRegistered(string kind)
        {
            return Type.GetTypeFromProgID(ProgId(kind), false) != null;
        }

        public static AppHost Start(RunJob job, string kind)
        {
            Type t = Type.GetTypeFromProgID(ProgId(kind), false);
            if (t == null)
                throw new WorkerError("engine_missing", "Microsoft " + Title(kind) + " is not installed", Json.Obj("component", kind));

            string name = ProcessName(kind);
            var before = ProcessRegistry.Snapshot(name);
            var since = DateTime.UtcNow;
            ProcessRegistry.BeginCreate(before, since, name);
            object raw;
            try
            {
                Log.Info("starting " + ProgId(kind));
                raw = Activator.CreateInstance(t);
            }
            catch (Exception ex)
            {
                ProcessRegistry.KillNewSince(before, since, "creation failed", name);
                ProcessRegistry.EndCreate();
                if (job.HardCancel) throw new WorkerError("cancelled", "cancelled");
                int hr = Com.HResult(ex);
                if (hr == unchecked((int)0x80040154) || hr == unchecked((int)0x800401F3))
                    throw new WorkerError("engine_missing", "Microsoft " + Title(kind) + " is not registered", Json.Obj("component", kind, "hresult", Com.Hex(hr)), ex);
                throw new WorkerError("office_not_ready", Title(kind) + " could not be started: " + ex.Message,
                    Json.Obj("component", kind, "hresult", Com.Hex(hr), "message", ex.Message), ex);
            }

            var host = new AppHost(kind) { App = raw };
            try { host.Identify(before, name, job); }
            finally { ProcessRegistry.EndCreate(); }

            if (job.HardCancel)
            {
                host.Shutdown();
                throw new WorkerError("cancelled", "cancelled");
            }

            try
            {
                dynamic app = host.App;
                if (kind == "excel")
                {
                    app.Visible = false;
                    app.DisplayAlerts = false;
                    app.ScreenUpdating = false;
                    app.AskToUpdateLinks = false;
                    app.EnableEvents = false;
                    app.AutomationSecurity = 3; // msoAutomationSecurityForceDisable: macros never run
                }
                else
                {
                    // PowerPoint can't hide its application window; presentations open without one.
                    host._savedAlerts = app.DisplayAlerts;
                    host._savedSecurity = app.AutomationSecurity;
                    app.DisplayAlerts = 1; // ppAlertsNone
                    app.AutomationSecurity = 3;
                }
                Log.Info(Title(kind) + " ready (version " + SafeVersion(app) + ")");
            }
            catch (Exception ex)
            {
                host.Shutdown();
                if (job.HardCancel) throw new WorkerError("cancelled", "cancelled");
                throw new WorkerError("office_not_ready", Title(kind) + " is not ready for automation: " + ex.Message,
                    Json.Obj("component", kind, "hresult", Com.Hex(Com.HResult(ex)), "message", ex.Message), ex);
            }
            return host;
        }

        private static string SafeVersion(dynamic app)
        {
            try { return (string)app.Version; } catch { return "?"; }
        }

        /// <summary>Finds the process behind the Application object from its main window.</summary>
        private void Identify(Dictionary<int, DateTime> before, string name, RunJob job)
        {
            int pid = 0;
            try
            {
                dynamic app = App;
                long hwnd = Kind == "excel" ? (long)app.Hwnd : (long)app.HWND;
                if (hwnd != 0)
                {
                    Native.GetWindowThreadProcessId(new IntPtr(hwnd), out uint p);
                    pid = (int)p;
                }
            }
            catch (Exception ex) { Log.Info("hwnd lookup failed: " + ex.Message); }

            var after = ProcessRegistry.Snapshot(name);
            var fresh = after.Where(kv => !(before.TryGetValue(kv.Key, out var st) && st == kv.Value)).Select(kv => kv.Key).ToList();
            if (pid == 0 && fresh.Count == 1) pid = fresh[0];

            if (pid != 0 && fresh.Contains(pid) && after.TryGetValue(pid, out var startUtc) && startUtc != DateTime.MinValue)
            {
                Proc = new OwnedProcess { Pid = pid, StartUtc = startUtc, Name = name };
                ProcessRegistry.Add(Proc);
                _mayQuit = true;
                Log.Info(Title(Kind) + " started as pid " + pid);
                job.EmitChildProcess(Proc);
            }
            else
            {
                // The user's own PowerPoint (or a process we can't tell apart): used, never quit or killed.
                _mayQuit = false;
                Log.Info(Title(Kind) + " instance not owned (pid " + pid + ", new processes: " + fresh.Count + ")");
            }
        }

        public bool IsAlive()
        {
            if (App == null) return false;
            if (Proc != null) return ProcessRegistry.IsAlive(Proc);
            try { var _ = App.Version; return true; } catch { return false; }
        }

        /// <summary>Something the user opened in this instance meanwhile (PowerPoint shares one process).</summary>
        private bool UserIsWorkingInIt()
        {
            try
            {
                if (Kind == "excel") return (bool)App.Visible || (bool)App.UserControl;
                return (int)App.Presentations.Count > 0;
            }
            catch { return false; }
        }

        public void Shutdown()
        {
            bool userWork = false;
            if (App != null)
            {
                userWork = UserIsWorkingInIt();
                if (_mayQuit && !userWork)
                {
                    try { App.Quit(); }
                    catch (Exception ex) { Log.Info(Title(Kind) + ".Quit: " + ex.Message); }
                }
                else if (Kind == "powerpoint")
                {
                    try { if (_savedAlerts != null) App.DisplayAlerts = _savedAlerts; } catch { }
                    try { if (_savedSecurity != null) App.AutomationSecurity = _savedSecurity; } catch { }
                }
                Com.Release((object)App);
                App = null;
            }
            GC.Collect();
            GC.WaitForPendingFinalizers();
            GC.Collect();
            GC.WaitForPendingFinalizers();

            if (Proc != null)
            {
                if (userWork) Log.Info(Title(Kind) + " pid " + Proc.Pid + " now holds the user's files: left running");
                else if (!ProcessRegistry.WaitExit(Proc, 5000))
                    ProcessRegistry.Kill(Proc, "still running 5 s after Quit");
                ProcessRegistry.Remove(Proc);
                Proc = null;
            }
        }
    }
}
