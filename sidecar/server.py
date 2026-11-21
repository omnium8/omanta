"""Omanta Omarchy Kokoro TTS sidecar.

FastAPI server bound to 127.0.0.1 — audio never leaves the machine.

Environment:
  OMANTA_HOST         (default 127.0.0.1)
  OMANTA_PORT         (default 8765)
  OMANTA_MODEL_DIR    Kokoro ONNX model directory to load on startup
  ONNX_PROVIDER        Optional ORT provider override (e.g. OpenVINOExecutionProvider)
"""

from __future__ import annotations

import os
import queue
import struct
import threading
from concurrent.futures import ThreadPoolExecutor
from typing import Optional

import uvicorn
from fastapi import FastAPI, HTTPException
from fastapi.responses import JSONResponse, Response, StreamingResponse
from pydantic import BaseModel

import engines

HOST = os.environ.get("OMANTA_HOST", "127.0.0.1")
PORT = int(os.environ.get("OMANTA_PORT", "8765"))

app = FastAPI(title="Omanta Kokoro TTS (ONNX)")

_worker = ThreadPoolExecutor(max_workers=1, thread_name_prefix="kokoro")
_engine: Optional[engines.Engine] = None
_loading = False
_load_error: Optional[str] = None


def _load_engine_task(model_dir: str) -> None:
    global _engine, _loading, _load_error
    _loading = True
    _load_error = None
    try:
        _engine = engines.Engine(model_dir)
        _load_error = None
    except Exception as exc:
        _engine = None
        _load_error = f"{type(exc).__name__}: {exc}"
    finally:
        _loading = False


def _load_engine(model_dir: str) -> None:
    _worker.submit(_load_engine_task, model_dir).result()


class LoadRequest(BaseModel):
    model_dir: str


class SpeakRequest(BaseModel):
    text: str
    voice: Optional[str] = None
    speed: float = 1.0
    language: str = "en"


@app.get("/health")
def health():
    return {
        "ok": _engine is not None,
        "loading": _loading,
        "error": _load_error,
        "model": _engine.name if _engine else None,
        "engine": _engine.kind if _engine else None,
        "providers": _engine.providers if _engine else [],
    }


@app.get("/voices")
def voices():
    if _engine is None:
        return {"voices": []}
    return {"voices": _engine.list_voices()}


@app.post("/load")
def load(req: LoadRequest):
    if not os.path.isdir(req.model_dir):
        raise HTTPException(status_code=400, detail="model_dir does not exist")
    if not engines.is_kokoro_model(req.model_dir):
        raise HTTPException(
            status_code=400,
            detail="model_dir needs kokoro-v1.0.onnx (or int8) + voices-v1.0.bin",
        )
    _load_engine(req.model_dir)
    if _engine is None:
        raise HTTPException(status_code=500, detail=_load_error or "load failed")
    return {
        "ok": True,
        "model": _engine.name,
        "engine": _engine.kind,
        "providers": _engine.providers,
    }


@app.post("/speak")
def speak(req: SpeakRequest):
    if _engine is None:
        raise HTTPException(status_code=409, detail=_load_error or "no model loaded")
    try:
        audio, sr = _worker.submit(
            _engine.synth,
            req.text,
            req.voice,
            req.speed,
            req.language,
        ).result()
    except ValueError as exc:
        raise HTTPException(status_code=400, detail=str(exc))
    except Exception as exc:
        raise HTTPException(status_code=500, detail=f"{type(exc).__name__}: {exc}")
    return Response(content=engines.wav_bytes(audio, sr), media_type="audio/wav")


@app.post("/speak_stream")
def speak_stream(req: SpeakRequest):
    """Stream PCM as Kokoro yields each phoneme batch."""
    if _engine is None:
        raise HTTPException(status_code=409, detail=_load_error or "no model loaded")

    segment_q: queue.Queue[tuple] = queue.Queue(maxsize=8)
    cancel = threading.Event()

    def produce() -> None:
        try:
            for audio, sr in _engine.iter_segments(
                req.text, req.voice, req.speed, req.language, cancel=cancel
            ):
                if cancel.is_set():
                    break
                pcm = engines.pcm16_bytes(audio)
                # Never block the single synth worker forever: if the client
                # has gone away, `cancel` is set and we stop instead of parking
                # on a full queue (which used to wedge every later request).
                while not cancel.is_set():
                    try:
                        segment_q.put(("frame", int(sr), pcm), timeout=0.5)
                        break
                    except queue.Full:
                        continue
        except Exception as exc:
            if not cancel.is_set():
                segment_q.put(("error", exc))
        finally:
            try:
                segment_q.put_nowait(("end",))
            except queue.Full:
                pass

    future = _worker.submit(produce)

    def frames():
        header_sent = False
        try:
            while True:
                msg = segment_q.get()
                tag = msg[0]
                if tag == "end":
                    break
                if tag == "error":
                    raise msg[1]
                _, sr, pcm = msg
                if not header_sent:
                    yield struct.pack(">I", int(sr))
                    header_sent = True
                yield struct.pack(">I", len(pcm)) + pcm
            future.result()
        finally:
            # Client disconnect raises GeneratorExit here: tell the producer to
            # stop and drain so its blocked put() (if any) unblocks and frees
            # the worker.
            cancel.set()
            try:
                while True:
                    segment_q.get_nowait()
            except queue.Empty:
                pass

    return StreamingResponse(frames(), media_type="application/octet-stream")


class WarmRequest(BaseModel):
    voice: Optional[str] = None
    language: str = "en"


@app.post("/warm")
def warm(req: WarmRequest):
    if _engine is None:
        return {"ok": False, "reason": "no model loaded"}
    try:
        _worker.submit(
            _engine.synth,
            "Ready.",
            req.voice or engines.DEFAULT_VOICE,
            1.0,
            req.language,
        ).result()
    except Exception as exc:
        return {"ok": False, "reason": f"{type(exc).__name__}: {exc}"}
    return {"ok": True}


@app.exception_handler(Exception)
def _unhandled(_request, exc):  # pragma: no cover
    return JSONResponse(status_code=500, content={"detail": str(exc)})


def main() -> None:
    startup_model = os.environ.get("OMANTA_MODEL_DIR")
    if startup_model and os.path.isdir(startup_model):
        _worker.submit(_load_engine_task, startup_model)
    uvicorn.run(app, host=HOST, port=PORT, log_level="warning")


if __name__ == "__main__":
    main()
