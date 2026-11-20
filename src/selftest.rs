//! Headless checks of the engine, run from the command line (`--selftest`, `--selftest-all`, `--selftest-rec`, `--selftest-load`).
use crate::engine::Engine;
use crate::model::*;

/// Headless check of the engine: a test tone through Volume and a 10-band EQ into a WAV recorder.
pub fn selftest(dir: &str) {
    let mut s = Session::new("Self Test");
    let app = std::env::var("KALANGO_AUDIO_SELFTEST_APP").ok();
    let a = s.add_block(if app.is_some() { "app_source" } else { "test_tone" }, 2.0, 2.0);
    let v = s.add_block("volume", 8.0, 2.0);
    let e = s.add_block("graphic_eq", 14.0, 2.0);
    let r = s.add_block("recorder", 20.0, 2.0);
    s.connect(a, v); s.connect(v, e); s.connect(e, r);
    for b in s.blocks.iter_mut() {
        if b.id == v { b.p.set("gain", 1.0); b.p.set("boost", 1.0); }
        if b.id == a { b.p.set("level", 0.9); if let Some(n) = &app { b.p.set_text("app", n); } }
        if b.id == r { b.p.set_text("folder", dir); b.p.set_text("file_name", "selftest %name %counter"); b.p.set_text("format", "WAV"); }
    }
    let eng = Engine::start(s.blocks.clone(), &s.name);
    std::thread::sleep(std::time::Duration::from_millis(if app.is_some() { 5000 } else { 2500 }));
    let st = eng.shared.rec.lock().unwrap().clone();
    for (k, v) in st { println!("recorder {k}: {} {:.2}s {} bytes {}", v.state, v.seconds, v.bytes, v.file); }
    drop(eng);
}

/// Headless checks of the newer blocks; each scenario records two seconds into its own folder.
pub fn selftest_all(dir: &str) {
    fn scenario(dir: &str, name: &str, build: impl Fn(&mut Session) -> u64, secs: u64) {
        let mut s = Session::new(name);
        let last = build(&mut s);
        let r = s.add_block("recorder", 40.0, 2.0);
        s.connect(last, r);
        for b in s.blocks.iter_mut() {
            if b.id == r { b.p.set_text("folder", &format!("{dir}/{name}")); b.p.set_text("file_name", "out"); b.p.set_text("format", "WAV"); }
        }
        let eng = Engine::start(s.blocks.clone(), &s.name);
        std::thread::sleep(std::time::Duration::from_millis(secs * 1000));
        let st = eng.shared.rec.lock().unwrap().clone();
        for (k, v) in st { println!("{name}: block {k}: {} {:.2}s", v.state, v.seconds); }
        drop(eng);
    }
    let tone = |s: &mut Session, f: f64, l: f64| { let t = s.add_block("test_tone", 2.0, 2.0); for b in s.blocks.iter_mut() { if b.id == t { b.p.set("freq", f); b.p.set("level", l); } } t };
    scenario(dir, "mixer", |s| {
        let a = tone(s, 440.0, 0.2); let b = tone(s, 880.0, 0.2);
        let m = s.add_block("mixer", 20.0, 2.0);
        s.connect(a, m); s.connect(b, m);
        for x in s.blocks.iter_mut() { if x.id == m { x.p.set("m1", 1.0); } }
        m
    }, 2);
    scenario(dir, "ducking", |s| {
        let ov = tone(s, 1000.0, 0.1); let bg = tone(s, 200.0, 0.5);
        let d = s.add_block("ducking", 20.0, 2.0);
        s.connect(bg, d); s.connect(ov, d);
        for x in s.blocks.iter_mut() { if x.id == d { x.p.set("overlay", ov as f64); } }
        d
    }, 2);
    scenario(dir, "sync", |s| {
        let a = tone(s, 440.0, 0.5);
        let y = s.add_block("delay", 20.0, 2.0);
        s.connect(a, y);
        for x in s.blocks.iter_mut() { if x.id == y { x.p.set("delay", 300.0); } }
        y
    }, 2);
    scenario(dir, "paramEQ", |s| {
        let a = tone(s, 1000.0, 0.3);
        let y = s.add_block("parametric_eq", 20.0, 2.0);
        s.connect(a, y);
        for x in s.blocks.iter_mut() { if x.id == y { x.p.set("b1_gain", 12.0); } }
        y
    }, 2);
    scenario(dir, "denoise", |s| {
        let a = tone(s, 1000.0, 0.3);
        let y = s.add_block("noise_reduction", 20.0, 2.0);
        s.connect(a, y);
        y
    }, 2);
    scenario(dir, "denoise2", |s| {
        let a = tone(s, 1000.0, 0.3);
        let y = s.add_block("noise_reduction", 20.0, 2.0);
        s.connect(a, y);
        for x in s.blocks.iter_mut() { if x.id == y { x.p.set("learn_seq", 1.0); } }
        y
    }, 5);
    if let Ok(url) = std::env::var("KALANGO_AUDIO_RTMP_URL") {
        scenario(dir, "rtmp", |s| {
            let a = tone(s, 440.0, 0.3);
            let y = s.add_block("rtmp_stream", 20.0, 2.0);
            s.connect(a, y);
            for x in s.blocks.iter_mut() { if x.id == y { x.p.set_text("url", &url); x.p.set_text("key", "k"); x.p.set("quality", 0.0); } }
            y
        }, 6);
    }
    scenario(dir, "broadcast", |s| {
        let a = tone(s, 440.0, 0.3);
        let y = s.add_block("radio_stream", 20.0, 2.0);
        s.connect(a, y);
        y
    }, 1);
    scenario(dir, "transcribe", |s| {
        let a = tone(s, 440.0, 0.3);
        let y = s.add_block("transcribe", 20.0, 2.0);
        s.connect(a, y);
        y
    }, 1);
}


/// Headless checks of the recorder's file rules (split, stop, silence, formats).
pub fn selftest_rec(dir: &str) {
    fn go(dir: &str, name: &str, level: f64, secs: u64, set: &dyn Fn(&mut Params)) {
        let mut s = Session::new(name);
        let t = s.add_block("test_tone", 2.0, 2.0);
        let r = s.add_block("recorder", 10.0, 2.0);
        s.connect(t, r);
        for b in s.blocks.iter_mut() {
            if b.id == t { b.p.set("level", level); }
            if b.id == r { b.p.set_text("folder", &format!("{dir}/{name}")); b.p.set_text("file_name", "take %counter"); b.p.set_text("format", "WAV"); set(&mut b.p); }
        }
        let eng = Engine::start(s.blocks.clone(), name);
        std::thread::sleep(std::time::Duration::from_millis(secs * 1000));
        for (_, v) in eng.shared.rec.lock().unwrap().iter() { println!("{name}: state={}", v.state); }
        drop(eng);
    }
    go(dir, "split", 0.3, 5, &|p| { p.set("split_on", 1.0); p.set("split_size", 0.03); p.set_text("split_unit", "time*60"); });
    go(dir, "stop", 0.3, 3, &|p| { p.set("stop_on", 1.0); p.set("stop_size", 0.02); p.set_text("stop_unit", "time*60"); });
    go(dir, "silence_remove", 0.0, 4, &|p| { p.set("silence_on", 1.0); p.set("silence_action", 1.0); p.set("silence_time", 1.0); });
    go(dir, "silence_end", 0.0, 4, &|p| { p.set("silence_on", 1.0); p.set("silence_action", 3.0); p.set("silence_time", 1.0); });
    go(dir, "aiff", 0.3, 2, &|p| { p.set_text("format", "AIFF"); });
    go(dir, "mp3", 0.3, 2, &|p| { p.set_text("format", "MP3"); p.set("bitrate", 128.0); });
    go(dir, "wav24", 0.3, 2, &|p| { p.set("depth", 24.0); });
    go(dir, "paused", 0.3, 2, &|p| { p.set("pause", 1.0); });
}

/// Runs a heavy chain for a few seconds and reports how busy the audio thread was.
pub fn selftest_load() {
    let mut s = Session::new("Load");
    let a = s.add_block("test_tone", 2.0, 2.0);
    let chain = ["graphic_eq", "compressor", "noise_reduction", "voice_denoise", "loudness", "stereo_widen", "parametric_eq", "tone_control", "hum_removal"];
    let mut prev = a;
    for k in chain { let b = s.add_block(k, 8.0, 2.0); s.connect(prev, b); prev = b; }
    let eng = Engine::start(s.blocks.clone(), "Load");
    std::thread::sleep(std::time::Duration::from_millis(4000));
    let l = eng.shared.load.load(std::sync::atomic::Ordering::Relaxed);
    let o = eng.shared.overruns.load(std::sync::atomic::Ordering::Relaxed);
    println!("audio load {:.1}% , chunks that ran late: {}", l as f32 / 10.0, o);
}

