"""Speaks each narration line with Kokoro (a neural voice), one WAV per line:
  <kokoro venv>/Scripts/python kokoro_voice.py <models dir> <voice> <speed> <out dir> <narration.json>
Run by make.py with --voice kokoro:<voice>; the models and the Python that runs them come
from an existing Kokoro install (KOKORO_DIR)."""
import json, sys
from pathlib import Path

import soundfile as sf
from kokoro_onnx import Kokoro

models, voice, speed, out, narration = sys.argv[1:6]
out = Path(out)
out.mkdir(parents=True, exist_ok=True)
lines = json.loads(Path(narration).read_text(encoding="utf-8"))
tts = Kokoro(str(Path(models) / "kokoro-v1.0.onnx"), str(Path(models) / "voices-v1.0.bin"))
for i, line in enumerate(lines, 1):
    said = line.get("say", line["text"])
    audio, rate = tts.create(said, voice=voice, speed=float(speed), lang="en-us")
    sf.write(out / f"{i:02}.wav", audio, rate)
    print(f"{i:02}", round(len(audio) / rate, 2))
