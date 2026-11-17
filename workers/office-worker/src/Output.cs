using System;
using System.Collections.Generic;
using System.IO;
using System.Text;

namespace OfficeWorker
{
    /// <summary>stdout carries only JSON lines (UTF-8, no BOM, one object per line, flushed each time).</summary>
    internal static class Out
    {
        private static readonly object Lock = new object();
        private static StreamWriter _stdout;

        public static void Init()
        {
            var utf8 = new UTF8Encoding(false);
            try { Console.OutputEncoding = utf8; } catch { /* no console attached */ }
            try { Console.InputEncoding = utf8; } catch { /* no console attached */ }

            _stdout = new StreamWriter(Console.OpenStandardOutput(), utf8) { AutoFlush = false, NewLine = "\n" };

            var err = new StreamWriter(Console.OpenStandardError(), utf8) { AutoFlush = true, NewLine = "\n" };
            var syncErr = TextWriter.Synchronized(err);
            Console.SetError(syncErr);
            // Anything that accidentally writes to Console.Out must not corrupt the JSON stream.
            Console.SetOut(syncErr);
        }

        public static void Emit(Dictionary<string, object> message)
        {
            string line = Json.Serialize(message);
            lock (Lock)
            {
                try
                {
                    _stdout.Write(line);
                    _stdout.Write('\n');
                    _stdout.Flush();
                }
                catch (IOException) { /* parent went away */ }
                catch (ObjectDisposedException) { }
            }
        }

        public static void Hello(string plugin)
        {
            Emit(Json.Obj("type", "hello", "protocol", 2, "plugin", plugin, "pluginVersion", Program.PluginVersion));
        }

        public static void Error(string code, Dictionary<string, object> details)
        {
            Emit(Json.Obj("type", "error", "code", code, "details", details ?? new Dictionary<string, object>()));
        }
    }

    internal static class Log
    {
        public static void Info(string message)
        {
            try { Console.Error.WriteLine("[office-worker " + DateTime.Now.ToString("HH:mm:ss.fff") + "] " + message); }
            catch { /* ignore */ }
        }
    }

    /// <summary>A failure with a protocol error code.</summary>
    internal sealed class WorkerError : Exception
    {
        public string Code { get; }
        public Dictionary<string, object> Details { get; }

        public WorkerError(string code, string message, Dictionary<string, object> details = null, Exception inner = null)
            : base(message, inner)
        {
            Code = code;
            Details = details ?? new Dictionary<string, object>();
            if (!Details.ContainsKey("message") && !string.IsNullOrEmpty(message)) Details["message"] = message;
        }
    }
}
