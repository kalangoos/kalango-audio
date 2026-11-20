//! The KalangoOS line glyphs (assets/ui, the same ones as the system's apps), drawn in a token colour and redrawn
//! when the theme changes.

use super::theme;
use gtk::prelude::*;
use gtk::{gdk, glib};

/// The glyph's svg by name (the file names of assets/ui, without .svg).
fn source(name: &str) -> Option<&'static str> {
    macro_rules! glyphs {
        ($($n:literal),* $(,)?) => {
            match name { $($n => Some(include_str!(concat!("../../assets/ui/", $n, ".svg"))),)* _ => None }
        };
    }
    glyphs!(
        "acima", "agrupar", "ajustes", "ajustes-d", "alerta", "alfinete", "anel", "anterior", "apps-n", "apps-pontos", "baixar", "busca",
        "cadeado", "camadas", "camera", "casa", "cerebro", "check", "chevron", "chevron-baixo", "chevron-n", "clipe", "codigo", "colar",
        "colunas", "compartilhar", "configuracoes", "copiar", "cpu", "desfazer", "desligar", "disco", "doc", "download", "energia",
        "engrenagem", "enter", "enviar", "erro", "escudo", "estrela-n", "fechar", "feito", "fone", "galeria", "globo", "grade", "imagem",
        "info", "lapis", "layout", "lista", "lixeira", "local", "mais", "maximizar", "menos", "menu", "microfone", "microfone-d",
        "minimizar", "monitor", "musica", "olho", "ordenar", "painel-direita", "painel-esquerda", "paleta", "parar", "pasta", "pausa",
        "ponteiro", "pontos-v", "proxima", "raio", "raio-e", "ramo", "recortar", "reiniciar", "relogio", "sair", "seta-baixo", "seta-cima",
        "seta-direita", "seta-esquerda", "sino", "sol", "spark", "teclado", "terminal", "tesoura", "tocar", "usuario", "video-d", "volume",
        "volume-baixo", "volume-mudo", "wifi",
    )
}

/// The svg recoloured and sized: every `="#RRGGBB"` takes the colour; width and height become `px`.
fn svg(name: &str, color: &str, px: i32) -> Option<String> {
    let s = source(name)?;
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("=\"#") {
        out.push_str(&rest[..i + 2]);
        out.push_str(color);
        rest = &rest[i + 2 + 7..];
    }
    out.push_str(rest);
    // drawn at the pixel size it is shown (and twice that for sharp HiDPI screens)
    let end = out.find('>').unwrap_or(0);
    let mut root = out[..end].to_string();
    for attr in ["width", "height"] {
        let key = format!(" {attr}=\"");
        if let Some(i) = root.find(&key) {
            let a = i + key.len();
            let b = a + root[a..].find('"').unwrap_or(0);
            root.replace_range(a..b, &(px * 2).to_string());
        }
    }
    out.replace_range(..end, &root);
    Some(out)
}

pub fn texture(name: &str, token: &str, px: i32) -> Option<gdk::Texture> {
    let s = svg(name, &theme::hex(token), px)?;
    gdk::Texture::from_bytes(&glib::Bytes::from_owned(s.into_bytes())).ok()
}

/// A glyph in a token colour ("text", "muted", "accent"…), `px` square; follows the theme.
pub fn icon(name: &str, token: &str, px: i32) -> gtk::Image {
    let img = gtk::Image::from_paintable(texture(name, token, px).as_ref());
    img.set_pixel_size(px);
    img.set_size_request(px, px);
    img.set_halign(gtk::Align::Center);
    img.set_valign(gtk::Align::Center);
    img.set_can_target(false);
    follow(&img, name, token, px);
    img
}

/// Changes a glyph (or its colour) later.
pub fn set(img: &gtk::Image, name: &str, token: &str, px: i32) {
    img.set_paintable(texture(name, token, px).as_ref());
    unsafe {
        img.set_data("ka-icon", (name.to_string(), token.to_string(), px));
    }
}

thread_local! {
    static LIVE: std::cell::RefCell<Vec<glib::WeakRef<gtk::Image>>> = const { std::cell::RefCell::new(Vec::new()) };
    static HOOKED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn redraw_all() {
    LIVE.with(|l| {
        l.borrow_mut().retain(|w| match w.upgrade() {
            Some(i) => {
                let d = unsafe { i.data::<(String, String, i32)>("ka-icon").map(|p| p.as_ref().clone()) };
                if let Some((n, t, p)) = d {
                    i.set_paintable(texture(&n, &t, p).as_ref());
                }
                true
            }
            None => false,
        })
    });
}

fn follow(img: &gtk::Image, name: &str, token: &str, px: i32) {
    unsafe {
        img.set_data("ka-icon", (name.to_string(), token.to_string(), px));
    }
    if !HOOKED.with(|h| h.replace(true)) {
        theme::on_change(redraw_all);
    }
    LIVE.with(|l| {
        let mut l = l.borrow_mut();
        if l.len() % 256 == 255 {
            l.retain(|w| w.upgrade().is_some());
        }
        l.push(img.downgrade());
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_glyph_recolours_and_resizes() {
        for n in ["tocar", "casa", "spark", "microfone", "engrenagem"] {
            let s = super::svg(n, "#123456", 20).unwrap();
            assert!(s.contains("#123456") && s.contains("width=\"40\""), "{n}: {s}");
            assert!(!s.contains("#E2E2E6"), "{n} kept its old colour");
        }
        assert!(super::source("nao-existe").is_none());
    }
}
