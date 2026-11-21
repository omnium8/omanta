//! Where omanta keeps things, following the XDG base directories.

use std::env;
use std::path::PathBuf;

fn home() -> PathBuf {
    env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/tmp"))
}

fn xdg(var: &str, fallback: &str) -> PathBuf {
    match env::var_os(var) {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => home().join(fallback),
    }
}

/// `~/.config/omanta/config.toml`
pub fn config_file() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config").join("omanta/config.toml")
}

/// `~/.local/share/omanta`: the Kokoro models and the sidecar's Python venv.
pub fn data_dir() -> PathBuf {
    xdg("XDG_DATA_HOME", ".local/share").join("omanta")
}

pub fn model_dir() -> PathBuf {
    data_dir().join("models/Kokoro-82M-ONNX")
}

pub fn venv_dir() -> PathBuf {
    data_dir().join("venv")
}

/// `~/.local/state/omanta`: the last session of each agent.
pub fn state_dir() -> PathBuf {
    xdg("XDG_STATE_HOME", ".local/state").join("omanta")
}

/// `$XDG_RUNTIME_DIR/omanta`: the sockets. Private to the user.
pub fn runtime_dir() -> PathBuf {
    match env::var_os("XDG_RUNTIME_DIR") {
        Some(v) if !v.is_empty() => PathBuf::from(v).join("omanta"),
        _ => PathBuf::from(format!("/tmp/omanta-{}", unsafe { libc::getuid() })),
    }
}

pub fn daemon_socket() -> PathBuf {
    runtime_dir().join("daemon.sock")
}

pub fn chat_socket() -> PathBuf {
    runtime_dir().join("chat.sock")
}

/// The Kokoro sidecar's Python source: the package installs it in
/// /usr/share/omanta/sidecar; a checkout has it beside Cargo.toml.
pub fn sidecar_dir() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(v) = env::var_os("OMANTA_SIDECAR_DIR") {
        candidates.push(PathBuf::from(v));
    }
    if let Ok(exe) = env::current_exe() {
        // target/release/omanta -> the checkout's sidecar/
        if let Some(root) = exe.parent().and_then(|p| p.parent()).and_then(|p| p.parent()) {
            candidates.push(root.join("sidecar"));
        }
    }
    candidates.push(PathBuf::from("/usr/share/omanta/sidecar"));
    candidates.into_iter().find(|p| p.join("server.py").is_file())
}

/// The theme Omarchy is using now.
pub fn omarchy_colors() -> PathBuf {
    home().join(".local/state/omarchy/current/theme/colors.toml")
}

/// Finds a program on PATH, then in ~/.local/bin, which a launcher's PATH can miss.
pub fn find_program(bin: &str) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = env::var_os("PATH").map(|p| env::split_paths(&p).collect()).unwrap_or_default();
    dirs.push(home().join(".local/bin"));
    dirs.into_iter().map(|d| d.join(bin)).find(|p| is_executable(p))
}

fn is_executable(p: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    p.metadata().map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}
