//! Sessions (design "Kalango Audio · Sessões (início)"): a card per session with its chain in miniature, its state
//! (recording, active, scheduled, stopped), the live stereo level while it runs and its last recording; the agent's
//! suggestion when a browser is playing sound; and a strip of templates to start from.

use super::widgets::*;
use super::{icons, Change, Layout, Ui};
use crate::core::{fmt_duration, fmt_time};
use crate::i18n::{tr, trf};
use crate::model::Session;
use gtk::prelude::*;
use gtk::{glib, Align};
use std::cell::RefCell;
use std::rc::Rc;

/// A running card's live parts, refreshed on every tick.
struct Live {
    uid: u64,
    meter: gtk::DrawingArea,
    status: gtk::Label,
}

struct Page {
    grid: gtk::FlowBox,
    templates: gtk::FlowBox,
    subtitle: gtk::Label,
    suggestion: gtk::Box,
    suggestion_title: gtk::Label,
    dismissed: RefCell<Vec<String>>,
    live: RefCell<Vec<Live>>,
}

fn columns(l: Layout) -> u32 {
    match l {
        Layout::Wide => 3,
        Layout::Medium => 2,
        Layout::Narrow => 1,
    }
}

/// When a session's next scheduled window opens, as "today 20:00" / "Mon 09:00".
fn next_schedule(s: &Session) -> Option<String> {
    let t = s.timers.iter().find(|t| t.on)?;
    let hm = format!("{:02}:{:02}", t.start / 60, t.start % 60);
    let (wd, min) = crate::engine::local_now();
    if t.mode == 0 {
        let d = t.date;
        return Some(if d == crate::engine::local_date() { trf("today {time}", &[("time", hm)]) } else { format!("{:02}/{:02} {hm}", d % 100, (d / 100) % 100) });
    }
    let days = [tr("Sun"), tr("Mon"), tr("Tue"), tr("Wed"), tr("Thu"), tr("Fri"), tr("Sat")];
    for k in 0..7u32 {
        let d = (wd + k) % 7;
        if t.days & (1 << d) != 0 && (k > 0 || (t.start as u32) > min) {
            return Some(if k == 0 { trf("today {time}", &[("time", hm)]) } else { format!("{} {hm}", days[d as usize]) });
        }
    }
    Some(hm)
}

fn card(ui: &Rc<Ui>, idx: usize, page: &Rc<Page>) -> gtk::Widget {
    let c = ui.core.borrow();
    let s = &c.sessions[idx];
    let uid = s.uid;
    let running = c.is_running(uid);
    let recording = c.is_recording(uid);
    let sched = if running { None } else { next_schedule(s) };
    let card = vbox(0, "ka-cartao ka-sessao");
    if recording {
        card.add_css_class("gravando");
    }
    card.set_size_request(300, -1);
    let body = vbox(0, "");
    body.set_margin_start(16);
    body.set_margin_end(16);
    body.set_margin_top(16);
    body.set_margin_bottom(12);
    card.append(&body);

    // the chain and the menu
    let top = hbox(0, "");
    top.append(&chain(&ordered_kinds(s, 6), 28, 14));
    top.append(&spacer());
    let more = gtk::MenuButton::new();
    more.add_css_class("ka-icone");
    more.set_child(Some(&icons::icon("pontos-v", "muted", 16)));
    more.set_valign(Align::Start);
    more.set_tooltip_text(Some(tr("More")));
    let pop = gtk::Popover::new();
    pop.add_css_class("menu");
    let items = vbox(2, "");
    for (label_text, what) in [(tr("Open"), 0), (tr("Duplicate"), 1), (tr("Delete"), 2)] {
        let b = gtk::Button::with_label(label_text);
        b.add_css_class("ka-item-menu");
        let (u, p) = (ui.clone(), pop.clone());
        b.connect_clicked(move |_| {
            p.popdown();
            let Some(i) = u.core.borrow().index_of(uid) else { return };
            match what {
                0 => u.open_session(i),
                1 => {
                    {
                        let mut c = u.core.borrow_mut();
                        c.cur = i;
                        c.duplicate_session();
                    }
                    u.changed(Change::Sessions);
                }
                _ => {
                    let removed = {
                        let mut c = u.core.borrow_mut();
                        c.cur = i;
                        let s = c.sessions[i].clone();
                        c.delete_session();
                        s
                    };
                    u.changed(Change::Sessions);
                    let name = removed.name.clone();
                    let back = RefCell::new(Some((i, removed)));
                    u.toast(
                        &trf("“{name}” deleted", &[("name", name)]),
                        Some((
                            tr("Undo"),
                            Box::new(move |u: &Rc<Ui>| {
                                if let Some((i, s)) = back.borrow_mut().take() {
                                    let mut c = u.core.borrow_mut();
                                    let i = i.min(c.sessions.len());
                                    c.sessions.insert(i, s);
                                    drop(c);
                                    u.changed(Change::Sessions);
                                }
                            }),
                        )),
                    );
                }
            }
        });
        items.append(&b);
    }
    pop.set_child(Some(&items));
    more.set_popover(Some(&pop));
    top.append(&more);
    body.append(&top);

    let name = line(&s.name, "ka-titulo2");
    name.set_margin_top(14);
    body.append(&name);
    let kinds: Vec<String> = ordered_kinds(s, 8).iter().map(|k| crate::model::kind_info(k).map(|x| tr(x.title).to_lowercase()).unwrap_or_default()).collect();
    let desc = line(&kinds.join(" → "), "ka-fraco");
    desc.set_margin_top(4);
    body.append(&desc);

    // state, live meter, run/stop
    let state = hbox(8, "");
    state.set_margin_top(14);
    let (token, text) = if recording {
        ("red", trf("Recording · {time}", &[("time", fmt_time(c.running_for(uid).unwrap_or(0.0)))]))
    } else if running {
        ("teal", tr("Active · not recording").to_string())
    } else if let Some(when) = &sched {
        ("gold", trf("Scheduled · {when}", &[("when", when.clone())]))
    } else {
        ("dim", tr("Stopped").to_string())
    };
    state.append(&dot(token));
    let st = label(&text, "ka-texto2 ka-tnum");
    if token != "dim" {
        st.add_css_class(&color_class(token));
    }
    st.set_hexpand(true);
    state.append(&st);
    let run = button("", if running { "secundario" } else { "primario" }, Some(if running { "parar" } else { "tocar" }));
    run.set_tooltip_text(Some(if running { tr("Stop") } else { tr("Start") }));
    {
        let u = ui.clone();
        run.connect_clicked(move |_| {
            let i = u.core.borrow().index_of(uid);
            if let Some(i) = i {
                u.toggle(i);
            }
        });
    }
    state.append(&run);
    if running {
        let u = ui.clone();
        let m = meter(150, 11, move || {
            let c = u.core.borrow();
            let s = c.sessions.iter().find(|s| s.uid == uid)?;
            // the loudest block that writes or plays (the end of the chain)
            s.blocks.iter().filter_map(|b| c.disp.get(&(uid, b.id))).copied().max_by(|a, b| a.peak_max().partial_cmp(&b.peak_max()).unwrap())
        });
        state.append(&m);
        page.live.borrow_mut().push(Live { uid, meter: m, status: st.clone() });
    }
    body.append(&state);

    let div = hbox(0, "ka-linha-div");
    div.set_margin_top(12);
    body.append(&div);

    // the last recording (or the routing note when it only plays)
    let foot = hbox(12, "");
    foot.set_margin_top(10);
    let txt = vbox(2, "");
    txt.set_hexpand(true);
    let last = c.recs.iter().find(|r| c.session_of_recording(r).map(|x| x.uid) == Some(uid)).cloned();
    let records = s.blocks.iter().any(|b| b.kind == "recorder");
    if records {
        txt.append(&caps(tr("Last recording")));
        let when = last
            .as_ref()
            .map(|r| {
                let mut parts = vec![when_label(r.modified)];
                if let Some(d) = r.dur {
                    parts.push(fmt_duration(d));
                }
                if r.transcript().is_some() {
                    parts.push(tr("with transcript").to_string());
                }
                parts.join(" · ")
            })
            .unwrap_or_else(|| tr("Nothing recorded yet").to_string());
        txt.append(&line(&when, "ka-texto2"));
    } else {
        txt.append(&caps(tr("Routing")));
        txt.append(&line(tr("Does not record · only routes the sound"), "ka-texto2"));
    }
    foot.append(&txt);
    if let Some(r) = &last {
        foot.append(&wave(&r.path, 120, 28, "dim", || None));
    }
    body.append(&foot);
    let links = hbox(16, "");
    links.set_margin_top(8);
    let open = gtk::Button::with_label(tr("Open"));
    open.add_css_class("ka-link-botao");
    {
        let u = ui.clone();
        open.connect_clicked(move |_| {
            let i = u.core.borrow().index_of(uid);
            if let Some(i) = i {
                u.open_session(i);
            }
        });
    }
    let recs = gtk::Button::with_label(tr("Recordings"));
    recs.add_css_class("ka-link-botao");
    recs.add_css_class("fraco");
    {
        let u = ui.clone();
        recs.connect_clicked(move |_| u.go("gravacoes"));
    }
    links.append(&open);
    links.append(&recs);
    body.append(&links);

    // a click on the card opens the editor
    let click = gtk::GestureClick::new();
    {
        let u = ui.clone();
        click.connect_released(move |_, _, _, _| {
            let i = u.core.borrow().index_of(uid);
            if let Some(i) = i {
                u.open_session(i);
            }
        });
    }
    card.add_controller(click);
    card.set_cursor_from_name(Some("pointer"));
    card.upcast()
}

/// "today 14:02", "yesterday 20:00", "Mon 10:15"
pub fn when_label(epoch: u64) -> String {
    let Ok(t) = glib::DateTime::from_unix_local(epoch as i64) else { return String::new() };
    let Ok(now) = glib::DateTime::now_local() else { return String::new() };
    let hm = format!("{:02}:{:02}", t.hour(), t.minute());
    let days = (now.ymd().0 as i64 * 400 + now.day_of_year() as i64) - (t.ymd().0 as i64 * 400 + t.day_of_year() as i64);
    match days {
        0 => trf("today {time}", &[("time", hm)]),
        1 => trf("yesterday {time}", &[("time", hm)]),
        2..=6 => {
            let names = [tr("Mon"), tr("Tue"), tr("Wed"), tr("Thu"), tr("Fri"), tr("Sat"), tr("Sun")];
            format!("{} {hm}", names[(t.day_of_week() as usize).saturating_sub(1) % 7])
        }
        _ => format!("{:02}/{:02} {hm}", t.day_of_month(), t.month()),
    }
}

fn new_card(ui: &Rc<Ui>) -> gtk::Widget {
    let b = gtk::Button::new();
    b.add_css_class("ka-tracejado");
    b.add_css_class("ka-nova-sessao");
    let v = vbox(8, "");
    v.set_valign(Align::Center);
    v.set_halign(Align::Center);
    let t = tile("mais", "accent", 40);
    t.set_halign(Align::Center);
    v.append(&t);
    let l = gtk::Label::new(Some(tr("New session")));
    l.add_css_class("ka-titulo3");
    v.append(&l);
    let l2 = gtk::Label::new(Some(tr("Drag blocks or start from a template")));
    l2.add_css_class("ka-fraco");
    v.append(&l2);
    b.set_child(Some(&v));
    b.set_size_request(300, 214);
    let u = ui.clone();
    b.connect_clicked(move |_| u.go("modelos"));
    b.upcast()
}

fn template_tile(ui: &Rc<Ui>, i: usize) -> gtk::Widget {
    let c = ui.core.borrow();
    let t = &c.templates[i];
    let b = gtk::Button::new();
    b.add_css_class("ka-cartao2");
    b.add_css_class("ka-modelo-mini");
    let row = hbox(10, "");
    let kinds: Vec<String> = {
        let s = t.instantiate();
        ordered_kinds(&s, 3)
    };
    row.append(&chain(&kinds, 28, 10));
    row.append(&line(tr(&t.name), "ka-texto"));
    b.set_child(Some(&row));
    let u = ui.clone();
    b.connect_clicked(move |_| {
        let i = u.core.borrow_mut().new_with_prefs(i);
        u.changed(Change::Sessions);
        u.open_session(i);
    });
    b.upcast()
}

fn refresh(ui: &Rc<Ui>, page: &Rc<Page>) {
    {
        let mut c = ui.core.borrow_mut();
        if c.rec_scanned.is_none() {
            c.scan_recordings();
        }
    }
    page.live.borrow_mut().clear();
    while let Some(ch) = page.grid.first_child() {
        page.grid.remove(&ch);
    }
    let n = ui.core.borrow().sessions.len();
    let mut anims = vec![];
    for i in 0..n {
        let w = card(ui, i, page);
        let a = animated(&w);
        page.grid.append(&a);
        anims.push(a);
    }
    page.grid.append(&new_card(ui));
    let (active, recording) = {
        let c = ui.core.borrow();
        (c.running.len(), c.sessions.iter().filter(|s| c.is_recording(s.uid)).count())
    };
    page.subtitle.set_text(&trf(
        "{n} sessions · {a} active now · {r} recording",
        &[("n", n.to_string()), ("a", active.to_string()), ("r", recording.to_string())],
    ));
    if page.grid.is_mapped() {
        cascade(&anims);
    }
    page.grid.set_max_children_per_line(columns(ui.layout()));
}

fn refresh_templates(ui: &Rc<Ui>, page: &Rc<Page>) {
    while let Some(ch) = page.templates.first_child() {
        page.templates.remove(&ch);
    }
    // the four the person most likely wants: an app, a meeting, a narration, a live
    let wanted = ["Record an app", "Online meeting", "Clean narration", "RTMP live"];
    let idx: Vec<usize> = {
        let c = ui.core.borrow();
        wanted.iter().filter_map(|w| c.templates.iter().position(|t| t.name == *w)).collect()
    };
    for i in idx {
        page.templates.append(&template_tile(ui, i));
    }
    page.templates.set_max_children_per_line(match ui.layout() {
        Layout::Narrow => 1,
        Layout::Medium => 2,
        Layout::Wide => 4,
    });
}

/// A browser playing sound: the agent offers to record it (it never starts without approval).
fn check_suggestion(ui: &Rc<Ui>, page: &Rc<Page>) {
    let apps = ui.core.borrow().apps.lock().map(|a| a.clone()).unwrap_or_default();
    let browser = apps.iter().find(|a| {
        let n = format!("{} {}", a.name, a.binary).to_lowercase();
        ["firefox", "chrom", "brave", "edge", "vivaldi", "navegador", "browser"].iter().any(|b| n.contains(b))
    });
    let show = browser.filter(|a| !page.dismissed.borrow().contains(&a.name));
    match show {
        Some(a) => {
            page.suggestion_title.set_text(&trf("Sound playing in {app}", &[("app", a.name.clone())]));
            unsafe {
                page.suggestion.set_data("app", a.name.clone());
            }
            page.suggestion.set_visible(true);
        }
        None => page.suggestion.set_visible(false),
    }
}

pub fn page(ui: &Rc<Ui>) -> gtk::Widget {
    let root = vbox(0, "ka-pagina ka-inicio");
    // header
    let head = hbox(8, "");
    let titles = vbox(4, "");
    titles.set_hexpand(true);
    titles.append(&label(tr("Sessions"), "ka-titulo"));
    let subtitle = label("", "ka-fraco");
    titles.append(&subtitle);
    head.append(&titles);
    let tpl = button(tr("Templates"), "secundario", Some("grade"));
    let newb = button(tr("New session"), "primario", Some("mais"));
    {
        let u = ui.clone();
        tpl.connect_clicked(move |_| u.go("modelos"));
        let u = ui.clone();
        newb.connect_clicked(move |_| u.go("modelos"));
    }
    head.append(&tpl);
    head.append(&newb);
    root.append(&head);

    // the agent's suggestion
    let sug = hbox(12, "ka-sugestao");
    sug.set_margin_top(16);
    sug.append(&tile("spark", "accent", 32));
    let st = vbox(2, "");
    st.set_hexpand(true);
    let sug_title = line("", "ka-titulo3");
    st.append(&sug_title);
    st.append(&line(tr("The agent can record the tab’s sound and your microphone and transcribe at the end. Nothing starts without your approval."), "ka-fraco"));
    sug.append(&st);
    let later = button(tr("Not now"), "fantasma", None);
    let plan = button(tr("See the plan"), "primario", Some("spark"));
    sug.append(&later);
    sug.append(&plan);
    sug.set_visible(false);
    root.append(&sug);

    let cap = caps(tr("Your sessions"));
    cap.set_margin_top(20);
    cap.set_margin_bottom(8);
    root.append(&cap);
    let grid = gtk::FlowBox::new();
    grid.set_selection_mode(gtk::SelectionMode::None);
    grid.set_homogeneous(true);
    grid.set_column_spacing(16);
    grid.set_row_spacing(16);
    grid.set_min_children_per_line(1);
    grid.set_valign(Align::Start);
    grid.add_css_class("ka-grade");
    root.append(&grid);

    let th = hbox(0, "");
    th.set_margin_top(24);
    th.set_margin_bottom(8);
    th.append(&caps(tr("Start with a template")));
    th.append(&spacer());
    let all = gtk::Button::with_label(tr("See all"));
    all.add_css_class("ka-link-botao");
    {
        let u = ui.clone();
        all.connect_clicked(move |_| u.go("modelos"));
    }
    th.append(&all);
    root.append(&th);
    let templates = gtk::FlowBox::new();
    templates.set_selection_mode(gtk::SelectionMode::None);
    templates.set_homogeneous(true);
    templates.set_column_spacing(16);
    templates.set_row_spacing(16);
    templates.set_valign(Align::Start);
    root.append(&templates);

    let page = Rc::new(Page {
        grid,
        templates,
        subtitle,
        suggestion: sug.clone(),
        suggestion_title: sug_title,
        dismissed: RefCell::new(vec![]),
        live: RefCell::new(vec![]),
    });
    {
        let (p, s) = (page.clone(), sug.clone());
        later.connect_clicked(move |_| {
            if let Some(a) = unsafe { s.data::<String>("app").map(|x| x.as_ref().clone()) } {
                p.dismissed.borrow_mut().push(a);
            }
            s.set_visible(false);
        });
        let u = ui.clone();
        plan.connect_clicked(move |_| super::agente::ask(&u, tr("record the meeting in the browser and transcribe it")));
    }
    refresh(ui, &page);
    refresh_templates(ui, &page);
    {
        let p = page.clone();
        ui.on_change(move |ui, c| match c {
            Change::Sessions | Change::Running | Change::Recordings => refresh(ui, &p),
            Change::Layout => {
                p.grid.set_max_children_per_line(columns(ui.layout()));
                refresh_templates(ui, &p);
            }
            _ => {}
        });
    }
    {
        let p = page.clone();
        ui.on_page(move |ui, name| {
            let here = name == "sessoes";
            ui.core.borrow().want_apps.store(here, std::sync::atomic::Ordering::Relaxed);
            if here {
                refresh(ui, &p);
            }
        });
    }
    {
        let p = page.clone();
        let n = std::cell::Cell::new(0u32);
        ui.on_tick(move |ui| {
            if ui.page() != "sessoes" {
                return;
            }
            for l in p.live.borrow().iter() {
                l.meter.queue_draw();
            }
            // twice a second: the recording time and the browser suggestion
            n.set(n.get() + 1);
            if n.get() % 15 == 0 {
                let c = ui.core.borrow();
                for l in p.live.borrow().iter() {
                    if c.is_recording(l.uid) {
                        l.status.set_text(&trf("Recording · {time}", &[("time", fmt_time(c.running_for(l.uid).unwrap_or(0.0)))]));
                    }
                }
                drop(c);
                check_suggestion(ui, &p);
            }
        });
    }
    let sc = scroller(&root);
    sc.upcast()
}
