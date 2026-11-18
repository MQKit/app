"""Text to speech with Kokoro, a natural-sounding voice model that runs on the PC.

Called by the Text to speech tool (crates/worker-core/src/plugins/tts.rs) when the
person chooses a natural voice, with the portable Python and ONNX Runtime the app
downloads, plus the Kokoro packages and model (src-tauri/engines.python.toml):

    python -I kokoro_tts.py <paths> <model> <voices> <voice> <speed> <text file> <wav>

<paths>: folders to import from (site-packages), separated by ";". The text file is
UTF-8. Prints one line per step on stdout: "progress <0..1>", then "done <seconds>",
or "error <code>" (no_text, bad_voice, failed) and exits with 1.
"""

import re
import sys
import wave

# The voice's first letter is its language (Kokoro's naming: af_heart = American
# English, female): the language its words are read in.
LANGS = {"a": "en-us", "b": "en-gb", "e": "es", "f": "fr-fr", "i": "it", "p": "pt-br", "h": "hi"}
# A short silence between sentences, longer between paragraphs (seconds).
PAUSE, PARAGRAPH = 0.25, 0.6
# Longest piece read at once: Kokoro reads up to about 500 phonemes per call.
MAX_CHARS = 350


def pieces(text):
    """(sentence, pause after) in order: paragraphs, then sentences, then words if a
    sentence is too long."""
    out = []
    for para in re.split(r"\n\s*\n", text.replace("\r\n", "\n")):
        para = " ".join(para.split())
        if not para:
            continue
        sentences = [s for s in re.split(r"(?<=[.!?…;:])\s+", para) if s]
        for i, s in enumerate(sentences):
            while len(s) > MAX_CHARS:
                cut = s.rfind(" ", 0, MAX_CHARS)
                cut = cut if cut > 0 else MAX_CHARS
                out.append((s[:cut], 0.0))
                s = s[cut:].lstrip()
            out.append((s, PARAGRAPH if i == len(sentences) - 1 else PAUSE))
    return out


def main():
    paths, model, voices, voice, speed, text_file, wav_path = sys.argv[1:8]
    sys.path[:0] = [p for p in paths.split(";") if p]
    lang = LANGS.get(voice[:1])
    if not lang:
        print("error bad_voice", flush=True)
        return 1
    with open(text_file, encoding="utf-8-sig") as f:
        parts = pieces(f.read())
    if not parts:
        print("error no_text", flush=True)
        return 1

    import numpy as np
    from kokoro_onnx import Kokoro

    tts = Kokoro(model, voices)
    total_chars = sum(len(s) for s, _ in parts)
    done_chars = 0
    seconds = 0.0
    with wave.open(wav_path, "wb") as out:
        out.setnchannels(1)
        out.setsampwidth(2)
        rate = None
        for sentence, pause in parts:
            samples, sr = tts.create(sentence, voice=voice, speed=float(speed), lang=lang)
            if rate is None:
                rate = sr
                out.setframerate(rate)
            pcm = (np.clip(samples, -1.0, 1.0) * 32767).astype("<i2")
            out.writeframes(pcm.tobytes())
            if pause:
                out.writeframes(np.zeros(int(rate * pause), dtype="<i2").tobytes())
            seconds += len(samples) / rate + pause
            done_chars += len(sentence)
            print(f"progress {done_chars / total_chars:.3f}", flush=True)
    print(f"done {seconds:.1f}", flush=True)
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Exception as e:  # reported, not a traceback in the user's face
        print(f"error failed {type(e).__name__}: {e}"[:300], flush=True)
        sys.exit(1)
