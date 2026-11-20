mod audio;
mod dsp;
mod engine;
mod media;
mod ladspa;
mod model;
mod script;
mod ui;

use eframe::egui;

/// The app icon (KalangoOS art for Kalango Audio), embedded so the window has it without installing.
const ICON_PNG: &[u8] = include_bytes!("../assets/icons/hicolor/128x128/apps/os.kalango.Audio.png");

/// The icon as RGBA pixels, scaled to `size` x `size`.
pub fn icon_rgba(size: u32) -> Vec<u8> {
    match image::load_from_memory_with_format(ICON_PNG, image::ImageFormat::Png) {
        Ok(img) => img.resize_exact(size, size, image::imageops::FilterType::Triangle).to_rgba8().into_raw(),
        Err(_) => vec![0; (size * size * 4) as usize],
    }
}

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if let Some(i) = args.iter().position(|a| a == "--write-icon") {
        let path = args.get(i + 1).cloned().unwrap_or_else(|| "kalango-audio.png".into());
        if let Some(img) = image::RgbaImage::from_raw(256, 256, icon_rgba(256)) { let _ = img.save(&path); println!("ícone salvo em {path}"); }
        return Ok(());
    }
    if let Some(i) = args.iter().position(|a| a == "--selftest") {
        let dir = args.get(i + 1).cloned().unwrap_or_else(|| "/tmp/kalango-audio-selftest".into());
        ui::selftest(&dir);
        return Ok(());
    }
    if let Some(i) = args.iter().position(|a| a == "--selftest-all") {
        let dir = args.get(i + 1).cloned().unwrap_or_else(|| "/tmp/kalango-audio-selftest-all".into());
        ui::selftest_all(&dir);
        return Ok(());
    }
    if let Some(i) = args.iter().position(|a| a == "--selftest-rec") {
        let dir = args.get(i + 1).cloned().unwrap_or_else(|| "/tmp/kalango-audio-selftest-rec".into());
        ui::selftest_rec(&dir);
        return Ok(());
    }
    if args.iter().any(|a| a == "--selftest-load") {
        ui::selftest_load();
        return Ok(());
    }
    let shot = args.iter().position(|a| a == "--screenshot").and_then(|i| args.get(i + 1).cloned());
    let mut vp = egui::ViewportBuilder::default()
        .with_inner_size([1280.0, 800.0])
        .with_min_inner_size([900.0, 560.0])
        .with_title("Kalango Audio")
        .with_app_id("os.kalango.Audio")
        .with_icon(egui::IconData { rgba: icon_rgba(64), width: 64, height: 64 });
    if shot.is_some() {
        vp = vp.with_active(false);
    }
    let options = eframe::NativeOptions { viewport: vp, ..Default::default() };
    eframe::run_native(
        "Kalango Audio",
        options,
        Box::new(move |cc| Ok(Box::new(ui::App::new(cc, shot.clone())))),
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_embedded_icon_decodes_with_a_transparent_corner() {
        let px = super::icon_rgba(64);
        assert_eq!(px.len(), 64 * 64 * 4);
        assert_eq!(px[3], 0, "the corner is transparent");
        let centre = ((32 * 64 + 32) * 4) as usize;
        assert!(px[centre + 3] > 200, "the middle is opaque");
    }
}
