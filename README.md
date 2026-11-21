# omanta

Talk with any coding agent by voice, in the terminal. Your dictation goes in,
and a local Kokoro voice reads the reply aloud. The agent asks for approval by
voice, and you answer "yes", "no", or what to do instead. omanta also reads
any selected text aloud.

```
╭ omanta ──────────────────────────────────────────────── Claude Code ╮
│ › What does hello.txt in this folder say?                           │
│   · reading hello.txt                                               │
│ It says the secret word is pineapple.                               │
╰ ○ ctrl+space to talk ───────────────────────────────── voice ready ╯
╭ message Claude Code ────────────────────────────────────────────────╮
│ ›                                                                   │
╰─────────────── ctrl+space talk · esc stop · ctrl+o settings · f1 keys ╯
```

## Parts

One binary, `omanta`:

| Command | What it does |
| --- | --- |
| `omanta` | The chat TUI |
| `omanta daemon` | The voice service (systemd user unit `omanta.service`): keeps Kokoro warm, speaks a sentence queue, pauses other media while it speaks |
| `omanta talk` | Start talking, or stop and send; opens the chat when it is not open |
| `omanta stop` | Stop the voice, the recording, and the agent's turn |
| `omanta dictate start\|stop\|toggle` | Omarchy dictation for the Hyprland keys; no Voxtype overlay while omanta has focus |
| `omanta speak-selection` | Read the selected text aloud; press again to stop |
| `omanta say TEXT` / `omanta status` | Use the voice from scripts |
| `omanta setup` | Download the voice model and build its Python environment |

The chat and the daemon talk over Unix sockets in `$XDG_RUNTIME_DIR/omanta/`.

## Agents

omanta runs the agent's CLI in the background, in a machine-readable mode:

- **Claude Code** through `claude -p --input-format stream-json --output-format stream-json`,
  with approvals through `--permission-prompt-tool stdio`.
- **ACP agents** (Agent Client Protocol, JSON-RPC on stdio): OpenCode, Grok
  (`grok agent stdio`), Cursor Agent, Gemini CLI, GitHub Copilot, Qwen Code, Kimi,
  Hermes, and Codex through `codex-acp`.

The agent uses its own login. Log in once in its own TUI: `ctrl+l` opens it in
a terminal (for Cursor Agent, `cursor-agent login`). When an ACP agent asks for
a login, omanta calls `authenticate` and tries once more, so a login made in
the TUI is picked up. omanta follows the agent's permission rules, so it asks only for
what the agent itself would ask.

## How a conversation works

1. **Speech in.** In settings (`ctrl+o`), choose one:
   - **Talk key** (`ctrl+space`): omanta records and sends. With Superwhisper,
     omanta records with `pw-record` and gives the file to `superwhisper transcribe`.
     With Voxtype, the running daemon records (`voxtype record start --file=…`).
     Both use the model that is already loaded, and neither types into other windows.
   - **Your own dictation**: your dictation tool (Omarchy's hold-RightAlt with
     Voxtype) types into the message line.

   While Voxtype records, from any key, and the recording started while
   omanta had the focus, the message line shows your voice as a live ASCII
   waveform in the theme's colors; while it transcribes, a sweep. A recording
   started in another window shows Voxtype's own overlay instead, never both.
   Settings (`ctrl+o`) set the waveform height, 1 to 4 rows; the message line
   has the same height. **Background transparency** (off, 0.1 to 0.9) lets the wallpaper
   show through the omanta window only, in foot, Omarchy's terminal. omanta
   sets it with OSC 11 and gives foot its own background back on exit.
   omanta reads Voxtype's state file (`$XDG_RUNTIME_DIR/voxtype/state`) and
   the microphone level from `voxtype-audio-bridge`. Without the bridge, the
   waveform is drawn, not measured.

   **Send after a pause** (off, 2, 3, 4 or 5 s) sends dictated text by itself. omanta
   tells dictation from typing by speed: a dictation tool types a phrase with
   about a millisecond between characters. Text you type waits for enter. The
   message line counts down, and any key resets it.
2. **Voice rules.** The agent gets a system prompt that makes it talk like a
   person: short answers, no markdown, paths and symbols said as words.
3. **Speech out.** The reply is read aloud sentence by sentence while it streams.
   The next sentence is synthesized while the current one plays. Each sentence
   appears on the screen when its voice starts, so you read and hear the same
   thing; until then the reply shows "thinking…". Code blocks are not read
   aloud and appear with the sentence before them. Talking, or `esc`, stops the
   voice and shows the whole reply.
4. **Approvals by voice.** "Claude Code wants to run a command. Allow it?" Answer
   by voice, by typing, or with `alt+y` / `alt+a` / `alt+n`. "Always" for a shell
   command allows only the programs in that command. **Auto mode** (`tab`)
   allows every action without asking, for this run; `>> auto` shows it.
5. **Status, not noise.** Tool steps show as short lines ("reading main.rs").
   omanta does not read them aloud.
6. **A live feel.** An ASCII wheel in the theme's colors turns backward while
   the agent thinks. While the voice speaks, it walks along the status line
   with the voice's progress and leaves a rippling trail in all the theme's
   colors. The voice service reports the progress (`progress` in its status).

## Keys

| Key | Action |
| --- | --- |
| `ctrl+space` | Talk; press again to send (set another key in settings) |
| `enter` | Send what you typed or dictated |
| `tab` | Auto mode on or off: allow every action without asking, for this run |
| `esc` | Stop the voice, the recording, and the agent |
| `alt+y` / `alt+a` / `alt+n` | Allow / always allow / deny an action |
| `alt+1`…`alt+9` / `alt+s` | Pick an answer / skip a question |
| `ctrl+n` / `ctrl+r` | New conversation / continue the last one |
| `ctrl+o` | Settings |
| `ctrl+l` | Log in to the agent |
| `↑` `↓` `pgup` `pgdn` | Scroll |
| `f1` | Show the keys |
| `ctrl+c` / `ctrl+q` | Quit |

### Keys that work in any window

Add these to `~/.config/hypr/bindings.lua`:

```lua
o.bind("SUPER + ALT + SPACE", "Talk to agent", "omanta talk")
o.bind("SUPER + SHIFT + P", "Read selection aloud", "omanta speak-selection")
```

To hide Voxtype's overlay while omanta has focus, send Omarchy's dictation
keys through `omanta dictate`. It adds `--no-osd` when the focused window is
omanta, and runs `omarchy-dictation` for every other window:

```lua
if o.dictation_installed() then
  o.rebind("ALT + SPACE", "Toggle dictation", "omanta dictate toggle || omarchy-dictation toggle")
  hl.unbind("ALT + Alt_R")
  o.bind_hold("ALT + Alt_R", "Start dictation (push-to-talk)", "omanta dictate start || omarchy-dictation start",
    "Stop dictation (push-to-talk)", "omanta dictate stop || omarchy-dictation stop", { ignore_mods = true })
end
```

## Install

```sh
bin/install   # makepkg -fsi, then `omanta setup` and the voice service
```

`omanta setup` downloads the Kokoro model (about 340 MB) to
`~/.local/share/omanta/models` and builds the voice's Python environment with uv,
on a pinned, uv-managed Python. A mise or pacman Python update cannot break it.

## Files

| Path | Contents |
| --- | --- |
| `~/.config/omanta/config.toml` | Settings |
| `~/.local/share/omanta/` | The voice model and its Python environment |
| `~/.local/state/omanta/sessions.json` | The last conversation of each agent |

## Develop

```sh
bin/build     # target/release/omanta
bin/test      # unit tests
```

A checkout runs as is: `target/release/omanta daemon` finds `sidecar/` beside
`Cargo.toml`.

omanta started as the Linux port of Samanta (`omnium8/samanta`). The old Tauri
version is archived in `omnium8/omanta-tauri`.
