// Records the app window (PrintWindow, so other windows on top don't matter) at a
// steady frame rate into ffmpeg: a frame is repeated when capturing falls behind.
using System;
using System.Diagnostics;
using System.Drawing;
using System.Drawing.Imaging;
using System.IO;
using System.Runtime.InteropServices;
using System.Threading;

public static class Recorder
{
    [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint f);
    [DllImport("user32.dll")] static extern bool SetProcessDPIAware();
    [DllImport("user32.dll")] static extern bool IsIconic(IntPtr h);
    [DllImport("user32.dll")] static extern bool ShowWindow(IntPtr h, int cmd);
    struct RECT { public int L, T, R, B; }

    public static string Run(IntPtr hwnd, string ffmpeg, string output, string stopFile, int fps)
    {
        SetProcessDPIAware();
        // A minimized window can't be captured (it is a 160-pixel stub): shown again first.
        if (IsIconic(hwnd))
        {
            ShowWindow(hwnd, 9); // SW_RESTORE
            Thread.Sleep(700);
        }
        RECT r; GetWindowRect(hwnd, out r);
        int w = r.R - r.L, h = r.B - r.T;
        var psi = new ProcessStartInfo(ffmpeg,
            "-loglevel error -y -f rawvideo -pix_fmt bgra -s " + w + "x" + h + " -r " + fps +
            " -i - -c:v libx264rgb -preset ultrafast -crf 0 \"" + output + "\"")
        { UseShellExecute = false, RedirectStandardInput = true, CreateNoWindow = true };
        var ff = Process.Start(psi);
        var stdin = ff.StandardInput.BaseStream;
        var bmp = new Bitmap(w, h, PixelFormat.Format32bppArgb);
        var buf = new byte[w * h * 4];
        // Wall-clock time of the first frame, to line up sound recorded apart (the tour's voice).
        long startEpoch = DateTimeOffset.UtcNow.ToUnixTimeMilliseconds();
        var clock = Stopwatch.StartNew();
        double interval = 1000.0 / fps;
        long written = 0, captured = 0;
        double worst = 0;
        while (!File.Exists(stopFile))
        {
            var t0 = clock.Elapsed.TotalMilliseconds;
            using (var g = Graphics.FromImage(bmp))
            {
                var hdc = g.GetHdc();
                PrintWindow(hwnd, hdc, 2);
                g.ReleaseHdc(hdc);
            }
            var data = bmp.LockBits(new Rectangle(0, 0, w, h), ImageLockMode.ReadOnly, PixelFormat.Format32bppArgb);
            Marshal.Copy(data.Scan0, buf, 0, buf.Length);
            bmp.UnlockBits(data);
            captured++;
            worst = Math.Max(worst, clock.Elapsed.TotalMilliseconds - t0);
            // Fill the timeline up to now with this frame.
            long due = (long)(clock.Elapsed.TotalMilliseconds / interval) + 1;
            while (written < due) { stdin.Write(buf, 0, buf.Length); written++; }
            var wait = written * interval - clock.Elapsed.TotalMilliseconds;
            if (wait > 1) Thread.Sleep((int)wait);
        }
        stdin.Close();
        ff.WaitForExit();
        return w + "x" + h + " frames " + written + " captured " + captured + " worst " + Math.Round(worst) + " ms start " + startEpoch;
    }
}
