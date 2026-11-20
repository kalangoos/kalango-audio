//! The block library of the editor: a search over the names and descriptions, then the kinds of block in collapsible
//! groups (sources, effects, outputs, meters, advanced). Clicking an item (or Enter on it) adds it after the selected
//! block; dragging it puts it on the canvas or in the middle of a wire.

use super::widgets::*;
use super::{icons, Change, Ui};
use crate::i18n::{n_, tr};
use crate::model::{Cat, KINDS};
use gtk::prelude::*;
use gtk::{gdk, glib, Align};
use std::cell::RefCell;
use std::rc::Rc;

/// The short line under each kind in the library (the long description is in the inspector).
pub fn short_desc(kind: &str) -> &'static str {
    match kind {
        "app_source" => n_("Sound of an open program"),
        "device_input" => n_("Microphone or interface"),
        "system_sound" => n_("Everything that is playing"),
        "test_tone" => n_("Steady tone for testing"),
        "device_output" => n_("Speakers or headphones"),
        "recorder" => n_("Saves to a file"),
        "rtmp_stream" => n_("Live video platforms"),
        "radio_stream" => n_("Icecast or Shoutcast"),
        "graphic_eq" => n_("10 bands, 31 Hz–16 kHz"),
        "balance" => n_("Left and right sides"),
        "tone_control" => n_("Two simple controls"),
        "channels" => n_("Swap, join or mute"),
        "low_pass" => n_("Cuts the highs"),
        "high_pass" => n_("Cuts the lows"),
        "band_pass" => n_("Only one range"),
        "high_shelf" => n_("Raise or cut the highs"),
        "low_shelf" => n_("Raise or cut the lows"),
        "loudness" => n_("Gentle, fuller sound"),
        "stereo_widen" => n_("Mono across both sides"),
        "pan" => n_("Between left and right"),
        "compressor" => n_("Holds the peaks"),
        "volume" => n_("Louder or quieter"),
        "level_meter" => n_("Peak and average bars"),
        "needle_meter" => n_("Classic VU needle"),
        "click_removal" => n_("Takes out clicks"),
        "hum_removal" => n_("Mains hum, 50/60 Hz"),
        "source_switch" => n_("One of two sources"),
        "transcribe" => n_("Speech becomes text"),
        "ladspa_plugin" => n_("System effects"),
        "voice_denoise" => n_("Takes noise out of speech"),
        "convolution" => n_("Impulse response"),
        "noise_reduction" => n_("Learns and lowers noise"),
        "ducking" => n_("Ducking"),
        "mixer" => n_("Up to 5 sources"),
        "delay" => n_("Sync in milliseconds"),
        "time_shift" => n_("Pause the live sound"),
        "parametric_eq" => n_("Up to 16 bands"),
        _ => "",
    }
}

/// The groups, in the order of the design.
pub const GROUPS: [(Cat, &str); 5] = [
    (Cat::Source, n_("Sources")),
    (Cat::Effect, n_("Effects")),
    (Cat::Output, n_("Outputs")),
    (Cat::Meter, n_("Meters")),
    (Cat::Advanced, n_("Advanced")),
];

/// Does a kind match the search (its translated name, short line or description, ignoring case and accents)?
pub fn matches(kind: &str, query: &str) -> bool {
    let q = fold(query.trim());
    if q.is_empty() {
        return true;
    }
    let Some(k) = crate::model::kind_info(kind) else { return false };
    [tr(k.title), tr(k.desc), tr(short_desc(kind)), k.title].iter().any(|t| fold(t).contains(&q))
}

/// Lower case without accents ("Rádio" → "radio").
pub fn fold(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| match c {
            'á' | 'à' | 'â' | 'ã' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            c => c,
        })
        .collect()
}

struct Group {
    head: gtk::Box,
    rev: gtk::Revealer,
    items: Vec<(String, gtk::Box)>,
}

pub struct Library {
    pub root: gtk::Box,
    groups: Vec<Group>,
    picked: RefCell<Vec<Box<dyn Fn()>>>,
}

impl Library {
    pub fn new(ui: &Rc<Ui>) -> Rc<Library> {
        let root = vbox(0, "ka-biblioteca");
        root.set_vexpand(true);
        let search = entry(tr("Search blocks"), Some("busca"));
        search.set_margin_start(12);
        search.set_margin_end(12);
        search.set_margin_top(12);
        search.set_margin_bottom(4);
        root.append(&search);
        let list = vbox(0, "ka-biblioteca-lista");
        let sc = scroller(&list);
        root.append(&sc);

        let lib = Rc::new_cyclic(|weak: &std::rc::Weak<Library>| {
            let mut groups = vec![];
            for (cat, name) in GROUPS {
                let head = hbox(0, "ka-biblioteca-grupo");
                let toggle = gtk::Button::new();
                toggle.add_css_class("ka-grupo-botao");
                let row = hbox(0, "");
                row.append(&caps(tr(name)));
                row.append(&spacer());
                let chevron = icons::icon("chevron-baixo", "muted", 14);
                row.append(&chevron);
                toggle.set_child(Some(&row));
                toggle.set_hexpand(true);
                head.append(&toggle);
                list.append(&head);
                let rev = gtk::Revealer::new();
                rev.set_transition_type(gtk::RevealerTransitionType::SlideDown);
                rev.set_transition_duration(180);
                rev.set_reveal_child(true);
                let body = vbox(0, "");
                rev.set_child(Some(&body));
                list.append(&rev);
                {
                    let (r, ch) = (rev.clone(), chevron.clone());
                    toggle.connect_clicked(move |_| {
                        let open = !r.reveals_child();
                        r.set_reveal_child(open);
                        icons::set(&ch, if open { "chevron-baixo" } else { "chevron" }, "muted", 14);
                    });
                }
                let mut items = vec![];
                for k in KINDS.iter().filter(|k| k.cat == cat) {
                    let it = item(ui, k.id, weak.clone());
                    body.append(&it);
                    items.push((k.id.to_string(), it));
                }
                groups.push(Group { head, rev, items });
            }
            Library { root, groups, picked: RefCell::new(vec![]) }
        });
        {
            let l = lib.clone();
            search.connect_changed(move |e| l.filter(&e.text()));
        }
        lib
    }

    /// Called after a block was added from the library (the narrow window closes its popover).
    pub fn on_pick(&self, f: impl Fn() + 'static) {
        self.picked.borrow_mut().push(Box::new(f));
    }

    fn picked(&self) {
        for f in self.picked.borrow().iter() {
            f();
        }
    }

    fn filter(&self, q: &str) {
        for g in &self.groups {
            let mut any = false;
            for (k, w) in &g.items {
                let m = matches(k, q);
                w.set_visible(m);
                any |= m;
            }
            g.head.set_visible(any);
            g.rev.set_visible(any);
            if !q.trim().is_empty() && any {
                g.rev.set_reveal_child(true);
            }
        }
    }
}

/// Adds a block after the selected one (automatic wiring) or in the first free place.
fn add(ui: &Rc<Ui>, kind: &str) {
    let id = ui.core.borrow_mut().add_from_library(kind, None);
    if id.is_some() {
        ui.changed(Change::Sessions);
        ui.changed(Change::Selection);
    }
}

fn item(ui: &Rc<Ui>, kind: &'static str, lib: std::rc::Weak<Library>) -> gtk::Box {
    let row = hbox(10, "ka-biblioteca-item");
    row.set_focusable(true);
    row.append(&ktile(kind, 24));
    let texts = vbox(0, "");
    texts.set_hexpand(true);
    texts.set_valign(Align::Center);
    let info = crate::model::kind_info(kind);
    texts.append(&line(info.map(|k| tr(k.title)).unwrap_or(kind), "ka-biblioteca-nome"));
    texts.append(&line(tr(short_desc(kind)), "ka-biblioteca-desc"));
    row.append(&texts);
    row.set_tooltip_text(info.map(|k| tr(k.desc)));

    let click = gtk::GestureClick::new();
    {
        let (u, l) = (ui.clone(), lib.clone());
        click.connect_released(move |g, _, _, _| {
            g.set_state(gtk::EventSequenceState::Claimed);
            add(&u, kind);
            if let Some(l) = l.upgrade() {
                l.picked();
            }
        });
    }
    row.add_controller(click);
    let keys = gtk::EventControllerKey::new();
    {
        let (u, l) = (ui.clone(), lib.clone());
        keys.connect_key_pressed(move |_, k, _, _| {
            if matches!(k, gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::ISO_Enter) {
                add(&u, kind);
                if let Some(l) = l.upgrade() {
                    l.picked();
                }
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
    }
    row.add_controller(keys);

    // drag to the canvas: the ghost is a block card with a dashed border, tilted a little
    let src = gtk::DragSource::new();
    src.set_actions(gdk::DragAction::COPY);
    src.connect_prepare(move |_, _, _| Some(gdk::ContentProvider::for_value(&kind.to_value())));
    {
        let (r, l) = (row.clone(), lib.clone());
        src.connect_drag_begin(move |_, drag| {
            r.add_css_class("arrastando");
            if let Some(icon) = gtk::DragIcon::for_drag(drag).downcast_ref::<gtk::DragIcon>() {
                icon.set_child(Some(&ghost(kind)));
            }
            drag.set_hotspot(88, 50);
            if let Some(l) = l.upgrade() {
                l.picked();
            }
        });
        let r = row.clone();
        src.connect_drag_end(move |_, _, _| r.remove_css_class("arrastando"));
    }
    row.add_controller(src);
    row
}

/// What follows the pointer while a block is dragged: its card, dashed, at 85 %, tilted −2°.
fn ghost(kind: &str) -> gtk::Widget {
    let card = vbox(0, "ka-bloco ka-fantasma");
    card.set_size_request(176, 100);
    let head = hbox(8, "");
    head.append(&ktile(kind, 24));
    let t = vbox(1, "");
    t.set_hexpand(true);
    t.append(&line(crate::model::kind_info(kind).map(|k| tr(k.title)).unwrap_or(kind), "ka-bloco-titulo"));
    let mut b = crate::model::Session::new("");
    let id = b.add_block(kind, 0.0, 0.0);
    t.append(&line(&super::editor::summary(b.block(id).unwrap(), &[]), "ka-bloco-resumo"));
    head.append(&t);
    head.append(&icons::icon("desligar", "dim", 14));
    card.append(&head);
    let f = gtk::Fixed::new();
    f.put(&card, 0.0, 0.0);
    f.set_child_transform(
        &card,
        Some(&gtk::gsk::Transform::new().translate(&gtk::graphene::Point::new(4.0, 6.0)).rotate(-2.0)),
    );
    f.set_size_request(186, 112);
    f.set_opacity(0.85);
    f.upcast()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_is_in_a_group_with_a_short_line() {
        for k in KINDS {
            assert!(GROUPS.iter().any(|g| g.0 == k.cat), "{}", k.id);
            assert!(!short_desc(k.id).is_empty(), "{} has no short line", k.id);
        }
    }

    #[test]
    fn search_ignores_case_and_accents() {
        assert_eq!(fold("Rádio Ação"), "radio acao");
        assert!(matches("recorder", "RECORD"));
        assert!(matches("compressor", "peaks"));
        assert!(!matches("recorder", "equalizer"));
        assert!(matches("graphic_eq", ""));
    }
}

/// A block kind's tile that keeps its size (the glyph inside asks to expand).
fn ktile(kind: &str, size: i32) -> gtk::Box {
    let t = kind_tile(kind, size);
    t.set_hexpand(false);
    t
}
