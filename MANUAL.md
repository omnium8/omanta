# Manual

Talk with a coding agent by voice. Your dictation goes in. A local Kokoro
voice reads the reply aloud. The agent asks for approval by voice, and you
answer "yes", "no", or what to do instead. omanta also reads selected text
aloud.

Install first: [INSTALL.md](INSTALL.md).

## Commands

One binary, `omanta`:

| Command | What it does |
| --- | --- |
| `omanta` | The chat TUI |
| `omanta daemon` | The voice service (systemd user unit `omanta.service`): keeps Kokoro warm, speaks a sentence queue, pauses other media while it speaks |
| `omanta talk` | Start talking, or stop and send; opens the chat when it is not open |
| `omanta stop` | Stop the voice, the recording, and the agent's turn |
| `omanta dictate start\|stop\|toggle` | Omarchy dictation for the Hyprland keys; no Voxtype overlay while omanta has focus |
| `omanta speak-selection` | Read the selected text aloud; press again to stop |
| `omanta say TEXT` | Speak text from a script |
| `omanta status` | What the voice service is doing |
| `omanta setup` | Download the voice model and build its Python environment |

The chat and the daemon talk over Unix sockets in `$XDG_RUNTIME_DIR/omanta/`.

If a command says it cannot connect to `daemon.sock`:

```sh
systemctl --user enable --now omanta
```

## Agents

omanta runs the agent's CLI in the background, in a machine-readable mode.

- **Claude Code** through `claude -p --input-format stream-json --output-format stream-json`, with approvals through `--permission-prompt-tool stdio`.
- **ACP agents** (JSON-RPC on stdio): OpenCode, Grok (`grok agent stdio`), Cursor Agent, Gemini CLI, GitHub Copilot, Qwen Code, Kimi, Hermes, and Codex through `codex-acp`.

The agent uses its own login. Log in once from settings (`ctrl+o`, **Log in**),
or `ctrl+l` in the chat. For Cursor Agent that runs `cursor-agent login`. When
an ACP agent asks for a login, omanta calls `authenticate` and tries once more,
so a login made in the TUI is picked up.

omanta follows the agent's permission rules. It asks only for what the agent
itself would ask.

Pick the agent and the working folder in settings. The working folder takes
effect on the next conversation (`ctrl+n`).

## How a conversation works

1. **Speech in.** In settings (`ctrl+o`), choose one:
   - **Talk key** (`ctrl+space` by default): omanta records and sends. With Superwhisper, omanta records with `pw-record` and gives the file to `superwhisper transcribe`. With Voxtype, the running daemon records (`voxtype record start --file=…`). Both use the model that is already loaded. Neither types into other windows.
   - **Your own dictation**: your dictation tool (Omarchy's hold-RightAlt with Voxtype) types into the message line.

   While Voxtype records, and the recording started while omanta had the
   focus, the message line shows your voice as a live ASCII waveform in the
   theme's colors. While it transcribes, a sweep. A recording started in
   another window shows Voxtype's own overlay instead, never both.

   Settings set the waveform height, 1 to 4 rows. The message line has the
   same height. **Background transparency** (off, 0.1 to 0.9) lets the
   wallpaper show through the omanta window only, in foot, Omarchy's
   terminal.

   **Send after a pause** (off, instant, 2, 3, 4 or 5 s) sends dictated text
   by itself. omanta tells dictation from typing by speed. Text you type
   waits for enter. The message line counts down, and any key resets it.

2. **Voice rules.** The agent gets a system prompt that makes it talk like a
   person: short answers, no markdown, paths and symbols said as words.

3. **Speech out.** The reply is read aloud sentence by sentence while it
   streams. The next sentence is synthesized while the current one plays.
   Each sentence appears on the screen when its voice starts, so you read
   and hear the same thing. Until then the reply shows "thinking…". Code
   blocks are not read aloud. Talking, or `esc`, stops the voice and shows
   the whole reply.

4. **Approvals by voice.** "Claude Code wants to run a command. Allow it?"
   Answer by voice, by typing, or with `alt+y` / `alt+a` / `alt+n`. Spoken
   "yes", "no", or "always" are enough. Anything else is sent to the agent
   as what to do instead. "Always" for a shell command allows only the
   programs in that command. **Auto mode** (`tab`) allows every action
   without asking, for this run. The footer shows `tab approve everything`
   when it is off and `tab >> fix everything`, in all the theme's colors,
   when it is on.

5. **Status, not noise.** Tool steps show as short lines ("reading main.rs").
   omanta does not read them aloud.

## Keys

| Key | Action |
| --- | --- |
| `ctrl+space` | Talk; press again to send (set another key in settings) |
| `enter` | Send what you typed or dictated |
| `tab` | Auto mode on or off, for this run |
| `esc` | Stop the voice, the recording, and the agent |
| `alt+y` / `alt+a` / `alt+n` | Allow / always allow / deny an action |
| `alt+1`…`alt+9` / `alt+s` | Pick an answer / skip a question |
| `ctrl+n` / `ctrl+r` | New conversation / continue the last one |
| `ctrl+o` | Settings |
| `ctrl+l` | Log in to the agent |
| `↑` `↓` `pgup` `pgdn` | Scroll |
| `f1` | Show the keys |
| `ctrl+c` / `ctrl+q` | Quit |

Settings (`ctrl+o`): left and right change a value. Enter edits text or runs
an action. Esc closes. Each change is saved at once.

### Keys in any window

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

## Settings and files

| Path | Contents |
| --- | --- |
| `~/.config/omanta/config.toml` | Settings |
| `~/.local/share/omanta/` | The voice model and its Python environment |
| `~/.local/state/omanta/sessions.json` | The last conversation of each agent |

Default voice is `af_aoede`. In the voice name, `a` is American, `b` is
British; `f` is female, `m` is male.

The voice sidecar listens on `127.0.0.1` only (default port 8765).

## If something is wrong

- **No voice.** Run `omanta status`. If it cannot connect, start the service:
  `systemctl --user enable --now omanta`. If the model is missing, run
  `omanta setup`.
- **Agent not installed.** Settings show `(not installed)` next to the name.
  Put that agent's CLI on `PATH`, then open settings again.
- **Not logged in.** `ctrl+l`, or settings → **Log in**.
- **Talk key does nothing.** The agent's STT service must already be running
  with its model loaded. Settings → **Speech to text** is Superwhisper or
  Voxtype.
- **Read-aloud misses the selection.** Copy to the clipboard, or install
  `wtype`.
