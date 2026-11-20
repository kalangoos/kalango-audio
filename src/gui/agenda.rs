//! Schedule and automations (design "Kalango Audio · Agenda e automações"). Left: the week with every session's
//! timers as coloured bands (drag a band's bottom edge to change its end, drag the band to move it; 15-minute
//! snaps), the red line at "now", "New schedule" and the list of timers with their switches. Right: automations as
//! "When … / In … / Do …" cards, "New automation", "Write with the agent", the editor of the person's scripts with
//! line numbers, "Run now" and the script log. Narrow: the week becomes today's list and the automations go below.
//! Also the script API help (`script_help`), opened from the window menu.

use super::gravacoes::modal;
use super::widgets::*;
use super::{icons, Change, Layout, Ui};
use crate::i18n::{tr, trf};
use crate::model::{Automation, Timer, EVENTS};
use crate::presets::Script;
use gtk::prelude::*;
use gtk::{glib, Align};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

// ---------------------------------------------------------------- pure helpers

/// "20:00" → minutes after midnight (also "8:5", "20h", "2000"); 24:00 is allowed as an end.
pub fn parse_hm(s: &str) -> Option<u16> {
    let s = s.trim().trim_end_matches(['h', 'H']);
    let (h, m) = if let Some((h, m)) = s.split_once([':', 'h', 'H', '.']) {
        (h.trim().parse::<u16>().ok()?, if m.trim().is_empty() { 0 } else { m.trim().parse::<u16>().ok()? })
    } else if s.len() == 4 && s.chars().all(|c| c.is_ascii_digit()) {
        (s[..2].parse().ok()?, s[2..].parse().ok()?)
    } else {
        (s.parse::<u16>().ok()?, 0)
    };
    if m >= 60 || h > 24 || (h == 24 && m > 0) {
        return None;
    }
    Some(h * 60 + m)
}

/// Minutes → "20:00" (past midnight wraps: 1500 → "01:00").
pub fn fmt_hm(min: u32) -> String {
    let m = min % 1440;
    format!("{:02}:{:02}", m / 60, m % 60)
}

/// The timer's "To" from its "From" and length.
pub fn to_of(start: u16, minutes: u16) -> u16 {
    ((start as u32 + minutes as u32) % 1440) as u16
}

/// A timer's length from its "From" and "To": a "To" at or before "From" ends on the next day.
pub fn length_between(from: u16, to: u16) -> u16 {
    if to > from {
        to - from
    } else {
        to + 1440 - from
    }
}

/// "2026-11-19" (or "19/11/2026") → 20261119.
pub fn parse_date(s: &str) -> Option<u32> {
    let s = s.trim();
    let (y, m, d) = if let Some((a, rest)) = s.split_once('-') {
        let (b, c) = rest.split_once('-')?;
        (a.parse::<u32>().ok()?, b.parse::<u32>().ok()?, c.parse::<u32>().ok()?)
    } else {
        let mut p = s.split('/');
        let (d, m, y) = (p.next()?.parse::<u32>().ok()?, p.next()?.parse::<u32>().ok()?, p.next()?.parse::<u32>().ok()?);
        (y, m, d)
    };
    if !(1970..=9999).contains(&y) || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some(y * 10000 + m * 100 + d)
}

pub fn fmt_date(d: u32) -> String {
    format!("{:04}-{:02}-{:02}", d / 10000, (d / 100) % 100, d % 100)
}

/// The timer's day bit (Sunday = bit 0) for a column of the week (Monday = 0).
pub fn bit_of_column(col: usize) -> u8 {
    1 << ((col + 1) % 7)
}

/// "every day", "Mon to Fri", "Mon, Wed" (short names from the system's language).
pub fn days_summary(days: u8) -> String {
    let names = [tr("Mon"), tr("Tue"), tr("Wed"), tr("Thu"), tr("Fri"), tr("Sat"), tr("Sun")];
    let on: Vec<usize> = (0..7).filter(|c| days & bit_of_column(*c) != 0).collect();
    if on.len() == 7 {
        return tr("every day").to_string();
    }
    if on.is_empty() {
        return tr("no day").to_string();
    }
    // one run of three or more days in a row: "Mon to Fri"
    if on.len() >= 3 && on.windows(2).all(|w| w[1] == w[0] + 1) {
        return trf("{a} to {b}", &[("a", names[on[0]].to_string()), ("b", names[*on.last().unwrap()].to_string())]);
    }
    on.iter().map(|c| names[*c]).collect::<Vec<_>>().join(", ")
}

/// A band in the week: which timer, which column (Monday = 0), its minutes in that day.
#[derive(Clone, Debug, PartialEq)]
pub struct Band {
    /// (session index, timer index)
    pub key: (usize, usize),
    pub col: usize,
    pub start: u32,
    pub end: u32,
    /// false for the part of a window that ran past midnight
    pub first: bool,
}

/// The bands of the timers for a week whose days are `dates` (yyyymmdd, Monday first). A window past midnight goes on
/// into the next column. Switched-off timers are drawn too (the UI dims them).
pub fn week_bands(timers: &[((usize, usize), Timer)], dates: &[u32; 7]) -> Vec<Band> {
    let mut out = vec![];
    for (key, t) in timers {
        let start = t.start as u32;
        let end = start + t.minutes.max(1) as u32;
        let cols: Vec<usize> = if t.mode == 0 {
            dates.iter().position(|d| *d == t.date).into_iter().collect()
        } else {
            (0..7).filter(|c| t.days & bit_of_column(*c) != 0).collect()
        };
        for c in cols {
            out.push(Band { key: *key, col: c, start, end: end.min(1440), first: true });
            if end > 1440 && c + 1 < 7 {
                out.push(Band { key: *key, col: c + 1, start: 0, end: (end - 1440).min(1440), first: false });
            }
        }
        // a weekly window from Sunday that spills into Monday
        if t.mode == 1 && end > 1440 && t.days & bit_of_column(6) != 0 {
            out.push(Band { key: *key, col: 0, start: 0, end: (end - 1440).min(1440), first: false });
        }
    }
    out
}

/// The hours the week shows: 08–22, widened to fit every band.
pub fn hour_range(bands: &[Band]) -> (u32, u32) {
    let mut a = 8;
    let mut b = 22;
    for x in bands {
        a = a.min(x.start / 60);
        b = b.max(x.end.div_ceil(60));
    }
    (a, b.min(24))
}

/// Minutes rounded to the nearest 15.
pub fn snap15(min: f64) -> i32 {
    ((min / 15.0).round() * 15.0) as i32
}

/// Where a minute of the day sits in the week's body (px), with `first_hour` at the top and `hour_h` px per hour.
pub fn y_of(min: u32, first_hour: u32, hour_h: f64) -> f64 {
    (min as f64 - first_hour as f64 * 60.0) / 60.0 * hour_h
}

/// The timer after its band's bottom edge was dragged by `dy` px: the new length (15-minute snaps, at least 15).
pub fn resized_length(t: &Timer, dy: f64, hour_h: f64) -> u16 {
    let end = t.start as f64 + t.minutes as f64 + dy / hour_h * 60.0;
    let end = snap15(end);
    (end - t.start as i32).clamp(15, 1440) as u16
}

/// The timer after its band was dragged by (`dcol` columns, `dy` px) from column `col`: days or date move with it,
/// the start moves in 15-minute steps (kept inside the day). `shift_date` adds days to a yyyymmdd date.
pub fn moved_timer(t: &Timer, col: usize, dcol: i32, dy: f64, hour_h: f64, shift_date: impl Fn(u32, i32) -> u32) -> Timer {
    let mut n = t.clone();
    let start = snap15(t.start as f64 + dy / hour_h * 60.0).clamp(0, 1440 - 15);
    n.start = start as u16;
    let to = (col as i32 + dcol).clamp(0, 6) as usize;
    if to != col {
        if t.mode == 0 {
            n.date = shift_date(t.date, to as i32 - col as i32);
        } else {
            n.days = (t.days & !bit_of_column(col)) | bit_of_column(to);
        }
    }
    n
}

/// "ran 14:50" from the script log: the last line where an automation ran `script`.
pub fn last_run(log: &[String], script: &str) -> Option<String> {
    let needle = format!("“{script}”");
    log.iter().rev().find(|l| l.contains(" → ") && l.contains(&needle)).and_then(|l| l.split_whitespace().next()).map(|t| t.chars().take(5).collect())
}

/// Text with a line number for each of its lines, for the gutter of the script editor.
pub fn line_numbers(text: &str) -> String {
    let n = text.split('\n').count().max(1);
    (1..=n).map(|i| i.to_string()).collect::<Vec<_>>().join("\n")
}

// ---------------------------------------------------------------- the page

const HOUR_H: f64 = 26.5;
const GUTTER: f64 = 44.0;
const SESSION_TOKENS: [&str; 6] = ["accent", "teal", "gold", "blue", "coral", "purple"];

struct Page {
    left: gtk::Box,
    right: gtk::Box,
    columns: gtk::Box,
    week_card: gtk::Box,
    today_list: gtk::Box,
    days_head: gtk::Box,
    body: gtk::Overlay,
    grid: gtk::DrawingArea,
    fixed: gtk::Fixed,
    timers: gtk::Box,
    autos: gtk::Box,
    /// the bands placed in the week (for resizing on width changes)
    bands: RefCell<Vec<(Band, gtk::Box)>>,
    hours: RefCell<Vec<(u32, gtk::Label)>>,
    range: Cell<(u32, u32)>,
    dates: RefCell<[u32; 7]>,
    today_col: Cell<usize>,
    // script editor
    script: RefCell<String>,
    script_pick: gtk::Box,
    script_title: gtk::Label,
    code: gtk::TextView,
    numbers: gtk::Label,
    save: gtk::Button,
    rename: gtk::Button,
    delete: gtk::Button,
    copy: gtk::Button,
    log: gtk::Label,
    log_len: Cell<usize>,
    loading: Cell<bool>,
}

fn builtin(name: &str) -> bool {
    crate::script::BUILTIN.iter().any(|(n, _)| *n == name)
}

/// The name shown for a script (built-ins are translated).
fn script_label(name: &str) -> String {
    if builtin(name) {
        tr(name).to_string()
    } else {
        name.to_string()
    }
}

fn token_of(session: usize) -> &'static str {
    SESSION_TOKENS[session % SESSION_TOKENS.len()]
}

/// This week's dates (Monday first) and today's column, from the local clock.
fn this_week() -> ([u32; 7], usize) {
    let now = glib::DateTime::now_local().unwrap_or_else(|_| glib::DateTime::from_unix_utc(0).unwrap());
    let col = (now.day_of_week() as usize).saturating_sub(1) % 7;
    let mut d = [0u32; 7];
    for (i, slot) in d.iter_mut().enumerate() {
        if let Ok(t) = now.add_days(i as i32 - col as i32) {
            *slot = t.year() as u32 * 10000 + t.month() as u32 * 100 + t.day_of_month() as u32;
        }
    }
    (d, col)
}

fn shift_date(d: u32, days: i32) -> u32 {
    glib::DateTime::from_local((d / 10000) as i32, ((d / 100) % 100) as i32, (d % 100) as i32, 12, 0, 0.0)
        .ok()
        .and_then(|t| t.add_days(days).ok())
        .map(|t| t.year() as u32 * 10000 + t.month() as u32 * 100 + t.day_of_month() as u32)
        .unwrap_or(d)
}

/// "Mon to Fri · 20:00 to 22:00 · stops by itself · quits the sources at the end"
fn timer_summary(t: &Timer) -> String {
    let today = crate::engine::local_date();
    let when = if t.mode == 0 {
        let past = t.date < today;
        let mut s = trf("{date} (once)", &[("date", fmt_date(t.date))]);
        if past {
            s = format!("{s} · {}", tr("already happened"));
        }
        s
    } else {
        days_summary(t.days)
    };
    let mut parts = vec![when, trf("{from} to {to}", &[("from", fmt_hm(t.start as u32)), ("to", fmt_hm(t.start as u32 + t.minutes as u32))])];
    parts.push(tr("stops by itself").to_string());
    if t.quit_sources {
        parts.push(tr("quits the sources at the end").to_string());
    }
    parts.join(" · ")
}

fn all_timers(ui: &Rc<Ui>) -> Vec<((usize, usize), Timer)> {
    let c = ui.core.borrow();
    c.sessions.iter().enumerate().flat_map(|(si, s)| s.timers.iter().enumerate().map(move |(ti, t)| ((si, ti), t.clone()))).collect()
}

fn set_timer(ui: &Rc<Ui>, key: (usize, usize), t: Timer) {
    {
        let mut c = ui.core.borrow_mut();
        if let Some(slot) = c.sessions.get_mut(key.0).and_then(|s| s.timers.get_mut(key.1)) {
            *slot = t;
        }
    }
    ui.changed(Change::Sessions);
}

// ---------------- the week

fn place_bands(page: &Rc<Page>) {
    let w = page.grid.width() as f64;
    let col_w = ((w - GUTTER) / 7.0).max(1.0);
    let (first, _) = page.range.get();
    for (b, widget) in page.bands.borrow().iter() {
        let x = GUTTER + b.col as f64 * col_w + 2.0;
        let y = y_of(b.start, first, HOUR_H).max(0.0);
        let h = (y_of(b.end, first, HOUR_H) - y).max(12.0);
        widget.set_size_request((col_w - 4.0).max(8.0) as i32, (h - 1.0) as i32);
        page.fixed.move_(widget, x, y);
    }
    for (h, l) in page.hours.borrow().iter() {
        page.fixed.move_(l, 0.0, (y_of(h * 60, first, HOUR_H) - 7.0).max(0.0));
    }
}

fn band_widget(ui: &Rc<Ui>, page: &Rc<Page>, b: &Band, name: &str, t: &Timer) -> gtk::Box {
    let token = token_of(b.key.0);
    let w = vbox(0, "ka-faixa");
    w.add_css_class(&color_class(token));
    if !t.on {
        w.add_css_class("desligada");
    }
    w.set_overflow(gtk::Overflow::Hidden);
    let body = vbox(1, "");
    body.set_vexpand(true);
    body.append(&line(name, "ka-faixa-nome"));
    if b.end - b.start >= 90 {
        body.append(&line(&format!("{}–{}", fmt_hm(t.start as u32), fmt_hm(t.start as u32 + t.minutes as u32)), "ka-faixa-hora"));
    }
    w.append(&body);
    w.set_tooltip_text(Some(&format!("{name} · {}", timer_summary(t))));
    let key = b.key;
    if b.first {
        // the bottom edge changes the end
        let handle = hbox(0, "ka-faixa-alca");
        handle.set_size_request(-1, 6);
        handle.set_cursor_from_name(Some("ns-resize"));
        w.append(&handle);
        let drag = gtk::GestureDrag::new();
        let h0 = Cell::new(0);
        {
            let w2 = w.clone();
            let h0 = h0.clone();
            drag.connect_drag_begin(move |g, _, _| {
                g.set_state(gtk::EventSequenceState::Claimed);
                h0.set(w2.height());
            });
        }
        {
            let w2 = w.clone();
            let h0 = h0.clone();
            drag.connect_drag_update(move |_, _, dy| {
                // follows the pointer in 15-minute steps
                let step = HOUR_H / 4.0;
                let h = ((h0.get() as f64 + dy) / step).round() * step;
                w2.set_size_request(w2.width_request(), h.max(step) as i32);
            });
        }
        {
            let u = ui.clone();
            let t = t.clone();
            drag.connect_drag_end(move |_, _, dy| {
                if dy.abs() < 2.0 {
                    return;
                }
                let mut n = t.clone();
                n.minutes = resized_length(&t, dy, HOUR_H);
                set_timer(&u, key, n);
            });
        }
        handle.add_controller(drag);
    }
    // dragging the band moves it (another day, another time); a click opens the editor
    let mv = gtk::GestureDrag::new();
    {
        let (pg, t, col) = (page.clone(), t.clone(), b.col);
        let w2 = w.clone();
        mv.connect_drag_update(move |_, dx, dy| {
            if dx.abs() + dy.abs() > 4.0 {
                w2.add_css_class("arrastando");
                let col_w = ((pg.grid.width() as f64 - GUTTER) / 7.0).max(1.0);
                let dcol = (dx / col_w).round();
                let step = HOUR_H / 4.0;
                if let Some(p) = w2.parent().and_downcast::<gtk::Fixed>() {
                    let (first, _) = pg.range.get();
                    let x = GUTTER + (col as f64 + dcol).clamp(0.0, 6.0) * col_w + 2.0;
                    let y = (y_of(t.start as u32, first, HOUR_H) + (dy / step).round() * step).max(0.0);
                    p.move_(&w2, x, y);
                }
            }
        });
        let w3 = w.clone();
        let (u, pg, t) = (ui.clone(), page.clone(), t.clone());
        mv.connect_drag_end(move |_, dx, dy| {
            w3.remove_css_class("arrastando");
            if dx.abs() + dy.abs() <= 4.0 {
                timer_dialog(&u, Some(key));
                return;
            }
            let col_w = ((pg.grid.width() as f64 - GUTTER) / 7.0).max(1.0);
            let dcol = (dx / col_w).round() as i32;
            let n = moved_timer(&t, col, dcol, dy, HOUR_H, shift_date);
            set_timer(&u, key, n);
        });
    }
    body.add_controller(mv);
    body.set_cursor_from_name(Some("grab"));
    w
}

fn refresh_week(ui: &Rc<Ui>, page: &Rc<Page>) {
    let (dates, today_col) = this_week();
    *page.dates.borrow_mut() = dates;
    page.today_col.set(today_col);
    // the day names
    clear(&page.days_head);
    let g = hbox(0, "");
    g.set_size_request(GUTTER as i32, -1);
    page.days_head.append(&g);
    let names = [tr("Mon"), tr("Tue"), tr("Wed"), tr("Thu"), tr("Fri"), tr("Sat"), tr("Sun")];
    for (i, d) in dates.iter().enumerate() {
        let l = label(&format!("{} {}", names[i], d % 100), if i == today_col { "ka-semana-dia hoje" } else { "ka-semana-dia" });
        l.set_xalign(0.5);
        l.set_hexpand(true);
        page.days_head.append(&l);
    }
    // the bands
    while let Some(c) = page.fixed.first_child() {
        page.fixed.remove(&c);
    }
    page.bands.borrow_mut().clear();
    page.hours.borrow_mut().clear();
    let timers = all_timers(ui);
    let bands = week_bands(&timers, &dates);
    let range = hour_range(&bands);
    page.range.set(range);
    page.grid.set_content_height(((range.1 - range.0) as f64 * HOUR_H) as i32 + 8);
    for h in (range.0..=range.1).step_by(2) {
        let l = label(&format!("{h:02}:00"), "ka-fraco2 ka-tnum");
        page.fixed.put(&l, 0.0, 0.0);
        page.hours.borrow_mut().push((h, l));
    }
    let names: Vec<String> = ui.core.borrow().sessions.iter().map(|s| s.name.clone()).collect();
    for b in &bands {
        let t = &timers.iter().find(|x| x.0 == b.key).unwrap().1;
        let w = band_widget(ui, page, b, names.get(b.key.0).map(String::as_str).unwrap_or(""), t);
        page.fixed.put(&w, 0.0, 0.0);
        page.bands.borrow_mut().push((b.clone(), w));
    }
    place_bands(page);
    page.grid.queue_draw();

    // narrow: today's list
    clear(&page.today_list);
    let mut today: Vec<&Band> = bands.iter().filter(|b| b.col == today_col).collect();
    today.sort_by_key(|b| b.start);
    page.today_list.append(&caps(&format!("{} · {} {}", tr("Today"), names_day(today_col), dates[today_col] % 100)));
    if today.is_empty() {
        page.today_list.append(&label(tr("Nothing scheduled today."), "ka-fraco"));
    }
    for b in today {
        let t = &timers.iter().find(|x| x.0 == b.key).unwrap().1;
        let r = hbox(10, "ka-hoje-linha");
        r.add_css_class(&color_class(token_of(b.key.0)));
        r.append(&label(&format!("{}–{}", fmt_hm(b.start), fmt_hm(b.end)), "ka-texto2 ka-tnum"));
        r.append(&line(names.get(b.key.0).map(String::as_str).unwrap_or(""), "ka-titulo3"));
        if !t.on {
            r.append(&label(tr("off"), "ka-fraco2"));
        }
        page.today_list.append(&r);
    }
}

fn names_day(col: usize) -> &'static str {
    [tr("Mon"), tr("Tue"), tr("Wed"), tr("Thu"), tr("Fri"), tr("Sat"), tr("Sun")][col]
}

fn draw_grid(page: &Page, cr: &gtk::cairo::Context, w: f64, h: f64) {
    let (first, last) = page.range.get();
    let col_w = ((w - GUTTER) / 7.0).max(1.0);
    // today's column
    let tc = page.today_col.get() as f64;
    super::theme::source(cr, "accent", 0.04);
    cr.rectangle(GUTTER + tc * col_w, 0.0, col_w, h);
    let _ = cr.fill();
    super::theme::source(cr, "border", 1.0);
    cr.set_line_width(1.0);
    for hr in (first..=last).step_by(2) {
        let y = y_of(hr * 60, first, HOUR_H).round() + 0.5;
        cr.move_to(GUTTER, y);
        cr.line_to(w, y);
    }
    for c in 1..7 {
        let x = (GUTTER + c as f64 * col_w).round() + 0.5;
        cr.move_to(x, 0.0);
        cr.line_to(x, h);
    }
    let _ = cr.stroke();
    // now
    let (_, min) = crate::engine::local_now();
    if min >= first * 60 && min <= last * 60 {
        let y = y_of(min, first, HOUR_H);
        super::theme::source(cr, "red", 1.0);
        let x0 = GUTTER + tc * col_w;
        cr.rectangle(x0, y - 1.0, col_w, 2.0);
        let _ = cr.fill();
        cr.arc(x0, y, 4.0, 0.0, std::f64::consts::TAU);
        let _ = cr.fill();
    }
}

// ---------------- timers

fn refresh_timers(ui: &Rc<Ui>, page: &Rc<Page>) {
    clear(&page.timers);
    let timers = all_timers(ui);
    let names: Vec<String> = ui.core.borrow().sessions.iter().map(|s| s.name.clone()).collect();
    if timers.is_empty() {
        let e = para(tr("No schedules yet. “New schedule” starts a session at a time and stops it at another."), "ka-fraco");
        page.timers.append(&e);
    }
    for (key, t) in timers {
        let r = hbox(14, "ka-cartao ka-horario");
        if !t.on {
            r.add_css_class("desligado");
        }
        r.append(&icons::icon("relogio", if t.on { "text2" } else { "dim" }, 16));
        let txt = vbox(1, "");
        txt.set_hexpand(true);
        txt.set_valign(Align::Center);
        txt.append(&line(names.get(key.0).map(String::as_str).unwrap_or(""), "ka-titulo3"));
        txt.append(&line(&timer_summary(&t), "ka-fraco"));
        r.append(&txt);
        let sw = switch(t.on);
        {
            let (u, t) = (ui.clone(), t.clone());
            sw.connect_state_set(move |_, on| {
                let mut n = t.clone();
                n.on = on;
                set_timer(&u, key, n);
                glib::Propagation::Proceed
            });
        }
        r.append(&sw);
        let click = gtk::GestureClick::new();
        {
            let u = ui.clone();
            let sw = sw.clone();
            click.connect_released(move |g, _, x, y| {
                if let Some(w) = g.widget() {
                    if let Some(p) = w.compute_point(&sw, &gtk::graphene::Point::new(x as f32, y as f32)) {
                        if p.x() >= -4.0 && p.y() >= -4.0 && p.x() <= sw.width() as f32 + 4.0 && p.y() <= sw.height() as f32 + 4.0 {
                            return;
                        }
                    }
                }
                timer_dialog(&u, Some(key));
            });
        }
        r.add_controller(click);
        r.set_cursor_from_name(Some("pointer"));
        page.timers.append(&r);
    }
}

/// The editor of a timer (`None`: a new one).
fn timer_dialog(ui: &Rc<Ui>, key: Option<(usize, usize)>) {
    let sessions: Vec<String> = ui.core.borrow().sessions.iter().map(|s| s.name.clone()).collect();
    if sessions.is_empty() {
        ui.toast(tr("Create a session first: a schedule starts and stops a session."), None);
        return;
    }
    let today = crate::engine::local_date();
    let t = key
        .and_then(|k| ui.core.borrow().sessions.get(k.0).and_then(|s| s.timers.get(k.1)).cloned())
        .unwrap_or(Timer { days: 0b0111110, start: 20 * 60, minutes: 60, on: true, mode: 1, date: today, quit_sources: false });
    let d = vbox(14, "ka-horario-editor");
    d.append(&label(if key.is_some() { tr("Edit schedule") } else { tr("New schedule") }, "ka-titulo2"));

    let field = |title: &str, w: &gtk::Widget| {
        let b = vbox(6, "");
        b.append(&caps(title));
        b.append(w);
        b
    };
    let sess = dropdown(&sessions, key.map(|k| k.0).unwrap_or(ui.core.borrow().cur.min(sessions.len() - 1)));
    d.append(&field(tr("Session"), sess.upcast_ref()));
    let modes = vec![tr("Repeat on these days").to_string(), tr("On a date").to_string()];
    let mode = dropdown(&modes, if t.mode == 0 { 1 } else { 0 });
    d.append(&field(tr("When"), mode.upcast_ref()));
    let days_row = hbox(6, "");
    let mut toggles = vec![];
    for c in 0..7 {
        let b = gtk::ToggleButton::with_label(names_day(c));
        b.add_css_class("ka-filtro");
        b.set_active(t.days & bit_of_column(c) != 0);
        days_row.append(&b);
        toggles.push(b);
    }
    d.append(&days_row);
    let date = entry("2026-11-19", None);
    date.set_text(&fmt_date(if t.date > 0 { t.date } else { today }));
    d.append(&date);
    let times = hbox(12, "");
    let from = entry("20:00", None);
    from.set_text(&fmt_hm(t.start as u32));
    let to = entry("22:00", None);
    to.set_text(&fmt_hm(to_of(t.start, t.minutes) as u32));
    let ff = field(tr("From"), from.upcast_ref());
    ff.set_hexpand(true);
    let tf = field(tr("To"), to.upcast_ref());
    tf.set_hexpand(true);
    times.append(&ff);
    times.append(&tf);
    d.append(&times);
    let quit = gtk::CheckButton::with_label(tr("Quit the sources when done (the apps the session records)"));
    quit.set_active(t.quit_sources);
    d.append(&quit);
    let err = label("", "ka-fraco cor-red");
    err.set_visible(false);
    d.append(&err);
    let sync_mode = {
        let (days_row, date, mode) = (days_row.clone(), date.clone(), mode.clone());
        move || {
            let weekly = mode.selected() == 0;
            days_row.set_visible(weekly);
            date.set_visible(!weekly);
        }
    };
    sync_mode();
    mode.connect_selected_notify(move |_| sync_mode());

    let row = hbox(8, "");
    let del = button(tr("Delete"), "fantasma", Some("lixeira"));
    del.set_visible(key.is_some());
    row.append(&del);
    row.append(&spacer());
    let cancel = button(tr("Cancel"), "fantasma", None);
    let save = button(tr("Save"), "primario", None);
    row.append(&cancel);
    row.append(&save);
    d.append(&row);
    let close = modal(ui, &d, 460);
    {
        let close = close.clone();
        cancel.connect_clicked(move |_| close());
    }
    {
        let (u, close) = (ui.clone(), close.clone());
        del.connect_clicked(move |_| {
            if let Some(k) = key {
                let removed = {
                    let mut c = u.core.borrow_mut();
                    c.sessions.get_mut(k.0).filter(|s| k.1 < s.timers.len()).map(|s| s.timers.remove(k.1))
                };
                u.changed(Change::Sessions);
                if let Some(t) = removed {
                    let back = RefCell::new(Some(t));
                    u.toast(
                        tr("Schedule deleted"),
                        Some((
                            tr("Undo"),
                            Box::new(move |u: &Rc<Ui>| {
                                if let Some(t) = back.borrow_mut().take() {
                                    if let Some(s) = u.core.borrow_mut().sessions.get_mut(k.0) {
                                        let i = k.1.min(s.timers.len());
                                        s.timers.insert(i, t);
                                    }
                                    u.changed(Change::Sessions);
                                }
                            }),
                        )),
                    );
                }
            }
            close();
        });
    }
    save.connect_clicked({
        let (u, close) = (ui.clone(), close.clone());
        move |_| {
            let (Some(a), Some(b)) = (parse_hm(&from.text()), parse_hm(&to.text())) else {
                err.set_text(tr("Write the times as 20:00."));
                err.set_visible(true);
                return;
            };
            let weekly = mode.selected() == 0;
            let mut days = 0u8;
            for (c, b) in toggles.iter().enumerate() {
                if b.is_active() {
                    days |= bit_of_column(c);
                }
            }
            let dt = parse_date(&date.text());
            if weekly && days == 0 {
                err.set_text(tr("Choose at least one day."));
                err.set_visible(true);
                return;
            }
            if !weekly && dt.is_none() {
                err.set_text(tr("Write the date as 2026-11-19."));
                err.set_visible(true);
                return;
            }
            let a = a.min(1439);
            let n = Timer {
                days: if days == 0 { t.days } else { days },
                start: a,
                minutes: length_between(a, b % 1440),
                on: t.on,
                mode: if weekly { 1 } else { 0 },
                date: dt.unwrap_or(t.date),
                quit_sources: quit.is_active(),
            };
            let target = sess.selected() as usize;
            {
                let mut c = u.core.borrow_mut();
                match key {
                    Some(k) if k.0 == target => {
                        if let Some(slot) = c.sessions.get_mut(k.0).and_then(|s| s.timers.get_mut(k.1)) {
                            *slot = n;
                        }
                    }
                    other => {
                        if let Some(k) = other {
                            if let Some(s) = c.sessions.get_mut(k.0) {
                                if k.1 < s.timers.len() {
                                    s.timers.remove(k.1);
                                }
                            }
                        }
                        if let Some(s) = c.sessions.get_mut(target) {
                            s.timers.push(n);
                        }
                    }
                }
            }
            u.changed(Change::Sessions);
            close();
        }
    });
}

// ---------------- automations

fn chip_drop(items: &[String], selected: usize, token: &str) -> gtk::DropDown {
    let d = dropdown(items, selected);
    d.remove_css_class("ka-seletor");
    d.add_css_class("ka-chip-seletor");
    d.add_css_class(&color_class(token));
    d.set_halign(Align::Start);
    d
}

fn script_names(ui: &Rc<Ui>) -> Vec<String> {
    let mut v: Vec<String> = crate::script::BUILTIN.iter().map(|(n, _)| n.to_string()).collect();
    v.extend(ui.core.borrow().store.scripts.iter().map(|s| s.name.clone()));
    v
}

/// (session index, automation index, automation)
type AutoRow = (usize, usize, Automation);

fn refresh_autos(ui: &Rc<Ui>, page: &Rc<Page>) {
    clear(&page.autos);
    let (sessions, list, log): (Vec<String>, Vec<AutoRow>, Vec<String>) = {
        let c = ui.core.borrow();
        (
            c.sessions.iter().map(|s| s.name.clone()).collect(),
            c.sessions.iter().enumerate().flat_map(|(si, s)| s.automations.iter().enumerate().map(move |(ai, a)| (si, ai, a.clone()))).collect(),
            c.log.clone(),
        )
    };
    if list.is_empty() {
        page.autos.append(&para(tr("No automations yet. An automation runs a script when something happens: a recording finishes, a session starts or stops, a schedule begins or ends."), "ka-fraco"));
    }
    let scripts = script_names(ui);
    let mut script_items: Vec<String> = vec![tr("Choose a script").to_string()];
    script_items.extend(scripts.iter().map(|n| script_label(n)));
    let events: Vec<String> = EVENTS.iter().map(|e| tr(e.1).to_string()).collect();
    for (si, ai, a) in list {
        let card = vbox(0, "ka-cartao ka-automacao");
        if !a.on {
            card.add_css_class("desligada");
        }
        let grid = gtk::Grid::new();
        grid.set_row_spacing(6);
        grid.set_column_spacing(12);
        let ev_i = EVENTS.iter().position(|e| e.0 == a.event).unwrap_or(0);
        let ev = chip_drop(&events, ev_i, "accent");
        let ss = chip_drop(&sessions, si, "muted");
        let sc_i = scripts.iter().position(|n| *n == a.script).map(|i| i + 1).unwrap_or(0);
        let sc = chip_drop(&script_items, sc_i, "teal");
        for (r, (cap, w)) in [(tr("When"), ev.upcast_ref::<gtk::Widget>()), (tr("In"), ss.upcast_ref()), (tr("Do"), sc.upcast_ref())].into_iter().enumerate() {
            let l = caps(cap);
            l.set_valign(Align::Center);
            l.set_size_request(52, -1);
            grid.attach(&l, 0, r as i32, 1, 1);
            grid.attach(w, 1, r as i32, 1, 1);
        }
        grid.set_hexpand(true);
        let top = hbox(8, "");
        top.append(&grid);
        let side = vbox(6, "");
        let sw = switch(a.on);
        sw.set_halign(Align::End);
        side.append(&sw);
        let del = icon_button("lixeira", "dim", 14, tr("Delete automation"));
        del.set_halign(Align::End);
        side.append(&del);
        side.append(&spacer());
        if let Some(t) = last_run(&log, &a.script) {
            let ran = label(&trf("ran {time}", &[("time", t)]), "ka-fraco2");
            ran.set_halign(Align::End);
            ran.set_valign(Align::End);
            side.set_vexpand(true);
            ran.set_vexpand(true);
            side.append(&ran);
        }
        top.append(&side);
        card.append(&top);
        let key = (si, ai);
        let edit = {
            let u = ui.clone();
            move |f: &dyn Fn(&mut Automation)| {
                if let Some(a) = u.core.borrow_mut().sessions.get_mut(key.0).and_then(|s| s.automations.get_mut(key.1)) {
                    f(a);
                }
            }
        };
        {
            let edit = edit.clone();
            let card = card.clone();
            sw.connect_state_set(move |_, on| {
                edit(&|a| a.on = on);
                if on {
                    card.remove_css_class("desligada");
                } else {
                    card.add_css_class("desligada");
                }
                glib::Propagation::Proceed
            });
        }
        {
            let edit = edit.clone();
            ev.connect_selected_notify(move |d| {
                let e = EVENTS.get(d.selected() as usize).map(|e| e.0.to_string()).unwrap_or_default();
                edit(&|a| a.event = e.clone());
            });
        }
        {
            let (edit, scripts, pg, u) = (edit.clone(), scripts.clone(), page.clone(), ui.clone());
            sc.connect_selected_notify(move |d| {
                let i = d.selected() as usize;
                let name = if i == 0 { String::new() } else { scripts.get(i - 1).cloned().unwrap_or_default() };
                edit(&|a| a.script = name.clone());
                if !name.is_empty() {
                    open_script(&u, &pg, &name);
                }
            });
        }
        {
            let u = ui.clone();
            ss.connect_selected_notify(move |d| {
                let to = d.selected() as usize;
                if to == key.0 {
                    return;
                }
                {
                    let mut c = u.core.borrow_mut();
                    let moved = c.sessions.get_mut(key.0).filter(|s| key.1 < s.automations.len()).map(|s| s.automations.remove(key.1));
                    if let (Some(a), Some(s)) = (moved, c.sessions.get_mut(to)) {
                        s.automations.push(a);
                    }
                }
                let u2 = u.clone();
                glib::idle_add_local_once(move || u2.changed(Change::Sessions));
            });
        }
        {
            let u = ui.clone();
            del.connect_clicked(move |_| {
                let removed = {
                    let mut c = u.core.borrow_mut();
                    c.sessions.get_mut(key.0).filter(|s| key.1 < s.automations.len()).map(|s| s.automations.remove(key.1))
                };
                u.changed(Change::Sessions);
                if let Some(a) = removed {
                    let back = RefCell::new(Some(a));
                    u.toast(
                        tr("Automation deleted"),
                        Some((
                            tr("Undo"),
                            Box::new(move |u: &Rc<Ui>| {
                                if let Some(a) = back.borrow_mut().take() {
                                    if let Some(s) = u.core.borrow_mut().sessions.get_mut(key.0) {
                                        let i = key.1.min(s.automations.len());
                                        s.automations.insert(i, a);
                                    }
                                    u.changed(Change::Sessions);
                                }
                            }),
                        )),
                    );
                }
            });
        }
        page.autos.append(&card);
    }
}

// ---------------- the script editor

fn refresh_script_pick(ui: &Rc<Ui>, page: &Rc<Page>) {
    clear(&page.script_pick);
    let names = script_names(ui);
    let items: Vec<String> = names.iter().map(|n| script_label(n)).collect();
    let cur = page.script.borrow().clone();
    let d = dropdown(&items, names.iter().position(|n| *n == cur).unwrap_or(0));
    d.set_hexpand(true);
    let (u, pg) = (ui.clone(), page.clone());
    d.connect_selected_notify(move |d| {
        if let Some(n) = names.get(d.selected() as usize) {
            open_script(&u, &pg, n);
        }
    });
    page.script_pick.append(&d);
}

fn open_script(ui: &Rc<Ui>, page: &Rc<Page>, name: &str) {
    let text = ui.core.borrow().script_text(name).unwrap_or_default();
    *page.script.borrow_mut() = name.to_string();
    let b = builtin(name);
    page.script_title.set_text(&format!("{} · {}", tr("Script").to_uppercase(), script_label(name)));
    page.loading.set(true);
    page.code.buffer().set_text(&text);
    page.loading.set(false);
    page.numbers.set_text(&line_numbers(&text));
    page.code.set_editable(!b);
    page.save.set_visible(!b);
    page.save.set_sensitive(false);
    page.rename.set_visible(!b);
    page.delete.set_visible(!b);
    page.copy.set_visible(b);
    refresh_script_pick_selection(page, name);
}

fn refresh_script_pick_selection(page: &Rc<Page>, _name: &str) {
    // the drop-down is rebuilt when the list changes; keep it in step with the open script
    let _ = &page.script_pick;
}

fn unique_name(ui: &Rc<Ui>, base: &str) -> String {
    let taken = script_names(ui);
    let mut n = base.to_string();
    let mut i = 2;
    while taken.contains(&n) {
        n = format!("{base} {i}");
        i += 1;
    }
    n
}

fn save_script(ui: &Rc<Ui>, page: &Rc<Page>) {
    let name = page.script.borrow().clone();
    if builtin(&name) {
        return;
    }
    let buf = page.code.buffer();
    let text = buf.text(&buf.start_iter(), &buf.end_iter(), false).to_string();
    {
        let mut c = ui.core.borrow_mut();
        match c.store.scripts.iter_mut().find(|s| s.name == name) {
            Some(s) => s.text = text,
            None => c.store.scripts.push(Script { name: name.clone(), text }),
        }
    }
    page.save.set_sensitive(false);
    ui.changed(Change::Store);
}

fn rename_script(ui: &Rc<Ui>, page: &Rc<Page>) {
    let old = page.script.borrow().clone();
    let d = vbox(12, "");
    d.append(&label(tr("Rename script"), "ka-titulo2"));
    let e = entry(tr("Name"), None);
    e.set_text(&old);
    d.append(&e);
    let row = hbox(8, "");
    row.set_halign(Align::End);
    let cancel = button(tr("Cancel"), "fantasma", None);
    let ok = button(tr("Rename"), "primario", None);
    row.append(&cancel);
    row.append(&ok);
    d.append(&row);
    let close = modal(ui, &d, 400);
    {
        let close = close.clone();
        cancel.connect_clicked(move |_| close());
    }
    let go = {
        let (u, pg, e, close) = (ui.clone(), page.clone(), e.clone(), close.clone());
        Rc::new(move || {
            let new = e.text().trim().to_string();
            if new.is_empty() || new == old {
                close();
                return;
            }
            let new = unique_name(&u, &new);
            {
                let mut c = u.core.borrow_mut();
                if let Some(s) = c.store.scripts.iter_mut().find(|s| s.name == old) {
                    s.name = new.clone();
                }
                // automations follow the new name
                for s in c.sessions.iter_mut() {
                    for a in s.automations.iter_mut().filter(|a| a.script == old) {
                        a.script = new.clone();
                    }
                }
            }
            *pg.script.borrow_mut() = new;
            close();
            u.changed(Change::Store);
            u.changed(Change::Sessions);
        })
    };
    {
        let go = go.clone();
        ok.connect_clicked(move |_| go());
    }
    e.connect_activate(move |_| go());
    e.grab_focus();
}

fn refresh_log(ui: &Rc<Ui>, page: &Rc<Page>) {
    let c = ui.core.borrow();
    page.log_len.set(c.log.len());
    let lines: Vec<&str> = c.log.iter().rev().take(6).rev().map(String::as_str).collect();
    page.log.set_text(&if lines.is_empty() { tr("Nothing ran yet.").to_string() } else { lines.join("\n") });
}

// ---------------- layout

fn arrange(ui: &Rc<Ui>, page: &Rc<Page>) {
    let l = ui.layout();
    page.columns.set_orientation(if l == Layout::Wide { gtk::Orientation::Horizontal } else { gtk::Orientation::Vertical });
    page.columns.set_spacing(if l == Layout::Wide { 32 } else { 28 });
    // wide: the week gets the room, the automations a column of their own
    page.right.set_hexpand(l != Layout::Wide);
    page.right.set_size_request(if l == Layout::Wide { 540 } else { -1 }, -1);
    page.week_card.set_visible(l != Layout::Narrow);
    page.today_list.set_visible(l == Layout::Narrow);
    let _ = &page.left;
}

fn refresh(ui: &Rc<Ui>, page: &Rc<Page>) {
    refresh_week(ui, page);
    refresh_timers(ui, page);
    refresh_autos(ui, page);
    refresh_log(ui, page);
}

/// Older saved automations name built-in scripts in Portuguese: they now use the English names.
fn migrate_script_names(ui: &Rc<Ui>) {
    let mut c = ui.core.borrow_mut();
    for s in c.sessions.iter_mut() {
        for a in s.automations.iter_mut() {
            let n = crate::script::current_name(&a.script).to_string();
            if n != a.script {
                a.script = n;
            }
        }
    }
}

pub fn page(ui: &Rc<Ui>) -> gtk::Widget {
    migrate_script_names(ui);
    let root = vbox(0, "ka-pagina ka-agenda");
    let columns = hbox(32, "");
    root.append(&columns);

    // ---- left: the schedule
    let left = vbox(0, "");
    left.set_hexpand(true);
    let head = hbox(12, "");
    let titles = vbox(4, "");
    titles.set_hexpand(true);
    titles.append(&label(tr("Schedule"), "ka-titulo"));
    titles.append(&para(tr("Sessions that start and stop by themselves."), "ka-fraco"));
    head.append(&titles);
    let new_timer = button(tr("New schedule"), "primario", Some("mais"));
    new_timer.set_valign(Align::Start);
    head.append(&new_timer);
    left.append(&head);

    let week_card = vbox(0, "ka-cartao ka-semana");
    week_card.set_margin_top(16);
    let days_head = hbox(0, "ka-semana-cabeca");
    week_card.append(&days_head);
    let grid = gtk::DrawingArea::new();
    grid.set_hexpand(true);
    grid.add_css_class("ka-onda");
    let fixed = gtk::Fixed::new();
    fixed.set_overflow(gtk::Overflow::Hidden);
    let body = gtk::Overlay::new();
    body.set_child(Some(&grid));
    body.add_overlay(&fixed);
    week_card.append(&body);
    left.append(&week_card);
    let today_list = vbox(6, "ka-hoje");
    today_list.set_margin_top(16);
    left.append(&today_list);
    let tcap = caps(tr("Schedules"));
    tcap.set_margin_top(20);
    tcap.set_margin_bottom(8);
    left.append(&tcap);
    let timers = vbox(8, "");
    left.append(&timers);
    columns.append(&left);

    // ---- right: automations and scripts
    let right = vbox(0, "");
    right.set_hexpand(true);
    let rt = vbox(4, "");
    rt.append(&label(tr("Automations"), "ka-titulo"));
    rt.append(&para(tr("When something happens, do this. Only this app’s actions; nothing is deleted without going to the Trash."), "ka-fraco"));
    right.append(&rt);
    let autos = vbox(8, "");
    autos.set_margin_top(16);
    right.append(&autos);
    let arow = hbox(8, "");
    arow.set_margin_top(10);
    let new_auto = button(tr("New automation"), "secundario", Some("mais"));
    let write = gtk::MenuButton::new();
    write.add_css_class("ka-botao");
    write.add_css_class("ka-fantasma");
    {
        let r = hbox(6, "");
        r.append(&icons::icon("spark", "accent", 14));
        r.append(&gtk::Label::new(Some(tr("Write with the agent"))));
        write.set_child(Some(&r));
    }
    let wpop = gtk::Popover::new();
    let wbox = vbox(8, "");
    wbox.set_size_request(320, -1);
    wbox.append(&label(tr("Describe the automation"), "ka-titulo3"));
    let wentry = entry(tr("e.g. when a recording finishes, transcribe it"), Some("spark"));
    wbox.append(&wentry);
    wpop.set_child(Some(&wbox));
    write.set_popover(Some(&wpop));
    arow.append(&new_auto);
    arow.append(&write);
    right.append(&arow);

    // the script editor
    let shead = hbox(8, "");
    shead.set_margin_top(22);
    shead.set_margin_bottom(8);
    let script_title = label("", "ka-caps");
    script_title.set_hexpand(true);
    script_title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    shead.append(&script_title);
    let help = icon_button("info", "muted", 14, tr("Script help"));
    shead.append(&help);
    right.append(&shead);
    let tools = hbox(6, "");
    let script_pick = hbox(0, "");
    script_pick.set_hexpand(true);
    tools.append(&script_pick);
    let new_script = icon_button("mais", "text2", 14, tr("New script"));
    let rename = icon_button("lapis", "text2", 14, tr("Rename script"));
    let delete = icon_button("lixeira", "text2", 14, tr("Delete script"));
    tools.append(&new_script);
    tools.append(&rename);
    tools.append(&delete);
    right.append(&tools);
    let editor = vbox(0, "ka-poco ka-script");
    editor.set_margin_top(8);
    let code_row = hbox(10, "");
    let numbers = label("1", "ka-mono ka-script-linhas");
    numbers.set_xalign(1.0);
    numbers.set_yalign(0.0);
    numbers.set_valign(Align::Start);
    let code = gtk::TextView::new();
    code.add_css_class("ka-codigo");
    code.set_monospace(true);
    code.set_hexpand(true);
    code.set_wrap_mode(gtk::WrapMode::None);
    code.set_accepts_tab(true);
    code_row.append(&numbers);
    code_row.append(&code);
    let csc = gtk::ScrolledWindow::new();
    csc.set_child(Some(&code_row));
    csc.set_min_content_height(200);
    csc.set_max_content_height(320);
    csc.set_propagate_natural_height(true);
    editor.append(&csc);
    let erow = hbox(10, "");
    erow.set_margin_top(10);
    let run = button(tr("Run now"), "secundario", Some("tocar"));
    erow.append(&run);
    let api = para(tr("API: kalango.* and ctx.* · runs isolated, without network"), "ka-fraco2");
    erow.append(&api);
    let save = button(tr("Save"), "primario", None);
    let copy = button(tr("Copy to edit"), "fantasma", Some("copiar"));
    erow.append(&copy);
    erow.append(&save);
    editor.append(&erow);
    right.append(&editor);
    let lcap = caps(tr("Log"));
    lcap.set_margin_top(16);
    lcap.set_margin_bottom(6);
    right.append(&lcap);
    let log = label("", "ka-mono ka-fraco ka-script-registro");
    log.set_wrap(true);
    log.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    log.set_selectable(true);
    log.set_max_width_chars(1);
    log.set_hexpand(true);
    right.append(&log);
    columns.append(&right);

    let page = Rc::new(Page {
        left,
        right,
        columns,
        week_card,
        today_list,
        days_head,
        body,
        grid: grid.clone(),
        fixed,
        timers,
        autos,
        bands: RefCell::new(vec![]),
        hours: RefCell::new(vec![]),
        range: Cell::new((8, 22)),
        dates: RefCell::new([0; 7]),
        today_col: Cell::new(0),
        script: RefCell::new(String::new()),
        script_pick,
        script_title,
        code: code.clone(),
        numbers,
        save: save.clone(),
        rename: rename.clone(),
        delete: delete.clone(),
        copy: copy.clone(),
        log,
        log_len: Cell::new(usize::MAX),
        loading: Cell::new(false),
    });
    let _ = (&page.body, &page.dates);

    {
        let p = Rc::downgrade(&page);
        grid.set_draw_func(move |_, cr, w, h| {
            if let Some(p) = p.upgrade() {
                draw_grid(&p, cr, w as f64, h as f64);
            }
        });
        follow_theme(&grid);
        let p = Rc::downgrade(&page);
        grid.connect_resize(move |_, _, _| {
            if let Some(p) = p.upgrade() {
                // after this layout pass
                glib::idle_add_local_once(move || place_bands(&p));
            }
        });
    }
    {
        let u = ui.clone();
        new_timer.connect_clicked(move |_| timer_dialog(&u, None));
    }
    {
        let u = ui.clone();
        new_auto.connect_clicked(move |_| {
            let added = {
                let mut c = u.core.borrow_mut();
                let cur = c.cur.min(c.sessions.len().saturating_sub(1));
                match c.sessions.get_mut(cur) {
                    Some(s) => {
                        s.automations.push(Automation::new());
                        true
                    }
                    None => false,
                }
            };
            if added {
                u.changed(Change::Sessions);
            } else {
                u.toast(tr("Create a session first: an automation belongs to a session."), None);
            }
        });
    }
    {
        let (u, pop) = (ui.clone(), wpop.clone());
        wentry.connect_activate(move |e| {
            let text = e.text().trim().to_string();
            if text.is_empty() {
                return;
            }
            e.set_text("");
            pop.popdown();
            super::agente::ask(&u, &trf("write an automation: {request}", &[("request", text)]));
        });
    }
    {
        let u = ui.clone();
        help.connect_clicked(move |_| script_help(&u));
    }
    {
        let p = page.clone();
        code.buffer().connect_changed(move |b| {
            let text = b.text(&b.start_iter(), &b.end_iter(), false);
            p.numbers.set_text(&line_numbers(&text));
            if !p.loading.get() {
                p.save.set_sensitive(true);
            }
        });
    }
    {
        let (u, p) = (ui.clone(), page.clone());
        save.connect_clicked(move |_| save_script(&u, &p));
    }
    {
        let (u, p) = (ui.clone(), page.clone());
        new_script.connect_clicked(move |_| {
            let name = unique_name(&u, tr("My script"));
            let text = format!("// {}\n// needs: recording\n\nif (ctx.recording)\n  kalango.log(ctx.recording.name);\n", tr("What this script does."));
            u.core.borrow_mut().store.scripts.push(Script { name: name.clone(), text });
            *p.script.borrow_mut() = name;
            u.changed(Change::Store);
        });
    }
    {
        let (u, p) = (ui.clone(), page.clone());
        copy.connect_clicked(move |_| {
            let from = p.script.borrow().clone();
            let text = u.core.borrow().script_text(&from).unwrap_or_default();
            let name = unique_name(&u, &trf("{name} (copy)", &[("name", script_label(&from))]));
            u.core.borrow_mut().store.scripts.push(Script { name: name.clone(), text });
            *p.script.borrow_mut() = name;
            u.changed(Change::Store);
        });
    }
    {
        let (u, p) = (ui.clone(), page.clone());
        rename.connect_clicked(move |_| rename_script(&u, &p));
    }
    {
        let (u, p) = (ui.clone(), page.clone());
        delete.connect_clicked(move |_| {
            let name = p.script.borrow().clone();
            let removed = {
                let mut c = u.core.borrow_mut();
                let i = c.store.scripts.iter().position(|s| s.name == name);
                i.map(|i| (i, c.store.scripts.remove(i)))
            };
            if let Some((i, s)) = removed {
                *p.script.borrow_mut() = crate::script::BUILTIN[0].0.to_string();
                u.changed(Change::Store);
                let back = RefCell::new(Some((i, s)));
                u.toast(
                    &trf("“{name}” deleted", &[("name", name)]),
                    Some((
                        tr("Undo"),
                        Box::new(move |u: &Rc<Ui>| {
                            if let Some((i, s)) = back.borrow_mut().take() {
                                let mut c = u.core.borrow_mut();
                                let i = i.min(c.store.scripts.len());
                                c.store.scripts.insert(i, s);
                                drop(c);
                                u.changed(Change::Store);
                            }
                        }),
                    )),
                );
            }
        });
    }
    {
        let (u, p) = (ui.clone(), page.clone());
        run.connect_clicked(move |_| {
            if p.save.is_visible() && p.save.is_sensitive() {
                save_script(&u, &p);
            }
            let name = p.script.borrow().clone();
            u.core.borrow_mut().run_script_now(&name);
            refresh_log(&u, &p);
        });
    }

    // the first script shown: the first automation's, else the person's first, else the first built-in
    let first = {
        let c = ui.core.borrow();
        c.sessions
            .iter()
            .flat_map(|s| s.automations.iter())
            .map(|a| a.script.clone())
            .find(|n| !n.is_empty() && c.script_text(n).is_some())
            .or_else(|| c.store.scripts.first().map(|s| s.name.clone()))
            .unwrap_or_else(|| crate::script::BUILTIN[0].0.to_string())
    };
    *page.script.borrow_mut() = first.clone();
    refresh_script_pick(ui, &page);
    open_script(ui, &page, &first);
    refresh(ui, &page);
    arrange(ui, &page);
    {
        let p = page.clone();
        ui.on_change(move |ui, c| match c {
            Change::Sessions | Change::Running => {
                refresh(ui, &p);
            }
            Change::Store => {
                let cur = p.script.borrow().clone();
                let cur = if ui.core.borrow().script_text(&cur).is_some() { cur } else { crate::script::BUILTIN[0].0.to_string() };
                refresh_script_pick(ui, &p);
                if p.save.is_sensitive() && *p.script.borrow() == cur {
                    // unsaved edits stay in the editor
                } else {
                    open_script(ui, &p, &cur);
                }
                refresh_autos(ui, &p);
            }
            Change::Layout => arrange(ui, &p),
            _ => {}
        });
    }
    {
        let p = page.clone();
        ui.on_page(move |ui, name| {
            if name == "agenda" || name == "automacoes" {
                refresh(ui, &p);
            }
        });
    }
    {
        let p = page.clone();
        let n = Cell::new(0u32);
        ui.on_tick(move |ui| {
            let here = ui.page();
            if here != "agenda" && here != "automacoes" {
                return;
            }
            n.set(n.get() + 1);
            if !n.get().is_multiple_of(30) {
                return;
            }
            // once a second: the log, and the now-line
            if ui.core.borrow().log.len() != p.log_len.get() {
                refresh_log(ui, &p);
                refresh_autos(ui, &p);
            }
            if n.get().is_multiple_of(900) {
                p.grid.queue_draw();
            }
        });
    }
    let sc = scroller(&root);
    sc.upcast()
}

/// The script API's reference, over the window.
pub fn script_help(ui: &Rc<Ui>) {
    let d = vbox(12, "");
    let head = hbox(8, "");
    let t = label(tr("Script help"), "ka-titulo2");
    t.set_hexpand(true);
    head.append(&t);
    let x = icon_button("fechar", "muted", 14, tr("Close"));
    head.append(&x);
    d.append(&head);
    let text = label(&crate::script::help(), "ka-mono ka-texto2");
    text.set_selectable(true);
    text.set_yalign(0.0);
    text.set_xalign(0.0);
    let sc = gtk::ScrolledWindow::new();
    sc.set_child(Some(&text));
    sc.set_min_content_height(360);
    sc.set_max_content_height(560);
    sc.set_propagate_natural_height(true);
    sc.set_hscrollbar_policy(gtk::PolicyType::Automatic);
    sc.add_css_class("ka-poco");
    sc.add_css_class("ka-ajuda-scripts");
    d.append(&sc);
    let close = modal(ui, &d, 860);
    x.connect_clicked(move |_| close());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timer(days: u8, start: u16, minutes: u16) -> Timer {
        Timer { days, start, minutes, on: true, mode: 1, date: 0, quit_sources: false }
    }

    #[test]
    fn times_are_read_and_written() {
        assert_eq!(parse_hm("20:00"), Some(1200));
        assert_eq!(parse_hm(" 8:05 "), Some(485));
        assert_eq!(parse_hm("20h"), Some(1200));
        assert_eq!(parse_hm("2030"), Some(1230));
        assert_eq!(parse_hm("24:00"), Some(1440));
        assert_eq!(parse_hm("24:30"), None);
        assert_eq!(parse_hm("12:60"), None);
        assert_eq!(parse_hm("abc"), None);
        assert_eq!(fmt_hm(1230), "20:30");
        assert_eq!(fmt_hm(1500), "01:00");
        assert_eq!(parse_date("2026-11-19"), Some(20261119));
        assert_eq!(parse_date("19/11/2026"), Some(20261119));
        assert_eq!(parse_date("2026-13-01"), None);
        assert_eq!(fmt_date(20260105), "2026-01-05");
    }

    #[test]
    fn from_and_to_become_start_and_length() {
        assert_eq!(length_between(20 * 60, 22 * 60), 120);
        assert_eq!(length_between(23 * 60, 60), 120, "a To before From ends the next day");
        assert_eq!(length_between(600, 600), 1440);
        assert_eq!(to_of(23 * 60, 120), 60);
        assert_eq!(to_of(20 * 60, 120), 22 * 60);
        for (a, b) in [(0u16, 15u16), (1200, 1320), (1380, 30)] {
            assert_eq!(to_of(a, length_between(a, b)), b);
        }
    }

    #[test]
    fn day_bits_follow_the_model() {
        // Monday column = bit 1, Sunday column = bit 0
        assert_eq!(bit_of_column(0), 0b10);
        assert_eq!(bit_of_column(6), 0b1);
        assert_eq!(days_summary(0b0111110), "Mon to Fri");
        assert_eq!(days_summary(0b1111111), "every day");
        assert_eq!(days_summary(0b0001010), "Mon, Wed");
        assert_eq!(days_summary(0b1000001), "Sat, Sun");
        assert_eq!(days_summary(0), "no day");
    }

    #[test]
    fn the_week_places_every_window() {
        let dates = [20261116, 20261117, 20261118, 20261119, 20261120, 20261121, 20261122];
        let mut once = timer(0, 9 * 60, 120);
        once.mode = 0;
        once.date = 20261118;
        let mut elsewhere = once.clone();
        elsewhere.date = 20261201;
        let ts = vec![((0, 0), timer(0b0111110, 20 * 60, 120)), ((1, 0), once), ((1, 1), elsewhere), ((2, 0), timer(0b0000010, 23 * 60, 120))];
        let b = week_bands(&ts, &dates);
        assert_eq!(b.iter().filter(|x| x.key == (0, 0)).map(|x| x.col).collect::<Vec<_>>(), vec![0, 1, 2, 3, 4]);
        assert_eq!(b.iter().filter(|x| x.key == (1, 0)).map(|x| (x.col, x.start, x.end)).collect::<Vec<_>>(), vec![(2, 540, 660)]);
        assert!(!b.iter().any(|x| x.key == (1, 1)), "a date outside the week is not shown");
        let late: Vec<_> = b.iter().filter(|x| x.key == (2, 0)).cloned().collect();
        assert_eq!(late.len(), 2);
        assert_eq!((late[0].col, late[0].start, late[0].end, late[0].first), (0, 1380, 1440, true));
        assert_eq!((late[1].col, late[1].start, late[1].end, late[1].first), (1, 0, 60, false));
        assert_eq!(hour_range(&b), (0, 24));
        assert_eq!(hour_range(&week_bands(&ts[..2], &dates)), (8, 22));
        let sunday = week_bands(&[((0, 0), timer(0b1, 23 * 60, 120))], &dates);
        assert!(sunday.iter().any(|x| x.col == 0 && x.start == 0 && x.end == 60), "Sunday night spills into Monday");
    }

    #[test]
    fn dragging_snaps_to_quarter_hours() {
        assert_eq!(y_of(10 * 60, 8, 26.5), 53.0);
        assert_eq!(snap15(52.0), 45);
        assert_eq!(snap15(53.0), 60);
        let t = timer(0b10, 20 * 60, 120);
        // a quarter of an hour is 26.5 / 4 px
        assert_eq!(resized_length(&t, 26.5 / 4.0, 26.5), 135);
        assert_eq!(resized_length(&t, 3.0, 26.5), 120, "small moves stay on the same quarter");
        assert_eq!(resized_length(&t, -10_000.0, 26.5), 15, "never shorter than 15 min");
        let moved = moved_timer(&t, 0, 2, 26.5, 26.5, |d, n| d + n as u32);
        assert_eq!(moved.days, 0b1000, "Monday became Wednesday");
        assert_eq!(moved.start, 21 * 60);
        assert_eq!(moved.minutes, 120);
        let mut once = t.clone();
        once.mode = 0;
        once.date = 20261116;
        assert_eq!(moved_timer(&once, 0, 1, 0.0, 26.5, |d, n| d + n as u32).date, 20261117);
        let both = timer(0b110, 600, 60);
        assert_eq!(moved_timer(&both, 0, 3, 0.0, 26.5, |d, _| d).days, 0b10100, "only the dragged day moves");
    }

    #[test]
    fn the_log_says_when_an_automation_ran() {
        let log = vec![
            "14:50:02  Recording finished → running “Transcribe when done”".to_string(),
            "14:50:03  transcribing to “/tmp/a.txt”".to_string(),
            "15:01:00  Session started → running “Other”".to_string(),
        ];
        assert_eq!(last_run(&log, "Transcribe when done").as_deref(), Some("14:50"));
        assert_eq!(last_run(&log, "Nope"), None);
        assert_eq!(line_numbers("a\nb\nc"), "1\n2\n3");
        assert_eq!(line_numbers(""), "1");
    }
}
