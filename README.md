# omanta

Talk with any coding agent by voice, in the terminal. Your dictation goes in,
and a local Kokoro voice reads the reply aloud. The agent asks for approval by
voice, and you answer "yes", "no", or what to do instead. omanta also reads
any selected text aloud.

**[Install](INSTALL.md)** · **[Manual](MANUAL.md)**

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

`ctrl+space` talks. `esc` stops. `ctrl+o` opens settings. `tab` allows every
action for this run. Claude Code, Grok, OpenCode, Cursor Agent, Gemini, Codex,
Copilot, Qwen, Kimi, and Hermes are supported. The agent keeps its own login.

## Install

On Omarchy or Arch, from a clone of this repository:

```sh
bin/install
```

That builds the package, downloads the Kokoro voice (about 340 MB), and starts
the voice service. Full steps, uninstall, and a checkout-only build:
[INSTALL.md](INSTALL.md).

Keys, agents, settings, and troubleshooting: [MANUAL.md](MANUAL.md).

## License

MIT. See [LICENSE](LICENSE).
