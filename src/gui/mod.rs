//! The window, in GTK 4 and the KalangoOS look (Design System page "26 · Kalango Audio"): the rail on the left, the
//! bar with the search (or a request to the agent) and the window's buttons, and the pages — sessions, templates,
//! the session editor, recordings, schedule and automations, settings — plus the agent's panel, first use and the
//! error states. The app itself lives in [`crate::core::Core`]; the pages read and change it and are told when
//! something changed.

pub mod agente;
pub mod agenda;
pub mod ajustes;
pub mod editor;
pub mod editor_canvas;
pub mod editor_inspector;
pub mod editor_library;
pub mod estados;
pub mod gravacoes;
pub mod icons;
pub mod inicio;
pub mod modelos;
pub mod theme;
pub mod widgets;

use crate::core::Core;
use crate::i18n::tr;
use gtk::prelude::*;
use gtk::{gdk, gio, glib, Align, Orientation};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use widgets::{hbox, label, vbox};

pub const APP_ID: &str = "os.kalango.Audio";

/// What changed, for the pages to refresh only what they show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    /// sessions added, removed, renamed, or one's blocks changed
    Sessions,
    /// the selected block in the editor
    Selection,
    /// a session started or stopped
    Running,
    /// the recordings on disk
    Recordings,
    /// settings, presets, scripts, tags
    Store,
    /// the window's width class (wide, medium, narrow)
    Layout,
}

/// The window's width class: the screens adjust (columns that fit, panels that give way).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// 1280 px and up: three columns, side panels
    Wide,
    /// 900–1280 px: two columns, the inspector as a bottom sheet
    Medium,
    /// under 900 px: one column, the library behind a + button
    Narrow,
}

pub fn layout_for(width: i32) -> Layout {
    if width >= 1280 {
        Layout::Wide
    } else if width >= 900 {
        Layout::Medium
    } else {
        Layout::Narrow
    }
}

/// The pages of the rail, in order: (page id, glyph, name).
pub const RAIL: &[(&str, &str, &str)] = &[
    ("sessoes", "casa", "Sessions"),
    ("modelos", "grade", "Templates"),
    ("gravacoes", "musica", "Recordings"),
    ("agenda", "relogio", "Schedule"),
    ("automacoes", "raio", "Automations"),
    ("ajustes", "engrenagem", "Settings"),
];

pub struct Ui {
    pub core: RefCell<Core>,
    pub window: gtk::ApplicationWindow,
    pub stack: gtk::Stack,
    /// the area over the pages (agent panel, dialogs, toasts)
    pub overlay: gtk::Overlay,
    pub search: gtk::Entry,
    rail: RefCell<Vec<(String, gtk::Button)>>,
    page: RefCell<String>,
    layout: Cell<Layout>,
    on_change: RefCell<Vec<Box<dyn Fn(&Rc<Ui>, Change)>>>,
    on_tick: RefCell<Vec<Box<dyn Fn(&Rc<Ui>)>>>,
    on_page: RefCell<Vec<Box<dyn Fn(&Rc<Ui>, &str)>>>,
    toast_box: gtk::Box,
    pub app: gtk::Application,
}

impl Ui {
    /// Tells the pages something changed.
    pub fn changed(self: &Rc<Self>, c: Change) {
        let list = self.on_change.take();
        for f in &list {
            f(self, c);
        }
        let mut cur = self.on_change.borrow_mut();
        let new = std::mem::take(&mut *cur);
        *cur = list;
        cur.extend(new);
    }

    pub fn on_change(&self, f: impl Fn(&Rc<Ui>, Change) + 'static) {
        self.on_change.borrow_mut().push(Box::new(f));
    }

    /// Called ~30 times a second (live meters, timers).
    pub fn on_tick(&self, f: impl Fn(&Rc<Ui>) + 'static) {
        self.on_tick.borrow_mut().push(Box::new(f));
    }

    /// Called when another page is shown.
    pub fn on_page(&self, f: impl Fn(&Rc<Ui>, &str) + 'static) {
        self.on_page.borrow_mut().push(Box::new(f));
    }

    pub fn layout(&self) -> Layout {
        self.layout.get()
    }

    pub fn page(&self) -> String {
        self.page.borrow().clone()
    }

    /// Shows a page ("sessoes", "modelos", "editor", "gravacoes", "agenda", "automacoes", "ajustes", "boas-vindas",
    /// "sem-som").
    pub fn go(self: &Rc<Self>, page: &str) {
        let target = if page == "automacoes" { "agenda" } else { page };
        if self.stack.child_by_name(target).is_none() {
            return;
        }
        // the editor slides over the list it came from; the rail's pages dissolve
        self.stack.set_transition_type(if target == "editor" || self.page() == "editor" {
            gtk::StackTransitionType::SlideLeftRight
        } else {
            gtk::StackTransitionType::Crossfade
        });
        self.stack.set_visible_child_name(target);
        *self.page.borrow_mut() = page.to_string();
        let rail_page = match page {
            "editor" | "boas-vindas" | "sem-som" => "sessoes",
            p => p,
        };
        for (id, b) in self.rail.borrow().iter() {
            if id == rail_page {
                b.add_css_class("ativo");
            } else {
                b.remove_css_class("ativo");
            }
        }
        let list = self.on_page.take();
        for f in &list {
            f(self, page);
        }
        let mut cur = self.on_page.borrow_mut();
        let new = std::mem::take(&mut *cur);
        *cur = list;
        cur.extend(new);
    }

    /// Opens a session in the editor.
    pub fn open_session(self: &Rc<Self>, idx: usize) {
        self.core.borrow_mut().open(idx);
        self.changed(Change::Selection);
        self.go("editor");
    }

    /// Starts or stops a session.
    pub fn toggle(self: &Rc<Self>, idx: usize) {
        self.core.borrow_mut().toggle_run(idx);
        self.changed(Change::Running);
    }

    /// A short message at the bottom of the window (comes in from below with the rápida spring, goes in 4 s).
    pub fn toast(self: &Rc<Self>, text: &str, action: Option<(&str, Box<dyn Fn(&Rc<Ui>)>)>) {
        let t = hbox(12, "ka-aviso");
        t.append(&label(text, "ka-aviso-texto"));
        if let Some((a, f)) = action {
            let b = widgets::button(a, "secundario", None);
            let ui = self.clone();
            let t2 = t.clone();
            b.connect_clicked(move |_| {
                f(&ui);
                t2.set_visible(false);
            });
            t.append(&b);
        }
        let anim = widgets::animated(&t);
        self.toast_box.append(&anim);
        anim.entrar((0.0, 24.0), 1.0);
        let weak = anim.downgrade();
        let tb = self.toast_box.clone();
        glib::timeout_add_seconds_local_once(4, move || {
            if let Some(a) = weak.upgrade() {
                let tb = tb.clone();
                let a2 = a.clone();
                a.sair((0.0, 24.0), 1.0, move || tb.remove(&a2));
            }
        });
    }

    /// Asks before quitting while something records; otherwise saves and quits.
    pub fn quit(self: &Rc<Self>) {
        self.core.borrow_mut().request_quit();
        self.after_quit_request();
    }

    fn after_quit_request(self: &Rc<Self>) {
        let (now, ask) = {
            let c = self.core.borrow();
            (c.quit_now, c.quit_state == 1)
        };
        if now {
            self.core.borrow_mut().save_now();
            self.app.quit();
        } else if ask {
            estados::confirm_quit(self);
        }
    }
}

fn window_controls() -> gtk::Box {
    let c = hbox(8, "");
    c.set_valign(Align::Center);
    for (glyph, what, tip) in [("minimizar", 0, tr("Minimize")), ("maximizar", 1, tr("Maximize")), ("fechar", 2, tr("Close"))] {
        let b = gtk::Button::new();
        b.add_css_class("ka-controle-janela");
        b.set_child(Some(&icons::icon(glyph, "text", 16)));
        b.set_tooltip_text(Some(tip));
        b.set_valign(Align::Center);
        b.connect_clicked(move |b| {
            let Some(w) = b.root().and_downcast::<gtk::Window>() else { return };
            match what {
                0 => w.minimize(),
                1 => {
                    if w.is_maximized() {
                        w.unmaximize()
                    } else {
                        w.maximize()
                    }
                }
                _ => {
                    w.close();
                }
            }
        });
        c.append(&b);
    }
    c
}

fn menu_model() -> gio::Menu {
    let m = gio::Menu::new();
    let s1 = gio::Menu::new();
    s1.append(Some(tr("New session…")), Some("app.nova"));
    s1.append(Some(tr("Import session…")), Some("app.importar"));
    s1.append(Some(tr("Export session…")), Some("app.exportar"));
    m.append_section(None, &s1);
    let s2 = gio::Menu::new();
    s2.append(Some(tr("Undo")), Some("app.desfazer"));
    s2.append(Some(tr("Redo")), Some("app.refazer"));
    m.append_section(None, &s2);
    let s3 = gio::Menu::new();
    s3.append(Some(tr("Settings")), Some("app.ajustes"));
    s3.append(Some(tr("Script help")), Some("app.ajuda-scripts"));
    s3.append(Some(tr("About Kalango Audio")), Some("app.sobre"));
    s3.append(Some(tr("Quit")), Some("app.sair"));
    m.append_section(None, &s3);
    m
}

fn build(app: &gtk::Application) -> Rc<Ui> {
    let (w, h) = std::env::var("KALANGO_AUDIO_TAMANHO")
        .ok()
        .and_then(|s| s.split_once('x').and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?))))
        .unwrap_or((1500, 910));
    let window = gtk::ApplicationWindow::builder().application(app).title("Kalango Audio").default_width(w).default_height(h).build();
    window.add_css_class("ka-janela");
    window.set_icon_name(Some(APP_ID));
    let no_bar = gtk::Box::new(Orientation::Horizontal, 0);
    no_bar.set_visible(false);
    window.set_titlebar(Some(&no_bar));

    // the rail: the app's icon, then its pages
    let rail_box = vbox(4, "ka-trilho");
    let logo = gtk::Image::from_paintable(Some(&gdk::Texture::from_bytes(&glib::Bytes::from_static(include_bytes!(
        "../../assets/icons/hicolor/128x128/apps/os.kalango.Audio.png"
    )))
    .expect("app icon")));
    logo.set_pixel_size(36);
    logo.set_size_request(36, 36);
    logo.set_halign(Align::Center);
    logo.set_margin_bottom(4);
    rail_box.append(&logo);
    let sep = hbox(0, "ka-trilho-linha");
    rail_box.append(&sep);

    // the bar: name, search or request, menu, window buttons (drag it to move the window)
    let bar = hbox(10, "ka-barra");
    let name = label("Kalango Audio", "ka-titulo-app");
    name.set_hexpand(true);
    bar.append(&name);
    let search = gtk::Entry::new();
    search.add_css_class("ka-pedir");
    search.set_size_request(340, 32);
    search.set_valign(Align::Center);
    search.set_placeholder_text(Some(tr("Search or ask: “record the meeting and transcribe”")));
    search.set_primary_icon_paintable(icons::texture("spark", "accent", 13).as_ref());
    let sbox = gtk::Overlay::new();
    sbox.set_child(Some(&search));
    let key = label("Ctrl K", "ka-atalho");
    key.set_halign(Align::End);
    key.set_valign(Align::Center);
    key.set_margin_end(8);
    key.set_can_target(false);
    sbox.add_overlay(&key);
    bar.append(&sbox);
    let menu = gtk::MenuButton::new();
    menu.add_css_class("ka-icone");
    menu.set_child(Some(&icons::icon("menu", "muted", 16)));
    menu.set_valign(Align::Center);
    menu.set_menu_model(Some(&menu_model()));
    menu.set_tooltip_text(Some(tr("Menu")));
    bar.append(&menu);
    bar.append(&window_controls());
    let handle = gtk::WindowHandle::new();
    handle.set_child(Some(&bar));

    let stack = gtk::Stack::new();
    stack.set_transition_duration(220);
    stack.set_hexpand(true);
    stack.set_vexpand(true);
    stack.add_css_class("ka-paginas");
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&stack));
    let toast_box = vbox(8, "");
    toast_box.set_halign(Align::Center);
    toast_box.set_valign(Align::End);
    toast_box.set_margin_bottom(24);
    overlay.add_overlay(&toast_box);
    toast_box.set_can_target(true);

    let right = vbox(0, "");
    right.append(&handle);
    right.append(&overlay);
    right.set_hexpand(true);
    let root = hbox(0, "ka-raiz");
    root.append(&rail_box);
    root.append(&right);
    window.set_child(Some(&root));

    let core = if std::env::var_os("KALANGO_AUDIO_DEMO").is_some() { estados::demo_core() } else { Core::load() };
    let ui = Rc::new(Ui {
        core: RefCell::new(core),
        window: window.clone(),
        stack: stack.clone(),
        overlay,
        search: search.clone(),
        rail: RefCell::new(vec![]),
        page: RefCell::new(String::new()),
        layout: Cell::new(layout_for(w)),
        on_change: RefCell::new(vec![]),
        on_tick: RefCell::new(vec![]),
        on_page: RefCell::new(vec![]),
        toast_box,
        app: app.clone(),
    });

    for (id, glyph, title) in RAIL {
        let b = gtk::Button::new();
        b.add_css_class("ka-trilho-botao");
        b.set_child(Some(&icons::icon(glyph, "muted", 18)));
        b.set_tooltip_text(Some(tr(title)));
        b.set_halign(Align::Center);
        let u = ui.clone();
        let id2 = id.to_string();
        b.connect_clicked(move |_| u.go(&id2));
        rail_box.append(&b);
        ui.rail.borrow_mut().push((id.to_string(), b));
    }
    // the rail's glyphs light up with the page
    ui.on_page(|ui, page| {
        let rail_page = match page {
            "editor" | "boas-vindas" | "sem-som" => "sessoes",
            p => p,
        };
        for (id, b) in ui.rail.borrow().iter() {
            if let Some(img) = b.child().and_downcast::<gtk::Image>() {
                let glyph = RAIL.iter().find(|r| r.0 == id).map(|r| r.1).unwrap_or("casa");
                icons::set(&img, glyph, if id == rail_page { "accent" } else { "muted" }, 18);
            }
        }
    });

    // the pages
    stack.add_named(&inicio::page(&ui), Some("sessoes"));
    stack.add_named(&modelos::page(&ui), Some("modelos"));
    stack.add_named(&editor::page(&ui), Some("editor"));
    stack.add_named(&gravacoes::page(&ui), Some("gravacoes"));
    stack.add_named(&agenda::page(&ui), Some("agenda"));
    stack.add_named(&ajustes::page(&ui), Some("ajustes"));
    stack.add_named(&estados::welcome(&ui), Some("boas-vindas"));
    stack.add_named(&estados::no_sound(&ui), Some("sem-som"));

    // the search: a session or template by name, or (three words or more) a request to the agent
    {
        let u = ui.clone();
        search.connect_activate(move |e| {
            let text = e.text().to_string();
            if text.trim().is_empty() {
                return;
            }
            e.set_text("");
            let found = u.core.borrow().find_session(&text);
            if let Some(i) = found {
                u.open_session(i);
            } else {
                agente::ask(&u, &text);
            }
        });
    }

    // width classes
    {
        let u = ui.clone();
        window.connect_default_width_notify(move |w| {
            let l = layout_for(w.default_width());
            if l != u.layout.get() {
                u.layout.set(l);
                u.changed(Change::Layout);
            }
        });
    }
    {
        let u = ui.clone();
        window.connect_maximized_notify(move |w| {
            let width = if w.is_maximized() { w.width().max(w.default_width()) } else { w.default_width() };
            let l = layout_for(width);
            if l != u.layout.get() {
                u.layout.set(l);
                u.changed(Change::Layout);
            }
        });
    }

    // closing the window asks first while something records
    {
        let u = ui.clone();
        window.connect_close_request(move |_| {
            if u.core.borrow().quit_now {
                return glib::Propagation::Proceed;
            }
            u.quit();
            glib::Propagation::Stop
        });
    }

    // the app's live work: engines, meters, timers, events
    {
        let u = ui.clone();
        let was_running = Cell::new(0usize);
        let was_recs = Cell::new(true);
        glib::timeout_add_local(std::time::Duration::from_millis(33), move || {
            let (running, rescan, quit, dialogs, wants_editor) = {
                let mut c = u.core.borrow_mut();
                c.tick();
                let d = std::mem::take(&mut c.dialogs);
                let we = std::mem::replace(&mut c.wants_editor_setting, false);
                (c.running.len(), c.rec_scanned.is_none(), c.quit_now, d, we)
            };
            if running != was_running.get() {
                was_running.set(running);
                u.changed(Change::Running);
            }
            if rescan && !was_recs.get() {
                u.changed(Change::Recordings);
            }
            was_recs.set(!rescan);
            for d in dialogs {
                u.toast(&d, None);
            }
            if wants_editor {
                u.go("ajustes");
                u.toast(tr("Choose the audio editor that opens recordings."), None);
            }
            if crate::script::take_focus_request() {
                u.window.present();
            }
            let list = u.on_tick.take();
            for f in &list {
                f(&u);
            }
            let mut cur = u.on_tick.borrow_mut();
            let new = std::mem::take(&mut *cur);
            *cur = list;
            cur.extend(new);
            drop(cur);
            if quit {
                u.core.borrow_mut().save_now();
                u.app.quit();
                return glib::ControlFlow::Break;
            }
            glib::ControlFlow::Continue
        });
    }

    actions(app, &ui);
    ui
}

fn actions(app: &gtk::Application, ui: &Rc<Ui>) {
    let add = |name: &str, accels: &[&str], f: Box<dyn Fn(&Rc<Ui>)>| {
        let a = gio::SimpleAction::new(name, None);
        let u = ui.clone();
        a.connect_activate(move |_, _| f(&u));
        app.add_action(&a);
        if !accels.is_empty() {
            app.set_accels_for_action(&format!("app.{name}"), accels);
        }
    };
    add("nova", &["<Control>n"], Box::new(|u| u.go("modelos")));
    add("buscar", &["<Control>k"], Box::new(|u| {
        u.search.grab_focus();
    }));
    add("rodar", &["<Control>r"], Box::new(|u| {
        let (n, cur) = {
            let c = u.core.borrow();
            (c.sessions.len(), c.cur)
        };
        if n > 0 {
            u.toggle(cur);
        }
    }));
    add("desfazer", &["<Control>z"], Box::new(|u| {
        u.core.borrow_mut().undo();
        u.changed(Change::Sessions);
        u.changed(Change::Selection);
    }));
    add("refazer", &["<Control><Shift>z", "<Control>y"], Box::new(|u| {
        u.core.borrow_mut().redo();
        u.changed(Change::Sessions);
        u.changed(Change::Selection);
    }));
    add("ajustes", &["<Control>comma"], Box::new(|u| u.go("ajustes")));
    add("sair", &["<Control>q"], Box::new(|u| u.quit()));
    add("importar", &[], Box::new(|u| estados::import_dialog(u)));
    add("exportar", &[], Box::new(|u| estados::export_dialog(u)));
    add("sobre", &[], Box::new(|u| estados::about(u)));
    add("ajuda-scripts", &[], Box::new(|u| agenda::script_help(u)));
}

/// For checking screens without a compositor screenshot: after `KALANGO_AUDIO_CAPTURA_MS` (default 2500 ms) the
/// window is drawn into a PNG at that path; with `KALANGO_AUDIO_CAPTURA_SAIR` the app then quits.
fn capture_later(ui: &Rc<Ui>, path: String) {
    let ms = std::env::var("KALANGO_AUDIO_CAPTURA_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(2500);
    let u = ui.clone();
    glib::timeout_add_local_once(std::time::Duration::from_millis(ms), move || {
        let w = &u.window;
        let paintable = gtk::WidgetPaintable::new(Some(w));
        let (width, height) = (w.width() as f64, w.height() as f64);
        let snap = gtk::Snapshot::new();
        paintable.snapshot(&snap, width, height);
        if let (Some(node), Some(native)) = (snap.to_node(), w.native()) {
            if let Some(r) = native.renderer() {
                let tex = r.render_texture(&node, None);
                let _ = tex.save_to_png(&path);
            }
        }
        if std::env::var_os("KALANGO_AUDIO_CAPTURA_SAIR").is_some() {
            u.core.borrow_mut().stop_all();
            u.app.quit();
        }
    });
}

/// Runs the app (returns the exit code).
pub fn run() -> glib::ExitCode {
    let app = gtk::Application::builder().application_id(APP_ID).flags(gio::ApplicationFlags::NON_UNIQUE).build();
    app.connect_startup(|_| {
        if let Some(d) = gdk::Display::default() {
            theme::start(&d);
        }
    });
    app.connect_activate(|app| {
        let ui = build(app);
        let start = std::env::var("KALANGO_AUDIO_PAGINA").ok().unwrap_or_default();
        let first = {
            let c = ui.core.borrow();
            if !estados::sound_available() {
                "sem-som".to_string()
            } else if !start.is_empty() {
                start
            } else if c.sessions.is_empty() && !c.store.prefs.welcomed {
                "boas-vindas".to_string()
            } else {
                "sessoes".to_string()
            }
        };
        ui.go(&first);
        ui.window.present();
        estados::start_hooks(&ui);
        if let Ok(path) = std::env::var("KALANGO_AUDIO_CAPTURA") {
            capture_later(&ui, path);
        }
        // keep the window's Rc alive as long as the app runs
        unsafe {
            ui.window.set_data("ka-ui", ui.clone());
        }
    });
    app.run_with_args::<&str>(&[])
}
