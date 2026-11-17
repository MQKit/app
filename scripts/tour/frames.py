"""Torn frames in a window recording: PrintWindow sometimes returns the window half
redrawn (stripes of what was under a layer). Such a frame differs from the one before,
and within three frames the picture is back where it was; a real change stays.

  python frames.py <video>      lists them
make.py drops them from the tour (the frame before is shown in their place)."""
import subprocess, sys

W, H = 160, 112


def gray_frames(path):
    raw = subprocess.run(["ffmpeg", "-loglevel", "error", "-i", path, "-vf", f"scale={W}:{H}:flags=area,format=gray", "-f", "rawvideo", "-"],
                         capture_output=True, check=True).stdout
    n = len(raw) // (W * H)
    return [raw[i * W * H:(i + 1) * W * H] for i in range(n)]


def diff(a, b):
    return sum(abs(x - y) for x, y in zip(a, b)) / len(a)


def torn(path, jump=0.5, back=0.3, longest=3):
    """Indices of torn frames: a jump of at least `jump` (mean, 0-255) that comes back to
    within `back` of it in at most `longest` frames."""
    f = gray_frames(path)
    step = [0.0] + [diff(f[i - 1], f[i]) for i in range(1, len(f))]
    bad = set()
    for i in range(1, len(f)):
        d = step[i]
        if d < jump or i in bad:
            continue
        for j in range(i + 1, min(i + longest + 1, len(f))):
            if diff(f[i - 1], f[j]) < d * back:
                bad.update(range(i, j))
                break
    return sorted(bad)


if __name__ == "__main__":
    for p in sys.argv[1:]:
        b = torn(p)
        print(p, len(b), "torn:", ", ".join(f"{i} ({i / 30:.2f}s)" for i in b))
