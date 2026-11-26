//! The release checkpoint: a `.safetensors` file whose metadata carries the architecture as JSON.
//!
//! The file is memory-mapped and every float32 tensor is read in place, so the weights cost page cache (shared by
//! every process that maps the same file) rather than heap.

use std::collections::HashMap;
use std::fs::File;
use std::path::Path;

use memmap2::Mmap;
use serde_json::Value;

use crate::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockKind {
    Transformer,
    Mamba,
}

/// The architecture, always read from the checkpoint (as upstream does).
#[derive(Clone, Debug)]
pub struct Config {
    pub width: usize,
    pub num_heads: usize,
    pub n_fft: usize,
    pub hop: usize,
    pub sample_rate: u32,
    pub band_plan: Vec<(f64, f64)>,
    pub correction_bound: f32,
    pub chunk_seconds: f64,
    pub overlap_seconds: f64,
    pub exclusive_time: bool,
    pub exclusive_band: bool,
    pub causal: bool,
    pub lookahead_frames: usize,
    pub lookahead_choices: Vec<usize>,
    pub attention_window: usize,
    pub blocks: Vec<BlockKind>,
}

pub const MAMBA_STATE: usize = 16;
pub const MAMBA_CONV: usize = 4;
pub const MAMBA_EXPAND: usize = 2;

impl Config {
    fn from_json(raw: &Value) -> Result<Config, Error> {
        let bad = |what: &str| Error::Format(format!("configuração do modelo inválida ou não suportada: {what}"));
        let int = |name: &str, default: Option<u64>| -> Result<usize, Error> {
            match raw.get(name) {
                Some(v) => v.as_u64().map(|v| v as usize).ok_or_else(|| bad(name)),
                None => default.map(|v| v as usize).ok_or_else(|| bad(name)),
            }
        };
        let float = |name: &str, default: f64| -> Result<f64, Error> {
            match raw.get(name) {
                Some(v) => v.as_f64().ok_or_else(|| bad(name)),
                None => Ok(default),
            }
        };
        let flag = |name: &str| raw.get(name).and_then(Value::as_bool).unwrap_or(false);
        for (name, value) in [("representation", "bands"), ("decoder_type", "complex_mask_residual")] {
            if let Some(v) = raw.get(name) {
                if v.as_str() != Some(value) {
                    return Err(bad(name));
                }
            }
        }
        if raw.get("hint_checkpoint").is_some_and(|v| !v.is_null()) {
            return Err(bad("hint_checkpoint"));
        }
        if flag("dual_mode") || flag("mode_film") || int("offline_passes", Some(1))? != 1 {
            return Err(bad("dual_mode/mode_film/offline_passes"));
        }
        if int("mamba_state", Some(16))? != MAMBA_STATE
            || int("mamba_conv", Some(4))? != MAMBA_CONV
            || int("mamba_expand", Some(2))? != MAMBA_EXPAND
        {
            return Err(bad("dimensões do Mamba"));
        }
        let blocks = raw
            .get("block_types")
            .and_then(Value::as_array)
            .ok_or_else(|| bad("block_types"))?
            .iter()
            .map(|v| match v.as_str() {
                Some("transformer") => Ok(BlockKind::Transformer),
                Some("mamba") => Ok(BlockKind::Mamba),
                _ => Err(bad("block_types")),
            })
            .collect::<Result<Vec<_>, _>>()?;
        let band_plan = raw
            .get("band_plan")
            .and_then(Value::as_array)
            .ok_or_else(|| bad("band_plan"))?
            .iter()
            .map(|pair| match pair.as_array().map(|p| p.as_slice()) {
                Some([a, b]) => match (a.as_f64(), b.as_f64()) {
                    (Some(a), Some(b)) => Ok((a, b)),
                    _ => Err(bad("band_plan")),
                },
                _ => Err(bad("band_plan")),
            })
            .collect::<Result<Vec<_>, _>>()?;
        let axes = raw.get("exclusive_axes").and_then(Value::as_str).unwrap_or("time");
        let exclusive = raw.get("exclusive_self_attention").and_then(Value::as_bool).unwrap_or(true);
        let lookahead_choices = raw
            .get("lookahead_frames_choices")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_u64).map(|v| v as usize).collect())
            .unwrap_or_default();
        let config = Config {
            width: int("width", None)?,
            num_heads: int("num_heads", None)?,
            n_fft: int("n_fft", None)?,
            hop: int("hop_length", None)?,
            sample_rate: int("sample_rate", None)? as u32,
            band_plan,
            correction_bound: float("correction_bound", 0.5)? as f32,
            chunk_seconds: float("chunk_seconds", 12.0)?,
            overlap_seconds: float("overlap_seconds", 2.0)?,
            exclusive_time: exclusive && (axes == "both" || axes == "time"),
            exclusive_band: exclusive && (axes == "both" || axes == "band"),
            causal: flag("causal"),
            lookahead_frames: int("lookahead_frames", Some(0))?,
            lookahead_choices,
            attention_window: int("attention_window_frames", Some(0))?,
            blocks,
        };
        if !config.width.is_multiple_of(2 * config.num_heads) || !(config.width / config.num_heads).is_multiple_of(2) {
            return Err(bad("width/num_heads"));
        }
        if !config.n_fft.is_multiple_of(2) || config.hop == 0 || config.hop > config.n_fft / 2 {
            return Err(bad("n_fft/hop_length"));
        }
        Ok(config)
    }

    pub fn bins(&self) -> usize {
        self.n_fft / 2 + 1
    }

    /// Bin ranges `[start, end)` of the bands, as upstream's `band_edges` (Python's round half to even included).
    pub fn band_edges(&self) -> Vec<(usize, usize)> {
        let bins = self.bins();
        let hz_per_bin = self.sample_rate as f64 / self.n_fft as f64;
        let mut edges = Vec::new();
        let mut start = 0usize;
        for &(width_hz, upto_hz) in &self.band_plan {
            let width = (round_half_even(width_hz / hz_per_bin) as usize).max(1);
            let upto = (round_half_even(upto_hz / hz_per_bin) as usize).min(bins);
            while start < upto {
                let end = (start + width).min(upto);
                edges.push((start, end));
                start = end;
            }
        }
        if start < bins {
            edges.push((start, bins));
        }
        edges
    }
}

fn round_half_even(x: f64) -> f64 {
    let r = x.round();
    if (x - x.trunc()).abs() == 0.5 && r % 2.0 != 0.0 {
        r - x.signum()
    } else {
        r
    }
}

enum Store {
    Mapped(Mmap),
    Owned(Vec<f32>),
}

/// An opened checkpoint.
pub struct Checkpoint {
    store: Store,
    data_start: usize,
    tensors: HashMap<String, (Vec<usize>, usize, usize)>,
    pub config: Config,
}

impl Checkpoint {
    pub fn open(path: &Path) -> Result<Checkpoint, Error> {
        let file = File::open(path)?;
        // SAFETY: the map is read-only; the checkpoint must not be changed on disk while it is open (as with any
        // memory-mapped model file).
        let map = unsafe { Mmap::map(&file)? };
        let fail = |what: &str| Error::Format(format!("{}: {what}", path.display()));
        if map.len() < 8 {
            return Err(fail("arquivo curto demais para safetensors"));
        }
        let header_len = u64::from_le_bytes(map[..8].try_into().unwrap()) as usize;
        let data_start = 8usize.checked_add(header_len).filter(|&s| s <= map.len()).ok_or_else(|| fail("cabeçalho"))?;
        let header: Value = serde_json::from_slice(&map[8..data_start]).map_err(|_| fail("cabeçalho JSON"))?;
        let header = header.as_object().ok_or_else(|| fail("cabeçalho JSON"))?;
        let config = header
            .get("__metadata__")
            .and_then(|m| m.get("config"))
            .and_then(Value::as_str)
            .ok_or_else(|| fail("metadados sem 'config': não é um checkpoint do Voice Isolator"))?;
        let config: Value = serde_json::from_str(config).map_err(|_| fail("config JSON"))?;
        let config = Config::from_json(&config)?;
        let mut tensors = HashMap::new();
        let data_len = map.len() - data_start;
        for (name, info) in header {
            if name == "__metadata__" {
                continue;
            }
            if info.get("dtype").and_then(Value::as_str) != Some("F32") {
                return Err(fail(&format!("{name}: só float32 é suportado")));
            }
            let shape: Vec<usize> = info
                .get("shape")
                .and_then(Value::as_array)
                .ok_or_else(|| fail(name))?
                .iter()
                .map(|v| v.as_u64().map(|v| v as usize))
                .collect::<Option<_>>()
                .ok_or_else(|| fail(name))?;
            let offsets = info.get("data_offsets").and_then(Value::as_array).ok_or_else(|| fail(name))?;
            let (start, end) = match offsets.as_slice() {
                [a, b] => {
                    (a.as_u64().ok_or_else(|| fail(name))? as usize, b.as_u64().ok_or_else(|| fail(name))? as usize)
                }
                _ => return Err(fail(name)),
            };
            let count: usize = shape.iter().product();
            if end < start || end > data_len || end - start != 4 * count || start % 4 != 0 {
                return Err(fail(&format!("{name}: deslocamentos inválidos")));
            }
            tensors.insert(name.clone(), (shape, start, end));
        }
        let store = if (map.as_ptr() as usize + data_start).is_multiple_of(4) && cfg!(target_endian = "little") {
            Store::Mapped(map)
        } else {
            let mut owned = vec![0f32; data_len / 4];
            for (dst, src) in owned.iter_mut().zip(map[data_start..].as_chunks::<4>().0) {
                *dst = f32::from_le_bytes(*src);
            }
            Store::Owned(owned)
        };
        let data_start = if matches!(store, Store::Owned(_)) { 0 } else { data_start };
        Ok(Checkpoint { store, data_start, tensors, config })
    }

    /// The tensor `name`, which must have `shape`.
    pub fn get(&self, name: &str, shape: &[usize]) -> Result<Tensor, Error> {
        let (have, start, end) =
            self.tensors.get(name).ok_or_else(|| Error::Format(format!("peso ausente no checkpoint: {name}")))?;
        if have != shape {
            return Err(Error::Format(format!("{name}: forma {have:?}, esperada {shape:?}")));
        }
        let base = match &self.store {
            Store::Mapped(map) => unsafe { map.as_ptr().add(self.data_start + start) as *const f32 },
            Store::Owned(v) => unsafe { v.as_ptr().add(start / 4) },
        };
        Ok(Tensor { ptr: base, len: (end - start) / 4 })
    }
}

/// A read-only view of one tensor of a [`Checkpoint`]. It stays valid while the checkpoint it came from is alive;
/// the network keeps an `Arc` of the checkpoint next to every tensor it holds.
#[derive(Clone, Copy)]
pub struct Tensor {
    ptr: *const f32,
    len: usize,
}

unsafe impl Send for Tensor {}
unsafe impl Sync for Tensor {}

impl std::ops::Deref for Tensor {
    type Target = [f32];
    fn deref(&self) -> &[f32] {
        // SAFETY: see the type's documentation.
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }
}
