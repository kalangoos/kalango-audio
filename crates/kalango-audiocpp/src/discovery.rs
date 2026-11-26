//! Where the server program and the model weights are, and which backend to ask for.
//!
//! Nothing here downloads or installs anything: when a piece is missing the caller gets the place where it is
//! expected, so the person can be told what to put there.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::Backend;

/// Environment variable with the absolute path of `audiocpp_server`.
pub const ENV_SERVER: &str = "KALANGO_AUDIOCPP_SERVER";
/// Environment variable with the model: the `.gguf`/`.safetensors` file or the folder that holds it.
pub const ENV_MODEL: &str = "KALANGO_AUDIOCPP_MODEL";
/// Environment variable that forces the backend (`cuda`, `vulkan` or `cpu`).
pub const ENV_BACKEND: &str = "KALANGO_AUDIOCPP_BACKEND";

/// Checkpoint file names the real-time voice isolator is known by, best first.
const MODEL_FILES: [&str; 3] =
    ["denoiser_realtime.safetensors", "voice-isolator-realtime-f32.gguf", "voice-isolator-realtime.gguf"];

/// What was found, or where each piece is expected.
#[derive(Debug, Clone, PartialEq)]
pub struct Discovery {
    /// The server program, or the place it is expected (the variable's value, or the install location).
    pub server: Result<PathBuf, PathBuf>,
    /// The model file, or the place it is expected.
    pub model: Result<PathBuf, PathBuf>,
}

impl Discovery {
    pub fn ready(&self) -> Option<(&Path, &Path)> {
        match (&self.server, &self.model) {
            (Ok(s), Ok(m)) => Some((s, m)),
            _ => None,
        }
    }
}

/// The lookups `discover_with` needs, so tests do not depend on the machine.
pub struct Env<'a> {
    pub var: &'a dyn Fn(&str) -> Option<String>,
}

fn data_home(var: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    match var("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
        Some(d) => PathBuf::from(d),
        None => PathBuf::from(var("HOME").unwrap_or_default()).join(".local/share"),
    }
}

/// The default install place of the server program (`~/.local/share/kalango/audiocpp/bin/audiocpp_server`).
pub fn default_server_path(var: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    data_home(var).join("kalango/audiocpp/bin/audiocpp_server")
}

/// The default place of the model's folder (`~/.local/share/kalango/modelos/voice-isolator`).
pub fn default_model_dir(var: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    data_home(var).join("kalango/modelos/voice-isolator")
}

fn is_program(p: &Path) -> bool {
    p.metadata().map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

/// The checkpoint inside `path`: the file itself, or the known file name in a folder (a folder holding only another
/// realtime `.gguf` also works).
pub fn resolve_model(path: &Path) -> Option<PathBuf> {
    if path.is_file() {
        return Some(path.to_path_buf());
    }
    if !path.is_dir() {
        return None;
    }
    if let Some(f) = MODEL_FILES.iter().map(|n| path.join(n)).find(|p| p.is_file()) {
        return Some(f);
    }
    let mut found: Vec<PathBuf> = std::fs::read_dir(path)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file() && p.extension().is_some_and(|x| x == "gguf") && p.to_string_lossy().contains("realtime")
        })
        .collect();
    found.sort();
    found.into_iter().next()
}

/// Looks for both pieces. A path given in the environment is the only place looked at for that piece (a typo is
/// reported, not hidden by a fallback); without it the install location is used.
pub fn discover_with(env: &Env) -> Discovery {
    let var = env.var;
    let server_path =
        var(ENV_SERVER).filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| default_server_path(var));
    let server = if server_path.is_absolute() && is_program(&server_path) { Ok(server_path) } else { Err(server_path) };
    let model_path =
        var(ENV_MODEL).filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| default_model_dir(var));
    let model = resolve_model(&model_path).ok_or(model_path);
    Discovery { server, model }
}

/// [`discover_with`] on the real environment.
pub fn discover() -> Discovery {
    discover_with(&Env { var: &|k| std::env::var(k).ok() })
}

/// The backend to ask the server for: the one in `KALANGO_AUDIOCPP_BACKEND`, else CUDA when an NVIDIA driver is
/// loaded, else the CPU. The server then reports what it really uses (`Server::reported_backend`), and that is what
/// the rest of the program trusts.
pub fn choose_backend(var: &dyn Fn(&str) -> Option<String>, nvidia_present: bool) -> Backend {
    match var(ENV_BACKEND).as_deref() {
        Some("cuda") => Backend::Cuda,
        Some("vulkan") => Backend::Vulkan,
        Some("cpu") => Backend::Cpu,
        _ if nvidia_present => Backend::Cuda,
        _ => Backend::Cpu,
    }
}

/// Whether an NVIDIA driver is loaded (its control device exists).
pub fn nvidia_present() -> bool {
    Path::new("/dev/nvidiactl").exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("kalango-discovery-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn lookup(map: HashMap<&'static str, String>) -> impl Fn(&str) -> Option<String> {
        move |k| map.get(k).cloned()
    }

    fn program(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn nothing_installed_reports_the_default_places() {
        let home = tmp("none");
        let var = lookup(HashMap::from([("HOME", home.to_string_lossy().into_owned())]));
        let d = discover_with(&Env { var: &var });
        assert_eq!(d.server, Err(home.join(".local/share/kalango/audiocpp/bin/audiocpp_server")));
        assert_eq!(d.model, Err(home.join(".local/share/kalango/modelos/voice-isolator")));
        assert!(d.ready().is_none());
    }

    #[test]
    fn the_install_places_are_found_and_xdg_data_home_wins() {
        let home = tmp("installed");
        program(&home.join("data/kalango/audiocpp/bin/audiocpp_server"));
        let model_dir = home.join("data/kalango/modelos/voice-isolator");
        std::fs::create_dir_all(&model_dir).unwrap();
        std::fs::write(model_dir.join("denoiser_realtime.safetensors"), b"x").unwrap();
        let var = lookup(HashMap::from([
            ("HOME", "/nonexistent".to_string()),
            ("XDG_DATA_HOME", home.join("data").to_string_lossy().into_owned()),
        ]));
        let d = discover_with(&Env { var: &var });
        let (server, model) = d.ready().expect("both found");
        assert!(server.ends_with("audiocpp_server") && model.ends_with("denoiser_realtime.safetensors"));
    }

    #[test]
    fn the_environment_decides_and_a_wrong_value_is_reported_not_replaced() {
        let home = tmp("env");
        // the install place has everything, but the variables point elsewhere
        program(&home.join(".local/share/kalango/audiocpp/bin/audiocpp_server"));
        let other = home.join("elsewhere/server");
        program(&other);
        let var = lookup(HashMap::from([
            ("HOME", home.to_string_lossy().into_owned()),
            (ENV_SERVER, other.to_string_lossy().into_owned()),
            (ENV_MODEL, "/typo/model.gguf".to_string()),
        ]));
        let d = discover_with(&Env { var: &var });
        assert_eq!(d.server, Ok(other));
        assert_eq!(d.model, Err(PathBuf::from("/typo/model.gguf")));
    }

    #[test]
    fn a_file_that_is_not_executable_or_a_relative_path_is_not_a_server() {
        let home = tmp("exec");
        let plain = home.join("plain");
        std::fs::write(&plain, b"x").unwrap();
        let var = lookup(HashMap::from([
            ("HOME", home.to_string_lossy().into_owned()),
            (ENV_SERVER, plain.to_string_lossy().into_owned()),
        ]));
        assert!(discover_with(&Env { var: &var }).server.is_err());
        let var = lookup(HashMap::from([
            ("HOME", home.to_string_lossy().into_owned()),
            (ENV_SERVER, "audiocpp_server".to_string()),
        ]));
        assert_eq!(discover_with(&Env { var: &var }).server, Err(PathBuf::from("audiocpp_server")));
    }

    #[test]
    fn a_model_folder_resolves_to_the_realtime_checkpoint() {
        let d = tmp("model");
        assert_eq!(resolve_model(&d), None);
        std::fs::write(d.join("denoiser.safetensors"), b"offline only").unwrap();
        assert_eq!(resolve_model(&d), None, "the offline checkpoint alone is not a realtime model");
        std::fs::write(d.join("denoiser_realtime.safetensors"), b"x").unwrap();
        assert_eq!(resolve_model(&d), Some(d.join("denoiser_realtime.safetensors")));
        let g = tmp("gguf");
        std::fs::write(g.join("voice-isolator-realtime-q8.gguf"), b"x").unwrap();
        assert_eq!(resolve_model(&g), Some(g.join("voice-isolator-realtime-q8.gguf")));
        assert_eq!(
            resolve_model(&d.join("denoiser_realtime.safetensors")),
            Some(d.join("denoiser_realtime.safetensors"))
        );
    }

    #[test]
    fn the_backend_follows_the_variable_then_the_driver() {
        let none = lookup(HashMap::new());
        assert_eq!(choose_backend(&none, true), Backend::Cuda);
        assert_eq!(choose_backend(&none, false), Backend::Cpu);
        let forced = lookup(HashMap::from([(ENV_BACKEND, "vulkan".to_string())]));
        assert_eq!(choose_backend(&forced, false), Backend::Vulkan);
        let cpu = lookup(HashMap::from([(ENV_BACKEND, "cpu".to_string())]));
        assert_eq!(choose_backend(&cpu, true), Backend::Cpu);
    }
}
