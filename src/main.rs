mod audio;
mod core;
mod dsp;
mod engine;
mod gui;
mod i18n;
mod ladspa;
mod media;
mod meter;
mod model;
mod presets;
mod script;
mod selftest;

/// The app icon (KalangoOS art for Kalango Audio), embedded so `--write-icon` works without installing.
const ICON_PNG: &[u8] = include_bytes!("../assets/icons/hicolor/128x128/apps/os.kalango.Audio.png");

/// The icon as RGBA pixels, scaled to `size` x `size`.
pub fn icon_rgba(size: u32) -> Vec<u8> {
    match image::load_from_memory_with_format(ICON_PNG, image::ImageFormat::Png) {
        Ok(img) => img.resize_exact(size, size, image::imageops::FilterType::Triangle).to_rgba8().into_raw(),
        Err(_) => vec![0; (size * size * 4) as usize],
    }
}

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let arg_after = |flag: &str, default: &str| args.iter().position(|a| a == flag).map(|i| args.get(i + 1).cloned().unwrap_or_else(|| default.to_string()));
    if let Some(path) = arg_after("--write-icon", "kalango-audio.png") {
        if let Some(img) = image::RgbaImage::from_raw(256, 256, icon_rgba(256)) {
            let _ = img.save(&path);
        }
        return std::process::ExitCode::SUCCESS;
    }
    if let Some(dir) = arg_after("--selftest", "/tmp/kalango-audio-selftest") {
        selftest::selftest(&dir);
        return std::process::ExitCode::SUCCESS;
    }
    if let Some(dir) = arg_after("--selftest-all", "/tmp/kalango-audio-selftest-all") {
        selftest::selftest_all(&dir);
        return std::process::ExitCode::SUCCESS;
    }
    if let Some(dir) = arg_after("--selftest-rec", "/tmp/kalango-audio-selftest-rec") {
        selftest::selftest_rec(&dir);
        return std::process::ExitCode::SUCCESS;
    }
    if args.iter().any(|a| a == "--selftest-load") {
        selftest::selftest_load();
        return std::process::ExitCode::SUCCESS;
    }
    if gui::run() == gtk::glib::ExitCode::SUCCESS {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
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
