//! The Kokoro voice: a Python server (kokoro-onnx) on 127.0.0.1, in a venv
//! that uv builds on a pinned, uv-managed Python. A mise or pacman Python
//! update cannot break it.

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::sleep;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::paths;

pub const PYTHON: &str = "3.14";
const MODEL_FILES: [&str; 2] = ["kokoro-v1.0.onnx", "voices-v1.0.bin"];
const MODEL_URL: &str = "https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0";

/// What the voice is doing, for `status`.
#[derive(Default)]
pub struct Health {
    pub phase: String,
    pub problem: String,
    pub ready: bool,
}

pub type Shared = Arc<Mutex<Health>>;

fn set(h: &Shared, phase: &str, problem: &str, ready: bool) {
    let mut g = h.lock().unwrap();
    g.phase = phase.into();
    g.problem = problem.into();
    g.ready = ready;
}

pub fn base(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

pub fn models_present() -> bool {
    MODEL_FILES.iter().all(|f| paths::model_dir().join(f).is_file())
}

/// Downloads the Kokoro model files (about 340 MB) with curl. `verbose` shows
/// the progress (for `omanta setup`).
pub fn download_models(verbose: bool) -> anyhow::Result<()> {
    let dir = paths::model_dir();
    std::fs::create_dir_all(&dir)?;
    for f in MODEL_FILES {
        let target = dir.join(f);
        if target.is_file() {
            if verbose {
                println!("have {}", target.display());
            }
            continue;
        }
        if verbose {
            println!("downloading {f} …");
        }
        // One part file per process, so `omanta setup` and the daemon never
        // write the same file.
        let part = dir.join(format!("{f}.{}.part", std::process::id()));
        let ok = Command::new("curl")
            .args(["-L", "--fail", "--retry", "3", if verbose { "--progress-bar" } else { "--silent" }, "-o"])
            .arg(&part)
            .arg(format!("{MODEL_URL}/{f}"))
            .status()?
            .success();
        if !ok {
            anyhow::bail!("could not download {f}");
        }
        std::fs::rename(&part, &target)?;
    }
    Ok(())
}

fn python() -> PathBuf {
    paths::venv_dir().join("bin/python")
}

fn env_works() -> bool {
    python().is_file()
        && Command::new(python())
            .args(["-c", "import kokoro_onnx, uvicorn, fastapi, soundfile"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
}

/// Builds the venv from the sidecar's uv.lock when it is missing or broken.
/// `verbose` shows uv's output (for `omanta setup`).
pub fn ensure_env(sidecar: &Path, verbose: bool) -> anyhow::Result<()> {
    if env_works() {
        return Ok(());
    }
    if paths::find_program("uv").is_none() {
        anyhow::bail!("uv is missing: install it with `sudo pacman -S uv`");
    }
    let mut cmd = Command::new("uv");
    cmd.args(["sync", "--frozen", "--no-install-project", "--python", PYTHON, "--python-preference", "only-managed"])
        .arg("--project")
        .arg(sidecar)
        .env("UV_PROJECT_ENVIRONMENT", paths::venv_dir());
    if !verbose {
        cmd.stdout(Stdio::null()).stderr(Stdio::piped());
    }
    let out = cmd.output()?;
    if !out.status.success() || !env_works() {
        let err = String::from_utf8_lossy(&out.stderr);
        anyhow::bail!("uv could not build the voice environment: {}", err.lines().last().unwrap_or(""));
    }
    Ok(())
}

fn healthy(port: u16) -> Option<Value> {
    ureq::get(&format!("{}/health", base(port)))
        .timeout(Duration::from_millis(800))
        .call()
        .ok()?
        .into_json::<Value>()
        .ok()
}

fn spawn(sidecar: &Path, port: u16) -> std::io::Result<Child> {
    let mut cmd = Command::new(python());
    cmd.arg("server.py")
        .current_dir(sidecar)
        .env("OMANTA_HOST", "127.0.0.1")
        .env("OMANTA_PORT", port.to_string())
        .env("OMANTA_MODEL_DIR", paths::model_dir())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    // The voice must not outlive the daemon, even when the daemon is killed.
    unsafe {
        cmd.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            Ok(())
        });
    }
    cmd.spawn()
}

/// Keeps the voice running: installs, starts, warms, and restarts it.
/// `child` holds the process so the daemon can stop it on exit.
pub fn supervise(health: Shared, child: Arc<Mutex<Option<Child>>>, port: u16, voice: impl Fn() -> (String, String)) {
    let Some(sidecar) = paths::sidecar_dir() else {
        set(&health, "error", "the voice server (sidecar/server.py) is missing from the installation", false);
        return;
    };
    let mut failures = 0u32;
    loop {
        if !models_present() {
            set(&health, "downloading", "", false);
            if let Err(e) = download_models(false) {
                set(&health, "error", &format!("{e}: check the network, or run `omanta setup`"), false);
                sleep(Duration::from_secs(30));
                continue;
            }
        }
        if let Some(h) = healthy(port) {
            if h["engine"].is_null() && !h["loading"].as_bool().unwrap_or(false) {
                set(&health, "error", &format!("port {port} is in use by something that is not the voice"), false);
                sleep(Duration::from_secs(10));
                continue;
            }
        } else {
            set(&health, "installing", "", false);
            if let Err(e) = ensure_env(&sidecar, false) {
                set(&health, "error", &e.to_string(), false);
                sleep(Duration::from_secs(30));
                continue;
            }
            set(&health, "loading", "", false);
            match spawn(&sidecar, port) {
                Ok(c) => *child.lock().unwrap() = Some(c),
                Err(e) => {
                    set(&health, "error", &format!("could not start the voice: {e}"), false);
                    sleep(Duration::from_secs(10));
                    continue;
                }
            }
        }

        // Wait for the model, then say one word to warm it up.
        let start = Instant::now();
        let mut up = false;
        while start.elapsed() < Duration::from_secs(120) {
            if exited(&child) {
                break;
            }
            if healthy(port).is_some_and(|h| h["ok"].as_bool().unwrap_or(false)) {
                up = true;
                break;
            }
            sleep(Duration::from_millis(300));
        }
        if up {
            let (name, language) = voice();
            let _ = ureq::post(&format!("{}/warm", base(port)))
                .timeout(Duration::from_secs(30))
                .send_json(json!({"voice": name, "language": language}));
            set(&health, "ready", "", true);
            failures = 0;
            // Watch it; a crash brings the loop round again.
            while !exited(&child) && healthy(port).is_some() {
                sleep(Duration::from_secs(2));
            }
        }
        let why = take_stderr(&child);
        kill(&child);
        failures += 1;
        let problem = if why.is_empty() { "the voice stopped".to_string() } else { format!("the voice stopped: {why}") };
        set(&health, "error", &problem, false);
        // Back off when it keeps failing, so a broken setup does not spin.
        sleep(Duration::from_secs(if failures > 3 { 30 } else { 2 }));
    }
}

fn exited(child: &Arc<Mutex<Option<Child>>>) -> bool {
    match child.lock().unwrap().as_mut() {
        Some(c) => !matches!(c.try_wait(), Ok(None)),
        None => false,
    }
}

fn take_stderr(child: &Arc<Mutex<Option<Child>>>) -> String {
    let mut g = child.lock().unwrap();
    let Some(c) = g.as_mut() else { return String::new() };
    if !matches!(c.try_wait(), Ok(Some(_))) {
        return String::new();
    }
    let mut s = String::new();
    if let Some(mut e) = c.stderr.take() {
        let _ = e.read_to_string(&mut s);
    }
    s.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string()
}

pub fn kill(child: &Arc<Mutex<Option<Child>>>) {
    if let Some(mut c) = child.lock().unwrap().take() {
        unsafe {
            libc::kill(c.id() as i32, libc::SIGTERM);
        }
        for _ in 0..20 {
            if matches!(c.try_wait(), Ok(Some(_))) {
                return;
            }
            sleep(Duration::from_millis(100));
        }
        let _ = c.kill();
        let _ = c.wait();
    }
}

pub fn voices(port: u16) -> Vec<String> {
    ureq::get(&format!("{}/voices", base(port)))
        .timeout(Duration::from_secs(2))
        .call()
        .ok()
        .and_then(|r| r.into_json::<Value>().ok())
        .and_then(|v| v["voices"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()))
        .unwrap_or_default()
}
