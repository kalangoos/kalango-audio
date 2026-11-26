"""Builds synthetic test clips and PyTorch reference outputs for the Rust port.

Clips: LibriSpeech speech shipped in the upstream repo's demo/examples/sources
(CC BY 4.0) resampled to 48 kHz, mixed with synthetic pink noise, synthetic
"music" (harmonic tones with an envelope) and a reversed-speech babble at several
SNRs. Outputs are raw little-endian float32 files plus intermediates for one clip.
"""
import json
import os
import sys
import time

import numpy as np
import soundfile as sf
import torch

from inworld.voice_isolation import audio as viaudio
from inworld.voice_isolation import isolator as viso
from inworld.voice_isolation import streaming

REPO = sys.argv[1]
OUT = sys.argv[2]
WEIGHTS = sys.argv[3]
SR = 48000
torch.manual_seed(0)
rng = np.random.default_rng(1234)
os.makedirs(OUT, exist_ok=True)
# intermediates are large: INTER can put them on another disk
INTER = os.environ.get("INTER", os.path.join(OUT, "inter"))
os.makedirs(INTER, exist_ok=True)


def load48(name):
    data, sr = sf.read(os.path.join(REPO, "demo/examples/sources", name), dtype="float32")
    return viaudio.resample(torch.from_numpy(data)[None], sr, SR)[0].numpy()


def pink(n):
    white = rng.standard_normal(n)
    spec = np.fft.rfft(white)
    f = np.arange(spec.size)
    f[0] = 1
    return np.fft.irfft(spec / np.sqrt(f), n).astype(np.float32)


def music(n):
    t = np.arange(n) / SR
    out = np.zeros(n)
    notes = [220.0, 277.18, 329.63, 440.0, 369.99, 293.66]
    beat = SR // 2
    for k in range(0, n, beat):
        f0 = notes[(k // beat) % len(notes)]
        seg = slice(k, min(n, k + beat))
        env = np.exp(-3.0 * (t[seg] - t[k]))
        for h in range(1, 6):
            out[seg] += env * np.sin(2 * np.pi * f0 * h * t[seg]) / h
    return out.astype(np.float32)


def mix(speech, noise, snr_db):
    noise = noise[:speech.size]
    ps = np.mean(speech**2)
    pn = np.mean(noise**2)
    gain = np.sqrt(ps / (pn * 10**(snr_db / 10)))
    m = speech + gain * noise
    peak = np.abs(m).max()
    if peak > 0.95:
        m = m * (0.95 / peak)
    return m.astype(np.float32)


def save_inter(prefix, tensors):
    # plain sequential writes: large memory-mapped writes stalled the ntfs3 scratch volume
    shapes = {}
    for key, tensor in tensors.items():
        tensor.float().numpy().astype("<f4").tofile(os.path.join(INTER, f"{prefix}.{key}.f32"))
        shapes[key] = list(tensor.shape)
    with open(os.path.join(INTER, f"{prefix}.json"), "w") as f:
        json.dump(shapes, f)


STAGES = os.environ.get("STAGES", "offline,realtime").split(",")


def save(name, arr):
    np.asarray(arr, dtype="<f4").tofile(os.path.join(OUT, name))


a = load48("2035-147960-0000.flac")
b = load48("2277-149896-0032.flac")
clips = {
    "pink_0db_3s": mix(a[:3 * SR], pink(3 * SR), 0.0),
    "music_5db_3s": mix(b[:3 * SR], music(3 * SR), 5.0),
    "babble_10db_3s": mix(a[SR:4 * SR], b[::-1][:3 * SR].copy(), 10.0),
}
long_speech = np.concatenate([a, b])[:12 * SR]
clips["pink_5db_12s"] = mix(long_speech, pink(long_speech.size), 5.0)
clean = {"pink_0db_3s": a[:3 * SR], "music_5db_3s": b[:3 * SR],
         "babble_10db_3s": a[SR:4 * SR], "pink_5db_12s": long_speech}
for k, v in clips.items():
    save(f"{k}.in.f32", v)
    save(f"{k}.clean.f32", clean[k])
    sf.write(os.path.join(OUT, f"{k}.in.wav"), v, SR, subtype="FLOAT")

timing = {}
torch.set_num_threads(int(os.environ.get("THREADS", "8")))

if "offline" in STAGES:
    # offline model
    off = viso.load_model(os.path.join(WEIGHTS, "denoiser.safetensors"))
    for k, v in clips.items():
        x = torch.from_numpy(v)
        t0 = time.perf_counter()
        with torch.inference_mode():
            y = off.enhance(x)
        dt = time.perf_counter() - t0
        timing[f"offline/{k}"] = {"seconds": dt, "rtf": dt / (v.size / SR)}
        save(f"{k}.offline.f32", y.numpy())
        sf.write(os.path.join(OUT, f"{k}.offline.wav"), y.numpy(), SR, subtype="FLOAT")
        print("offline", k, timing[f"offline/{k}"], flush=True)

    # intermediates on the first clip, offline model
    x = torch.from_numpy(clips["pink_0db_3s"])[None]
    inter = {}
    with torch.inference_mode():
        spec = off.stft(x)
        inter["stft"] = torch.view_as_real(spec).contiguous()
        feats = off.encode(spec)
        inter["encode"] = feats.contiguous()
        for i, blk in enumerate(off.blocks):
            feats = blk(feats, False)
            inter[f"block{i}"] = feats.contiguous()
        raw = off.denoise_decoder(feats)
        inter["raw"] = raw.contiguous()
        inter["out"] = off(x).contiguous()
    save_inter("offline", inter)
    del off


if "realtime" in STAGES:
    # realtime model, look-ahead 2, pushed in 20 ms pieces with 2-frame blocks
    rt = viso.load_model(os.path.join(WEIGHTS, "denoiser_realtime.safetensors"))
    rt.set_lookahead(2)
    x = torch.from_numpy(clips["pink_0db_3s"])[None]
    inter = {}
    with torch.inference_mode():
        spec = rt.stft(x)
        feats = rt.encode(spec)
        inter["encode"] = feats.contiguous()
        for i, blk in enumerate(rt.blocks):
            feats = blk(feats, True)
            inter[f"block{i}"] = feats.contiguous()
        inter["raw"] = rt.denoise_decoder(feats).contiguous()
        inter["forward"] = rt(x).contiguous()
    save_inter("realtime", inter)
    for k, v in clips.items():
        s = streaming.Stream(rt, block_frames=2)
        outs = []
        lat = []
        t0 = time.perf_counter()
        for start in range(0, v.size, 960):
            t1 = time.perf_counter()
            outs.append(s.push(v[start:start + 960]))
            lat.append(time.perf_counter() - t1)
        outs.append(s.flush(as_numpy=True))
        dt = time.perf_counter() - t0
        y = np.concatenate(outs)
        lat = np.array(lat) * 1000
        timing[f"realtime/{k}"] = {"seconds": dt, "rtf": dt / (v.size / SR),
                                   "push_ms_p50": float(np.percentile(lat, 50)),
                                   "push_ms_p99": float(np.percentile(lat, 99))}
        save(f"{k}.realtime.f32", y)
        sf.write(os.path.join(OUT, f"{k}.realtime.wav"), y, SR, subtype="FLOAT")
        print("realtime", k, timing[f"realtime/{k}"], flush=True)

with open(os.path.join(OUT, f"timing_torch_{"_".join(STAGES)}_{torch.get_num_threads()}t.json"), "w") as f:
    json.dump(timing, f, indent=1)
