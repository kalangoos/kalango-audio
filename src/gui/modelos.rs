//! Templates (design "Modelos (galeria)"): our own starting points, filtered by category. Each card shows its chain
//! in miniature; the selected one gets the accent border and the preview on the right shows the diagram of its
//! blocks, what will be created and the permissions that will be asked. "Create session" (or a double click on a
//! card) makes the session with the person's settings and opens the editor. Narrow window: the preview becomes a
//! sheet at the bottom and the grid goes to one column.

use super::widgets::*;
use super::{icons, theme, Change, Layout, Ui};
use crate::i18n::{tr, trf};
use crate::model::{kind_info, Template};
use gtk::prelude::*;
use gtk::{glib, Align};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

struct Page {
    grid: gtk::FlowBox,
    cards: RefCell<Vec<(usize, gtk::Box)>>,
    filters: RefCell<Vec<(String, gtk::ToggleButton)>>,
    filter: RefCell<String>,
    selected: Cell<usize>,
    body: gtk::Box,
    panel: gtk::Box,
    panel_scroll: gtk::ScrolledWindow,
    close_sheet: gtk::Button,
    p_name: gtk::Label,
    p_desc: gtk::Label,
    diagram: gtk::Overlay,
    diagram_fixed: gtk::Fixed,
    diagram_area: gtk::DrawingArea,
    created: gtk::Box,
    perms: gtk::Label,
    sheet_open: Cell<bool>,
}

fn columns(l: Layout) -> u32 {
    match l {
        Layout::Wide => 3,
        Layout::Medium => 2,
        Layout::Narrow => 1,
    }
}

/// The categories in the order the templates bring them ("Basic", "Record"…), each once.
pub fn categories(t: &[Template]) -> Vec<String> {
    let mut v: Vec<String> = vec![];
    for x in t {
        if !x.category.is_empty() && !v.contains(&x.category) {
            v.push(x.category.clone());
        }
    }
    v
}

/// What a block will do in the new session, in a few words.
fn role(b: &crate::model::Block) -> String {
    match b.kind.as_str() {
        "app_source" => {
            let app = b.p.text("app", "");
            if app == "Web Browser" {
                tr("You choose the tab or the app when it starts").to_string()
            } else {
                trf("Listens to: {app}", &[("app", crate::audio::app_label(&app))])
            }
        }
        "device_input" | "device_output" => tr("The system’s default device").to_string(),
        "recorder" => trf("{format} in {folder}", &[("format", b.p.text("format", "WAV")), ("folder", b.p.text("folder", ""))]),
        "transcribe" => tr("Local model, nothing leaves the computer").to_string(),
        k => kind_info(k).map(|i| tr(i.desc).to_string()).unwrap_or_default(),
    }
}

/// The note about what the system will ask the first time.
pub fn permissions_note(kinds: &[String]) -> String {
    let has = |k: &str| kinds.iter().any(|x| x == k);
    let mut what: Vec<&str> = vec![];
    if has("app_source") {
        what.push(tr("the app’s sound"));
    }
    if has("system_sound") {
        what.push(tr("the computer’s sound"));
    }
    if has("device_input") {
        what.push(tr("the microphone"));
    }
    match what.len() {
        0 => tr("Asks for no permission: it only uses what you add later.").to_string(),
        1 => trf("Asks for access to {a} the first time.", &[("a", what[0].to_string())]),
        _ => {
            let last = what.pop().unwrap_or_default();
            trf("Asks for access to {a} and {b} the first time.", &[("a", what.join(", ")), ("b", last.to_string())])
        }
    }
}

fn create(ui: &Rc<Ui>, t: usize) {
    let i = ui.core.borrow_mut().new_with_prefs(t);
    ui.changed(Change::Sessions);
    ui.open_session(i);
}

fn card(ui: &Rc<Ui>, page: &Rc<Page>, i: usize) -> gtk::Box {
    let c = ui.core.borrow();
    let t = &c.templates[i];
    let s = t.instantiate();
    let card = vbox(0, "ka-cartao ka-modelo");
    let well = hbox(0, "ka-poco ka-modelo-poco");
    well.set_size_request(-1, 64);
    let kinds = ordered_kinds(&s, 5);
    if kinds.is_empty() {
        let plus = hbox(0, "ka-tracejado ka-modelo-mais");
        plus.set_size_request(32, 32);
        plus.set_halign(Align::Center);
        plus.set_valign(Align::Center);
        let g = icons::icon(if t.manual { "ramo" } else { "mais" }, "muted", 14);
        g.set_hexpand(true);
        plus.append(&g);
        well.append(&plus);
        plus.set_hexpand(true);
    } else {
        let ch = chain(&kinds, 28, 22);
        ch.set_halign(Align::Center);
        ch.set_hexpand(true);
        well.append(&ch);
    }
    card.append(&well);
    let name = line(tr(&t.name), "ka-titulo3");
    name.set_margin_top(12);
    card.append(&name);
    let sub = line(tr(&t.subtitle), "ka-texto2");
    sub.set_margin_top(3);
    card.append(&sub);
    if !t.category.is_empty() {
        let ch = chip(tr(&t.category), "muted");
        ch.set_halign(Align::Start);
        ch.set_margin_top(8);
        card.append(&ch);
    }
    drop(c);
    let click = gtk::GestureClick::new();
    {
        let (u, p) = (ui.clone(), page.clone());
        click.connect_released(move |g, n, _, _| {
            g.set_state(gtk::EventSequenceState::Claimed);
            if n >= 2 {
                create(&u, i);
            } else {
                select(&u, &p, i);
                p.sheet_open.set(true);
                place(&u, &p);
            }
        });
    }
    card.add_controller(click);
    card.set_cursor_from_name(Some("pointer"));
    card.set_tooltip_text(Some(tr("Click to see it · double click to create")));
    card
}

/// The diagram: tiles at the template's grid positions, wires as curves (drawn under them).
fn fill_diagram(ui: &Rc<Ui>, page: &Rc<Page>) {
    while let Some(ch) = page.diagram_fixed.first_child() {
        page.diagram_fixed.remove(&ch);
    }
    let w = page.diagram_area.width().max(300) as f64;
    let h = page.diagram_area.height().max(200) as f64;
    let c = ui.core.borrow();
    let Some(t) = c.templates.get(page.selected.get()) else { return };
    let pts = diagram_points(t, w, h);
    for (tb, (x, y)) in t.blocks.iter().zip(&pts) {
        let v = vbox(4, "");
        let tile = kind_tile(&tb.kind, 36);
        tile.set_halign(Align::Center);
        v.append(&tile);
        let l = gtk::Label::new(kind_info(&tb.kind).map(|k| tr(k.title)));
        l.add_css_class("ka-fraco2");
        l.add_css_class("ka-diagrama-nome");
        l.set_ellipsize(gtk::pango::EllipsizeMode::End);
        l.set_max_width_chars(12);
        v.append(&l);
        v.set_size_request(96, -1);
        page.diagram_fixed.put(&v, x - 48.0, y - 18.0);
    }
    if t.blocks.is_empty() {
        let l = label(tr("An empty canvas: you add the blocks."), "ka-fraco");
        page.diagram_fixed.put(&l, 16.0, h / 2.0 - 8.0);
    }
    page.diagram_area.queue_draw();
}

/// Centres of the template's blocks, scaled from the grid into a `w` × `h` area (tiles of 36 with names below).
pub fn diagram_points(t: &Template, w: f64, h: f64) -> Vec<(f64, f64)> {
    if t.blocks.is_empty() {
        return vec![];
    }
    let (mut x0, mut x1, mut y0, mut y1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    for b in &t.blocks {
        x0 = x0.min(b.x as f64);
        x1 = x1.max(b.x as f64);
        y0 = y0.min(b.y as f64);
        y1 = y1.max(b.y as f64);
    }
    let (mx, my) = (56.0, 36.0);
    let sx = if x1 > x0 { (w - 2.0 * mx) / (x1 - x0) } else { 0.0 };
    let sy = if y1 > y0 { ((h - 2.0 * my - 16.0) / (y1 - y0)).min(sx.max(30.0)) } else { 0.0 };
    let used_h = (y1 - y0) * sy;
    let top = (h - 16.0 - used_h) / 2.0;
    t.blocks
        .iter()
        .map(|b| {
            let x = if x1 > x0 { mx + (b.x as f64 - x0) * sx } else { w / 2.0 };
            let y = top + (b.y as f64 - y0) * sy;
            (x, y)
        })
        .collect()
}

fn draw_wires(ui: &Rc<Ui>, page: &Page, cr: &gtk::cairo::Context, w: f64, h: f64) {
    let c = ui.core.borrow();
    let Some(t) = c.templates.get(page.selected.get()) else { return };
    let pts = diagram_points(t, w, h);
    cr.set_line_width(1.5);
    for (i, b) in t.blocks.iter().enumerate() {
        for o in &b.out {
            let (Some(a), Some(z)) = (pts.get(i), pts.get(*o)) else { continue };
            let (ax, ay) = (a.0 + 18.0, a.1);
            let (zx, zy) = (z.0 - 18.0, z.1);
            let dx = ((zx - ax) / 2.0).max(24.0);
            theme::source(cr, "accent", 0.75);
            cr.move_to(ax, ay);
            cr.curve_to(ax + dx, ay, zx - dx, zy, zx, zy);
            let _ = cr.stroke();
        }
    }
}

fn select(ui: &Rc<Ui>, page: &Rc<Page>, i: usize) {
    page.selected.set(i);
    for (k, c) in page.cards.borrow().iter() {
        if *k == i {
            c.add_css_class("selecionado");
        } else {
            c.remove_css_class("selecionado");
        }
    }
    let c = ui.core.borrow();
    let Some(t) = c.templates.get(i) else { return };
    page.p_name.set_text(tr(&t.name));
    page.p_desc.set_text(tr(&t.desc));
    let mut s = t.instantiate();
    crate::gui::ajustes::apply_prefs_to(&c.store.prefs, &mut s.blocks);
    drop(c);
    clear(&page.created);
    let mut kinds = vec![];
    for b in &s.blocks {
        kinds.push(b.kind.clone());
        let r = hbox(10, "ka-criado");
        let tl = kind_tile(&b.kind, 24);
        tl.set_valign(Align::Start);
        tl.set_hexpand(false);
        r.append(&tl);
        let txt = vbox(1, "");
        txt.set_hexpand(true);
        txt.append(&line(kind_info(&b.kind).map(|k| tr(k.title)).unwrap_or(""), "ka-texto"));
        txt.append(&line(&role(b), "ka-fraco"));
        r.append(&txt);
        page.created.append(&r);
    }
    if s.blocks.is_empty() {
        page.created.append(&label(tr("Nothing yet: an empty session."), "ka-fraco"));
    }
    page.perms.set_text(&permissions_note(&kinds));
    fill_diagram(ui, page);
}

fn apply_filter(page: &Rc<Page>) {
    let f = page.filter.borrow().clone();
    for (k, b) in page.filters.borrow().iter() {
        b.set_active(*k == f);
        if *k == f {
            b.add_css_class("ativo");
        } else {
            b.remove_css_class("ativo");
        }
    }
    page.grid.invalidate_filter();
}

fn place(ui: &Rc<Ui>, page: &Rc<Page>) {
    let l = ui.layout();
    page.grid.set_max_children_per_line(columns(l));
    let narrow = l == Layout::Narrow;
    page.body.set_orientation(if narrow { gtk::Orientation::Vertical } else { gtk::Orientation::Horizontal });
    if narrow {
        page.panel.add_css_class("folha");
        page.panel_scroll.set_size_request(-1, 340);
        page.panel.set_size_request(-1, -1);
        page.panel_scroll.set_vexpand(false);
        page.panel_scroll.set_visible(page.sheet_open.get());
        page.close_sheet.set_visible(true);
    } else {
        page.panel.remove_css_class("folha");
        page.panel_scroll.set_size_request(380, -1);
        page.panel_scroll.set_vexpand(true);
        page.panel_scroll.set_visible(true);
        page.close_sheet.set_visible(false);
    }
}

pub fn page(ui: &Rc<Ui>) -> gtk::Widget {
    let body = hbox(0, "ka-modelos");
    let left = vbox(0, "ka-pagina");
    left.set_hexpand(true);
    left.append(&label(tr("Templates"), "ka-titulo"));
    let sub = para(tr("Ready starting points. Everything can be changed later, block by block."), "ka-fraco");
    sub.set_margin_top(4);
    left.append(&sub);

    let filters_row = hbox(6, "");
    filters_row.set_margin_top(16);
    let fscroll = gtk::ScrolledWindow::new();
    fscroll.set_vscrollbar_policy(gtk::PolicyType::Never);
    fscroll.set_hscrollbar_policy(gtk::PolicyType::Automatic);
    fscroll.set_propagate_natural_height(true);
    fscroll.set_child(Some(&filters_row));
    left.append(&fscroll);

    let grid = gtk::FlowBox::new();
    grid.set_selection_mode(gtk::SelectionMode::None);
    grid.set_homogeneous(true);
    grid.set_column_spacing(14);
    grid.set_row_spacing(14);
    grid.set_min_children_per_line(1);
    grid.set_valign(Align::Start);
    grid.set_margin_top(16);
    grid.add_css_class("ka-grade");
    left.append(&grid);
    let left_scroll = scroller(&left);

    // the preview
    let panel = vbox(0, "ka-modelos-previa");
    let top = hbox(0, "");
    top.append(&caps(tr("Preview")));
    top.append(&spacer());
    let close_sheet = icon_button("fechar", "muted", 14, tr("Close"));
    top.append(&close_sheet);
    panel.append(&top);
    let p_name = line("", "ka-titulo2");
    p_name.set_margin_top(6);
    panel.append(&p_name);
    let p_desc = para("", "ka-texto2");
    p_desc.set_margin_top(6);
    panel.append(&p_desc);
    let diagram = gtk::Overlay::new();
    diagram.add_css_class("ka-poco");
    diagram.set_margin_top(14);
    let area = gtk::DrawingArea::new();
    area.set_content_height(220);
    area.set_hexpand(true);
    diagram.set_child(Some(&area));
    let fixed = gtk::Fixed::new();
    fixed.set_can_target(false);
    diagram.add_overlay(&fixed);
    panel.append(&diagram);
    let cap = caps(tr("What will be created"));
    cap.set_margin_top(20);
    cap.set_margin_bottom(8);
    panel.append(&cap);
    let created = vbox(10, "");
    panel.append(&created);
    let div = hbox(0, "ka-linha-div");
    div.set_margin_top(18);
    panel.append(&div);
    let perms = para("", "ka-fraco");
    perms.set_margin_top(12);
    panel.append(&perms);
    let grow = vbox(0, "");
    grow.set_vexpand(true);
    panel.append(&grow);
    let make = button(tr("Create session"), "primario", Some("mais"));
    make.set_margin_top(18);
    make.add_css_class("ka-largo");
    panel.append(&make);
    let panel_scroll = scroller(&panel);
    panel_scroll.set_hexpand(false);
    panel_scroll.add_css_class("ka-modelos-previa-rolagem");

    body.append(&left_scroll);
    body.append(&panel_scroll);

    let page = Rc::new(Page {
        grid: grid.clone(),
        cards: RefCell::new(vec![]),
        filters: RefCell::new(vec![]),
        filter: RefCell::new(String::new()),
        selected: Cell::new(0),
        body: body.clone(),
        panel,
        panel_scroll,
        close_sheet: close_sheet.clone(),
        p_name,
        p_desc,
        diagram,
        diagram_fixed: fixed,
        diagram_area: area.clone(),
        created,
        perms,
        sheet_open: Cell::new(false),
    });
    let _ = &page.diagram;
    {
        let (u, p) = (ui.clone(), Rc::downgrade(&page));
        area.set_draw_func(move |_, cr, w, h| {
            if let Some(p) = p.upgrade() {
                draw_wires(&u, &p, cr, w as f64, h as f64);
            }
        });
        follow_theme(&area);
        let (u, p) = (ui.clone(), Rc::downgrade(&page));
        area.connect_resize(move |_, _, _| {
            if let Some(p) = p.upgrade() {
                let u = u.clone();
                // placing the tiles while GTK is sizing would ask for another sizing
                glib::idle_add_local_once(move || fill_diagram(&u, &p));
            }
        });
    }

    // filters: All + the templates' categories
    let cats = categories(&ui.core.borrow().templates);
    for (key, text) in std::iter::once((String::new(), tr("All").to_string())).chain(cats.iter().map(|c| (c.clone(), tr(c).to_string()))) {
        let b = gtk::ToggleButton::with_label(&text);
        b.add_css_class("ka-filtro");
        let (p, k) = (page.clone(), key.clone());
        b.connect_clicked(move |_| {
            *p.filter.borrow_mut() = k.clone();
            apply_filter(&p);
        });
        filters_row.append(&b);
        page.filters.borrow_mut().push((key, b));
    }
    // the cards
    let n = ui.core.borrow().templates.len();
    let mut anims = vec![];
    for i in 0..n {
        let c = card(ui, &page, i);
        page.cards.borrow_mut().push((i, c.clone()));
        let a = animated(&c);
        unsafe {
            a.set_data("tpl", i);
        }
        grid.append(&a);
        anims.push(a);
    }
    {
        let (u, p) = (ui.clone(), page.clone());
        grid.set_filter_func(move |child| {
            let f = p.filter.borrow();
            if f.is_empty() {
                return true;
            }
            let Some(i) = child.child().and_then(|a| unsafe { a.data::<usize>("tpl").map(|x| *x.as_ref()) }) else { return true };
            u.core.borrow().templates.get(i).map(|t| t.category == *f).unwrap_or(true)
        });
    }
    {
        let (u, p) = (ui.clone(), page.clone());
        make.connect_clicked(move |_| create(&u, p.selected.get()));
        let (u, p) = (ui.clone(), page.clone());
        close_sheet.connect_clicked(move |_| {
            p.sheet_open.set(false);
            place(&u, &p);
        });
    }
    let first = ui.core.borrow().templates.iter().position(|t| t.name == "Online meeting").unwrap_or(0);
    select(ui, &page, first);
    apply_filter(&page);
    place(ui, &page);
    {
        let p = page.clone();
        ui.on_change(move |ui, c| match c {
            Change::Layout => place(ui, &p),
            Change::Store => select(ui, &p, p.selected.get()),
            _ => {}
        });
    }
    {
        let anims = anims.clone();
        ui.on_page(move |_, name| {
            if name == "modelos" {
                cascade(&anims);
            }
        });
    }
    body.upcast()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::templates;

    #[test]
    fn categories_come_in_order_once() {
        let c = categories(&templates());
        assert_eq!(c.first().map(String::as_str), Some("Basic"));
        let mut d = c.clone();
        d.dedup();
        assert_eq!(c.len(), d.len());
        assert!(c.contains(&"Stream".to_string()));
    }

    #[test]
    fn the_diagram_fits_its_area() {
        for t in templates() {
            let pts = diagram_points(&t, 340.0, 220.0);
            assert_eq!(pts.len(), t.blocks.len());
            for (x, y) in pts {
                assert!((0.0..=340.0).contains(&x) && (0.0..=220.0).contains(&y), "{}: {x},{y}", t.name);
            }
        }
    }

    #[test]
    fn permissions_follow_the_blocks() {
        let k = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(permissions_note(&k(&["app_source", "device_input", "recorder"])), "Asks for access to the app’s sound and the microphone the first time.");
        assert_eq!(permissions_note(&k(&["device_input"])), "Asks for access to the microphone the first time.");
        assert!(permissions_note(&[]).starts_with("Asks for no permission"));
    }
}
