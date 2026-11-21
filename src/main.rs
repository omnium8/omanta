//! omanta: talk with any coding agent by voice. A chat TUI, and a daemon that
//! keeps a local Kokoro voice warm and reads replies (or any selection) aloud.

mod agent;
mod chat;
mod config;
mod daemon;
mod ipc;
mod paths;
mod speech;
mod stt;
mod theme;

use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};

use ipc::Request;

#[derive(Parser)]
#[command(name = "omanta", version, about = "Talk with any coding agent by voice")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Open the chat (the default).
    Chat {
        /// Start listening right away.
        #[arg(long)]
        talk: bool,
    },
    /// Start talking, or stop and send. Opens the chat when it is not open.
    /// Bind it to a key in Hyprland.
    Talk,
    /// Stop the voice, the recording, and the agent's turn.
    Stop,
    /// Omarchy dictation (start, stop or toggle) for the Hyprland keys. While
    /// the omanta window has focus, Voxtype records without its overlay,
    /// because omanta shows its own.
    Dictate {
        #[arg(value_parser = ["start", "stop", "toggle"])]
        action: String,
    },
    /// Read the selected text aloud; press again to stop.
    SpeakSelection,
    /// Say something with the voice.
    Say {
        text: Vec<String>,
    },
    /// Show what the voice service is doing.
    Status,
    /// Download the voice model and build the voice environment.
    Setup,
    /// Run the voice service (systemctl --user enable --now omanta).
    Daemon,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command.unwrap_or(Command::Chat { talk: false }) {
        Command::Chat { talk } => chat::run(talk),
        Command::Daemon => daemon::run(),
        Command::Talk => talk(),
        Command::Dictate { action } => dictate(&action),
        Command::Stop => {
            let _ = ipc::call(&paths::chat_socket(), &Request::Chat { action: "stop".into() }, Duration::from_secs(1));
            ipc::call(&paths::daemon_socket(), &Request::Stop, Duration::from_secs(2)).map(|_| ())
        }
        Command::SpeakSelection => {
            ipc::call(&paths::daemon_socket(), &Request::SpeakSelection { toggle: true }, Duration::from_secs(5)).map(|_| ())
        }
        Command::Say { text } => {
            ipc::call(&paths::daemon_socket(), &Request::Say { text: text.join(" "), queue: false, id: None }, Duration::from_secs(2)).map(|_| ())
        }
        Command::Status => status(),
        Command::Setup => setup(),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("omanta: {e:#}");
            if e.to_string().contains("connect") && e.to_string().contains("daemon.sock") {
                eprintln!("omanta: the voice service is not running. Start it: systemctl --user enable --now omanta");
            }
            ExitCode::FAILURE
        }
    }
}

/// Hands "talk" to the open chat, or opens one that starts listening.
fn talk() -> anyhow::Result<()> {
    if ipc::call(&paths::chat_socket(), &Request::Chat { action: "talk".into() }, Duration::from_secs(1)).is_ok() {
        return Ok(());
    }
    let exe = std::env::current_exe()?;
    let mut cmd = if paths::find_program("omarchy-launch-or-focus-tui").is_some() {
        let mut c = std::process::Command::new("omarchy-launch-or-focus-tui");
        c.arg("--app-id=org.omarchy.omanta").arg(&exe).args(["chat", "--talk"]);
        c
    } else {
        let mut c = std::process::Command::new("xdg-terminal-exec");
        c.arg("--app-id=org.omarchy.omanta").arg("-e").arg(&exe).args(["chat", "--talk"]);
        c
    };
    cmd.spawn()?;
    Ok(())
}

fn dictate(action: &str) -> anyhow::Result<()> {
    use std::process::Command as Run;
    let output = |program: &str, args: &[&str]| {
        Run::new(program).args(args).output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default()
    };
    let window: serde_json::Value = serde_json::from_str(&output("hyprctl", &["activewindow", "-j"])).unwrap_or_default();
    let focused = window["class"] == "org.omarchy.omanta" || window["title"] == "omanta";
    let status = if focused && action != "stop" {
        Run::new("voxtype").args(["record", action, "--no-osd"]).status()?
    } else if focused || paths::find_program("omarchy-dictation").is_none() {
        Run::new("voxtype").args(["record", action]).status()?
    } else {
        Run::new("omarchy-dictation").arg(action).status()?
    };
    anyhow::ensure!(status.success(), "dictation {action} failed");
    Ok(())
}

fn status() -> anyhow::Result<()> {
    let v = ipc::call(&paths::daemon_socket(), &Request::Status, Duration::from_secs(2))?;
    let s: ipc::Status = serde_json::from_value(v)?;
    println!("voice:    {}", if s.ready { "ready" } else { &s.phase });
    println!("speaking: {} ({} queued)", s.speaking, s.queued);
    if !s.problem.is_empty() {
        println!("problem:  {}", s.problem);
    }
    Ok(())
}

fn setup() -> anyhow::Result<()> {
    println!("Voice model: {}", paths::model_dir().display());
    daemon::sidecar::download_models(true)?;
    let sidecar = paths::sidecar_dir().ok_or_else(|| anyhow::anyhow!("sidecar/server.py is missing from the installation"))?;
    println!("Voice environment: {} (Python {})", paths::venv_dir().display(), daemon::sidecar::PYTHON);
    daemon::sidecar::ensure_env(&sidecar, true)?;
    println!("Done. Start the voice service: systemctl --user enable --now omanta");
    Ok(())
}
