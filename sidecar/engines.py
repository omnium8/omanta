"""Kokoro TTS engine for Omanta Omarchy (kokoro-onnx / ONNX Runtime)."""

from __future__ import annotations

import asyncio
import io
import os
import queue
import threading
from typing import Iterator, Optional, Tuple

import numpy as np
import soundfile as sf

# Western preset voices commonly present in voices-v1.0.bin
KOKORO_VOICES = (
    "af_alloy", "af_aoede", "af_bella", "af_heart", "af_jessica", "af_kore",
    "af_nicole", "af_nova", "af_river", "af_sarah", "af_sky",
    "am_adam", "am_echo", "am_eric", "am_fenrir", "am_liam", "am_michael",
    "am_onyx", "am_puck", "am_santa",
    "bf_alice", "bf_emma", "bf_isabella", "bf_lily",
    "bm_daniel", "bm_fable", "bm_george", "bm_lewis",
    "ef_dora", "em_alex", "em_santa",
    "ff_siwis",
    "if_sara", "im_nicola",
    "pf_dora", "pm_alex", "pm_santa",
)

DEFAULT_VOICE = "af_aoede"

_MODEL_CANDIDATES = (
    "kokoro-v1.0.onnx",
    "kokoro-v1.0.int8.onnx",
    "model.onnx",
    "kokoro-v1_0.onnx",
)

_VOICES_CANDIDATES = (
    "voices-v1.0.bin",
    "voices.bin",
)

_LANG_TO_ONNX = {
    "en": "en-us",
    "en-us": "en-us",
    "a": "en-us",
    "en-gb": "en-gb",
    "b": "en-gb",
    "es": "es",
    "e": "es",
    "fr": "fr-fr",
    "f": "fr-fr",
    "it": "it",
    "i": "it",
    "pt": "pt-br",
    "pt-br": "pt-br",
    "p": "pt-br",
}


def _find_file(model_dir: str, names: tuple[str, ...]) -> Optional[str]:
    for name in names:
        path = os.path.join(model_dir, name)
        if os.path.isfile(path):
            return os.path.abspath(path)
    return None


def is_kokoro_model(model_dir: str) -> bool:
    return (
        _find_file(model_dir, _MODEL_CANDIDATES) is not None
        and _find_file(model_dir, _VOICES_CANDIDATES) is not None
    )


class Engine:
    kind = "kokoro-onnx"

    def __init__(self, model_dir: str):
        from kokoro_onnx import Kokoro

        self.model_dir = os.path.abspath(model_dir)
        model_path = _find_file(self.model_dir, _MODEL_CANDIDATES)
        voices_path = _find_file(self.model_dir, _VOICES_CANDIDATES)
        if not model_path or not voices_path:
            raise ValueError(f"Not a Kokoro ONNX model directory: {self.model_dir}")

        self.name = os.path.basename(self.model_dir.rstrip("/"))
        self.model_path = model_path
        self.voices_path = voices_path
        self.model = Kokoro(model_path, voices_path)
        self.providers = list(getattr(self.model.sess, "get_providers", lambda: [])())

    def list_voices(self) -> list[dict]:
        available = set(self.model.get_voices())
        out: list[dict] = []
        for name in KOKORO_VOICES:
            if name in available:
                out.append({"name": name, "path": name})
        # Include any extra voices shipped in the bin that we didn't list.
        for name in sorted(available):
            if name not in {v["name"] for v in out}:
                out.append({"name": name, "path": name})
        return out

    def _lang(self, language: Optional[str]) -> str:
        return _LANG_TO_ONNX.get((language or "en").lower(), "en-us")

    def _voice(self, voice: Optional[str]) -> str:
        name = voice or DEFAULT_VOICE
        available = self.model.get_voices()
        if name not in available:
            raise ValueError(f"Unknown voice: {name}")
        return name

    def iter_segments(
        self,
        text: str,
        voice: Optional[str] = None,
        speed: float = 1.0,
        language: Optional[str] = None,
        cancel: Optional[threading.Event] = None,
    ) -> Iterator[Tuple[np.ndarray, int]]:
        """Yield (audio, sample_rate) for each Kokoro phoneme batch.

        Pass a `cancel` event to abort early (e.g. the HTTP client hung up):
        both this consumer and the inner synth thread poll it, so a dropped
        stream tears down cleanly instead of parking a thread on a full queue.
        """
        if not text or not text.strip():
            raise ValueError("No text provided to synthesize.")

        voice_name = self._voice(voice)
        lang = self._lang(language)
        speed = float(np.clip(speed, 0.5, 2.0))
        cancel = cancel or threading.Event()
        out_q: queue.Queue[tuple] = queue.Queue(maxsize=4)
        error_box: list[BaseException] = []

        def runner() -> None:
            async def produce() -> None:
                try:
                    stream = self.model.create_stream(
                        text.strip(),
                        voice=voice_name,
                        speed=speed,
                        lang=lang,
                    )
                    async for audio, sr in stream:
                        if cancel.is_set():
                            break
                        arr = np.asarray(audio, dtype=np.float32).reshape(-1)
                        if not arr.size:
                            continue
                        # Bounded put, but never block forever: bail on cancel.
                        while not cancel.is_set():
                            try:
                                out_q.put(("frame", arr, int(sr)), timeout=0.5)
                                break
                            except queue.Full:
                                continue
                except BaseException as exc:  # noqa: BLE001 — surface to consumer
                    error_box.append(exc)
                finally:
                    try:
                        out_q.put_nowait(("end",))
                    except queue.Full:
                        pass

            asyncio.run(produce())

        thread = threading.Thread(target=runner, name="kokoro-stream", daemon=True)
        thread.start()
        saw_end = False
        try:
            while not cancel.is_set():
                try:
                    msg = out_q.get(timeout=0.5)
                except queue.Empty:
                    continue
                if msg[0] == "end":
                    saw_end = True
                    break
                _, audio, sr = msg
                yield audio, sr
        finally:
            # Normal finish, cancel, or GeneratorExit (consumer hung up):
            # signal the synth thread and drain so its put() unblocks.
            cancel.set()
            try:
                while True:
                    out_q.get_nowait()
            except queue.Empty:
                pass
            thread.join(timeout=30)
        # Only surface a synth error on a run that actually completed; during
        # a cancel/GeneratorExit unwind we must not raise a new exception.
        if saw_end and error_box:
            raise error_box[0]

    def synth(
        self,
        text: str,
        voice: Optional[str] = None,
        speed: float = 1.0,
        language: Optional[str] = None,
    ) -> Tuple[np.ndarray, int]:
        chunks: list[np.ndarray] = []
        sample_rate = 24000
        for audio, sr in self.iter_segments(text, voice, speed, language):
            chunks.append(audio)
            sample_rate = sr
        if not chunks:
            raise RuntimeError("Model produced no audio.")
        return np.concatenate(chunks), sample_rate


def wav_bytes(audio: np.ndarray, sample_rate: int) -> bytes:
    buf = io.BytesIO()
    sf.write(buf, audio, sample_rate, format="WAV", subtype="PCM_16")
    return buf.getvalue()


def pcm16_bytes(audio: np.ndarray) -> bytes:
    clipped = np.clip(audio, -1.0, 1.0)
    return (clipped * 32767.0).astype("<i2").tobytes()
