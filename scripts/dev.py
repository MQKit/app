"""Dev helper for the running debug app (see dev_bridge in src-tauri/src/lib.rs).
  python scripts/dev.py js "<body of an async function, use return>"
  python scripts/dev.py shot <file.png>      (screenshot of the app window)
"""
import json, os, subprocess, sys, time

root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
d = os.path.join(root, "testdata", "out", "dev")
os.makedirs(d, exist_ok=True)
cmd, arg = sys.argv[1], sys.argv[2]
if cmd == "open":
    # python scripts/dev.py open <tool> <file>...  → opens the tool with the files
    files = [os.path.abspath(f) for f in sys.argv[3:]]
    cmd, arg = "js", "const d=window.__dev; const f=await d.api.expand(%s); d.go({name:'tool',id:%s,files:f}); return f.map(x=>x.name)" % (json.dumps(files), json.dumps(arg))
    sys.argv = sys.argv[:3]
if cmd == "js":
    out = os.path.join(d, "out.json")
    if os.path.exists(out):
        os.remove(out)
    open(os.path.join(d, "in.js"), "w", encoding="utf-8").write(arg)
    for _ in range(int(float(sys.argv[3]) * 10) if len(sys.argv) > 3 else 100):
        time.sleep(0.1)
        if os.path.exists(out):
            print(open(out, encoding="utf-8").read())
            break
    else:
        print("no answer")
elif cmd == "shot":
    ps = r"""
Add-Type @'
using System; using System.Runtime.InteropServices;
public class W { [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
[DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
[DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
[DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint f);
public struct RECT { public int L, T, R, B; } }
'@
Add-Type -AssemblyName System.Drawing
$p = Get-Process mqkit | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
$h = $p.MainWindowHandle
$r = New-Object W+RECT; [W]::SetProcessDPIAware() | Out-Null; [W]::GetWindowRect($h, [ref]$r) | Out-Null
$bmp = New-Object System.Drawing.Bitmap ($r.R - $r.L), ($r.B - $r.T)
$g = [System.Drawing.Graphics]::FromImage($bmp); $hdc = $g.GetHdc()
[W]::PrintWindow($h, $hdc, 2) | Out-Null; $g.ReleaseHdc($hdc)
$bmp.Save('%s'); 'saved'
""" % os.path.abspath(arg)
    print(subprocess.run(["powershell", "-NoProfile", "-Command", ps], capture_output=True, text=True).stdout.strip())
