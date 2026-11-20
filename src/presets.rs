//! Saved settings: the app's preferences, presets (ours, built in, and the person's), and scripts the person wrote.
use crate::i18n::n_;
use crate::model::{Block, Params};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Prefs {
    pub audio_editor: String,
    pub default_folder: String,
    #[serde(default)]
    pub bar_dismissed: bool,
    /// default recording format for new Recorder blocks ("" = the block's own default)
    #[serde(default)]
    pub default_format: String,
    /// default input and output devices ("" = the system's)
    #[serde(default)]
    pub default_input: String,
    #[serde(default)]
    pub default_output: String,
    /// the first-use screen was dismissed
    #[serde(default)]
    pub welcomed: bool,
    /// sample rate for new Recorder blocks in Hz (0 = the session's)
    #[serde(default)]
    pub sample_rate: u32,
    /// bits per sample for new Recorder blocks (0 = the block's own default, 16)
    #[serde(default)]
    pub bit_depth: u32,
    /// file name pattern for new Recorder blocks, with %name, %date, %time, %counter ("" = the block's own default)
    #[serde(default)]
    pub file_name: String,
    /// new Recorder blocks start another file every `split_minutes`
    #[serde(default)]
    pub split_long: bool,
    #[serde(default = "sixty")]
    pub split_minutes: u32,
    /// new Recorder blocks pause while it is silent
    #[serde(default)]
    pub skip_silence: bool,
}

fn sixty() -> u32 {
    60
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Preset {
    pub kind: String,
    pub name: String,
    pub p: Params,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Script {
    pub name: String,
    pub text: String,
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Store {
    pub prefs: Prefs,
    pub presets: Vec<Preset>,
    pub scripts: Vec<Script>,
    /// tags given to recordings, by file path
    #[serde(default)]
    pub tags: std::collections::BTreeMap<String, Vec<String>>,
    /// recordings marked as favourite, by file path
    #[serde(default)]
    pub favourites: Vec<String>,
}

/// Our own equalizer presets, written as dB per band (31 Hz .. 16 kHz). The sliders store 0..1 with 0.5 = flat over a
/// +-12 dB range, so each value becomes 0.5 + dB / 24. Names are English msgids, shown with `tr`.
pub const EQ_PRESETS_DB: [(&str, [f64; 10]); 12] = [
    (n_("Flat"), [0.0; 10]),
    (n_("Clear voice"), [-6.0, -4.0, -2.0, 0.0, 1.0, 2.0, 3.0, 3.0, 2.0, 0.0]),
    (n_("Warm podcast"), [-4.0, 0.0, 2.0, 1.0, 0.0, -1.0, 1.0, 2.0, 1.0, 0.0]),
    (n_("More bass"), [6.0, 5.0, 4.0, 2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
    (n_("Less bass"), [-6.0, -5.0, -4.0, -2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
    (n_("More treble"), [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 2.0, 4.0, 5.0, 6.0]),
    (n_("Less treble"), [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, -2.0, -4.0, -5.0, -6.0]),
    (n_("Headphones"), [3.0, 2.0, 1.0, 0.0, 0.0, 0.0, -1.0, 0.0, 1.0, 1.0]),
    (n_("Laptop speaker"), [-8.0, -6.0, -3.0, 0.0, 2.0, 2.0, 2.0, 1.0, 0.0, -1.0]),
    (n_("Old radio"), [-12.0, -12.0, -6.0, 0.0, 3.0, 4.0, 2.0, -6.0, -12.0, -12.0]),
    (n_("Live music"), [2.0, 1.0, 0.0, -1.0, -1.0, 0.0, 1.0, 2.0, 2.0, 1.0]),
    (n_("Quiet at night"), [5.0, 4.0, 2.0, 0.0, -1.0, -1.0, 0.0, 2.0, 3.0, 3.0]),
];

pub fn eq_presets() -> Vec<(String, [f64; 10])> {
    EQ_PRESETS_DB
        .iter()
        .map(|(n, db)| {
            let mut a = [0.5; 10];
            for (i, x) in db.iter().enumerate() {
                a[i] = (0.5 + x / 24.0).clamp(0.0, 1.0);
            }
            (n.to_string(), a)
        })
        .collect()
}

/// Our own compressor modes: (name, amount 0..1, speed 0..1, make-up gain).
pub const COMPRESSOR_MODES: [(&str, f64, f64, f64); 5] = [
    (n_("Off"), 0.0, 0.0, 1.0),
    (n_("Speech"), 0.75, 0.6, 1.2),
    (n_("Music"), 0.5, 0.3, 1.1),
    (n_("Broadcast"), 0.9, 0.8, 1.3),
    (n_("Film"), 0.6, 0.4, 1.15),
];

/// Built-in presets; their names are English msgids (shown with `tr`).
pub fn builtin_presets() -> Vec<Preset> {
    let mut v = vec![];
    for (fmt, br, name) in [
        ("AAC", 64, n_("AAC · voice (64 kbps)")),
        ("MP3", 96, n_("MP3 · voice (96 kbps)")),
        ("MP3", 192, n_("MP3 · music (192 kbps)")),
        ("MP3", 320, n_("MP3 · highest (320 kbps)")),
        ("AAC", 160, n_("AAC · music (160 kbps)")),
        ("FLAC", 0, n_("FLAC · lossless")),
        ("WAV", 0, n_("WAV · uncompressed")),
    ] {
        let mut p = Params::default();
        p.set_text("format", fmt);
        if br > 0 {
            p.set("bitrate", br as f64);
        }
        v.push(Preset { kind: "recorder".into(), name: name.into(), p });
    }
    for (name, vals) in eq_presets() {
        let mut p = Params::default();
        for (i, x) in vals.iter().enumerate() {
            p.set(&format!("band{i}"), *x);
        }
        v.push(Preset { kind: "graphic_eq".into(), name, p });
    }
    for (name, amount, speed, makeup) in COMPRESSOR_MODES {
        let mut p = Params::default();
        p.set("amount", amount);
        p.set("speed", speed);
        p.set("makeup", makeup);
        v.push(Preset { kind: "compressor".into(), name: name.into(), p });
    }
    v
}

/// Copies a preset's settings into a block, leaving the block's other settings alone.
pub fn apply_preset(b: &mut Block, p: &Preset) {
    for (k, v) in &p.p.n {
        b.p.n.insert(k.clone(), *v);
    }
    for (k, v) in &p.p.s {
        b.p.s.insert(k.clone(), v.clone());
    }
}

/// The compressor mode whose values a block has now, if any.
pub fn compressor_mode_of(p: &Params) -> Option<&'static str> {
    let (a, s, m) = (p.num("amount", 1.0), p.num("speed", 0.0), p.num("makeup", 1.0));
    COMPRESSOR_MODES.iter().find(|x| (x.1 - a).abs() < 0.005 && (x.2 - s).abs() < 0.005 && (x.3 - m).abs() < 0.005).map(|x| x.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Session;

    #[test]
    fn presets_only_touch_their_own_settings() {
        let mut s = Session::new("t");
        let r = s.add_block("recorder", 0.0, 0.0);
        let preset = builtin_presets().into_iter().find(|p| p.name.starts_with("MP3 · highest")).unwrap();
        let b = s.blocks.iter_mut().find(|b| b.id == r).unwrap();
        b.p.set_text("file_name", "keep me");
        apply_preset(b, &preset);
        assert_eq!(b.p.text("format", ""), "MP3");
        assert_eq!(b.p.num("bitrate", 0.0), 320.0);
        assert_eq!(b.p.text("file_name", ""), "keep me");
    }

    #[test]
    fn our_equalizer_and_compressor_presets_are_well_formed() {
        let eq = eq_presets();
        assert_eq!(eq.len(), EQ_PRESETS_DB.len());
        assert!(eq.iter().all(|(_, v)| v.iter().all(|x| (0.0..=1.0).contains(x))));
        let flat = eq.iter().find(|(n, _)| n == "Flat").unwrap();
        assert!(flat.1.iter().all(|x| *x == 0.5));
        let bass = eq.iter().find(|(n, _)| n == "More bass").unwrap();
        assert!((bass.1[0] - 0.75).abs() < 1e-9, "+6 dB is 0.75 on the slider");
        assert!(COMPRESSOR_MODES.iter().all(|m| (0.0..=1.0).contains(&m.1) && (0.0..=1.0).contains(&m.2) && m.3 >= 1.0));
        let mut p = Params::default();
        p.set("amount", 0.75);
        p.set("speed", 0.6);
        p.set("makeup", 1.2);
        assert_eq!(compressor_mode_of(&p), Some("Speech"));
    }

    #[test]
    fn old_stores_without_the_new_fields_still_load() {
        let s: Store = serde_json::from_str(r#"{"prefs":{"audio_editor":"","default_folder":"~/x"},"presets":[],"scripts":[]}"#).unwrap();
        assert_eq!(s.prefs.default_folder, "~/x");
        assert!(s.tags.is_empty() && !s.prefs.welcomed);
        assert!(serde_json::from_str::<Store>("{ broken").is_err());
    }
}
