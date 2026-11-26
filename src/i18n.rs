//! UI translations, following the KalangoOS pattern (crates/kalango-i18n): source strings are English and double as
//! message ids (`tr("New session")`); each locale has a catalog (English → translation) embedded in the binary; the
//! language is the one chosen in the system (`LANGUAGE`, `LC_ALL`, `LC_MESSAGES`, `LANG`); a missing language or string
//! falls back to English. The pt_BR catalog is split per screen (`locales/pt_BR/*.json`) so screens can be worked on
//! apart; they are merged here.
//!
//! Placeholders are named: `trf("{n} sessions", &[("n", n.to_string())])`.

use std::collections::HashMap;
use std::sync::OnceLock;

/// One language's catalog files (one per screen), merged at start (a key in two files is a mistake the test catches).
macro_rules! screens {
    ($lang:literal) => {
        &[
            ("comum", include_str!(concat!("../locales/", $lang, "/comum.json"))),
            ("blocos", include_str!(concat!("../locales/", $lang, "/blocos.json"))),
            ("motor", include_str!(concat!("../locales/", $lang, "/motor.json"))),
            ("inicio", include_str!(concat!("../locales/", $lang, "/inicio.json"))),
            ("modelos", include_str!(concat!("../locales/", $lang, "/modelos.json"))),
            ("editor", include_str!(concat!("../locales/", $lang, "/editor.json"))),
            ("gravacoes", include_str!(concat!("../locales/", $lang, "/gravacoes.json"))),
            ("agenda", include_str!(concat!("../locales/", $lang, "/agenda.json"))),
            ("ajustes", include_str!(concat!("../locales/", $lang, "/ajustes.json"))),
            ("agente", include_str!(concat!("../locales/", $lang, "/agente.json"))),
            ("estados", include_str!(concat!("../locales/", $lang, "/estados.json"))),
            ("isolar", include_str!(concat!("../locales/", $lang, "/isolar.json"))),
        ]
    };
}

/// The KalangoOS languages (the same as the system's `kalango-i18n`): one catalog per language, except Portuguese
/// and Chinese, whose written forms differ by country.
const CATALOGS: &[(&str, &[(&str, &str)])] = &[
    ("pt_BR", screens!("pt_BR")),
    ("es", screens!("es")),
    ("fr", screens!("fr")),
    ("it", screens!("it")),
    ("de", screens!("de")),
    ("ja", screens!("ja")),
    ("ru", screens!("ru")),
    ("zh_CN", screens!("zh_CN")),
];

/// The pt_BR files (the reference every other language follows, in the tests).
#[cfg(test)]
const PT_BR: &[(&str, &str)] = CATALOGS[0].1;

static ACTIVE: OnceLock<HashMap<String, String>> = OnceLock::new();

/// Normalizes a POSIX locale (`pt_BR.UTF-8@euro`) to a catalog name: `pt` alone maps to `pt_BR`, `es_MX` to `es`,
/// `zh_CN`/`zh_SG`/`zh_Hans` to `zh_CN`; Traditional Chinese (`zh_TW`, `zh_HK`, `zh_MO`, `zh_Hant`) has no catalog yet
/// and stays in English rather than in simplified characters.
pub fn normalize(locale: &str) -> Option<&'static str> {
    let base = locale.split(['.', '@']).next().unwrap_or("").replace('-', "_");
    if base.is_empty() || base == "C" || base == "POSIX" {
        return None;
    }
    let mut parts = base.split('_');
    let lang = parts.next().unwrap_or("");
    if lang == "zh" {
        let traditional = parts.any(|p| matches!(p, "TW" | "HK" | "MO" | "Hant"));
        return (!traditional).then_some("zh_CN");
    }
    let names = || CATALOGS.iter().map(|(n, _)| *n);
    names().find(|n| *n == base).or_else(|| names().find(|n| n.split('_').next() == Some(lang)))
}

/// The catalog chosen by the system environment (or None for English).
pub fn system_locale() -> Option<&'static str> {
    if let Ok(list) = std::env::var("LANGUAGE") {
        if let Some(l) = list.split(':').find_map(normalize) {
            return Some(l);
        }
    }
    for var in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(v) = std::env::var(var) {
            if !v.is_empty() {
                return normalize(&v);
            }
        }
    }
    None
}

pub fn catalog(locale: Option<&str>) -> HashMap<String, String> {
    let mut out = HashMap::new();
    if let Some((_, files)) = CATALOGS.iter().find(|(n, _)| Some(*n) == locale) {
        for (_, json) in *files {
            if let Ok(m) = serde_json::from_str::<HashMap<String, String>>(json) {
                out.extend(m);
            }
        }
    }
    out
}

fn active() -> &'static HashMap<String, String> {
    // unit tests read the English source strings, whatever the machine's language
    ACTIVE.get_or_init(|| if cfg!(test) { HashMap::new() } else { catalog(system_locale()) })
}

/// Marks a string for translation without translating it yet (for `const` tables); translate with [`tr`] when shown.
pub const fn n_(msgid: &'static str) -> &'static str {
    msgid
}

/// Translates a UI string (English in the source).
pub fn tr(msgid: &str) -> &str {
    match active().get(msgid) {
        Some(t) => t.as_str(),
        None => msgid,
    }
}

/// Translates and fills named placeholders (`{name}`).
pub fn trf(msgid: &str, args: &[(&str, String)]) -> String {
    let mut s = tr(msgid).to_string();
    for (k, v) in args {
        s = s.replace(&format!("{{{k}}}"), v);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::Path;

    #[test]
    fn locale_detection() {
        assert_eq!(normalize("pt_BR.UTF-8"), Some("pt_BR"));
        assert_eq!(normalize("pt_PT.UTF-8"), Some("pt_BR"), "same language falls to the closest catalog");
        assert_eq!(normalize("en_US.UTF-8"), None);
        assert_eq!(normalize("C.UTF-8"), None);
        for (l, c) in [("es_MX.UTF-8", "es"), ("fr_FR.UTF-8", "fr"), ("it_IT", "it"), ("de_AT.UTF-8", "de"), ("ja_JP.UTF-8", "ja"),
            ("ru_RU.UTF-8", "ru"), ("zh_CN.UTF-8", "zh_CN"), ("zh_SG", "zh_CN"), ("zh_Hans", "zh_CN")] {
            assert_eq!(normalize(l), Some(c), "{l}");
        }
        for l in ["zh_TW.UTF-8", "zh_HK", "zh_Hant"] {
            assert_eq!(normalize(l), None, "{l}");
        }
    }

    /// The placeholders, line breaks and outer spaces of a text, which a translation keeps.
    fn shape(s: &str) -> (Vec<String>, usize, (String, String)) {
        let mut ph: Vec<String> = s.split('{').skip(1).filter_map(|p| p.split_once('}').map(|(x, _)| x.to_string())).collect();
        ph.sort();
        (ph, s.matches('\n').count(), (s[..s.len() - s.trim_start().len()].to_string(), s[s.trim_end().len()..].to_string()))
    }

    /// The keys of a catalog file in their order (the files keep pt_BR's order and layout).
    fn keys_in_order(json: &str) -> Vec<String> {
        // (serde_json's map is sorted: the order comes from the text, where each key starts a line after one space)
        let mut keys = Vec::new();
        let mut rest = json;
        while let Some(i) = rest.find("\n \"") {
            rest = &rest[i + 2..];
            let s: String = serde_json::Deserializer::from_str(rest).into_iter::<String>().next().unwrap().unwrap();
            keys.push(s);
        }
        keys
    }

    /// Every language has exactly pt_BR's keys, file by file, in pt_BR's order; each text keeps its placeholders,
    /// line breaks and outer spaces, and none is empty.
    #[test]
    fn every_language_has_every_string() {
        let mut problems = Vec::new();
        for (lang, files) in &CATALOGS[1..] {
            assert_eq!(files.len(), PT_BR.len());
            for ((file, json), (_, pt_json)) in files.iter().zip(PT_BR) {
                let pt: BTreeMap<String, String> = serde_json::from_str(pt_json).unwrap();
                let c: BTreeMap<String, String> = serde_json::from_str(json).unwrap_or_else(|e| panic!("locales/{lang}/{file}.json: {e}"));
                for k in pt.keys().filter(|k| !c.contains_key(*k)) {
                    problems.push(format!("locales/{lang}/{file}.json: missing '{k}'"));
                }
                for k in c.keys().filter(|k| !pt.contains_key(*k)) {
                    problems.push(format!("locales/{lang}/{file}.json: '{k}' is not in pt_BR/{file}.json"));
                }
                for (k, v) in &c {
                    if v.trim().is_empty() || shape(k) != shape(v) {
                        problems.push(format!("locales/{lang}/{file}.json: '{k}' → '{v}' is empty or changes placeholders, line breaks or outer spaces"));
                    }
                }
                if keys_in_order(json) != keys_in_order(pt_json) {
                    problems.push(format!("locales/{lang}/{file}.json: not in pt_BR/{file}.json's order"));
                }
            }
        }
        assert!(problems.is_empty(), "{}", problems.join("\n"));
        for (lang, _) in CATALOGS {
            let c = catalog(Some(lang));
            assert!(c.iter().filter(|(k, v)| k.len() > 12 && k != v).count() > c.len() / 2, "{lang} looks untranslated");
        }
    }

    /// The msgids marked in the sources (tr, trf, n_) and, for data files, the strings listed in them.
    fn used_ids() -> BTreeSet<String> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut used = BTreeSet::new();
        let mut dirs = vec![root];
        while let Some(d) = dirs.pop() {
            for e in std::fs::read_dir(d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    dirs.push(p);
                    continue;
                }
                if p.extension().is_some_and(|x| x == "rs") && p.file_name().is_some_and(|n| n != "i18n.rs") {
                    let text = std::fs::read_to_string(&p).unwrap();
                    for marker in ["tr(", "trf(", "n_("] {
                        let mut rest = text.as_str();
                        while let Some(i) = rest.find(marker) {
                            // (only a call: `str(` or `attr(` are not markers)
                            let before = &rest[..i];
                            rest = &rest[i + marker.len()..];
                            if before.chars().last().is_some_and(|c| c.is_alphanumeric() || c == '_') {
                                continue;
                            }
                            // the string may start on the next line
                            let after = rest.trim_start();
                            let Some(body) = after.strip_prefix('"') else { continue };
                            let bytes = body.as_bytes();
                            let mut end = 0;
                            while end < bytes.len() && !(bytes[end] == b'"' && (end == 0 || bytes[end - 1] != b'\\')) {
                                end += 1;
                            }
                            used.insert(body[..end].replace("\\\"", "\"").replace("\\n", "\n"));
                        }
                    }
                }
            }
        }
        // the templates' names, subtitles, descriptions and categories are shown translated
        let tpl: serde_json::Value = serde_json::from_str(include_str!("templates.json")).unwrap();
        for t in tpl.as_array().unwrap() {
            for k in ["name", "subtitle", "desc", "category"] {
                if let Some(s) = t[k].as_str().filter(|s| !s.is_empty()) {
                    used.insert(s.to_string());
                }
            }
        }
        used
    }

    /// Every string marked for translation has a pt_BR entry, no entry is left unused, no key is in two files, and
    /// placeholders survive translation.
    #[test]
    fn catalog_is_complete_and_has_no_leftovers() {
        let used = used_ids();
        let mut seen: BTreeMap<String, &str> = BTreeMap::new();
        for (file, json) in PT_BR {
            let m: HashMap<String, String> = serde_json::from_str(json).unwrap_or_else(|e| panic!("locales/pt_BR/{file}.json: {e}"));
            for k in m.keys() {
                if let Some(other) = seen.insert(k.clone(), file) {
                    panic!("'{k}' está em {other}.json e em {file}.json");
                }
            }
        }
        let pt = catalog(Some("pt_BR"));
        let missing: Vec<_> = used.iter().filter(|m| !pt.contains_key(*m)).collect();
        let unused: Vec<_> = pt.keys().filter(|k| !used.contains(*k)).collect();
        assert!(missing.is_empty(), "sem tradução pt_BR: {missing:#?}");
        assert!(unused.is_empty(), "traduções sem uso: {unused:#?}");
        for (k, v) in &pt {
            for part in k.split('{').skip(1) {
                let name = part.split('}').next().unwrap();
                assert!(v.contains(&format!("{{{name}}}")), "'{k}' → '{v}' perdeu {{{name}}}");
            }
        }
    }
}
