//! The inspector of the selected block (right panel, or the bottom sheet in smaller windows): its name and
//! description, on/off, presets, every setting of its kind, a live transfer curve and gain reduction for the
//! compressors, input and output meters, and Duplicate / Remove. With nothing selected it lists the shortcuts.
//! Selecting another block swaps the content with a 120 ms crossfade and an 8 px slide (rápida spring).

use super::editor::{self, comp_settings, device_name, input_disp, level_db, upstream, CHANNEL_MODES};
use super::editor_canvas::draw_reduction;
use super::widgets::*;
use super::{theme, Change, Ui};
use crate::core::{fmt_size, fmt_time, state_label, Core};
use crate::i18n::{n_, tr, trf};
use crate::model::{kind_info, Block};
use crate::presets::{apply_preset, Preset, COMPRESSOR_MODES};
use gtk::prelude::*;
use gtk::{gio, glib, Align};
use kalango_movimento as mov;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

type Live = Box<dyn Fn(&Core, u32)>;
/// A button's action in a row of buttons.
type Act = Box<dyn Fn(&Cx)>;

pub struct Inspector {
    ui: Rc<Ui>,
    pub root: gtk::Box,
    stack: gtk::Stack,
    /// what the content was built from: (session uid, block)
    built: RefCell<Option<(u64, Block)>>,
    built_none: Cell<bool>,
    live: RefCell<Vec<Live>>,
    n: Cell<u32>,
    frame: Cell<u32>,
    /// "Save as…" is open
    saving: Cell<bool>,
    /// a rebuild waits for an open drop-down list to close
    waiting: Cell<bool>,
}

impl Inspector {
    pub fn new(ui: &Rc<Ui>) -> Rc<Inspector> {
        let root = vbox(0, "ka-inspetor");
        root.set_vexpand(true);
        let stack = gtk::Stack::new();
        stack.set_transition_type(gtk::StackTransitionType::Crossfade);
        stack.set_transition_duration(120);
        stack.set_vexpand(true);
        stack.set_hhomogeneous(false);
        root.append(&stack);
        Rc::new(Inspector {
            ui: ui.clone(),
            root,
            stack,
            built: RefCell::new(None),
            built_none: Cell::new(false),
            live: RefCell::new(vec![]),
            n: Cell::new(0),
            frame: Cell::new(0),
            saving: Cell::new(false),
            waiting: Cell::new(false),
        })
    }

    fn current(&self) -> Option<(u64, Block)> {
        let c = self.ui.core.borrow();
        let s = c.session()?;
        let id = c.sel?;
        Some((s.uid, s.block(id)?.clone()))
    }

    /// The session changed: rebuilt only when the selected block changed from elsewhere (undo, the canvas…).
    pub fn on_sessions(self: &Rc<Self>) {
        let cur = self.current();
        let same = match (&cur, &*self.built.borrow()) {
            (Some(a), Some(b)) => a.0 == b.0 && a.1 == b.1,
            (None, None) => self.built_none.get(),
            _ => false,
        };
        if !same {
            let other_block = match (&cur, &*self.built.borrow()) {
                (Some(a), Some(b)) => a.0 != b.0 || a.1.id != b.1.id,
                _ => true,
            };
            self.rebuild(other_block);
        }
    }

    /// Whether a drop-down list (a popover) of the inspector is open now.
    pub fn list_open(&self) -> bool {
        fn walk(w: &gtk::Widget) -> bool {
            if w.downcast_ref::<gtk::Popover>().is_some_and(|p| p.is_mapped()) {
                return true;
            }
            let mut c = w.first_child();
            while let Some(x) = c {
                if walk(&x) {
                    return true;
                }
                c = x.next_sibling();
            }
            false
        }
        walk(self.stack.upcast_ref())
    }

    /// Every frame: the meters and the curve; texts four times a second.
    pub fn tick(&self) {
        let f = self.frame.get().wrapping_add(1);
        self.frame.set(f);
        let Ok(c) = self.ui.core.try_borrow() else { return };
        for l in self.live.borrow().iter() {
            l(&c, f);
        }
    }

    /// Writes into the selected block, keeping the "built from" copy in step (so the inspector is not rebuilt).
    fn edit(&self, id: u64, f: impl FnOnce(&mut Block)) {
        let mut copy = None;
        editor::with_block(&self.ui, id, |b| {
            f(b);
            copy = Some(b.clone());
        });
        if let (Some(b), Some(built)) = (copy, self.built.borrow_mut().as_mut()) {
            if built.1.id == b.id {
                built.1 = b;
            }
        }
    }

    /// Builds the content again; `animate` crossfades it in with the slide.
    pub fn rebuild(self: &Rc<Self>, animate: bool) {
        // never under an open drop-down list: the list would be destroyed while the person uses it; try again
        // once it closes
        if self.list_open() {
            if !self.waiting.replace(true) {
                let me = self.clone();
                glib::timeout_add_local(std::time::Duration::from_millis(250), move || {
                    if me.list_open() {
                        return glib::ControlFlow::Continue;
                    }
                    me.waiting.set(false);
                    me.rebuild(animate);
                    glib::ControlFlow::Break
                });
            }
            return;
        }
        let cur = self.current();
        self.live.borrow_mut().clear();
        let page = vbox(0, "ka-inspetor-pagina");
        match &cur {
            Some((_, b)) => self.block_page(&page, b),
            None => self.empty_page(&page),
        }
        let sc = scroller(&page);
        let anim = animated(&sc);
        let n = self.n.get() + 1;
        self.n.set(n);
        let name = format!("p{n}");
        self.stack.add_named(&anim, Some(&name));
        self.stack.set_transition_type(if animate { gtk::StackTransitionType::Crossfade } else { gtk::StackTransitionType::None });
        // keep the scroll position when the same block is rebuilt
        let keep = if !animate { self.stack.visible_child().and_then(|w| w.first_child()).and_downcast::<gtk::ScrolledWindow>().map(|s| s.vadjustment().value()) } else { None };
        self.stack.set_visible_child_name(&name);
        if let Some(v) = keep {
            let adj = sc.vadjustment();
            glib::idle_add_local_once(move || adj.set_value(v));
        }
        if animate {
            anim.definir(mov::Canal::X, 8.0);
            anim.ir(mov::Canal::X, 0.0, mov::RAPIDA);
        }
        // the old pages go once the crossfade is over
        let st = self.stack.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(if animate { 200 } else { 0 }), move || {
            let keep = st.visible_child_name();
            let mut w = st.first_child();
            while let Some(c) = w {
                w = c.next_sibling();
                if st.page(&c).name() != keep {
                    st.remove(&c);
                }
            }
        });
        self.built_none.set(cur.is_none());
        *self.built.borrow_mut() = cur;
    }

    fn empty_page(&self, page: &gtk::Box) {
        page.append(&label(tr("No block selected"), "ka-titulo2"));
        let p = para(tr("Click a block to see its settings. Drag a block from the library to the canvas, or onto a wire to put it in between."), "ka-texto2");
        p.set_margin_top(8);
        page.append(&p);
        let cap = caps(tr("Shortcuts"));
        cap.set_margin_top(24);
        cap.set_margin_bottom(12);
        page.append(&cap);
        // one keycap per row, 8 px apart (espaco/2); the keycap keeps its own height instead of filling the row
        let keys = vbox(8, "ka-atalhos");
        page.append(&keys);
        for (key, what) in [
            (tr("Space"), tr("Start or stop")),
            ("Ctrl R", tr("Record now")),
            ("Del", tr("Remove block")),
            ("Ctrl D", tr("Duplicate block")),
            ("Ctrl Z", tr("Undo")),
        ] {
            let row = hbox(12, "ka-atalho-linha");
            let k = label(key, "ka-atalho ka-tecla");
            k.set_xalign(0.5);
            k.set_valign(Align::Center);
            k.set_size_request(56, -1);
            row.append(&k);
            row.append(&label(what, "ka-texto2"));
            keys.append(&row);
        }
    }

    fn block_page(self: &Rc<Self>, page: &gtk::Box, b: &Block) {
        let id = b.id;
        let info = kind_info(&b.kind);
        // head: tile, name, description, on/off
        let head = hbox(12, "");
        let t = ktile(&b.kind, 32);
        t.set_valign(Align::Start);
        head.append(&t);
        let texts = vbox(2, "");
        texts.set_hexpand(true);
        texts.append(&line(info.map(|i| tr(i.title)).unwrap_or("?"), "ka-titulo2"));
        texts.append(&para(info.map(|i| tr(i.desc)).unwrap_or(""), "ka-fraco"));
        head.append(&texts);
        let sw = switch(b.enabled);
        sw.set_valign(Align::Start);
        sw.set_tooltip_text(Some(tr("Off, the block lets the sound through unchanged")));
        {
            let me = self.clone();
            sw.connect_active_notify(move |s| {
                let on = s.is_active();
                me.edit(id, |b| b.enabled = on);
                me.ui.changed(Change::Sessions);
            });
        }
        head.append(&sw);
        page.append(&head);

        // the error the block reports while running
        {
            let err = para("", "ka-texto2 cor-red");
            err.set_margin_top(12);
            err.set_visible(false);
            page.append(&err);
            let uid = self.built_uid();
            self.live.borrow_mut().push(Box::new(move |c, f| {
                if f % 8 != 0 {
                    return;
                }
                match c.block_error(uid, id) {
                    Some(e) => {
                        err.set_text(&e);
                        err.set_visible(true);
                    }
                    None => err.set_visible(false),
                }
            }));
        }

        self.presets(page, b);
        let body = vbox(16, "ka-ajustes");
        body.set_margin_top(16);
        page.append(&body);
        let cx = Cx { insp: self.clone(), id, body: body.clone(), b: b.clone() };
        settings(&cx);
        if matches!(b.kind.as_str(), "compressor" | "loudness") {
            self.curve(page, id);
        }
        self.meters(page, id);

        // duplicate, remove and the note
        let foot = hbox(8, "");
        foot.set_margin_top(24);
        let dup = button(tr("Duplicate"), "secundario", None);
        let rem = button(tr("Remove"), "fantasma", Some("lixeira"));
        {
            let u = self.ui.clone();
            dup.connect_clicked(move |_| editor::duplicate_block(&u, id));
            let u = self.ui.clone();
            rem.connect_clicked(move |_| editor::remove_block(&u, id));
        }
        foot.append(&dup);
        foot.append(&rem);
        page.append(&foot);
        let note = para(tr("Off, the block lets the sound through unchanged. Ctrl+Z undoes any change."), "ka-fraco2");
        note.set_margin_top(12);
        page.append(&note);
    }

    fn built_uid(&self) -> u64 {
        self.ui.core.borrow().session().map(|s| s.uid).unwrap_or(0)
    }

    fn presets(self: &Rc<Self>, page: &gtk::Box, b: &Block) {
        let list: Vec<(Preset, bool)> = self.ui.core.borrow().presets_for(&b.kind);
        let id = b.id;
        let cap = label(tr("Preset"), "ka-texto2");
        cap.set_margin_top(16);
        cap.set_margin_bottom(8);
        page.append(&cap);
        let row = hbox(8, "");
        let matching = list.iter().position(|(p, _)| p.p.n.iter().all(|(k, v)| (b.p.num(k, f64::NAN) - v).abs() < 0.005) && p.p.s.iter().all(|(k, v)| b.p.text(k, "") == *v));
        let mut names: Vec<String> = vec![if list.is_empty() { tr("None yet").to_string() } else { tr("Custom").to_string() }];
        names.extend(list.iter().map(|(p, builtin)| if *builtin { tr(&p.name).to_string() } else { format!("★ {}", p.name) }));
        let dd = dropdown(&names, matching.map(|i| i + 1).unwrap_or(0));
        fit_dropdown(&dd);
        dd.set_hexpand(true);
        {
            let me = self.clone();
            dd.connect_selected_notify(move |d| {
                let i = d.selected() as usize;
                if i == 0 {
                    return;
                }
                if let Some((p, _)) = list.get(i - 1) {
                    let p = p.clone();
                    me.edit(id, |b| apply_preset(b, &p));
                    let me = me.clone();
                    glib::idle_add_local_once(move || me.rebuild(false));
                }
            });
        }
        row.append(&dd);
        page.append(&row);
        let save = gtk::Button::with_label(tr("Save as…"));
        save.add_css_class("ka-link-botao");
        save.set_halign(Align::End);
        save.set_margin_top(8);
        page.append(&save);
        // the name of the new preset
        let ask = hbox(8, "");
        ask.set_margin_top(8);
        let name = entry(tr("Preset name"), None);
        name.set_hexpand(true);
        let title = kind_info(&b.kind).map(|k| tr(k.title)).unwrap_or("");
        name.set_text(&trf("My {block}", &[("block", title.to_lowercase())]));
        let ok = button(tr("Save"), "primario", None);
        let cancel = icon_button("fechar", "muted", 14, tr("Cancel"));
        ask.append(&name);
        ask.append(&ok);
        ask.append(&cancel);
        ask.set_visible(self.saving.get());
        page.append(&ask);
        {
            let (a, me, n) = (ask.clone(), self.clone(), name.clone());
            save.connect_clicked(move |_| {
                me.saving.set(true);
                a.set_visible(true);
                n.grab_focus();
            });
            let (a, me) = (ask.clone(), self.clone());
            cancel.connect_clicked(move |_| {
                me.saving.set(false);
                a.set_visible(false);
            });
            let me = self.clone();
            let kind = b.kind.clone();
            let do_save = Rc::new(move |n: &gtk::Entry| {
                let text = n.text().trim().to_string();
                if text.is_empty() {
                    return;
                }
                {
                    let mut c = me.ui.core.borrow_mut();
                    let Some(mut p) = c.session().and_then(|s| s.block(id)).map(|b| b.p.clone()) else { return };
                    // one-shot buttons are not settings
                    for k in ["pause", "split_req", "learn_seq", "jump_seq", "live_seq", "jump_s", "jump_live"] {
                        p.n.remove(k);
                    }
                    c.store.presets.retain(|q| !(q.kind == kind && q.name == text));
                    c.store.presets.push(Preset { kind: kind.clone(), name: text.clone(), p });
                }
                me.saving.set(false);
                me.ui.toast(&trf("Preset “{name}” saved", &[("name", text)]), None);
                me.ui.changed(Change::Store);
                let me = me.clone();
                glib::idle_add_local_once(move || me.rebuild(false));
            });
            let (d, n) = (do_save.clone(), name.clone());
            ok.connect_clicked(move |_| d(&n));
            name.connect_activate(move |n| do_save(n));
        }
    }

    /// Compressor and fuller sound: the transfer curve with the live point, and the gain reduction.
    fn curve(&self, page: &gtk::Box, id: u64) {
        let area = gtk::DrawingArea::new();
        area.set_content_height(150);
        area.set_hexpand(true);
        area.set_margin_top(16);
        area.add_css_class("ka-curva");
        {
            let u = self.ui.clone();
            area.set_draw_func(move |_, cr, w, h| {
                let Ok(c) = u.core.try_borrow() else { return };
                let Some(b) = c.session().and_then(|s| s.block(id)).cloned() else { return };
                let running = c.session().map(|s| c.is_running(s.uid)).unwrap_or(false);
                let input = if running { input_disp(&c, id).map(|d| level_db(d.rms_max())) } else { None };
                draw_curve(cr, w as f64, h as f64, &b, input);
            });
        }
        follow_theme(&area);
        page.append(&area);
        let row = hbox(0, "");
        row.set_margin_top(12);
        row.append(&line(tr("Gain reduction"), "ka-texto2"));
        let value = label("0 dB", "ka-valor");
        row.append(&value);
        page.append(&row);
        let bar = gtk::DrawingArea::new();
        bar.set_content_height(4);
        bar.set_hexpand(true);
        bar.set_margin_top(6);
        {
            let u = self.ui.clone();
            bar.set_draw_func(move |_, cr, w, h| {
                let r = u.core.try_borrow().ok().and_then(|c| editor::gain_reduction_of(&c, id)).unwrap_or(0.0);
                draw_reduction(cr, w as f64, h as f64, r);
            });
        }
        follow_theme(&bar);
        page.append(&bar);
        self.live.borrow_mut().push(Box::new(move |c, f| {
            let running = c.session().map(|s| c.is_running(s.uid)).unwrap_or(false);
            if running || f % 30 == 0 {
                area.queue_draw();
                bar.queue_draw();
            }
            if f % 8 == 0 {
                let r = editor::gain_reduction_of(c, id).unwrap_or(0.0);
                value.set_text(&format!("{:.1} dB", r).replace('.', ",").replace('-', "−"));
            }
        }));
    }

    /// Input and output meters with their level in dB.
    fn meters(&self, page: &gtk::Box, id: u64) {
        let uid = self.built_uid();
        let is_src = self.ui.core.borrow().session().and_then(|s| s.block(id)).map(|b| crate::model::is_source(&b.kind)).unwrap_or(false);
        let mut rows: Vec<(&str, bool)> = vec![];
        if !is_src {
            rows.push((tr("Input"), true));
        }
        rows.push((tr("Output"), false));
        for (i, (title, input)) in rows.into_iter().enumerate() {
            let head = hbox(0, "");
            head.set_margin_top(if i == 0 { 24 } else { 12 });
            head.append(&line(title, "ka-texto2"));
            let v = label("", "ka-fraco ka-tnum");
            head.append(&v);
            page.append(&head);
            let u = self.ui.clone();
            let read = move || {
                let c = u.core.try_borrow().ok()?;
                if input {
                    input_disp(&c, id)
                } else {
                    c.disp.get(&(uid, id)).copied()
                }
            };
            let m = meter(200, 12, read);
            m.set_hexpand(true);
            m.set_margin_top(6);
            page.append(&m);
            self.live.borrow_mut().push(Box::new(move |c, f| {
                let d = if input { input_disp(c, id) } else { c.disp.get(&(uid, id)).copied() };
                if c.is_running(uid) || f % 30 == 0 {
                    m.queue_draw();
                }
                if f % 8 == 0 {
                    v.set_text(&d.map(|d| format!("{:.0} dB", level_db(d.rms_max())).replace('-', "−")).unwrap_or_default());
                }
            }));
        }
    }
}

/// The transfer curve: input level (x, −60…0 dB) to output level (y), the dashed 1:1 line and the live point.
fn draw_curve(cr: &gtk::cairo::Context, w: f64, h: f64, b: &Block, input: Option<f32>) {
    theme::source(cr, "sunken", 1.0);
    rounded(cr, 0.0, 0.0, w, h, 6.0);
    let _ = cr.fill();
    let s = comp_settings(b);
    let to_x = |db: f64| (db + 60.0) / 60.0 * w;
    let to_y = |db: f64| h - ((db + 60.0) / 60.0 * h).clamp(-4.0, h + 4.0);
    theme::source(cr, "border", 1.0);
    cr.set_line_width(1.0);
    for k in 1..4 {
        let x = (w * k as f64 / 4.0).round() + 0.5;
        let y = (h * k as f64 / 4.0).round() + 0.5;
        cr.move_to(x, 0.0);
        cr.line_to(x, h);
        cr.move_to(0.0, y);
        cr.line_to(w, y);
    }
    let _ = cr.stroke();
    theme::source(cr, "dim", 1.0);
    cr.set_dash(&[3.0, 3.0], 0.0);
    cr.move_to(0.0, h);
    cr.line_to(w, 0.0);
    let _ = cr.stroke();
    cr.set_dash(&[], 0.0);
    let makeup = 20.0 * (s.makeup.max(1e-6) as f64).log10();
    let out = |i: f64| i + crate::dsp::Compressor::curve(i as f32, s.threshold_db, s.ratio) as f64 + makeup;
    theme::source(cr, "accent", 1.0);
    cr.set_line_width(2.0);
    for k in 0..=60 {
        let i = -60.0 + k as f64;
        let (x, y) = (to_x(i), to_y(out(i)));
        if k == 0 {
            cr.move_to(x, y);
        } else {
            cr.line_to(x, y);
        }
    }
    let _ = cr.stroke();
    if let Some(i) = input {
        let i = (i as f64).clamp(-60.0, 0.0);
        let (x, y) = (to_x(i), to_y(out(i)));
        cr.arc(x, y, 5.0, 0.0, std::f64::consts::TAU);
        theme::source(cr, "accent", 1.0);
        let _ = cr.fill_preserve();
        theme::source(cr, "knob", 1.0);
        cr.set_line_width(1.5);
        let _ = cr.stroke();
    }
    // axis names
    theme::source(cr, "dim", 1.0);
    cr.select_font_face("Inter", gtk::cairo::FontSlant::Normal, gtk::cairo::FontWeight::Normal);
    cr.set_font_size(10.0);
    cr.move_to(8.0, 16.0);
    let _ = cr.show_text(tr("Output"));
    let ext = cr.text_extents(tr("Input")).map(|e| e.width()).unwrap_or(30.0);
    cr.move_to(w - ext - 8.0, h - 8.0);
    let _ = cr.show_text(tr("Input"));
}

// ---------------------------------------------------------------- settings of each kind

/// What the settings builders need: the inspector, the block (as it was when built) and where to put the rows.
struct Cx {
    insp: Rc<Inspector>,
    id: u64,
    body: gtk::Box,
    b: Block,
}

impl Cx {
    fn ui(&self) -> &Rc<Ui> {
        &self.insp.ui
    }

    fn num(&self, k: &str, d: f64) -> f64 {
        self.b.p.num(k, d)
    }

    fn text_of(&self, k: &str, d: &str) -> String {
        self.b.p.text(k, d)
    }

    fn later_rebuild(&self) {
        let i = self.insp.clone();
        glib::idle_add_local_once(move || i.rebuild(false));
    }

    fn add(&self, w: &impl IsA<gtk::Widget>) {
        self.body.append(w);
    }

    fn note(&self, text: &str) {
        self.add(&para(text, "ka-fraco2"));
    }

    fn section(&self, text: &str) {
        let c = caps(text);
        c.set_margin_top(6);
        self.add(&c);
    }

    /// A slider writing `key` (shown with `fmt`).
    #[allow(clippy::too_many_arguments)]
    fn slider(&self, title: &str, key: &str, min: f64, max: f64, step: f64, default: f64, fmt: impl Fn(f64) -> String + 'static) -> gtk::Scale {
        self.slider_in(&self.body, title, key, min, max, step, default, fmt)
    }

    #[allow(clippy::too_many_arguments)]
    fn slider_in(&self, into: &gtk::Box, title: &str, key: &str, min: f64, max: f64, step: f64, default: f64, fmt: impl Fn(f64) -> String + 'static) -> gtk::Scale {
        let (row, s) = slider(title, min, max, step, self.num(key, default).clamp(min, max), fmt);
        let (i, id, k) = (self.insp.clone(), self.id, key.to_string());
        s.connect_value_changed(move |s| {
            let v = s.value();
            let k = k.clone();
            i.edit(id, move |b| b.p.set(&k, v));
        });
        into.append(&row);
        s
    }

    /// A drop-down of (label, value) writing a number.
    fn choice(&self, title: &str, items: &[(String, f64)], key: &str, default: f64, rebuild: bool) -> gtk::DropDown {
        let cur = self.num(key, default);
        let names: Vec<String> = items.iter().map(|x| x.0.clone()).collect();
        let sel = items.iter().position(|x| (x.1 - cur).abs() < 1e-6).unwrap_or(0);
        let d = dropdown(&names, sel);
        fit_dropdown(&d);
        let values: Vec<f64> = items.iter().map(|x| x.1).collect();
        let (i, id, k) = (self.insp.clone(), self.id, key.to_string());
        d.connect_selected_notify(move |d| {
            let Some(v) = values.get(d.selected() as usize).copied() else { return };
            let k2 = k.clone();
            i.edit(id, move |b| b.p.set(&k2, v));
            if rebuild {
                let i = i.clone();
                glib::idle_add_local_once(move || i.rebuild(false));
            }
        });
        self.row(title, &d);
        d
    }

    /// A drop-down of (label, value) writing a text.
    fn choice_text(&self, title: &str, items: &[(String, String)], key: &str, default: &str, rebuild: bool) -> gtk::DropDown {
        let cur = self.text_of(key, default);
        let names: Vec<String> = items.iter().map(|x| x.0.clone()).collect();
        let sel = items.iter().position(|x| x.1 == cur).unwrap_or(0);
        let d = dropdown(&names, sel);
        fit_dropdown(&d);
        let values: Vec<String> = items.iter().map(|x| x.1.clone()).collect();
        let (i, id, k) = (self.insp.clone(), self.id, key.to_string());
        d.connect_selected_notify(move |d| {
            let Some(v) = values.get(d.selected() as usize).cloned() else { return };
            let k2 = k.clone();
            i.edit(id, move |b| b.p.set_text(&k2, &v));
            if rebuild {
                let i = i.clone();
                glib::idle_add_local_once(move || i.rebuild(false));
            }
        });
        self.row(title, &d);
        d
    }

    /// A title over a control (drop-downs, fields).
    fn row(&self, title: &str, w: &impl IsA<gtk::Widget>) {
        let b = vbox(6, "ka-controle");
        if !title.is_empty() {
            b.append(&line(title, "ka-texto2"));
        }
        w.set_hexpand(true);
        b.append(w);
        self.add(&b);
    }

    /// A switch writing 0/1.
    fn toggle(&self, title: &str, key: &str, rebuild: bool) -> gtk::Switch {
        self.toggle_in(&self.body, title, key, rebuild)
    }

    fn toggle_in(&self, into: &gtk::Box, title: &str, key: &str, rebuild: bool) -> gtk::Switch {
        let r = hbox(12, "ka-controle");
        r.append(&para(title, "ka-texto2"));
        let s = switch(self.num(key, 0.0) > 0.5);
        let (i, id, k) = (self.insp.clone(), self.id, key.to_string());
        s.connect_active_notify(move |s| {
            let v = if s.is_active() { 1.0 } else { 0.0 };
            let k2 = k.clone();
            i.edit(id, move |b| b.p.set(&k2, v));
            if rebuild {
                let i = i.clone();
                glib::idle_add_local_once(move || i.rebuild(false));
            }
        });
        r.append(&s);
        into.append(&r);
        s
    }

    /// A text field writing `key`.
    fn text(&self, title: &str, key: &str, hint: &str, password: bool) -> gtk::Entry {
        let e = entry(hint, None);
        e.set_text(&self.text_of(key, ""));
        if password {
            e.set_visibility(false);
            e.set_input_purpose(gtk::InputPurpose::Password);
        }
        let (i, id, k) = (self.insp.clone(), self.id, key.to_string());
        e.connect_changed(move |e| {
            let v = e.text().to_string();
            let k2 = k.clone();
            i.edit(id, move |b| b.p.set_text(&k2, &v));
        });
        self.row(title, &e);
        e
    }

    /// A path field with a button that opens the file (or folder) chooser.
    fn file(&self, title: &str, key: &str, hint: &str, folder: bool) -> gtk::Entry {
        let r = hbox(6, "");
        let e = entry(hint, None);
        e.set_hexpand(true);
        e.set_text(&self.text_of(key, ""));
        let (i, id, k) = (self.insp.clone(), self.id, key.to_string());
        e.connect_changed(move |e| {
            let v = e.text().to_string();
            let k2 = k.clone();
            i.edit(id, move |b| b.p.set_text(&k2, &v));
        });
        r.append(&e);
        let pick = icon_button("pasta", "text2", 16, if folder { tr("Choose a folder") } else { tr("Choose a file") });
        pick.add_css_class("ka-escolher");
        {
            let (u, e) = (self.ui().clone(), e.clone());
            let title = title.to_string();
            pick.connect_clicked(move |_| {
                let d = gtk::FileDialog::new();
                d.set_title(&title);
                let cur = crate::core::expand_home(&e.text());
                if folder && cur.is_dir() {
                    d.set_initial_folder(Some(&gio::File::for_path(&cur)));
                } else if let Some(p) = cur.parent().filter(|p| p.is_dir()) {
                    d.set_initial_folder(Some(&gio::File::for_path(p)));
                }
                let e = e.clone();
                let done = move |r: Result<gio::File, glib::Error>| {
                    if let Some(p) = r.ok().and_then(|f| f.path()) {
                        e.set_text(&p.to_string_lossy());
                    }
                };
                if folder {
                    d.select_folder(Some(&u.window), gio::Cancellable::NONE, done);
                } else {
                    d.open(Some(&u.window), gio::Cancellable::NONE, done);
                }
            });
        }
        r.append(&pick);
        self.row(title, &r);
        e
    }

    /// A number field writing `key`.
    #[allow(clippy::too_many_arguments)]
    fn spin(&self, into: &gtk::Box, key: &str, min: f64, max: f64, step: f64, default: f64, digits: u32) -> gtk::SpinButton {
        let s = gtk::SpinButton::with_range(min, max, step);
        s.add_css_class("ka-numero");
        s.set_digits(digits);
        s.set_value(self.num(key, default));
        let (i, id, k) = (self.insp.clone(), self.id, key.to_string());
        s.connect_value_changed(move |s| {
            let v = s.value();
            let k2 = k.clone();
            i.edit(id, move |b| b.p.set(&k2, v));
        });
        into.append(&s);
        s
    }

    /// A live text line (state of a stream, a recorder…) refreshed four times a second.
    fn live_label(&self, class: &str, f: impl Fn(&Core, &Block) -> Option<(String, bool)> + 'static) -> gtk::Label {
        let l = para("", class);
        l.set_visible(false);
        self.add(&l);
        let (id, l2) = (self.id, l.clone());
        self.insp.live.borrow_mut().push(Box::new(move |c, frame| {
            if frame % 8 != 0 {
                return;
            }
            let Some(b) = c.session().and_then(|s| s.block(id)) else { return };
            // (only what changed: a text set again resizes the panel, and an open list above it with it)
            match f(c, b) {
                Some((t, err)) => {
                    if l2.text() != t {
                        l2.set_text(&t);
                    }
                    if !l2.is_visible() {
                        l2.set_visible(true);
                    }
                    if err != l2.has_css_class("cor-red") {
                        if err {
                            l2.add_css_class("cor-red");
                        } else {
                            l2.remove_css_class("cor-red");
                        }
                    }
                }
                None => {
                    if l2.is_visible() {
                        l2.set_visible(false)
                    }
                }
            }
        }));
        l
    }

    /// The live state of a stream or transcription: "streaming · 00:12:03".
    fn stream_state(&self) {
        let uid = self.insp.built_uid();
        self.live_label("ka-texto2 cor-teal", move |c, b| {
            let r = c.rec_status(uid, b.id)?;
            if r.state.is_empty() {
                return None;
            }
            Some((format!("{} · {}", state_label(&r.state), fmt_time(r.seconds)), r.state.starts_with("error")))
        });
    }

    fn button_row(&self, items: Vec<(String, Act)>) -> gtk::Box {
        let r = gtk::FlowBox::new();
        r.set_selection_mode(gtk::SelectionMode::None);
        r.set_column_spacing(6);
        r.set_row_spacing(6);
        r.set_max_children_per_line(6);
        let out = hbox(0, "");
        let me = Rc::new(Cx { insp: self.insp.clone(), id: self.id, body: self.body.clone(), b: self.b.clone() });
        for (t, f) in items {
            let b = button(&t, "secundario", None);
            let me = me.clone();
            b.connect_clicked(move |_| f(&me));
            r.insert(&b, -1);
        }
        out.append(&r);
        self.add(&out);
        out
    }
}

/// A drop-down whose chosen text is cut with "…" so long names (devices, programs) never widen the panel; the list
/// that opens shows them whole.
fn fit_dropdown(d: &gtk::DropDown) {
    let make = |cut: bool| {
        let f = gtk::SignalListItemFactory::new();
        f.connect_setup(move |_, item| {
            let l = gtk::Label::new(None);
            l.set_xalign(0.0);
            l.set_ellipsize(gtk::pango::EllipsizeMode::End);
            if cut {
                l.set_max_width_chars(1);
                l.set_hexpand(true);
            } else {
                // every row asks for the same width: the list (and its popover) never changes size while it
                // scrolls through names of different lengths
                l.set_width_chars(34);
                l.set_max_width_chars(34);
            }
            if let Some(i) = item.downcast_ref::<gtk::ListItem>() {
                i.set_child(Some(&l));
            }
        });
        f.connect_bind(|_, item| {
            let Some(i) = item.downcast_ref::<gtk::ListItem>() else { return };
            let text = i.item().and_downcast::<gtk::StringObject>().map(|s| s.string().to_string()).unwrap_or_default();
            if let Some(l) = i.child().and_downcast::<gtk::Label>() {
                l.set_text(&text);
            }
        });
        f
    };
    d.set_factory(Some(&make(true)));
    d.set_list_factory(Some(&make(false)));
}

fn pct(v: f64) -> String {
    format!("{} %", (v * 100.0).round() as i64)
}

fn hz(v: f64) -> String {
    if v >= 1000.0 {
        format!("{} kHz", format!("{:.2}", v / 1000.0).trim_end_matches('0').trim_end_matches('.').replace('.', ","))
    } else {
        format!("{} Hz", v.round() as i64)
    }
}

fn dbs(v: f64) -> String {
    format!("{:+.1} dB", v).replace('.', ",").replace('-', "−")
}

fn sided(v: f64) -> String {
    if v.abs() < 0.01 {
        tr("Centre").to_string()
    } else if v < 0.0 {
        trf("Left {n}", &[("n", ((-v * 100.0).round() as i64).to_string())])
    } else {
        trf("Right {n}", &[("n", ((v * 100.0).round() as i64).to_string())])
    }
}

fn s(t: &str) -> String {
    tr(t).to_string()
}

/// Every kind's settings (the same ones the app always had).
fn settings(cx: &Cx) {
    let kind = cx.b.kind.clone();
    match kind.as_str() {
        "volume" => {
            cx.slider(tr("Volume"), "gain", 0.0, 1.0, 0.01, 1.0, pct);
            cx.choice(tr("Boost"), &[(tr("None").to_string(), 1.0), ("2×".into(), 2.0), ("3×".into(), 3.0), ("4×".into(), 4.0)], "boost", 1.0, true);
            cx.toggle(tr("Mute"), "muted", false);
            let boost = cx.num("boost", 1.0);
            cx.note(&trf("The total goes from 0 % to {max} %.", &[("max", ((100.0 * boost) as i64).to_string())]));
        }
        "device_input" | "system_sound" => {
            device_choice(cx);
            if kind == "system_sound" {
                cx.note(tr("To hear the system, choose a “monitor” source."));
            }
        }
        "device_output" => {
            device_choice(cx);
            cx.slider(tr("Volume"), "gain", 0.0, 1.0, 0.01, 1.0, pct);
            cx.toggle(tr("No sound"), "muted", false);
        }
        "app_source" => app_source(cx),
        "test_tone" => {
            cx.slider(tr("Frequency"), "freq", 50.0, 4000.0, 1.0, 440.0, hz);
            cx.slider(tr("Level"), "level", 0.0, 1.0, 0.01, 0.25, pct);
        }
        "graphic_eq" => graphic_eq(cx),
        "balance" => {
            cx.slider(tr("Balance"), "balance", -1.0, 1.0, 0.01, 0.0, sided);
        }
        "pan" => {
            cx.slider(tr("Position"), "pan", -1.0, 1.0, 0.01, 0.0, sided);
        }
        "tone_control" => {
            cx.slider(tr("Bass"), "bass", 0.0, 1.0, 0.01, 0.5, |v| dbs((v - 0.5) * 24.0));
            cx.slider(tr("Treble"), "treble", 0.0, 1.0, 0.01, 0.5, |v| dbs((v - 0.5) * 24.0));
            cx.note(tr("In the middle nothing changes; at the ends, ±12 dB."));
        }
        "low_pass" => {
            cx.slider(tr("Cut-off"), "freq", 100.0, 20000.0, 10.0, 10000.0, hz);
        }
        "high_pass" => {
            cx.slider(tr("Cut-off"), "freq", 20.0, 2000.0, 1.0, 80.0, hz);
        }
        "band_pass" => {
            cx.slider(tr("Centre"), "freq", 50.0, 15000.0, 5.0, 1000.0, hz);
            cx.slider("Q", "q", 0.1, 10.0, 0.1, 1.0, |v| format!("{v:.1}").replace('.', ","));
        }
        "high_shelf" | "low_shelf" => {
            let d = if kind == "high_shelf" { 6000.0 } else { 150.0 };
            cx.slider(tr("Frequency"), "freq", 50.0, 18000.0, 5.0, d, hz);
            cx.slider(tr("Gain"), "gain", -24.0, 24.0, 0.5, 0.0, dbs);
        }
        "compressor" | "loudness" => {
            if kind == "compressor" {
                let cur = crate::presets::compressor_mode_of(&cx.b.p);
                let mut names: Vec<String> = COMPRESSOR_MODES.iter().map(|m| s(m.0)).collect();
                names.push(tr("Custom").to_string());
                let sel = cur.and_then(|c| COMPRESSOR_MODES.iter().position(|m| m.0 == c)).unwrap_or(COMPRESSOR_MODES.len());
                let d = dropdown(&names, sel);
                fit_dropdown(&d);
                let (i, id) = (cx.insp.clone(), cx.id);
                d.connect_selected_notify(move |d| {
                    if let Some(m) = COMPRESSOR_MODES.get(d.selected() as usize) {
                        let m = *m;
                        i.edit(id, move |b| {
                            b.p.set("amount", m.1);
                            b.p.set("speed", m.2);
                            b.p.set("makeup", m.3);
                        });
                        let i = i.clone();
                        glib::idle_add_local_once(move || i.rebuild(false));
                    }
                });
                cx.row(tr("Mode"), &d);
            }
            let def = if kind == "loudness" { 0.0 } else { 1.0 };
            cx.slider(tr("Amount"), "amount", 0.0, 1.0, 0.01, def, pct);
            cx.slider(tr("Speed"), "speed", 0.0, 1.0, 0.01, 0.0, pct);
            cx.slider(tr("Make-up"), "makeup", 1.0, 4.0, 0.01, 1.0, |v| dbs(20.0 * v.max(1e-6).log10()));
        }
        "channels" => {
            let items: Vec<(String, f64)> = CHANNEL_MODES.iter().enumerate().map(|(i, m)| (s(m), i as f64)).collect();
            cx.choice(tr("Channels"), &items, "mode", 0.0, false);
        }
        "hum_removal" => {
            cx.slider(tr("Mains frequency"), "base", 40.0, 70.0, 1.0, 50.0, hz);
            cx.slider(tr("Harmonics"), "amount", 1.0, 8.0, 1.0, 3.0, |v| format!("{}", v.round() as i64));
        }
        "click_removal" => {
            cx.slider(tr("Sensitivity"), "sens", 1.0, 6.0, 0.1, 3.0, |v| format!("{v:.1}").replace('.', ","));
        }
        "mixer" => mixer(cx),
        "ducking" => {
            let ups = upstream(&cx.ui().core.borrow(), cx.id);
            if ups.is_empty() {
                cx.note(tr("Connect two sources to this block."));
            } else {
                let items: Vec<(String, f64)> = ups.iter().map(|u| (u.1.clone(), u.0 as f64)).collect();
                cx.choice(tr("Main source (goes in front when it is loud enough)"), &items, "overlay", ups[0].0 as f64, false);
            }
            cx.slider(tr("Threshold"), "threshold", -60.0, 0.0, 1.0, -30.0, |v| format!("{} dB", v.round() as i64).replace('-', "−"));
            cx.slider(tr("The others drop to"), "ducked", 0.0, 1.0, 0.01, 0.4, pct);
            cx.note(tr("When the main source goes over the threshold the others go down; when it is quiet they come back to full volume."));
        }
        "noise_reduction" => {
            cx.slider(tr("Reduction"), "reduction", 0.0, 40.0, 0.5, 6.0, |v| format!("{:.1} dB", v).replace('.', ","));
            let uid = cx.insp.built_uid();
            let learn = button(tr("Learn the noise"), "secundario", Some("escudo"));
            learn.set_halign(Align::Start);
            {
                let (i, id) = (cx.insp.clone(), cx.id);
                learn.connect_clicked(move |_| i.edit(id, |b| {
                    let n = b.p.num("learn_seq", 0.0);
                    b.p.set("learn_seq", n + 1.0);
                }));
            }
            cx.add(&learn);
            cx.live_label("ka-texto2", move |c, b| {
                let learned = c.rec_status(uid, b.id).map(|r| r.seconds).unwrap_or(0.0);
                Some((if learned > 0.0 { trf("{s} s of noise learned", &[("s", format!("{learned:.1}").replace('.', ","))]) } else { tr("No noise learned yet").to_string() }, false))
            });
            cx.note(tr("Start the session and click “Learn the noise” while only the noise is playing. Three seconds of sound are used."));
        }
        "ladspa_plugin" => ladspa(cx),
        "stereo_widen" => {
            cx.slider(tr("Crossover frequency"), "crossover", 20.0, 2000.0, 5.0, 500.0, hz);
            cx.note(tr("Gives a mono sound a feeling of stereo. A sound that is already stereo passes unchanged."));
        }
        "voice_denoise" => {
            cx.toggle(tr("Process in mono"), "mono", false);
            cx.note(tr("In mono the stereo input is joined into one channel, which saves processor."));
            let uid = cx.insp.built_uid();
            cx.live_label("ka-texto2", move |c, b| {
                let vad = c.rec_status(uid, b.id).map(|r| r.seconds).unwrap_or(0.0);
                c.session().filter(|s| c.is_running(s.uid))?;
                Some((if vad > 0.5 { trf("Voice present ({n})", &[("n", pct(vad))]) } else { tr("No voice").to_string() }, false))
            });
        }
        "convolution" => {
            cx.slider(tr("Input gain"), "pregain", -24.0, 12.0, 0.5, 0.0, dbs);
            cx.file(tr("Impulse response (an audio file)"), "ir", "~/impulses/room.wav", false);
            let uid = cx.insp.built_uid();
            cx.live_label("ka-texto2", move |c, b| {
                let r = c.rec_status(uid, b.id)?;
                if r.state.is_empty() {
                    return None;
                }
                Some((state_label(&r.state), r.state.starts_with("error")))
            });
            cx.note(tr("It is loaded when the session runs, and converted to the session’s rate when that is different."));
        }
        "delay" => {
            cx.slider(tr("Delay"), "delay", 0.0, 1000.0, 1.0, 0.0, |v| format!("{} ms", v.floor() as i64));
            cx.note(tr("In whole milliseconds, up to 1000 ms: enough to line the sound up with the video of a call or a stream."));
        }
        "time_shift" => time_shift(cx),
        "source_switch" => {
            let ups = upstream(&cx.ui().core.borrow(), cx.id);
            let items: Vec<(String, f64)> = (0..2)
                .map(|i| {
                    let name = ups.get(i).map(|u| u.1.clone()).unwrap_or_else(|| trf("Input {n}", &[("n", (i + 1).to_string())]));
                    (trf("Input {n}: {name}", &[("n", (i + 1).to_string()), ("name", name)]), i as f64)
                })
                .collect();
            cx.choice(tr("Goes on"), &items, "switch", 0.0, false);
        }
        "parametric_eq" => parametric(cx),
        "radio_stream" => radio(cx),
        "rtmp_stream" => {
            cx.text(tr("Server address"), "url", "rtmp://live.example.com/app", false);
            cx.text(tr("Stream key"), "key", "", true);
            let items: Vec<(String, f64)> = [(n_("Light"), 0), (n_("Standard"), 1), (n_("High"), 2)]
                .iter()
                .map(|(t, v)| {
                    let (vb, _) = crate::engine::rtmp_quality(*v);
                    (format!("{} · {} Mbps", tr(t), vb / 1000), *v as f64)
                })
                .collect();
            cx.choice(tr("Quality"), &items, "quality", 1.0, false);
            cx.note(tr("Sends your sound with a still black picture (through ffmpeg)."));
            cx.stream_state();
        }
        "transcribe" => {
            cx.file(tr("Model file (whisper.cpp, .bin)"), "model", "~/models/ggml-base.bin", false);
            cx.text(tr("Language (auto, pt, en…)"), "language", "auto", false);
            cx.text(tr("Program"), "command", "whisper-cli", false);
            cx.file(tr("Folder"), "folder", &crate::engine::default_transcripts_dir(), true);
            cx.text(tr("File name"), "file_name", crate::engine::DEFAULT_TRANSCRIPT_NAME, false);
            cx.note(tr("Every 10 seconds the sound becomes text and is added to a .txt file. Needs whisper.cpp (whisper-cli) installed."));
            cx.stream_state();
        }
        "recorder" => recorder(cx),
        "level_meter" | "needle_meter" => {
            cx.note(tr("This block shows the level of the sound that passes through it. It has no settings."));
        }
        _ => {}
    }
}

fn device_choice(cx: &Cx) {
    let devices = cx.ui().core.borrow().devices.clone();
    let cur = cx.text_of("device", "default");
    let mut items: Vec<(String, String)> = vec![(device_name("default", &devices), "default".into())];
    items.extend(devices.iter().filter(|d| d.0 != "default").map(|d| (d.1.clone(), d.0.clone())));
    if !items.iter().any(|i| i.1 == cur) {
        items.push((trf("{device} (not connected)", &[("device", cur.clone())]), cur));
    }
    let d = cx.choice_text(tr("Device"), &items, "device", "default", false);
    d.set_enable_search(items.len() > 8);
    if items.len() > 8 {
        d.set_expression(Some(gtk::PropertyExpression::new(gtk::StringObject::static_type(), None::<gtk::Expression>, "string")));
    }
}

fn app_source(cx: &Cx) {
    let now = cx.ui().core.borrow().apps.lock().map(|a| a.clone()).unwrap_or_default();
    let cur = cx.text_of("app", "Web Browser");
    let mut items: Vec<(String, String)> = ["Web Browser", "Call App", "Any Application"].iter().map(|k| (crate::audio::app_label(k), k.to_string())).collect();
    for a in &now {
        let key = format!("name:{}", a.name.to_lowercase());
        if !items.iter().any(|i| i.1 == key) {
            items.push((trf("{app} (playing now)", &[("app", a.name.clone())]), key));
        }
    }
    for d in crate::audio::installed_apps().iter().take(400) {
        let key = format!("desktop:{}", d.id);
        if !items.iter().any(|i| i.1 == key) {
            items.push((d.name.clone(), key));
        }
    }
    if !items.iter().any(|i| i.1 == cur) {
        items.push((crate::audio::app_label(&cur), cur.clone()));
    }
    let d = cx.choice_text(tr("Program"), &items, "app", "Web Browser", false);
    d.set_enable_search(true);
    d.set_expression(Some(gtk::PropertyExpression::new(gtk::StringObject::static_type(), None::<gtk::Expression>, "string")));
    let apps = cx.ui().core.borrow().apps.clone();
    cx.live_label("ka-texto2", move |_, b| {
        let want = b.p.text("app", "Web Browser");
        let now = apps.lock().map(|a| a.clone()).unwrap_or_default();
        let m: Vec<String> = now.iter().filter(|a| crate::audio::app_matches(&want, a)).map(|a| a.name.clone()).collect();
        Some(if m.is_empty() {
            (tr("This program is not playing anything yet. When it starts, its sound is picked up in a second or two.").to_string(), false)
        } else {
            (trf("Capturing: {apps}", &[("apps", m.join(", "))]), false)
        })
    });
    cx.toggle(tr("Open an address when the session starts"), "url_on", true);
    if cx.num("url_on", 0.0) > 0.5 {
        cx.text("", "url", "https://…", false);
    }
    cx.note(tr("Captures only this program’s sound (through PipeWire), on any output it uses, even with other programs playing."));
    let more = gtk::Expander::new(Some(tr("More options")));
    more.add_css_class("ka-expansor");
    let inner = vbox(16, "");
    inner.set_margin_top(12);
    cx.toggle_in(&inner, tr("Mix in the microphone"), "capture_inputs", true);
    let split = cx.toggle_in(&inner, tr("Split into the channels"), "split_channels", false);
    split.set_sensitive(cx.num("capture_inputs", 0.0) > 0.5);
    inner.append(&para(tr("With the channels split, the microphone is on the left and the program on the right."), "ka-fraco2"));
    inner.append(&para(tr("When the program stops playing, the block keeps sending silence so the recording has no gaps."), "ka-fraco2"));
    more.set_child(Some(&inner));
    cx.add(&more);
}

fn graphic_eq(cx: &Cx) {
    let row = hbox(2, "ka-eq");
    row.set_homogeneous(true);
    for i in 0..10 {
        let col = vbox(4, "");
        let key = format!("band{i}");
        let v = cx.num(&key, 0.5);
        let val = label(&format!("{:+.0}", (v - 0.5) * 24.0).replace('-', "−"), "ka-fraco2 ka-tnum");
        val.set_xalign(0.5);
        col.append(&val);
        let sc = gtk::Scale::with_range(gtk::Orientation::Vertical, 0.0, 1.0, 0.01);
        sc.set_inverted(true);
        sc.set_value(v);
        sc.set_draw_value(false);
        sc.set_size_request(-1, 120);
        sc.set_halign(Align::Center);
        sc.add_mark(0.5, gtk::PositionType::Left, None);
        let (ins, id) = (cx.insp.clone(), cx.id);
        sc.connect_value_changed(move |s| {
            let v = s.value();
            val.set_text(&format!("{:+.0}", (v - 0.5) * 24.0).replace('-', "−"));
            let k = key.clone();
            ins.edit(id, move |b| b.p.set(&k, v));
        });
        col.append(&sc);
        let f = crate::dsp::EQ_FREQS[i];
        let fl = label(&if f >= 1000.0 { format!("{}k", f / 1000.0) } else { format!("{}", f as i64) }, "ka-fraco2");
        fl.set_xalign(0.5);
        col.append(&fl);
        row.append(&col);
    }
    cx.add(&row);
    cx.button_row(vec![(
        tr("Flatten").to_string(),
        Box::new(|cx: &Cx| {
            cx.insp.edit(cx.id, |b| {
                for i in 0..10 {
                    b.p.set(&format!("band{i}"), 0.5);
                }
            });
            cx.later_rebuild();
        }),
    )]);
}

fn mixer(cx: &Cx) {
    let ups = upstream(&cx.ui().core.borrow(), cx.id);
    cx.note(tr("The tracks follow the order in which the blocks were connected (up to 5)."));
    for i in 0..5 {
        let card = vbox(12, "ka-cartao2 ka-faixa-mixer");
        let top = hbox(6, "");
        let name = ups.get(i).map(|u| u.1.clone()).unwrap_or_else(|| trf("Track {n}", &[("n", (i + 1).to_string())]));
        top.append(&line(&name, if ups.get(i).is_some() { "ka-titulo3" } else { "ka-fraco" }));
        for (key, text, tip) in [(format!("m{i}"), "M", tr("Mute this track")), (format!("s{i}"), "S", tr("Hear only this track"))] {
            let t = gtk::ToggleButton::with_label(text);
            t.add_css_class("ka-filtro");
            t.set_tooltip_text(Some(tip));
            t.set_active(cx.num(&key, 0.0) > 0.5);
            let (ins, id) = (cx.insp.clone(), cx.id);
            t.connect_toggled(move |t| {
                let v = if t.is_active() { 1.0 } else { 0.0 };
                let k = key.clone();
                ins.edit(id, move |b| b.p.set(&k, v));
            });
            top.append(&t);
        }
        card.append(&top);
        cx.slider_in(&card, tr("Volume"), &format!("g{i}"), 0.0, 1.0, 0.01, 1.0, pct);
        cx.slider_in(&card, tr("Balance"), &format!("b{i}"), -1.0, 1.0, 0.01, 0.0, sided);
        cx.slider_in(&card, tr("Position"), &format!("p{i}"), -1.0, 1.0, 0.01, 0.0, sided);
        let faded = cx.num(&format!("fo{i}"), 0.0) > 0.5;
        let fade = button(if faded { tr("Fade back in (100 %)") } else { tr("Fade out (0 %)") }, "secundario", None);
        fade.set_halign(Align::Start);
        {
            let (ins, id, k) = (cx.insp.clone(), cx.id, format!("fo{i}"));
            fade.connect_clicked(move |_| {
                let k2 = k.clone();
                ins.edit(id, move |b| {
                    let v = b.p.num(&k2, 0.0);
                    b.p.set(&k2, if v > 0.5 { 0.0 } else { 1.0 });
                });
                let ins = ins.clone();
                glib::idle_add_local_once(move || ins.rebuild(false));
            });
        }
        card.append(&fade);
        cx.add(&card);
    }
}

fn ladspa(cx: &Cx) {
    let plugins = crate::ladspa::cached();
    let cur = cx.text_of("plugin", "");
    let mut items: Vec<(String, String)> = vec![(tr("Choose a plug-in").to_string(), String::new())];
    items.extend(plugins.iter().map(|p| (format!("{} · {}", p.name, p.maker), p.key())));
    if !cur.is_empty() && !plugins.iter().any(|p| p.key() == cur) {
        items.push((tr("Plug-in not found").to_string(), cur.clone()));
    }
    let d = cx.choice_text(tr("Plug-in"), &items, "plugin", "", true);
    d.set_enable_search(true);
    d.set_expression(Some(gtk::PropertyExpression::new(gtk::StringObject::static_type(), None::<gtk::Expression>, "string")));
    if plugins.is_empty() {
        cx.note(tr("No LADSPA plug-in was found."));
    }
    cx.button_row(vec![(
        tr("Look again").to_string(),
        Box::new(|cx: &Cx| {
            crate::ladspa::rescan();
            cx.later_rebuild();
        }),
    )]);
    if let Some(info) = plugins.iter().find(|p| p.key() == cur) {
        for c in &info.controls {
            let key = format!("c{}", c.port);
            if c.toggle {
                cx.toggle(&c.name, &key, false);
            } else {
                let (min, max) = (c.min as f64, (c.max.max(c.min + 1e-6)) as f64);
                let step = if c.integer { 1.0 } else { ((max - min) / 200.0).max(1e-4) };
                let int = c.integer;
                cx.slider(&c.name, &key, min, max, step, c.default as f64, move |v| if int { format!("{}", v.round() as i64) } else { format!("{v:.2}").replace('.', ",") });
            }
        }
    }
    let uid = cx.insp.built_uid();
    cx.live_label("ka-texto2", move |c, b| {
        let r = c.rec_status(uid, b.id)?;
        r.state.starts_with("error").then(|| (state_label(&r.state), true))
    });
    let dirs = crate::ladspa::search_dirs().iter().map(|d| d.display().to_string()).collect::<Vec<_>>().join(", ");
    cx.note(&trf("Plug-ins are looked for in: {dirs}", &[("dirs", dirs)]));
}

fn time_shift(cx: &Cx) {
    let uid = cx.insp.built_uid();
    cx.live_label("ka-titulo3", move |c, b| {
        let lag = c.rec_status(uid, b.id).map(|r| r.seconds).unwrap_or(0.0);
        Some((if lag < 0.05 { tr("Live").to_string() } else { trf("{s} s behind the live sound", &[("s", format!("{lag:.1}").replace('.', ","))]) }, false))
    });
    let jump = |secs: f64| -> Box<dyn Fn(&Cx)> {
        Box::new(move |cx: &Cx| {
            cx.insp.edit(cx.id, |b| {
                let n = b.p.num("jump_seq", 0.0);
                b.p.set("jump_s", secs);
                b.p.set("jump_live", 0.0);
                b.p.set("jump_seq", n + 1.0);
            })
        })
    };
    let paused = cx.num("paused", 0.0) > 0.5;
    cx.button_row(vec![
        ("−30 s".to_string(), jump(30.0)),
        ("−10 s".to_string(), jump(10.0)),
        (
            if paused { tr("Play").to_string() } else { tr("Pause").to_string() },
            Box::new(|cx: &Cx| {
                cx.insp.edit(cx.id, |b| {
                    let p = b.p.num("paused", 0.0) > 0.5;
                    b.p.set("paused", if p { 0.0 } else { 1.0 });
                });
                cx.later_rebuild();
            }),
        ),
        ("+10 s".to_string(), jump(-10.0)),
        ("+30 s".to_string(), jump(-30.0)),
        (
            tr("Back to live").to_string(),
            Box::new(|cx: &Cx| {
                cx.insp.edit(cx.id, |b| {
                    let n = b.p.num("live_seq", 0.0);
                    b.p.set("jump_live", 1.0);
                    b.p.set("live_seq", n + 1.0);
                })
            }),
        ),
    ]);
    cx.note(tr("Keeps the last two minutes so you can pause and go back in the live sound."));
}

const PEQ_TYPES: [&str; 7] = [n_("Peak"), n_("Low shelf"), n_("High shelf"), n_("Low-pass"), n_("High-pass"), n_("Band-pass"), n_("Notch")];

fn parametric(cx: &Cx) {
    let n = (cx.num("bands", 3.0) as usize).min(16);
    for i in 0..n {
        let card = vbox(12, "ka-cartao2 ka-faixa-mixer");
        let top = hbox(8, "");
        top.append(&line(&trf("Band {n}", &[("n", (i + 1).to_string())]), "ka-titulo3"));
        let on = switch(cx.num(&format!("b{i}_on"), 1.0) > 0.5);
        {
            let (ins, id, k) = (cx.insp.clone(), cx.id, format!("b{i}_on"));
            on.connect_active_notify(move |s| {
                let v = if s.is_active() { 1.0 } else { 0.0 };
                let k2 = k.clone();
                ins.edit(id, move |b| b.p.set(&k2, v));
            });
        }
        top.append(&on);
        card.append(&top);
        let names: Vec<String> = PEQ_TYPES.iter().map(|t| s(t)).collect();
        let t = dropdown(&names, (cx.num(&format!("b{i}_type"), 0.0) as usize).min(PEQ_TYPES.len() - 1));
        fit_dropdown(&t);
        {
            let (ins, id, k) = (cx.insp.clone(), cx.id, format!("b{i}_type"));
            t.connect_selected_notify(move |d| {
                let v = d.selected() as f64;
                let k2 = k.clone();
                ins.edit(id, move |b| b.p.set(&k2, v));
            });
        }
        card.append(&t);
        // frequency on a logarithmic slider: the scale holds log10(Hz)
        {
            let key = format!("b{i}_freq");
            let f = cx.num(&key, 1000.0).clamp(20.0, 20000.0);
            let (row, sc) = slider(tr("Frequency"), 20f64.log10(), 20000f64.log10(), 0.005, f.log10(), |v| hz(10f64.powf(v)));
            let (ins, id) = (cx.insp.clone(), cx.id);
            sc.connect_value_changed(move |s| {
                let v = 10f64.powf(s.value()).round();
                let k = key.clone();
                ins.edit(id, move |b| b.p.set(&k, v));
            });
            card.append(&row);
        }
        cx.slider_in(&card, tr("Gain"), &format!("b{i}_gain"), -24.0, 24.0, 0.5, 0.0, dbs);
        cx.slider_in(&card, "Q", &format!("b{i}_q"), 0.1, 10.0, 0.1, 1.0, |v| format!("{v:.1}").replace('.', ","));
        cx.add(&card);
    }
    let mut acts: Vec<(String, Act)> = vec![];
    if n < 16 {
        acts.push((
            tr("Add a band").to_string(),
            Box::new(move |cx: &Cx| {
                cx.insp.edit(cx.id, move |b| {
                    b.p.set("bands", (n + 1) as f64);
                    b.p.set(&format!("b{n}_type"), 0.0);
                    b.p.set(&format!("b{n}_freq"), 1000.0);
                    b.p.set(&format!("b{n}_gain"), 0.0);
                    b.p.set(&format!("b{n}_q"), 1.0);
                    b.p.set(&format!("b{n}_on"), 1.0);
                });
                cx.later_rebuild();
            }),
        ));
    }
    if n > 0 {
        acts.push((
            tr("Remove the last band").to_string(),
            Box::new(move |cx: &Cx| {
                cx.insp.edit(cx.id, move |b| b.p.set("bands", (n - 1) as f64));
                cx.later_rebuild();
            }),
        ));
    }
    cx.button_row(acts);
}

fn radio(cx: &Cx) {
    let servers: Vec<(String, String)> = ["Icecast 2", "Shoutcast 1", "Shoutcast 2"].iter().map(|x| (x.to_string(), x.to_string())).collect();
    cx.choice_text(tr("Server type"), &servers, "server", "Icecast 2", true);
    let cur = cx.text_of("server", "Icecast 2");
    if cur != "Icecast 2" {
        cx.note(tr("Uses Shoutcast’s classic source connection, on the port + 1."));
    }
    cx.text(tr("Server address"), "host", "radio.example.com", false);
    let port = hbox(8, "");
    cx.spin(&port, "port", 1.0, 65535.0, 1.0, 8000.0, 0);
    cx.row(tr("Port"), &port);
    cx.text(tr("User"), "user", "source", false);
    cx.text(tr("Password"), "password", "", true);
    if cur == "Icecast 2" {
        cx.text(tr("Mount point"), "mount", "/stream", false);
    } else if cur == "Shoutcast 2" {
        cx.text(tr("Stream ID (optional)"), "stream_id", "", false);
    }
    cx.text(tr("Station name"), "name", "", false);
    cx.text(tr("Genre"), "genre", "", false);
    cx.text(tr("Website"), "url", "https://…", false);
    cx.toggle(tr("Show in the public directory"), "public", false);
    cx.choice_text(tr("Format"), &[("MP3".into(), "MP3".into()), ("AAC".into(), "AAC".into())], "format", "MP3", false);
    cx.choice(tr("Quality"), &bitrates(), "bitrate", 128.0, false);
    cx.stream_state();
}

fn bitrates() -> Vec<(String, f64)> {
    vec![(tr("Speech · 32 kbps").to_string(), 32.0), (tr("Medium · 128 kbps").to_string(), 128.0), (tr("High · 256 kbps").to_string(), 256.0)]
}

const TOKENS: [(&str, &str); 13] = [
    (n_("Session"), "%name"),
    (n_("Counter"), "%counter"),
    (n_("Date"), "%date"),
    (n_("Time"), "%time"),
    (n_("Year"), "%year"),
    (n_("Month"), "%month"),
    (n_("Day"), "%day"),
    (n_("Hours"), "%hour"),
    (n_("Minutes"), "%minute"),
    (n_("Seconds"), "%second"),
    (n_("Title"), "%tag_title"),
    (n_("Artist"), "%tag_artist"),
    (n_("Album"), "%tag_album"),
];

fn recorder(cx: &Cx) {
    let name = cx.text(tr("File name"), "file_name", crate::engine::DEFAULT_FILE_NAME, false);
    {
        let flow = gtk::FlowBox::new();
        flow.set_selection_mode(gtk::SelectionMode::None);
        flow.set_column_spacing(4);
        flow.set_row_spacing(4);
        flow.set_max_children_per_line(8);
        for (t, tok) in TOKENS {
            let b = gtk::Button::with_label(tr(t));
            b.add_css_class("ka-ficha");
            b.set_tooltip_text(Some(tok));
            let e = name.clone();
            b.connect_clicked(move |_| {
                let n = e.text().to_string();
                e.set_text(format!("{} {tok}", n.trim_end()).trim_start());
            });
            flow.insert(&b, -1);
        }
        cx.add(&flow);
    }
    cx.file(tr("Folder"), "folder", &crate::engine::default_recordings_dir(), true);
    let ff = crate::engine::ffmpeg_available();
    let formats: Vec<(String, String)> = ["WAV", "AIFF", "MP3", "AAC", "ALAC", "FLAC"]
        .iter()
        .map(|f| {
            let needs = !matches!(*f, "WAV" | "AIFF");
            (if needs && !ff { trf("{format} (needs ffmpeg)", &[("format", f.to_string())]) } else { f.to_string() }, f.to_string())
        })
        .collect();
    cx.choice_text(tr("Format"), &formats, "format", "WAV", true);
    let cur = cx.text_of("format", "WAV");
    match cur.as_str() {
        "WAV" => {
            cx.choice(tr("Resolution"), &[(tr("16 bits").to_string(), 16.0), (tr("24 bits").to_string(), 24.0), (tr("32 bits (floating point)").to_string(), 32.0)], "depth", 16.0, true);
            if cx.num("depth", 16.0) as i64 == 16 {
                cx.choice(tr("Dither"), &[(tr("None").to_string(), 0.0), (tr("Flat TPDF").to_string(), 1.0), (tr("With noise shaping").to_string(), 2.0)], "dither", 0.0, false);
            }
        }
        "MP3" | "AAC" => {
            cx.choice(tr("Quality"), &bitrates(), "bitrate", 256.0, false);
        }
        _ => {}
    }
    if matches!(cur.as_str(), "MP3" | "AAC" | "ALAC" | "FLAC") && !ff {
        let w = para(tr("ffmpeg is not installed, so the file will be saved as WAV."), "ka-fraco2 cor-gold");
        cx.add(&w);
    }

    // more quality options
    {
        let ex = gtk::Expander::new(Some(tr("More quality options")));
        ex.add_css_class("ka-expansor");
        let inner = vbox(16, "");
        inner.set_margin_top(12);
        let sub = Cx { insp: cx.insp.clone(), id: cx.id, body: inner.clone(), b: cx.b.clone() };
        let mut rates: Vec<(String, f64)> = vec![(tr("The session’s").to_string(), 0.0)];
        rates.extend([22050u32, 32000, 44100, 48000, 88200, 96000].iter().map(|r| (format!("{r} Hz"), *r as f64)));
        sub.choice(tr("Sample rate"), &rates, "q_rate", 0.0, false);
        sub.choice(tr("Channels"), &[(tr("Stereo").to_string(), 0.0), (tr("Mono").to_string(), 1.0)], "q_channels", 0.0, false);
        if matches!(cur.as_str(), "AAC" | "ALAC") {
            sub.choice(tr("Container"), &[(tr(".m4a (opens in more places)").to_string(), 0.0), (tr(".mp4 (survives interruptions)").to_string(), 1.0)], "q_mp4", 0.0, false);
        }
        if cur == "MP3" {
            sub.toggle(tr("Variable bit rate"), "q_vbr", false);
        }
        if matches!(cur.as_str(), "ALAC" | "FLAC" | "AIFF") {
            sub.choice(tr("Bits per sample"), &[(tr("16 bits").to_string(), 16.0), (tr("24 bits").to_string(), 24.0)], "depth", 16.0, false);
        }
        ex.set_child(Some(&inner));
        cx.add(&ex);
    }
    // tags
    {
        let ex = gtk::Expander::new(Some(tr("Tags")));
        ex.add_css_class("ka-expansor");
        let inner = vbox(16, "");
        inner.set_margin_top(12);
        let sub = Cx { insp: cx.insp.clone(), id: cx.id, body: inner.clone(), b: cx.b.clone() };
        for (t, k) in [
            (n_("Title"), "tag_title"),
            (n_("Artist"), "tag_artist"),
            (n_("Album"), "tag_album"),
            (n_("Genre"), "tag_genre"),
            (n_("Year"), "tag_year"),
            (n_("Track"), "tag_track"),
            (n_("Number of tracks"), "tag_trackcount"),
            (n_("Disc"), "tag_disc"),
            (n_("Number of discs"), "tag_disccount"),
            (n_("Comment"), "tag_comment"),
        ] {
            sub.text(tr(t), k, "", false);
        }
        sub.file(tr("Cover (picture)"), "tag_art", "", false);
        sub.toggle(tr("Compilation"), "tag_compilation", false);
        inner.append(&para(tr("Tags go into MP3, AAC, ALAC, FLAC, WAV and AIFF files. They can also go into the file name with the Title, Artist and Album buttons."), "ka-fraco2"));
        ex.set_child(Some(&inner));
        cx.add(&ex);
    }

    cx.section(tr("File limits"));
    limit(cx, "split", tr("Start another file every"), &[(n_("megabytes"), "space*1"), (n_("gigabytes"), "space*1024"), (n_("minutes"), "time*60"), (n_("hours"), "time*3600")]);
    limit(cx, "stop", tr("Stop recording after"), &[(n_("minutes"), "time*60"), (n_("hours"), "time*3600"), (n_("megabytes"), "space*1"), (n_("gigabytes"), "space*1024")]);
    cx.toggle(tr("Detect silence"), "silence_on", true);
    if cx.num("silence_on", 0.0) > 0.5 {
        cx.choice(tr("Then"), &[(tr("Skip the silence").to_string(), 1.0), (tr("Start another file").to_string(), 2.0), (tr("Stop recording").to_string(), 3.0)], "silence_action", 1.0, false);
        cx.slider(tr("After"), "silence_time", 0.5, 99.9, 0.5, crate::engine::SILENCE_SECONDS, |v| format!("{v:.1} s").replace('.', ","));
        cx.slider(tr("Below"), "silence_db", -90.0, -10.0, 1.0, crate::engine::SILENCE_DB, |v| format!("{} dB", v.round() as i64).replace('-', "−"));
    }

    let paused = cx.num("pause", 0.0) > 0.5;
    cx.button_row(vec![
        (
            if paused { tr("Continue").to_string() } else { tr("Pause").to_string() },
            Box::new(|cx: &Cx| {
                cx.insp.edit(cx.id, |b| {
                    let p = b.p.num("pause", 0.0) > 0.5;
                    b.p.set("pause", if p { 0.0 } else { 1.0 });
                });
                cx.later_rebuild();
            }),
        ),
        (
            tr("Split the file").to_string(),
            Box::new(|cx: &Cx| {
                cx.insp.edit(cx.id, |b| {
                    let n = b.p.num("split_req", 0.0);
                    b.p.set("split_req", n + 1.0);
                })
            }),
        ),
    ]);
    let uid = cx.insp.built_uid();
    cx.live_label("ka-texto2", move |c, b| {
        let r = c.rec_status(uid, b.id)?;
        if r.state.is_empty() {
            return None;
        }
        let mut t = format!("{} · {} · {}", state_label(&r.state), fmt_time(r.seconds), fmt_size(r.bytes));
        if !r.file.is_empty() {
            t.push('\n');
            t.push_str(&r.file);
        }
        Some((t, r.state.starts_with("error")))
    });
}

fn limit(cx: &Cx, key: &str, title: &str, units: &[(&str, &str)]) {
    cx.toggle(title, &format!("{key}_on"), true);
    if cx.num(&format!("{key}_on"), 0.0) > 0.5 {
        let r = hbox(8, "");
        let sp = cx.spin(&r, &format!("{key}_size"), 0.1, 10000.0, 0.5, 1.0, 1);
        sp.set_hexpand(true);
        let items: Vec<(String, String)> = units.iter().map(|(t, k)| (s(t), k.to_string())).collect();
        let cur = cx.text_of(&format!("{key}_unit"), units[0].1);
        let names: Vec<String> = items.iter().map(|i| i.0.clone()).collect();
        let d = dropdown(&names, items.iter().position(|i| i.1 == cur).unwrap_or(0));
        fit_dropdown(&d);
        let (ins, id, k) = (cx.insp.clone(), cx.id, format!("{key}_unit"));
        d.connect_selected_notify(move |d| {
            if let Some(v) = items.get(d.selected() as usize).map(|i| i.1.clone()) {
                let k2 = k.clone();
                ins.edit(id, move |b| b.p.set_text(&k2, &v));
            }
        });
        r.append(&d);
        cx.add(&r);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn frequencies_read_well() {
        assert_eq!(super::hz(440.0), "440 Hz");
        assert_eq!(super::hz(1000.0), "1 kHz");
        assert_eq!(super::hz(12500.0), "12,5 kHz");
        assert_eq!(super::dbs(1.58), "+1,6 dB");
        assert_eq!(super::dbs(-4.2), "−4,2 dB");
    }
}

/// A block kind's tile that keeps its size (the glyph inside asks to expand).
fn ktile(kind: &str, size: i32) -> gtk::Box {
    let t = kind_tile(kind, size);
    t.set_hexpand(false);
    t
}
