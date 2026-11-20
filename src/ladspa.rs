//! Effect plug-ins (LADSPA): third-party effects installed on the system, shown as the "Plug-in LADSPA" block.
//! Plug-ins are found in the usual folders, loaded at run time, and run on the sound like any other block.
use crate::i18n::tr;
use crate::dsp::Frame;
use libloading::Library;
use std::ffi::{c_char, c_ulong, c_void, CStr};
use std::path::{Path, PathBuf};

#[repr(C)]
struct PortRangeHint {
    hint: i32,
    lower: f32,
    upper: f32,
}

#[repr(C)]
struct Descriptor {
    unique_id: c_ulong,
    label: *const c_char,
    properties: i32,
    name: *const c_char,
    maker: *const c_char,
    copyright: *const c_char,
    port_count: c_ulong,
    port_descriptors: *const i32,
    port_names: *const *const c_char,
    port_range_hints: *const PortRangeHint,
    implementation_data: *mut c_void,
    instantiate: Option<unsafe extern "C" fn(*const Descriptor, c_ulong) -> *mut c_void>,
    connect_port: Option<unsafe extern "C" fn(*mut c_void, c_ulong, *mut f32)>,
    activate: Option<unsafe extern "C" fn(*mut c_void)>,
    run: Option<unsafe extern "C" fn(*mut c_void, c_ulong)>,
    run_adding: Option<unsafe extern "C" fn(*mut c_void, c_ulong)>,
    set_run_adding_gain: Option<unsafe extern "C" fn(*mut c_void, f32)>,
    deactivate: Option<unsafe extern "C" fn(*mut c_void)>,
    cleanup: Option<unsafe extern "C" fn(*mut c_void)>,
}

const INPUT: i32 = 1;
const OUTPUT: i32 = 2;
const CONTROL: i32 = 4;
const AUDIO: i32 = 8;

#[derive(Clone, Debug)]
pub struct Control {
    pub port: usize,
    pub name: String,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub log: bool,
    pub integer: bool,
    pub toggle: bool,
}

#[derive(Clone, Debug)]
pub struct PluginInfo {
    pub path: PathBuf,
    pub index: usize,
    pub label: String,
    pub name: String,
    pub maker: String,
    pub controls: Vec<Control>,
    pub ins: Vec<usize>,
    pub outs: Vec<usize>,
}

impl PluginInfo {
    /// How a block remembers its plug-in.
    pub fn key(&self) -> String { format!("{}#{}", self.path.display(), self.index) }
}

type EntryFn = unsafe extern "C" fn(c_ulong) -> *const Descriptor;

fn cstr(p: *const c_char) -> String {
    if p.is_null() { String::new() } else { unsafe { CStr::from_ptr(p).to_string_lossy().to_string() } }
}

pub fn search_dirs() -> Vec<PathBuf> {
    if let Some(p) = std::env::var_os("LADSPA_PATH") { return std::env::split_paths(&p).collect(); }
    let home = std::env::var("HOME").unwrap_or_default();
    vec![PathBuf::from("/usr/lib/ladspa"), PathBuf::from("/usr/lib64/ladspa"), PathBuf::from("/usr/lib/x86_64-linux-gnu/ladspa"), PathBuf::from("/usr/local/lib/ladspa"), PathBuf::from(home).join(".ladspa")]
}

unsafe fn describe(path: &Path, d: &Descriptor, index: usize) -> PluginInfo {
    let (mut controls, mut ins, mut outs) = (vec![], vec![], vec![]);
    for p in 0..d.port_count as usize {
        let desc = *d.port_descriptors.add(p);
        let name = cstr(*d.port_names.add(p));
        let h = &*d.port_range_hints.add(p);
        if desc & AUDIO != 0 { if desc & INPUT != 0 { ins.push(p); } else if desc & OUTPUT != 0 { outs.push(p); } }
        else if desc & CONTROL != 0 && desc & INPUT != 0 {
            let (below, above) = (h.hint & 1 != 0, h.hint & 2 != 0);
            let log = h.hint & 0x10 != 0;
            let (mut min, mut max) = (if below { h.lower } else { 0.0 }, if above { h.upper } else { 1.0 });
            if h.hint & 8 != 0 { min *= 48000.0; max *= 48000.0; } // bounds given as fractions of the sample rate
            let pick = |frac: f32| if log && min > 0.0 && max > 0.0 { (min.ln() * (1.0 - frac) + max.ln() * frac).exp() } else { min * (1.0 - frac) + max * frac };
            let default = match h.hint & 0x3C0 {
                0x40 => min, 0x80 => pick(0.25), 0xC0 => pick(0.5), 0x100 => pick(0.75), 0x140 => max,
                0x200 => 0.0, 0x240 => 1.0, 0x280 => 100.0, 0x2C0 => 440.0,
                _ => min,
            };
            controls.push(Control { port: p, name, min, max, default: default.clamp(min.min(max), max.max(min)), log, integer: h.hint & 0x20 != 0, toggle: h.hint & 4 != 0 });
        }
    }
    PluginInfo { path: path.to_path_buf(), index, label: cstr(d.label), name: cstr(d.name), maker: cstr(d.maker), controls, ins, outs }
}

/// Reads every effect in one plug-in file (only ones with 1 or 2 audio inputs and outputs of the same count).
pub fn load_file(path: &Path) -> Vec<PluginInfo> {
    let mut v = vec![];
    unsafe {
        let Ok(lib) = Library::new(path) else { return v };
        let Ok(entry) = lib.get::<EntryFn>(b"ladspa_descriptor\0") else { return v };
        let entry = *entry;
        let mut i = 0;
        loop {
            let d = entry(i as c_ulong);
            if d.is_null() { break; }
            let info = describe(path, &*d, i);
            if (info.ins.len() == 1 || info.ins.len() == 2) && info.ins.len() == info.outs.len() { v.push(info); }
            i += 1;
        }
        std::mem::forget(lib); // plug-ins stay loaded for the life of the program
    }
    v
}

static CACHE: std::sync::Mutex<Option<Vec<PluginInfo>>> = std::sync::Mutex::new(None);

/// The plug-ins found on this computer (looked up once; `rescan` looks again).
pub fn cached() -> Vec<PluginInfo> {
    let mut g = CACHE.lock().unwrap();
    if g.is_none() { *g = Some(scan()); }
    g.clone().unwrap_or_default()
}

pub fn rescan() { *CACHE.lock().unwrap() = Some(scan()); }

/// Like `find`, but from the list already read (cheap enough to call every chunk).
pub fn find_cached(key: &str) -> Option<PluginInfo> {
    let (path, idx) = key.rsplit_once('#')?;
    let idx: usize = idx.parse().ok()?;
    cached().into_iter().find(|p| p.path.to_string_lossy() == path && p.index == idx).or_else(|| find(key))
}

pub fn find(key: &str) -> Option<PluginInfo> {
    let (path, idx) = key.rsplit_once('#')?;
    let idx: usize = idx.parse().ok()?;
    load_file(Path::new(path)).into_iter().find(|p| p.index == idx)
}

pub fn scan() -> Vec<PluginInfo> {
    let mut v = vec![];
    for dir in search_dirs() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let mut files: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().map(|e| e == "so").unwrap_or(false)).collect();
        files.sort();
        for f in files { v.extend(load_file(&f)); }
    }
    v
}

/// A running copy of a plug-in. A mono plug-in is run twice, once per side.
pub struct Instance {
    lib: &'static Library,
    desc: &'static Descriptor,
    handles: Vec<*mut c_void>,
    info: PluginInfo,
    controls: Vec<f32>,
    inp: Vec<Vec<f32>>,
    out: Vec<Vec<f32>>,
}

unsafe impl Send for Instance {}

impl Instance {
    pub fn new(info: &PluginInfo, rate: f32) -> Result<Instance, String> {
        unsafe {
            let lib: &'static Library = Box::leak(Box::new(Library::new(&info.path).map_err(|e| e.to_string())?));
            let entry = *lib.get::<EntryFn>(b"ladspa_descriptor\0").map_err(|e| e.to_string())?;
            let d = entry(info.index as c_ulong);
            if d.is_null() { return Err(tr("the plug-in is no longer there").into()); }
            let desc: &'static Descriptor = &*d;
            let inst = desc.instantiate.ok_or_else(|| tr("the plug-in cannot be started").to_string())?;
            let sides = if info.ins.len() == 1 { 2 } else { 1 };
            let mut handles = vec![];
            for _ in 0..sides {
                let h = inst(d, rate as c_ulong);
                if h.is_null() { return Err(tr("the plug-in refused to start").into()); }
                handles.push(h);
            }
            let controls: Vec<f32> = info.controls.iter().map(|c| c.default).collect();
            let mut s = Instance { lib, desc, handles, info: info.clone(), controls, inp: vec![vec![]; 2], out: vec![vec![]; 2] };
            // controls that are not inputs (outputs, etc.) still need somewhere to write
            s.activate();
            Ok(s)
        }
    }

    unsafe fn activate(&mut self) {
        if let Some(a) = self.desc.activate { for h in &self.handles { a(*h); } }
    }

    pub fn set_control(&mut self, i: usize, v: f32) { if let Some(c) = self.controls.get_mut(i) { *c = v; } }

    pub fn process(&mut self, buf: &mut [Frame]) {
        let n = buf.len();
        let mut sink = vec![0.0f32; 1]; // for output-control ports
        unsafe {
            for ch in 0..2 { self.inp[ch].clear(); self.inp[ch].extend(buf.iter().map(|f| f[ch])); self.out[ch].clear(); self.out[ch].resize(n, 0.0); }
            let connect = self.desc.connect_port.unwrap();
            let run = self.desc.run.unwrap();
            for (hi, h) in self.handles.iter().enumerate() {
                for p in 0..self.desc.port_count as usize {
                    let pd = *self.desc.port_descriptors.add(p);
                    if pd & CONTROL != 0 {
                        if let Some(ci) = self.info.controls.iter().position(|c| c.port == p) { connect(*h, p as c_ulong, &mut self.controls[ci] as *mut f32); }
                        else { connect(*h, p as c_ulong, sink.as_mut_ptr()); }
                    }
                }
                // audio ports: a mono plug-in handles side `hi`, a stereo one has both sides
                for (k, p) in self.info.ins.iter().enumerate() { let side = if self.handles.len() == 2 { hi } else { k }; connect(*h, *p as c_ulong, self.inp[side].as_mut_ptr()); }
                for (k, p) in self.info.outs.iter().enumerate() { let side = if self.handles.len() == 2 { hi } else { k }; connect(*h, *p as c_ulong, self.out[side].as_mut_ptr()); }
                run(*h, n as c_ulong);
            }
        }
        for (i, f) in buf.iter_mut().enumerate() { *f = [self.out[0][i], self.out[1][i]]; }
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        unsafe {
            for h in &self.handles {
                if let Some(d) = self.desc.deactivate { d(*h); }
                if let Some(c) = self.desc.cleanup { c(*h); }
            }
        }
        let _ = self.lib;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GAIN_PLUGIN: &str = r#"
#include <stdlib.h>
typedef struct { float *gain, *in, *out; } H;
typedef struct { int hint; float lo, hi; } Hint;
typedef struct { unsigned long id; const char *label; int props; const char *name, *maker, *copy; unsigned long n; const int *pd; const char * const *pn; const Hint *ph; void *impl;
  void *(*inst)(const void *, unsigned long); void (*conn)(void *, unsigned long, float *); void (*act)(void *); void (*run)(void *, unsigned long); void (*runadd)(void *, unsigned long); void (*setadd)(void *, float); void (*deact)(void *); void (*clean)(void *); } D;
static void *inst(const void *d, unsigned long r) { return calloc(1, sizeof(H)); }
static void conn(void *h, unsigned long p, float *x) { H *s = h; if (p == 0) s->gain = x; else if (p == 1) s->in = x; else s->out = x; }
static void run(void *h, unsigned long n) { H *s = h; for (unsigned long i = 0; i < n; i++) s->out[i] = s->in[i] * *s->gain; }
static void clean(void *h) { free(h); }
static const int pd[3] = { 1 | 4, 1 | 8, 2 | 8 };
static const char *pn[3] = { "Gain", "Input", "Output" };
static const Hint ph[3] = { { 1 | 2 | 0xC0, 0.0f, 4.0f }, { 0, 0, 0 }, { 0, 0, 0 } };
static const D d = { 4242, "test_gain", 0, "Test Gain", "me", "none", 3, pd, pn, ph, 0, inst, conn, 0, run, 0, 0, 0, clean };
const D *ladspa_descriptor(unsigned long i) { return i == 0 ? &d : 0; }
"#;

    fn build_plugin() -> Option<PathBuf> {
        if !crate::media::have("gcc") { return None; }
        let dir = std::env::temp_dir().join(format!("kalango-audio-ladspa-{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok()?;
        let src = dir.join("gain.c");
        std::fs::write(&src, GAIN_PLUGIN).ok()?;
        let so = dir.join("test_gain.so");
        let ok = std::process::Command::new("gcc").args(["-shared", "-fPIC", "-O1", "-o"]).arg(&so).arg(&src).status().ok()?.success();
        ok.then_some(so)
    }

    #[test]
    fn a_ladspa_plugin_is_found_described_and_run() {
        let Some(so) = build_plugin() else { return };
        let found = load_file(&so);
        assert_eq!(found.len(), 1);
        let p = &found[0];
        assert_eq!(p.name, "Test Gain");
        assert_eq!((p.ins.len(), p.outs.len()), (1, 1));
        assert_eq!(p.controls.len(), 1);
        assert_eq!(p.controls[0].name, "Gain");
        assert_eq!((p.controls[0].min, p.controls[0].max), (0.0, 4.0));
        let mut inst = Instance::new(p, 48000.0).unwrap();
        let mut buf = vec![[0.5f32, -0.25]; 480];
        inst.set_control(0, 2.0);
        inst.process(&mut buf);
        assert!((buf[10][0] - 1.0).abs() < 1e-6 && (buf[10][1] + 0.5).abs() < 1e-6, "a mono plug-in runs once per side");
        inst.set_control(0, 0.0);
        let mut buf = vec![[0.5f32, 0.5]; 480];
        inst.process(&mut buf);
        assert!(buf[10][0] == 0.0);
    }

    #[test]
    fn missing_or_broken_plugin_files_are_skipped() {
        assert!(load_file(Path::new("/not/here.so")).is_empty());
        assert!(!search_dirs().is_empty());
    }

    #[test]
    fn the_plugin_block_runs_a_plugin_inside_the_audio_chain() {
        let Some(so) = build_plugin() else { return };
        let info = &load_file(&so)[0];
        let mut p = crate::model::default_params("ladspa_plugin");
        p.set_text("plugin", &info.key());
        p.set("c0", 3.0);
        let b = crate::model::Block { id: 1, kind: "ladspa_plugin".into(), x: 0.0, y: 0.0, enabled: true, outs: vec![], p };
        let mut d = crate::dsp::Dsp::default();
        let mut buf = vec![[0.1f32, 0.1]; 480];
        d.process(&b, &mut buf);
        assert!((buf[5][0] - 0.3).abs() < 1e-5, "{}", buf[5][0]);
        assert_eq!(d.note, "running");
        // a plug-in that is not there is reported and the sound passes
        let mut p = crate::model::default_params("ladspa_plugin");
        p.set_text("plugin", "/not/here.so#0");
        let b = crate::model::Block { id: 1, kind: "ladspa_plugin".into(), x: 0.0, y: 0.0, enabled: true, outs: vec![], p };
        let mut d = crate::dsp::Dsp::default();
        let mut buf = vec![[0.1f32, 0.1]; 480];
        d.process(&b, &mut buf);
        assert!(d.note.starts_with("error"));
        assert!((buf[5][0] - 0.1).abs() < 1e-6);
    }
}
