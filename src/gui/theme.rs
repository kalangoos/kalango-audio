//! The KalangoOS look: the Design System's colour tokens (dark and light), our style sheet, and following the
//! system's theme. KalangoOS keeps the person's choice in ~/.config/kalango/tema.json ("escuro", "claro" or
//! "automatico": light from 7h to 19h) and the accent colour there too; elsewhere we follow GTK's dark preference.
//! The theme is applied live: the sheet is reloaded and every glyph and drawing is redone.

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

const DARK: &str = include_str!("../../assets/css/tokens-dark.css");
const LIGHT: &str = include_str!("../../assets/css/tokens-light.css");
/// Our sheet, split per screen like the catalogs (each written only with @k_* tokens).
const SHEETS: &[&str] = &[
    include_str!("../../assets/css/base.css"),
    include_str!("../../assets/css/inicio.css"),
    include_str!("../../assets/css/modelos.css"),
    include_str!("../../assets/css/editor.css"),
    include_str!("../../assets/css/gravacoes.css"),
    include_str!("../../assets/css/agenda.css"),
    include_str!("../../assets/css/ajustes.css"),
    include_str!("../../assets/css/agente.css"),
    include_str!("../../assets/css/estados.css"),
];

/// The accent colours KalangoOS offers (its "destaque"), by id: the token that becomes the accent.
const ACCENTS: &[(&str, &str)] =
    &[("accent", "k_accent"), ("teal", "k_teal"), ("blue", "k_blue"), ("coral", "k_coral"), ("gold", "k_gold"), ("purple", "k_purple"), ("cream", "k_cream")];

thread_local! {
    static PROVIDER: RefCell<Option<gtk::CssProvider>> = const { RefCell::new(None) };
    static LIGHT_NOW: Cell<bool> = const { Cell::new(false) };
    static ACCENT: RefCell<String> = RefCell::new("k_accent".into());
    static TOKENS: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
    static LISTENERS: RefCell<Vec<Box<dyn Fn()>>> = const { RefCell::new(Vec::new()) };
    static MONITOR: RefCell<Option<gio::FileMonitor>> = const { RefCell::new(None) };
}

fn parse_tokens(css: &str) -> HashMap<String, String> {
    css.lines()
        .filter_map(|l| {
            let l = l.trim().strip_prefix("@define-color ")?;
            let (k, v) = l.split_once(' ')?;
            Some((k.to_string(), v.trim().trim_end_matches(';').to_string()))
        })
        .collect()
}

/// What the system asks for: (light?, accent token).
fn wanted() -> (bool, String) {
    if let Ok(v) = std::env::var("KALANGO_AUDIO_TEMA") {
        return (v == "claro" || v == "light", "k_accent".into());
    }
    let path = glib::user_config_dir().join("kalango/tema.json");
    if let Some(v) = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok()) {
        let estilo = v["estilo"].as_str().unwrap_or("escuro");
        let light = match estilo {
            "claro" => true,
            "automatico" => {
                let h = glib::DateTime::now_local().map(|d| d.hour()).unwrap_or(12);
                (7..19).contains(&h)
            }
            _ => false,
        };
        let accent = v["destaque"].as_str().and_then(|d| ACCENTS.iter().find(|(id, _)| *id == d)).map(|(_, t)| t.to_string()).unwrap_or("k_accent".into());
        return (light, accent);
    }
    let dark = gtk::Settings::default().map(|s| s.is_gtk_application_prefer_dark_theme()).unwrap_or(true);
    (!dark, "k_accent".into())
}

fn css(light: bool, accent: &str) -> String {
    let mut s = String::from(if light { LIGHT } else { DARK });
    if accent != "k_accent" {
        s.push_str(&format!("\n@define-color k_accent @{accent};\n@define-color k_accent_text @{accent};\n"));
    }
    for sheet in SHEETS {
        s.push('\n');
        s.push_str(sheet);
    }
    s
}

fn apply(light: bool, accent: String) {
    let changed = LIGHT_NOW.with(|l| l.get()) != light || ACCENT.with(|a| *a.borrow() != accent);
    let mut tokens = parse_tokens(if light { LIGHT } else { DARK });
    if accent != "k_accent" {
        if let Some(c) = tokens.get(&accent).cloned() {
            tokens.insert("k_accent".into(), c.clone());
            tokens.insert("k_accent_text".into(), c);
        }
    }
    TOKENS.with(|t| *t.borrow_mut() = tokens);
    LIGHT_NOW.with(|l| l.set(light));
    ACCENT.with(|a| *a.borrow_mut() = accent.clone());
    if let Some(p) = PROVIDER.with(|p| p.borrow().clone()) {
        p.load_from_string(&css(light, &accent));
    }
    if let Some(s) = gtk::Settings::default() {
        s.set_gtk_application_prefer_dark_theme(!light);
    }
    if changed {
        let l = LISTENERS.with(|l| l.take());
        for f in &l {
            f();
        }
        LISTENERS.with(|x| {
            let mut x = x.borrow_mut();
            let new = std::mem::take(&mut *x);
            *x = l;
            x.extend(new);
        });
    }
}

/// Loads the sheet and starts following the system's theme.
pub fn start(display: &gdk::Display) {
    let p = gtk::CssProvider::new();
    // above the person's own ~/.config/gtk-4.0/gtk.css (USER priority), as the KalangoOS apps do
    gtk::style_context_add_provider_for_display(display, &p, gtk::STYLE_PROVIDER_PRIORITY_USER + 1);
    PROVIDER.with(|x| *x.borrow_mut() = Some(p));
    LIGHT_NOW.with(|l| l.set(!wanted().0)); // forces the first apply to count as a change
    let (l, a) = wanted();
    apply(l, a);
    // the person changes the theme in Configurações › Aparência: tema.json is rewritten
    let file = gio::File::for_path(glib::user_config_dir().join("kalango/tema.json"));
    if let Ok(m) = file.monitor_file(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE) {
        m.connect_changed(|_, _, _, _| {
            let (l, a) = wanted();
            apply(l, a);
        });
        MONITOR.with(|x| *x.borrow_mut() = Some(m));
    }
    if let Some(s) = gtk::Settings::default() {
        s.connect_gtk_application_prefer_dark_theme_notify(|_| {
            let (l, a) = wanted();
            apply(l, a);
        });
    }
    // "automático" changes at 7h and 19h
    glib::timeout_add_seconds_local(60, || {
        let (l, a) = wanted();
        apply(l, a);
        glib::ControlFlow::Continue
    });
}

/// Switches theme now (tests, captures).
pub fn set_light(light: bool) {
    let a = ACCENT.with(|a| a.borrow().clone());
    apply(light, a);
}

pub fn is_light() -> bool {
    LIGHT_NOW.with(|l| l.get())
}

/// Called after every theme change (drawings and glyphs redo their colours).
pub fn on_change(f: impl Fn() + 'static) {
    LISTENERS.with(|l| l.borrow_mut().push(Box::new(f)));
}

/// A token's colour as "#RRGGBB" ("k_accent", "accent" or "cor/accent" all work).
pub fn hex(token: &str) -> String {
    let key = token.trim_start_matches("cor/").trim_start_matches('@');
    let key = if key.starts_with("k_") { key.to_string() } else { format!("k_{key}") };
    TOKENS.with(|t| {
        let t = t.borrow();
        let mut v = t.get(&key).cloned().unwrap_or_else(|| "#FF00FF".into());
        // tokens may point at another token
        for _ in 0..3 {
            match v.strip_prefix('@') {
                Some(k) => v = t.get(k).cloned().unwrap_or_else(|| "#FF00FF".into()),
                None => break,
            }
        }
        v
    })
}

/// A token's colour for cairo, with an alpha.
pub fn rgba(token: &str, alpha: f32) -> gdk::RGBA {
    let c = gdk::RGBA::parse(hex(token)).unwrap_or(gdk::RGBA::new(1.0, 0.0, 1.0, 1.0));
    gdk::RGBA::new(c.red(), c.green(), c.blue(), alpha)
}

/// Sets a cairo source to a token's colour.
pub fn source(cr: &gtk::cairo::Context, token: &str, alpha: f64) {
    let c = rgba(token, 1.0);
    cr.set_source_rgba(c.red() as f64, c.green() as f64, c.blue() as f64, alpha);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_token_files_define_the_same_tokens() {
        let (d, l) = (parse_tokens(DARK), parse_tokens(LIGHT));
        assert!(d.len() > 30);
        let mut dk: Vec<_> = d.keys().collect();
        let mut lk: Vec<_> = l.keys().collect();
        dk.sort();
        lk.sort();
        assert_eq!(dk, lk);
        assert_eq!(d.get("k_bg").map(String::as_str), Some("#16171B"));
        assert_eq!(l.get("k_bg").map(String::as_str), Some("#F0F0F5"));
    }

    #[test]
    fn our_sheets_only_use_tokens() {
        // no literal colours: everything follows the theme
        for sheet in SHEETS {
            for (i, line) in sheet.lines().enumerate() {
                let code = line.split("/*").next().unwrap_or("");
                assert!(!code.contains('#') || code.trim_start().starts_with('#'), "cor literal na linha {}: {line}", i + 1);
                assert!(!code.contains("rgb("), "cor literal na linha {}: {line}", i + 1);
                assert!(!code.contains("box-shadow:") || code.contains("box-shadow: none"), "sombra na linha {}: {line}", i + 1);
            }
        }
    }
}
