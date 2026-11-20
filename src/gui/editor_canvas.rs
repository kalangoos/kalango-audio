//! The editor's canvas: a dot grid, the session's blocks (cards in a `gtk::Fixed`, placed with a transform so they pan
//! and zoom), the wires between them (cubic béziers drawn with cairo under the cards) and the ports (drawn over the
//! cards). Blocks move with the pointer and settle on the grid with the elástica spring; wires are made by dragging
//! from an output port to another block; blocks dragged from the library land on the grid or in the middle of a wire.
//! Positions are kept in the model in grid units of [`UNIT`] px.

use super::editor::{self, summary};
use super::widgets::*;
use super::{icons, theme, Change, Ui};
use crate::core::{fmt_size, fmt_time, state_label};
use crate::i18n::{tr, trf};
use crate::model::{is_source, kind_info, Block, Session};
use gtk::prelude::*;
use gtk::{gdk, glib, graphene, gsk, Align};
use kalango_movimento as mov;
use mov::Mola;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// One grid unit of the model (`Block.x/y`), in px at 100 %.
pub const UNIT: f64 = 34.0;
/// A block's card.
pub const BW: f64 = 176.0;
pub const BH: f64 = 100.0;
/// The dots of the grid, every 24 px at 100 %.
const DOTS: f64 = 24.0;
const PORT_R: f64 = 4.5;
/// The zoom steps of the − / + buttons.
const ZOOMS: [f64; 8] = [0.5, 0.625, 0.75, 0.875, 1.0, 1.25, 1.5, 2.0];
/// Under this the canvas stops shrinking to fit and scrolls instead.
const MIN_FIT: f64 = 0.75;

// ---------------------------------------------------------------- geometry (pure)

/// A position in px snapped to the nearest grid unit (returned in units).
pub fn snap_units(px: f64) -> f32 {
    (px / UNIT).round() as f32
}

/// The output port of a block at (x, y) px, and the input port.
pub fn out_port(x: f64, y: f64) -> (f64, f64) {
    (x + BW, y + BH / 2.0)
}

pub fn in_port(x: f64, y: f64) -> (f64, f64) {
    (x, y + BH / 2.0)
}

/// The control points of a wire from `a` (an output port) to `b` (an input port): it leaves to the right and enters
/// from the left, even when the input is behind the output.
pub fn wire_curve(a: (f64, f64), b: (f64, f64)) -> [(f64, f64); 4] {
    let dx = ((b.0 - a.0).abs() * 0.5).max(40.0);
    [a, (a.0 + dx, a.1), (b.0 - dx, b.1), b]
}

pub fn bezier_at(c: &[(f64, f64); 4], t: f64) -> (f64, f64) {
    let u = 1.0 - t;
    let (a, b, cc, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
    (a * c[0].0 + b * c[1].0 + cc * c[2].0 + d * c[3].0, a * c[0].1 + b * c[1].1 + cc * c[2].1 + d * c[3].1)
}

fn seg_dist(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let (vx, vy) = (b.0 - a.0, b.1 - a.1);
    let l2 = vx * vx + vy * vy;
    let t = if l2 > 0.0 { (((p.0 - a.0) * vx + (p.1 - a.1) * vy) / l2).clamp(0.0, 1.0) } else { 0.0 };
    let (qx, qy) = (a.0 + t * vx, a.1 + t * vy);
    ((p.0 - qx).powi(2) + (p.1 - qy).powi(2)).sqrt()
}

/// How far a point is from a wire (sampled in 32 straight pieces).
pub fn wire_distance(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let c = wire_curve(a, b);
    let mut best = f64::MAX;
    let mut prev = c[0];
    for i in 1..=32 {
        let q = bezier_at(&c, i as f64 / 32.0);
        best = best.min(seg_dist(p, prev, q));
        prev = q;
    }
    best
}

/// The wire (from, to) closest to `p` within `reach` px, among the session's wires (positions in px).
pub fn wire_at(blocks: &[(u64, f64, f64, Vec<u64>)], p: (f64, f64), reach: f64) -> Option<(u64, u64)> {
    let pos = |id: u64| blocks.iter().find(|b| b.0 == id).map(|b| (b.1, b.2));
    let mut best: Option<((u64, u64), f64)> = None;
    for (id, x, y, outs) in blocks {
        for o in outs {
            let Some((ox, oy)) = pos(*o) else { continue };
            let d = wire_distance(p, out_port(*x, *y), in_port(ox, oy));
            if d <= reach && best.map(|b| d < b.1).unwrap_or(true) {
                best = Some(((*id, *o), d));
            }
        }
    }
    best.map(|b| b.0)
}

/// Blocks to push one column (6 units) to the right so a block put at `at` does not cover them: the ones that
/// overlap it and everything at or right of it on those rows.
pub fn make_room(blocks: &[(u64, f32, f32)], new: u64, at: (f32, f32)) -> Vec<u64> {
    let (w, h) = ((BW / UNIT) as f32, (BH / UNIT) as f32);
    let overlaps = blocks.iter().any(|b| b.0 != new && (b.1 - at.0).abs() < w && (b.2 - at.1).abs() < h);
    if !overlaps {
        return vec![];
    }
    blocks.iter().filter(|b| b.0 != new && b.1 >= at.0 - w * 0.5 && (b.2 - at.1).abs() < h * 2.5).map(|b| b.0).collect()
}

/// Where a block dropped on a wire goes: where it was let go, but never over the block that feeds the wire.
pub fn insert_spot(from: (f32, f32), at: (f32, f32)) -> (f32, f32) {
    // one column = 6 units, as the templates lay blocks out
    if at.0 < from.0 + 6.0 && (at.1 - from.1).abs() < (BH / UNIT) as f32 + 1.0 {
        (from.0 + 6.0, at.1)
    } else {
        at
    }
}

/// Automatic wiring for a block dropped on empty space: the closest block on its left feeds it (sources feed the
/// closest block on their right instead). Returns (from, to).
pub fn auto_link(s: &Session, new: u64) -> Option<(u64, u64)> {
    let nb = s.block(new)?;
    let d = |b: &Block| ((b.x - nb.x).powi(2) + (b.y - nb.y).powi(2)).sqrt();
    let ends = |k: &str| matches!(k, "rtmp_stream" | "radio_stream");
    if is_source(&nb.kind) {
        let to = s.blocks.iter().filter(|b| b.id != new && b.x > nb.x && !is_source(&b.kind)).min_by(|a, b| d(a).total_cmp(&d(b)))?;
        Some((new, to.id))
    } else {
        let from = s.blocks.iter().filter(|b| b.id != new && b.x < nb.x && !ends(&b.kind)).min_by(|a, b| d(a).total_cmp(&d(b)))?;
        Some((from.id, new))
    }
}

/// The zoom that shows `content` (w, h px at 100 %) in `avail` with a margin, never above 100 % nor under 75 %.
pub fn fit_zoom(content: (f64, f64), avail: (f64, f64), margin: f64) -> f64 {
    if content.0 <= 0.0 || content.1 <= 0.0 {
        return 1.0;
    }
    let z = ((avail.0 - 2.0 * margin) / content.0).min((avail.1 - 2.0 * margin) / content.1);
    z.clamp(MIN_FIT, 1.0)
}

// ---------------------------------------------------------------- the canvas

struct View {
    id: u64,
    kind: String,
    card: gtk::Box,
    x: Mola,
    y: Mola,
    summary: gtk::Label,
    status_row: gtk::Box,
    status_dot: gtk::Box,
    status: gtk::Label,
    gr: gtk::DrawingArea,
    meter: gtk::DrawingArea,
    error: gtk::Label,
    power: gtk::Button,
    power_icon: gtk::Image,
    /// what the texts show now, to set them only when they change
    shown: String,
}

enum Gest {
    None,
    Move { id: u64, from: (f64, f64), moved: bool },
    Wire { from: u64, at: (f64, f64) },
    Pan { from: (f64, f64), moved: bool, wire: Option<(u64, u64)> },
}

pub struct Canvas {
    ui: Rc<Ui>,
    pub root: gtk::Overlay,
    below: gtk::DrawingArea,
    fixed: gtk::Fixed,
    above: gtk::DrawingArea,
    views: RefCell<Vec<View>>,
    uid: Cell<u64>,
    pan: Cell<(f64, f64)>,
    zoom: Cell<f64>,
    /// the view follows the content (until the person pans or zooms)
    fitting: Cell<bool>,
    /// px at the bottom covered by the inspector sheet
    inset: Cell<f64>,
    gest: RefCell<Gest>,
    pub wire_sel: Cell<Option<(u64, u64)>>,
    /// while a block from the library is dragged over: the pointer and the wire under it
    drop_at: Cell<Option<(f64, f64)>>,
    drop_wire: Cell<Option<(u64, u64)>>,
    /// a block that just arrived starts here (px) and springs to its place
    spawn: RefCell<Option<(u64, (f64, f64))>>,
    phase: Cell<f64>,
    animating: Cell<bool>,
    zoom_label: gtk::Label,
    pill: gtk::Box,
    status: gtk::Box,
    load: gtk::Label,
    rate: gtk::Label,
    wiring: gtk::Button,
    banner: gtk::Box,
    banner_title: gtk::Label,
    banner_text: gtk::Label,
    banner_button: gtk::Button,
    banner_icon: gtk::Image,
    /// the banner's block and whether its trouble is access (Allow…) or the device (choose another)
    banner_for: Cell<(u64, bool)>,
    tip: gtk::Label,
    hint: gtk::Label,
    /// called when a block is pressed (the editor reopens a closed sheet)
    pub on_press: RefCell<Option<Box<dyn Fn()>>>,
}

fn transform(x: f64, y: f64, z: f64) -> gsk::Transform {
    gsk::Transform::new().translate(&graphene::Point::new(x as f32, y as f32)).scale(z as f32, z as f32)
}

impl Canvas {
    pub fn new(ui: &Rc<Ui>) -> Rc<Canvas> {
        let root = gtk::Overlay::new();
        root.add_css_class("ka-canvas");
        root.set_hexpand(true);
        root.set_vexpand(true);
        root.set_overflow(gtk::Overflow::Hidden);
        let below = gtk::DrawingArea::new();
        below.set_hexpand(true);
        below.set_vexpand(true);
        below.set_focusable(true);
        root.set_child(Some(&below));
        let fixed = gtk::Fixed::new();
        fixed.set_halign(Align::Start);
        fixed.set_valign(Align::Start);
        root.add_overlay(&fixed);
        root.set_clip_overlay(&fixed, true);
        let above = gtk::DrawingArea::new();
        above.set_can_target(false);
        root.add_overlay(&above);
        follow_theme(&below);
        follow_theme(&above);

        // the error banner (no access to a device)
        let banner = hbox(12, "ka-faixa-erro");
        banner.set_valign(Align::Start);
        banner.set_margin_start(16);
        banner.set_margin_end(16);
        banner.set_margin_top(16);
        let banner_icon = icons::icon("microfone", "red", 16);
        banner.append(&banner_icon);
        let bt = vbox(2, "");
        bt.set_hexpand(true);
        let banner_title = line("", "ka-faixa-titulo");
        let banner_text = line("", "ka-fraco");
        bt.append(&banner_title);
        bt.append(&banner_text);
        banner.append(&bt);
        let allow = button(tr("Allow…"), "secundario", None);
        banner.append(&allow);
        banner.set_visible(false);
        root.add_overlay(&banner);

        // the tip while dragging a block over a wire
        let tip = label("", "ka-dica-soltar");
        tip.set_halign(Align::Start);
        tip.set_valign(Align::Start);
        tip.set_can_target(false);
        tip.set_visible(false);
        root.add_overlay(&tip);

        // the wiring hint (manual mode)
        let hint = label(tr("Manual wiring: drag from a block’s output dot to another block. Click a wire and press Delete to remove it."), "ka-fraco ka-dica-fio");
        hint.set_halign(Align::Start);
        hint.set_valign(Align::Start);
        hint.set_margin_start(16);
        hint.set_margin_top(12);
        hint.set_can_target(false);
        hint.set_visible(false);
        root.add_overlay(&hint);

        // zoom pill (bottom left)
        let pill = hbox(0, "ka-zoom");
        pill.set_halign(Align::Start);
        pill.set_valign(Align::End);
        pill.set_margin_start(16);
        pill.set_margin_bottom(16);
        let minus = icon_button("menos", "text2", 12, tr("Zoom out"));
        let zoom_label = label("100 %", "ka-zoom-valor ka-tnum");
        let plus = icon_button("mais", "text2", 12, tr("Zoom in"));
        let fit = gtk::Button::with_label(tr("Fit"));
        fit.add_css_class("ka-zoom-ajustar");
        pill.append(&minus);
        pill.append(&zoom_label);
        pill.append(&plus);
        let sep = hbox(0, "ka-linha-div");
        sep.set_margin_top(6);
        sep.set_margin_bottom(6);
        pill.append(&sep);
        pill.append(&fit);
        root.add_overlay(&pill);

        // status (bottom right)
        let status = hbox(8, "ka-canvas-estado");
        status.set_halign(Align::End);
        status.set_valign(Align::End);
        status.set_margin_end(16);
        status.set_margin_bottom(22);
        let load = label("", "ka-fraco2 ka-tnum");
        let rate = label("", "ka-fraco2");
        let wiring = gtk::Button::with_label("");
        wiring.add_css_class("ka-link-botao");
        wiring.add_css_class("ka-fiacao");
        status.append(&load);
        status.append(&label("·", "ka-fraco2"));
        status.append(&rate);
        status.append(&label("·", "ka-fraco2"));
        status.append(&wiring);
        root.add_overlay(&status);

        let cv = Rc::new(Canvas {
            ui: ui.clone(),
            root,
            below,
            fixed,
            above,
            views: RefCell::new(vec![]),
            uid: Cell::new(0),
            pan: Cell::new((0.0, 0.0)),
            zoom: Cell::new(1.0),
            fitting: Cell::new(true),
            inset: Cell::new(0.0),
            gest: RefCell::new(Gest::None),
            wire_sel: Cell::new(None),
            drop_at: Cell::new(None),
            drop_wire: Cell::new(None),
            spawn: RefCell::new(None),
            phase: Cell::new(0.0),
            animating: Cell::new(false),
            zoom_label,
            pill,
            status,
            load,
            rate,
            wiring,
            banner,
            banner_title,
            banner_text,
            banner_button: allow,
            banner_icon,
            banner_for: Cell::new((0, true)),
            tip,
            hint,
            on_press: RefCell::new(None),
        });

        {
            let w = Rc::downgrade(&cv);
            cv.below.set_draw_func(move |_, cr, width, height| {
                if let Some(c) = w.upgrade() {
                    c.draw_below(cr, width as f64, height as f64);
                }
            });
            let w = Rc::downgrade(&cv);
            cv.above.set_draw_func(move |_, cr, _, _| {
                if let Some(c) = w.upgrade() {
                    c.draw_above(cr);
                }
            });
            let w = Rc::downgrade(&cv);
            cv.below.connect_resize(move |_, _, _| {
                if let Some(c) = w.upgrade() {
                    if c.fitting.get() {
                        c.fit();
                    }
                }
            });
        }
        {
            let w = Rc::downgrade(&cv);
            minus.connect_clicked(move |_| {
                if let Some(c) = w.upgrade() {
                    c.step_zoom(-1);
                }
            });
            let w = Rc::downgrade(&cv);
            plus.connect_clicked(move |_| {
                if let Some(c) = w.upgrade() {
                    c.step_zoom(1);
                }
            });
            let w = Rc::downgrade(&cv);
            fit.connect_clicked(move |_| {
                if let Some(c) = w.upgrade() {
                    c.fitting.set(true);
                    c.fit();
                }
            });
            let u = ui.clone();
            cv.wiring.connect_clicked(move |_| editor::toggle_manual(&u));
        }
        {
            let w = Rc::downgrade(&cv);
            cv.banner_button.connect_clicked(move |b| {
                let Some(c) = w.upgrade() else { return };
                let (id, access) = c.banner_for.get();
                if access {
                    open_privacy(b);
                } else {
                    c.ui.core.borrow_mut().sel = Some(id);
                    c.ui.changed(Change::Selection);
                }
            });
        }
        cv.gestures();
        cv.drop_target();
        cv
    }

    // ------------------------------------------------------------ view ↔ screen

    fn to_screen(&self, p: (f64, f64)) -> (f64, f64) {
        let (z, (px, py)) = (self.zoom.get(), self.pan.get());
        (p.0 * z + px, p.1 * z + py)
    }

    fn to_canvas(&self, p: (f64, f64)) -> (f64, f64) {
        let (z, (px, py)) = (self.zoom.get(), self.pan.get());
        ((p.0 - px) / z, (p.1 - py) / z)
    }

    /// The blocks as drawn now (animated positions in px) with their wires.
    fn geometry(&self) -> Vec<(u64, f64, f64, Vec<u64>)> {
        let Ok(c) = self.ui.core.try_borrow() else { return vec![] };
        let Some(s) = c.session() else { return vec![] };
        let views = self.views.borrow();
        s.blocks
            .iter()
            .map(|b| {
                let (x, y) = views.iter().find(|v| v.id == b.id).map(|v| (v.x.x, v.y.x)).unwrap_or((b.x as f64 * UNIT, b.y as f64 * UNIT));
                (b.id, x, y, b.outs.clone())
            })
            .collect()
    }

    fn block_at(&self, p: (f64, f64)) -> Option<u64> {
        // the top-most (last drawn) first
        self.views.borrow().iter().rev().find(|v| p.0 >= v.x.x && p.0 <= v.x.x + BW && p.1 >= v.y.x && p.1 <= v.y.x + BH).map(|v| v.id)
    }

    fn port_at(&self, p: (f64, f64)) -> Option<u64> {
        let reach = 10.0 / self.zoom.get().max(0.3);
        self.views
            .borrow()
            .iter()
            .filter(|v| has_output(&v.kind))
            .find(|v| {
                let o = out_port(v.x.x, v.y.x);
                (p.0 - o.0).hypot(p.1 - o.1) <= reach
            })
            .map(|v| v.id)
    }

    /// The block whose input is at (or whose card covers) `p`.
    fn input_at(&self, p: (f64, f64), not: u64) -> Option<u64> {
        let reach = 14.0 / self.zoom.get().max(0.3);
        let views = self.views.borrow();
        views
            .iter()
            .filter(|v| v.id != not && !is_source(&v.kind))
            .find(|v| {
                let i = in_port(v.x.x, v.y.x);
                (p.0 - i.0).hypot(p.1 - i.1) <= reach || (p.0 >= v.x.x && p.0 <= v.x.x + BW && p.1 >= v.y.x && p.1 <= v.y.x + BH)
            })
            .map(|v| v.id)
    }

    fn place(&self) {
        let (z, (px, py)) = (self.zoom.get(), self.pan.get());
        for v in self.views.borrow().iter() {
            self.fixed.set_child_transform(&v.card, Some(&transform(v.x.x * z + px, v.y.x * z + py, z)));
        }
        self.below.queue_draw();
        self.above.queue_draw();
    }

    fn set_view(&self, zoom: f64, pan: (f64, f64)) {
        self.zoom.set(zoom);
        self.pan.set(pan);
        self.zoom_label.set_text(&format!("{} %", (zoom * 100.0).round() as i32));
        self.place();
    }

    fn step_zoom(&self, dir: i32) {
        let z = self.zoom.get();
        let next = if dir > 0 { ZOOMS.iter().copied().find(|s| *s > z + 0.001) } else { ZOOMS.iter().rev().copied().find(|s| *s < z - 0.001) };
        if let Some(n) = next {
            let (w, h) = (self.below.width() as f64, self.below.height() as f64 - self.inset.get());
            self.zoom_about(n, (w / 2.0, h / 2.0));
        }
    }

    fn zoom_about(&self, z: f64, at: (f64, f64)) {
        let c = self.to_canvas(at);
        self.fitting.set(false);
        self.set_view(z, (at.0 - c.0 * z, at.1 - c.1 * z));
    }

    /// Shows all the blocks: centred when they fit, from the top left (and scrolling) when they do not.
    pub fn fit(&self) {
        let (w, h) = (self.below.width() as f64, self.below.height() as f64 - self.inset.get());
        if w < 10.0 || h < 10.0 {
            return;
        }
        let g: Vec<(f64, f64)> = {
            let Ok(c) = self.ui.core.try_borrow() else { return };
            c.session().map(|s| s.blocks.iter().map(|b| (b.x as f64 * UNIT, b.y as f64 * UNIT)).collect()).unwrap_or_default()
        };
        if g.is_empty() {
            self.set_view(1.0, (0.0, 0.0));
            return;
        }
        let (x0, y0) = g.iter().fold((f64::MAX, f64::MAX), |a, p| (a.0.min(p.0), a.1.min(p.1)));
        let (x1, y1) = g.iter().fold((f64::MIN, f64::MIN), |a, p| (a.0.max(p.0 + BW), a.1.max(p.1 + BH)));
        let margin = 24.0;
        let z = fit_zoom((x1 - x0, y1 - y0), (w, h), margin);
        let (cw, ch) = ((x1 - x0) * z, (y1 - y0) * z);
        let top = if self.banner.is_visible() || self.hint.is_visible() { 72.0 } else { margin };
        let px = if cw + 2.0 * margin <= w { (w - cw) / 2.0 - x0 * z } else { margin - x0 * z };
        // a little above the middle, like the designs
        let py = if ch + top + margin <= h { ((h - ch) * 0.4).max(top) - y0 * z } else { top - y0 * z };
        self.set_view(z, (px, py));
    }

    /// The bottom of the canvas covered by the inspector sheet (px): the pill and the status move above it.
    pub fn set_inset(&self, px: f64) {
        if (self.inset.get() - px).abs() < 0.5 {
            return;
        }
        self.inset.set(px);
        if self.fitting.get() {
            self.fit();
        }
        self.pill.set_margin_bottom(16 + px as i32);
        self.status.set_margin_bottom(22 + px as i32);
    }

    pub fn set_narrow(&self, narrow: bool) {
        self.pill.set_visible(!narrow);
        self.status.set_visible(!narrow);
    }

    // ------------------------------------------------------------ blocks

    /// Follows the open session: cards for new blocks, gone ones removed, moved ones spring to their place.
    pub fn sync(self: &Rc<Self>) {
        let (uid, blocks) = {
            let c = self.ui.core.borrow();
            match c.session() {
                Some(s) => (s.uid, s.blocks.clone()),
                None => (0, vec![]),
            }
        };
        if uid != self.uid.get() {
            for v in self.views.borrow_mut().drain(..) {
                self.fixed.remove(&v.card);
            }
            self.uid.set(uid);
            self.fitting.set(true);
            self.wire_sel.set(None);
        }
        let spawn = self.spawn.borrow_mut().take();
        let mut moving = false;
        {
            let mut views = self.views.borrow_mut();
            views.retain(|v| {
                let keep = blocks.iter().any(|b| b.id == v.id);
                if !keep {
                    self.fixed.remove(&v.card);
                }
                keep
            });
            for b in &blocks {
                let (tx, ty) = (b.x as f64 * UNIT, b.y as f64 * UNIT);
                if let Some(v) = views.iter_mut().find(|v| v.id == b.id) {
                    let dragging = matches!(&*self.gest.borrow(), Gest::Move { id, .. } if *id == b.id);
                    if !dragging && (v.x.alvo != tx || v.y.alvo != ty) {
                        v.x.ir(tx, mov::SUAVE);
                        v.y.ir(ty, mov::SUAVE);
                        moving = true;
                    }
                } else {
                    let mut v = self.build_view(b);
                    match spawn {
                        Some((id, from)) if id == b.id => {
                            v.x.pular(from.0);
                            v.y.pular(from.1);
                            v.x.ir(tx, mov::ELASTICA);
                            v.y.ir(ty, mov::ELASTICA);
                            moving = true;
                        }
                        _ => {
                            v.x.pular(tx);
                            v.y.pular(ty);
                        }
                    }
                    self.fixed.put(&v.card, 0.0, 0.0);
                    views.push(v);
                }
            }
        }
        if let Some(sel) = self.wire_sel.get() {
            if !blocks.iter().any(|b| b.id == sel.0 && b.outs.contains(&sel.1)) {
                self.wire_sel.set(None);
            }
        }
        self.refresh(true);
        if self.fitting.get() {
            self.fit();
        } else {
            self.place();
        }
        if moving {
            self.animate();
        }
    }

    fn build_view(&self, b: &Block) -> View {
        let card = vbox(0, "ka-bloco");
        card.set_size_request(BW as i32, BH as i32);
        let head = hbox(8, "");
        head.append(&ktile(&b.kind, 24));
        let texts = vbox(1, "");
        texts.set_hexpand(true);
        let title = line(kind_info(&b.kind).map(|k| tr(k.title)).unwrap_or("?"), "ka-bloco-titulo");
        let summary = line("", "ka-bloco-resumo");
        texts.append(&title);
        texts.append(&summary);
        head.append(&texts);
        let power = gtk::Button::new();
        power.add_css_class("ka-bloco-energia");
        power.set_valign(Align::Start);
        let power_icon = icons::icon("desligar", "teal", 14);
        power.set_child(Some(&power_icon));
        power.set_tooltip_text(Some(tr("Turn on or off")));
        {
            let (u, id) = (self.ui.clone(), b.id);
            power.connect_clicked(move |_| editor::toggle_block(&u, id));
        }
        head.append(&power);
        card.append(&head);

        let status_row = hbox(6, "ka-bloco-linha");
        let status_dot = dot("red");
        status_dot.add_css_class("ka-pulsa");
        status_row.append(&status_dot);
        let status = label("", "ka-bloco-resumo ka-tnum");
        status.set_wrap(true);
        status.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        status.set_lines(2);
        status.set_ellipsize(gtk::pango::EllipsizeMode::End);
        status.set_max_width_chars(1);
        status.set_hexpand(true);
        status_row.append(&status);
        let gr = gtk::DrawingArea::new();
        gr.set_content_width(40);
        gr.set_content_height(4);
        gr.set_valign(Align::Center);
        gr.set_visible(false);
        {
            let (u, id) = (self.ui.clone(), b.id);
            gr.set_draw_func(move |_, cr, w, h| {
                let r = u.core.try_borrow().ok().and_then(|c| editor::gain_reduction_of(&c, id)).unwrap_or(0.0);
                draw_reduction(cr, w as f64, h as f64, r);
            });
        }
        follow_theme(&gr);
        status_row.append(&gr);
        card.append(&status_row);
        let fill = gtk::Box::new(gtk::Orientation::Vertical, 0);
        fill.set_vexpand(true);
        card.append(&fill);
        let error = line(tr("No access · Allow…"), "ka-bloco-erro");
        error.set_visible(false);
        card.append(&error);
        let (u, uid, id) = (self.ui.clone(), self.uid.get(), b.id);
        let meter = meter(BW as i32 - 24, 9, move || u.core.try_borrow().ok().and_then(|c| c.disp.get(&(uid, id)).copied()));
        meter.set_hexpand(true);
        card.append(&meter);
        View {
            id: b.id,
            kind: b.kind.clone(),
            card,
            x: Mola::new(0.0),
            y: Mola::new(0.0),
            summary,
            status_row,
            status_dot,
            status,
            gr,
            meter,
            error,
            power,
            power_icon,
            shown: String::new(),
        }
    }

    /// Updates what the cards say (summary, recorder time, errors, on/off, selection) and the banner and status.
    /// `force` also redraws the meters' wells.
    pub fn refresh(&self, force: bool) {
        let c = self.ui.core.borrow();
        let Some(s) = c.session() else { return };
        let uid = s.uid;
        let running = c.is_running(uid);
        let mut first_error: Option<(u64, String, String, Trouble)> = None;
        for v in self.views.borrow_mut().iter_mut() {
            let Some(b) = s.block(v.id) else { continue };
            let err = c.block_error(uid, b.id).map(|e| {
                let t = trouble(b, &e);
                (e, t)
            });
            let rec = c.rec_status(uid, b.id);
            let selected = c.sel == Some(b.id);
            let sum = summary(b, &c.devices);
            let (dot_on, status) = editor::status_line(b, running, rec.as_ref(), &c);
            let gr = matches!(b.kind.as_str(), "compressor" | "loudness");
            let t = err.as_ref().map(|e| e.1);
            let key = format!("{sum}|{status}|{dot_on}|{}|{selected}|{:?}", b.enabled, t);
            if let Some((e, t)) = &err {
                if first_error.is_none() && *t != Trouble::Waiting {
                    first_error = Some((b.id, b.kind.clone(), e.clone(), *t));
                }
            }
            if gr && running {
                v.gr.queue_draw();
            }
            if !force && key == v.shown {
                continue;
            }
            v.shown = key;
            let bad = matches!(t, Some(Trouble::Access) | Some(Trouble::Device));
            match t {
                Some(Trouble::Access) => v.summary.set_text(tr("No permission")),
                Some(Trouble::Device) => v.summary.set_text(tr("Device unavailable")),
                Some(Trouble::Waiting) => v.summary.set_text(tr("Waiting for sound")),
                None => v.summary.set_text(&sum),
            }
            set_class(&v.summary, "cor-red", bad);
            v.error.set_text(if t == Some(Trouble::Access) { tr("No access · Allow…") } else { tr("Choose another device") });
            v.status.set_text(&status);
            v.status_dot.set_visible(dot_on);
            v.gr.set_visible(gr);
            v.status_row.set_visible(!status.is_empty() || gr);
            v.error.set_visible(bad);
            v.meter.set_visible(!bad);
            set_class(&v.card, "selecionado", selected);
            set_class(&v.card, "erro", bad);
            set_class(&v.card, "desligado", !b.enabled);
            icons::set(&v.power_icon, "desligar", if b.enabled { "teal" } else { "dim" }, 14);
            v.power.set_tooltip_text(Some(if b.enabled { tr("Turn off (the sound passes unchanged)") } else { tr("Turn on") }));
            v.meter.queue_draw();
        }
        // banner: no access to a device, or a device that did not open
        match &first_error {
            Some((id, kind, e, t)) => {
                let title = match (kind.as_str(), t) {
                    ("device_input", Trouble::Access) => tr("No access to the microphone"),
                    (_, Trouble::Access) => tr("No access to the computer’s sound"),
                    ("device_output", _) => tr("Could not play the sound"),
                    _ => tr("Could not open the device"),
                };
                self.banner_title.set_text(title);
                let detail = if *t == Trouble::Access { trf("KalangoOS is not letting Kalango Audio use it ({err}).", &[("err", e.clone())]) } else { e.clone() };
                self.banner_text.set_text(&detail);
                self.banner_for.set((*id, *t == Trouble::Access));
                icons::set(&self.banner_icon, if kind == "device_input" { "microfone" } else if *t == Trouble::Access { "volume" } else { "alerta" }, "red", 16);
                if let Some(l) = self.banner_button.child().and_then(|r| r.last_child()).and_downcast::<gtk::Label>() {
                    l.set_text(if *t == Trouble::Access { tr("Allow…") } else { tr("Choose a device") });
                }
                self.banner.set_visible(true);
            }
            None => self.banner.set_visible(false),
        }
        self.hint.set_visible(s.manual && first_error.is_none());
        // status
        let rate = if s.sample_rate == 0 { 48000 } else { s.sample_rate };
        let khz = format!("{} kHz", format!("{:.1}", rate as f64 / 1000.0).trim_end_matches(".0").replace('.', ","));
        self.rate.set_text(&trf("{rate} · 10 ms block", &[("rate", khz)]));
        match c.load_of(uid) {
            Some((l, _)) => self.load.set_text(&trf("Audio load {n} %", &[("n", ((l * 100.0).round() as i32).to_string())])),
            None => self.load.set_text(tr("Engine idle")),
        }
        self.wiring.set_label(if s.manual { tr("Manual wiring") } else { tr("Automatic wiring") });
        self.wiring.set_tooltip_text(Some(tr("Click to switch between automatic and manual wiring")));
    }

    /// Every frame while the editor shows: meters and the dashes running along the wires.
    pub fn tick(&self, running: bool) {
        if running {
            for v in self.views.borrow().iter() {
                v.meter.queue_draw();
            }
            self.phase.set((self.phase.get() + 1.0) % 1200.0);
            self.below.queue_draw();
        }
    }

    fn animate(self: &Rc<Self>) {
        if self.animating.replace(true) {
            return;
        }
        let w = Rc::downgrade(self);
        let last = Cell::new(0i64);
        self.fixed.add_tick_callback(move |_, clock| {
            let Some(c) = w.upgrade() else { return glib::ControlFlow::Break };
            let t = clock.frame_time();
            let dt = if last.get() == 0 { 1.0 / 60.0 } else { ((t - last.get()) as f64 / 1e6).clamp(0.0, 0.05) };
            last.set(t);
            let mut all = true;
            for v in c.views.borrow_mut().iter_mut() {
                if !v.x.parada() {
                    v.x.passo(dt);
                }
                if !v.y.parada() {
                    v.y.passo(dt);
                }
                all &= v.x.parada() && v.y.parada();
            }
            c.place();
            if all {
                c.animating.set(false);
                return glib::ControlFlow::Break;
            }
            glib::ControlFlow::Continue
        });
    }

    // ------------------------------------------------------------ drawing

    fn draw_below(&self, cr: &gtk::cairo::Context, w: f64, h: f64) {
        theme::source(cr, "bg", 1.0);
        let _ = cr.paint();
        // dot grid
        let (z, (px, py)) = (self.zoom.get(), self.pan.get());
        let step = DOTS * z;
        if step >= 6.0 {
            theme::source(cr, "border", 1.0);
            let r = (1.0 * z).clamp(0.6, 1.4);
            let mut x = px.rem_euclid(step);
            while x < w {
                let mut y = py.rem_euclid(step);
                while y < h {
                    cr.rectangle(x - r / 2.0, y - r / 2.0, r, r);
                    y += step;
                }
                x += step;
            }
            let _ = cr.fill();
        }
        // wires
        let (running, enabled): (bool, Vec<(u64, bool, f32)>) = {
            let Ok(c) = self.ui.core.try_borrow() else { return };
            let Some(s) = c.session() else { return };
            let uid = s.uid;
            (c.is_running(uid), s.blocks.iter().map(|b| (b.id, b.enabled, c.disp.get(&(uid, b.id)).map(|d| d.peak_max()).unwrap_or(0.0))).collect())
        };
        let geo = self.geometry();
        let pos = |id: u64| geo.iter().find(|b| b.0 == id).map(|b| (b.1, b.2));
        cr.set_line_cap(gtk::cairo::LineCap::Round);
        for (id, x, y, outs) in &geo {
            let (on, level) = enabled.iter().find(|e| e.0 == *id).map(|e| (e.1, e.2)).unwrap_or((true, 0.0));
            for o in outs {
                let Some((ox, oy)) = pos(*o) else { continue };
                let a = self.to_screen(out_port(*x, *y));
                let b = self.to_screen(in_port(ox, oy));
                let c = wire_curve(a, b);
                let hot = self.drop_wire.get() == Some((*id, *o)) || self.wire_sel.get() == Some((*id, *o));
                cr.new_path();
                cr.move_to(c[0].0, c[0].1);
                cr.curve_to(c[1].0, c[1].1, c[2].0, c[2].1, c[3].0, c[3].1);
                let live = running && on;
                theme::source(cr, if live || hot { "accent" } else { "divider" }, 1.0);
                cr.set_line_width(if hot { 3.0 } else { 2.0 });
                let _ = cr.stroke_preserve();
                if live && level > 0.01 {
                    // the signal runs along the wire, faster when louder
                    theme::source(cr, "realce", 0.55);
                    cr.set_line_width(2.0);
                    cr.set_dash(&[3.0, 9.0], -self.phase.get() * (0.6 + 2.4 * level as f64));
                    let _ = cr.stroke_preserve();
                    cr.set_dash(&[], 0.0);
                }
                cr.new_path();
            }
        }
    }

    fn draw_above(&self, cr: &gtk::cairo::Context) {
        let z = self.zoom.get();
        let r = PORT_R * z.clamp(0.75, 1.5);
        for v in self.views.borrow().iter() {
            let tok = kind_token(&v.kind);
            if !is_source(&v.kind) {
                let p = self.to_screen(in_port(v.x.x, v.y.x));
                cr.new_path();
                cr.arc(p.0, p.1, r, 0.0, std::f64::consts::TAU);
                theme::source(cr, "bg", 1.0);
                let _ = cr.fill_preserve();
                theme::source(cr, tok, 1.0);
                cr.set_line_width(1.5);
                let _ = cr.stroke();
            }
            if has_output(&v.kind) {
                let p = self.to_screen(out_port(v.x.x, v.y.x));
                cr.new_path();
                cr.arc(p.0, p.1, r, 0.0, std::f64::consts::TAU);
                theme::source(cr, tok, 1.0);
                let _ = cr.fill();
            }
        }
        // the wire being drawn by hand
        if let Gest::Wire { from, at } = &*self.gest.borrow() {
            if let Some(v) = self.views.borrow().iter().find(|v| v.id == *from) {
                let a = self.to_screen(out_port(v.x.x, v.y.x));
                let c = wire_curve(a, *at);
                cr.new_path();
                cr.move_to(c[0].0, c[0].1);
                cr.curve_to(c[1].0, c[1].1, c[2].0, c[2].1, c[3].0, c[3].1);
                theme::source(cr, "accent", 1.0);
                cr.set_line_width(2.0);
                cr.set_dash(&[6.0, 4.0], 0.0);
                let _ = cr.stroke();
                cr.set_dash(&[], 0.0);
            }
        }
        // the "+" in the middle of the wire a dragged block would go into
        if let Some((a, b)) = self.drop_wire.get() {
            let geo = self.geometry();
            let pos = |id: u64| geo.iter().find(|g| g.0 == id).map(|g| (g.1, g.2));
            if let (Some(pa), Some(pb)) = (pos(a), pos(b)) {
                let c = wire_curve(self.to_screen(out_port(pa.0, pa.1)), self.to_screen(in_port(pb.0, pb.1)));
                let m = bezier_at(&c, 0.5);
                rounded(cr, m.0 - 11.0, m.1 - 11.0, 22.0, 22.0, 4.0);
                theme::source(cr, "accent", 1.0);
                let _ = cr.fill();
                theme::source(cr, "accent_fg", 1.0);
                cr.set_line_width(2.0);
                cr.move_to(m.0 - 5.0, m.1);
                cr.line_to(m.0 + 5.0, m.1);
                cr.move_to(m.0, m.1 - 5.0);
                cr.line_to(m.0, m.1 + 5.0);
                let _ = cr.stroke();
            }
        }
    }

    // ------------------------------------------------------------ pointer

    fn gestures(self: &Rc<Self>) {
        let drag = gtk::GestureDrag::new();
        drag.set_button(gdk::BUTTON_PRIMARY);
        {
            let w = Rc::downgrade(self);
            drag.connect_drag_begin(move |g, x, y| {
                let Some(c) = w.upgrade() else { return };
                c.below.grab_focus();
                let p = c.to_canvas((x, y));
                let manual_ok = true;
                let gest = if let Some(id) = c.port_at(p).filter(|_| manual_ok) {
                    Gest::Wire { from: id, at: (x, y) }
                } else if let Some(id) = c.block_at(p) {
                    let from = c.views.borrow().iter().find(|v| v.id == id).map(|v| (v.x.x, v.y.x)).unwrap_or((0.0, 0.0));
                    c.wire_sel.set(None);
                    let changed = c.ui.core.borrow().sel != Some(id);
                    if changed {
                        c.ui.core.borrow_mut().sel = Some(id);
                        c.ui.changed(Change::Selection);
                    }
                    // the block goes on top of the others
                    if let Some(v) = c.views.borrow().iter().find(|v| v.id == id) {
                        if c.fixed.last_child().as_ref() != Some(v.card.upcast_ref()) {
                            c.fixed.remove(&v.card);
                            c.fixed.put(&v.card, 0.0, 0.0);
                        }
                    }
                    {
                        let mut views = c.views.borrow_mut();
                        if let Some(i) = views.iter().position(|v| v.id == id) {
                            let v = views.remove(i);
                            views.push(v);
                        }
                    }
                    c.place();
                    if let Some(f) = c.on_press.borrow().as_ref() {
                        f();
                    }
                    Gest::Move { id, from, moved: false }
                } else {
                    let wire = wire_at(&c.geometry(), p, 6.0 / c.zoom.get());
                    Gest::Pan { from: c.pan.get(), moved: false, wire }
                };
                g.set_state(gtk::EventSequenceState::Claimed);
                *c.gest.borrow_mut() = gest;
            });
        }
        {
            let w = Rc::downgrade(self);
            drag.connect_drag_update(move |g, dx, dy| {
                let Some(c) = w.upgrade() else { return };
                let z = c.zoom.get();
                let mut gest = c.gest.borrow_mut();
                match &mut *gest {
                    Gest::Move { id, from, moved } => {
                        if dx.hypot(dy) > 3.0 {
                            *moved = true;
                        }
                        if *moved {
                            let (nx, ny) = (from.0 + dx / z, from.1 + dy / z);
                            if let Some(v) = c.views.borrow_mut().iter_mut().find(|v| v.id == *id) {
                                v.x.pular(nx);
                                v.y.pular(ny);
                            }
                            drop(gest);
                            c.place();
                        }
                    }
                    Gest::Wire { at, .. } => {
                        if let Some((sx, sy)) = g.start_point() {
                            *at = (sx + dx, sy + dy);
                        }
                        drop(gest);
                        c.above.queue_draw();
                    }
                    Gest::Pan { from, moved, .. } => {
                        if dx.hypot(dy) > 3.0 {
                            *moved = true;
                        }
                        if *moved {
                            let p = (from.0 + dx, from.1 + dy);
                            drop(gest);
                            c.fitting.set(false);
                            c.pan.set(p);
                            c.place();
                        }
                    }
                    Gest::None => {}
                }
            });
        }
        {
            let w = Rc::downgrade(self);
            drag.connect_drag_end(move |g, dx, dy| {
                let Some(c) = w.upgrade() else { return };
                let gest = std::mem::replace(&mut *c.gest.borrow_mut(), Gest::None);
                match gest {
                    Gest::Move { id, moved: true, .. } => {
                        c.fitting.set(false);
                        // settle on the grid (elástica) and keep the place in the session
                        let target = c.views.borrow().iter().find(|v| v.id == id).map(|v| (snap_units(v.x.x), snap_units(v.y.x)));
                        if let Some((ux, uy)) = target {
                            if let Some(v) = c.views.borrow_mut().iter_mut().find(|v| v.id == id) {
                                v.x.ir(ux as f64 * UNIT, mov::ELASTICA);
                                v.y.ir(uy as f64 * UNIT, mov::ELASTICA);
                            }
                            editor::with_block(&c.ui, id, |b| {
                                b.x = ux;
                                b.y = uy;
                            });
                            c.animate();
                            c.ui.changed(Change::Sessions);
                        }
                    }
                    Gest::Wire { from, .. } => {
                        let end = g.start_point().map(|(sx, sy)| c.to_canvas((sx + dx, sy + dy)));
                        if let Some(to) = end.and_then(|p| c.input_at(p, from)) {
                            if let Some(s) = c.ui.core.borrow_mut().session_mut() {
                                s.connect(from, to);
                            }
                            c.ui.changed(Change::Sessions);
                        }
                        c.above.queue_draw();
                    }
                    Gest::Move { id, moved: false, .. } => {
                        // the card's "No access · Allow…" line
                        let p = g.start_point().map(|(sx, sy)| c.to_canvas((sx + dx, sy + dy)));
                        let v = c.views.borrow().iter().find(|v| v.id == id).map(|v| (v.y.x, v.error.is_visible(), v.error.text().to_string()));
                        if let (Some(p), Some((y, true, text))) = (p, v) {
                            if p.1 > y + BH - 26.0 && text == tr("No access · Allow…") {
                                open_privacy(&c.root);
                            }
                        }
                    }
                    Gest::Pan { moved: false, wire, .. } => {
                        c.wire_sel.set(wire);
                        let had = c.ui.core.borrow().sel.is_some();
                        if had {
                            c.ui.core.borrow_mut().sel = None;
                            c.ui.changed(Change::Selection);
                        }
                        c.below.queue_draw();
                    }
                    _ => {}
                }
            });
        }
        self.root.add_controller(drag);

        // right click: a block's menu
        let menu = gtk::GestureClick::new();
        menu.set_button(gdk::BUTTON_SECONDARY);
        {
            let w = Rc::downgrade(self);
            menu.connect_pressed(move |_, _, x, y| {
                let Some(c) = w.upgrade() else { return };
                let Some(id) = c.block_at(c.to_canvas((x, y))) else { return };
                c.ui.core.borrow_mut().sel = Some(id);
                c.ui.changed(Change::Selection);
                c.block_menu(id, x, y);
            });
        }
        self.root.add_controller(menu);

        // wheel: pan; Ctrl + wheel: zoom where the pointer is
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
        {
            let w = Rc::downgrade(self);
            let pointer = Rc::new(Cell::new((0.0, 0.0)));
            let motion = gtk::EventControllerMotion::new();
            let p2 = pointer.clone();
            motion.connect_motion(move |_, x, y| p2.set((x, y)));
            self.root.add_controller(motion);
            scroll.connect_scroll(move |s, dx, dy| {
                let Some(c) = w.upgrade() else { return glib::Propagation::Proceed };
                if s.current_event_state().contains(gdk::ModifierType::CONTROL_MASK) {
                    let z = (c.zoom.get() * if dy < 0.0 { 1.1 } else { 1.0 / 1.1 }).clamp(ZOOMS[0], ZOOMS[ZOOMS.len() - 1]);
                    c.zoom_about(z, pointer.get());
                } else {
                    let (px, py) = c.pan.get();
                    c.fitting.set(false);
                    c.pan.set((px - dx * 40.0, py - dy * 40.0));
                    c.place();
                }
                glib::Propagation::Stop
            });
        }
        self.root.add_controller(scroll);
    }

    fn block_menu(self: &Rc<Self>, id: u64, x: f64, y: f64) {
        let enabled = self.ui.core.borrow().session().and_then(|s| s.block(id)).map(|b| b.enabled).unwrap_or(true);
        let pop = gtk::Popover::new();
        pop.add_css_class("menu");
        pop.set_has_arrow(false);
        pop.set_parent(&self.root);
        pop.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        let items = vbox(2, "");
        for (text, what) in [(if enabled { tr("Turn off") } else { tr("Turn on") }, 0), (tr("Duplicate"), 1), (tr("Remove"), 2)] {
            let b = gtk::Button::with_label(text);
            b.add_css_class("ka-item-menu");
            if let Some(l) = b.child().and_downcast::<gtk::Label>() {
                l.set_xalign(0.0);
            }
            let (u, p) = (self.ui.clone(), pop.clone());
            b.connect_clicked(move |_| {
                p.popdown();
                match what {
                    0 => editor::toggle_block(&u, id),
                    1 => editor::duplicate_block(&u, id),
                    _ => editor::remove_block(&u, id),
                }
            });
            items.append(&b);
        }
        pop.set_child(Some(&items));
        pop.connect_closed(|p| {
            let p = p.clone();
            glib::idle_add_local_once(move || p.unparent());
        });
        pop.popup();
    }

    /// Delete: the selected wire if there is one (true), otherwise nothing (the editor removes the block).
    pub fn delete_wire(&self) -> bool {
        let Some((a, b)) = self.wire_sel.take() else { return false };
        if let Some(s) = self.ui.core.borrow_mut().session_mut() {
            s.disconnect(a, b);
        }
        self.ui.changed(Change::Sessions);
        true
    }

    // ------------------------------------------------------------ drag and drop from the library

    fn drop_target(self: &Rc<Self>) {
        let t = gtk::DropTarget::new(glib::Type::STRING, gdk::DragAction::COPY);
        {
            let w = Rc::downgrade(self);
            t.connect_motion(move |_, x, y| {
                let Some(c) = w.upgrade() else { return gdk::DragAction::empty() };
                c.drop_at.set(Some((x, y)));
                let wire = wire_at(&c.geometry(), c.to_canvas((x, y)), 18.0 / c.zoom.get());
                if wire != c.drop_wire.get() {
                    c.drop_wire.set(wire);
                    c.below.queue_draw();
                    c.above.queue_draw();
                }
                match wire {
                    Some((a, b)) => {
                        let name = |id: u64| {
                            c.ui.core.borrow().session().and_then(|s| s.block(id)).and_then(|b| kind_info(&b.kind)).map(|k| tr(k.title).to_string()).unwrap_or_default()
                        };
                        c.tip.set_text(&trf("Drop to insert between {a} and {b}", &[("a", name(a)), ("b", name(b))]));
                        c.tip.set_margin_start((x - 20.0).max(8.0) as i32);
                        c.tip.set_margin_top((y + BH * c.zoom.get() * 0.5 + 24.0) as i32);
                        c.tip.set_visible(true);
                    }
                    None => c.tip.set_visible(false),
                }
                gdk::DragAction::COPY
            });
        }
        {
            let w = Rc::downgrade(self);
            t.connect_leave(move |_| {
                if let Some(c) = w.upgrade() {
                    c.end_drop_hover();
                }
            });
        }
        {
            let w = Rc::downgrade(self);
            t.connect_drop(move |_, value, x, y| {
                let Some(c) = w.upgrade() else { return false };
                let Ok(kind) = value.get::<String>() else { return false };
                let wire = c.drop_wire.get();
                c.end_drop_hover();
                c.drop_block(&kind, (x, y), wire);
                true
            });
        }
        self.root.add_controller(t);
    }

    fn end_drop_hover(&self) {
        self.drop_at.set(None);
        self.drop_wire.set(None);
        self.tip.set_visible(false);
        self.below.queue_draw();
        self.above.queue_draw();
    }

    /// A block from the library let go at `at` (screen px): on a wire it goes in between, elsewhere it lands on the
    /// grid (and, with automatic wiring, joins its closest neighbour).
    fn drop_block(self: &Rc<Self>, kind: &str, at: (f64, f64), wire: Option<(u64, u64)>) {
        if kind_info(kind).is_none() {
            return;
        }
        // the ghost was held by its middle
        let p = self.to_canvas(at);
        let corner = (p.0 - BW / 2.0, p.1 - BH / 2.0);
        let mut units = (snap_units(corner.0), snap_units(corner.1));
        let id = {
            let mut c = self.ui.core.borrow_mut();
            // in the middle of a wire it goes at least one column right of the block that feeds it
            if let Some(from) = wire.and_then(|(a, _)| c.session().and_then(|s| s.block(a)).map(|b| (b.x, b.y))) {
                units = insert_spot(from, units);
            }
            let id = match wire {
                Some((a, b)) => c.insert_on_wire(kind, a, b, units),
                None => c.add_from_library(kind, Some(units)),
            };
            if let (Some(id), Some(s)) = (id, c.session_mut()) {
                let list: Vec<(u64, f32, f32)> = s.blocks.iter().map(|b| (b.id, b.x, b.y)).collect();
                for m in make_room(&list, id, units) {
                    if let Some(b) = s.blocks.iter_mut().find(|b| b.id == m) {
                        b.x += 6.0;
                    }
                }
                if wire.is_none() && !s.manual {
                    if let Some((a, b)) = auto_link(s, id) {
                        s.connect(a, b);
                    }
                }
            }
            id
        };
        if let Some(id) = id {
            *self.spawn.borrow_mut() = Some((id, corner));
            self.fitting.set(false);
            self.ui.changed(Change::Sessions);
            self.ui.changed(Change::Selection);
        }
    }
}

/// What kind of trouble a block's error is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trouble {
    /// the system does not let us capture (KalangoOS privacy): "Allow…"
    Access,
    /// the device is gone or busy: choose another one
    Device,
    /// an Application block waiting for its program to play: not an error
    Waiting,
}

/// Sorts a block's error: capture blocks that cannot open are blocked by the system's privacy (the usual case in
/// KalangoOS); an output that cannot open is a device problem; an Application block waiting for sound is fine.
pub fn trouble(b: &Block, err: &str) -> Trouble {
    if b.kind == "app_source" {
        let waiting = trf("Waiting for “{app}” to play some sound", &[("app", crate::audio::app_label(&b.p.text("app", "Web Browser")))]);
        if err == waiting {
            return Trouble::Waiting;
        }
    }
    match b.kind.as_str() {
        "device_input" | "system_sound" | "app_source" => Trouble::Access,
        _ => Trouble::Device,
    }
}

fn has_output(kind: &str) -> bool {
    !matches!(kind, "rtmp_stream" | "radio_stream")
}

fn set_class(w: &impl IsA<gtk::Widget>, class: &str, on: bool) {
    if on {
        w.add_css_class(class);
    } else {
        w.remove_css_class(class);
    }
}

/// The compressor's gain reduction as a short bar (0 to −20 dB), in coral like the design.
pub fn draw_reduction(cr: &gtk::cairo::Context, w: f64, h: f64, db: f32) {
    theme::source(cr, "sunken", 1.0);
    rounded(cr, 0.0, 0.0, w, h, h / 2.0);
    let _ = cr.fill();
    let f = ((-db) as f64 / 20.0).clamp(0.0, 1.0);
    if f > 0.005 {
        theme::source(cr, "coral", 1.0);
        rounded(cr, w * (1.0 - f), 0.0, w * f, h, h / 2.0);
        let _ = cr.fill();
    }
}

/// Opens the system's privacy settings (microphone access); falls back to `xdg-open`.
pub fn open_privacy(w: &impl IsA<gtk::Widget>) {
    let ctx = w.display().app_launch_context();
    if gtk::gio::AppInfo::launch_default_for_uri("settings://privacy", Some(&ctx)).is_err() {
        let _ = std::process::Command::new("xdg-open").arg("settings://privacy").spawn();
    }
}

/// The recorder's line on its card while running: "● 00:42:13 · 312 MB".
pub fn recorder_line(st: &crate::engine::RecStatus) -> String {
    if st.state == "recording" || st.state == "paused" || st.state == "silent" {
        format!("{} · {}", fmt_time(st.seconds), fmt_size(st.bytes))
    } else {
        state_label(&st.state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapping_goes_to_the_nearest_unit() {
        assert_eq!(snap_units(0.0), 0.0);
        assert_eq!(snap_units(UNIT * 2.4), 2.0);
        assert_eq!(snap_units(UNIT * 2.6), 3.0);
        assert_eq!(snap_units(-UNIT * 0.6), -1.0);
    }

    #[test]
    fn a_wire_is_hit_near_its_curve_only() {
        let a = out_port(0.0, 0.0);
        let b = in_port(300.0, 200.0);
        // the ends and the middle are on the wire
        assert!(wire_distance(a, a, b) < 0.5);
        assert!(wire_distance(b, a, b) < 0.5);
        let m = bezier_at(&wire_curve(a, b), 0.5);
        assert!(wire_distance(m, a, b) < 1.0);
        // far away is not
        assert!(wire_distance((a.0, a.1 + 150.0), a, b) > 40.0);
        let blocks = vec![(1, 0.0, 0.0, vec![2]), (2, 300.0, 200.0, vec![])];
        assert_eq!(wire_at(&blocks, (m.0 + 3.0, m.1), 6.0), Some((1, 2)));
        assert_eq!(wire_at(&blocks, (m.0, m.1 + 60.0), 6.0), None);
    }

    #[test]
    fn wires_leave_right_and_enter_left_even_backwards() {
        let c = wire_curve((100.0, 0.0), (0.0, 0.0));
        assert!(c[1].0 > c[0].0 && c[2].0 < c[3].0);
    }

    #[test]
    fn room_is_made_only_when_the_new_block_covers_another() {
        let blocks = vec![(1, 2.0, 2.0), (2, 8.0, 2.0), (3, 14.0, 2.0), (9, 8.0, 2.0)];
        let mut moved = make_room(&blocks, 9, (8.0, 2.0));
        moved.sort();
        assert_eq!(moved, vec![2, 3]);
        assert!(make_room(&blocks, 9, (8.0, 12.0)).is_empty());
    }

    #[test]
    fn a_block_put_on_a_wire_never_covers_its_feeder() {
        assert_eq!(insert_spot((2.0, 2.0), (4.0, 3.0)), (8.0, 3.0));
        assert_eq!(insert_spot((2.0, 2.0), (12.0, 3.0)), (12.0, 3.0));
        assert_eq!(insert_spot((2.0, 2.0), (4.0, 12.0)), (4.0, 12.0));
    }

    #[test]
    fn errors_are_sorted_by_what_the_person_can_do() {
        let mut s = Session::new("t");
        let mic = s.add_block("device_input", 0.0, 0.0);
        let out = s.add_block("device_output", 6.0, 0.0);
        let app = s.add_block("app_source", 0.0, 6.0);
        assert_eq!(trouble(s.block(mic).unwrap(), "Could not open “default”"), Trouble::Access);
        assert_eq!(trouble(s.block(out).unwrap(), "Could not open “default”"), Trouble::Device);
        let waiting = trf("Waiting for “{app}” to play some sound", &[("app", crate::audio::app_label("Web Browser"))]);
        assert_eq!(trouble(s.block(app).unwrap(), &waiting), Trouble::Waiting);
    }

    #[test]
    fn a_dropped_block_joins_its_closest_neighbour() {
        let mut s = Session::new("t");
        let tone = s.add_block("test_tone", 2.0, 2.0);
        let vol = s.add_block("volume", 8.0, 2.0);
        let far = s.add_block("recorder", 30.0, 20.0);
        let new = s.add_block("compressor", 14.0, 3.0);
        assert_eq!(auto_link(&s, new), Some((vol, new)));
        let src = s.add_block("device_input", 2.0, 9.0);
        assert_eq!(auto_link(&s, src).map(|l| l.0), Some(src));
        assert_ne!(auto_link(&s, src).map(|l| l.1), Some(tone));
        let _ = far;
    }

    #[test]
    fn fitting_never_goes_under_75_or_over_100_percent() {
        assert_eq!(fit_zoom((400.0, 200.0), (1200.0, 800.0), 24.0), 1.0);
        assert_eq!(fit_zoom((4000.0, 200.0), (1200.0, 800.0), 24.0), MIN_FIT);
        let z = fit_zoom((1300.0, 300.0), (1200.0, 800.0), 24.0);
        assert!(z > MIN_FIT && z < 1.0);
    }
}

/// A block kind's tile that keeps its size (the glyph inside asks to expand).
fn ktile(kind: &str, size: i32) -> gtk::Box {
    let t = kind_tile(kind, size);
    t.set_hexpand(false);
    t
}
