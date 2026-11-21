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
╰ ○ alt+space to talk ─────────────────────────────────── voice ready ╯
╭ message Claude Code ────────────────────────────────────────────────╮
│ ›                                                                   │
╰────────────── alt+space talk · esc stop · ctrl+o settings · f1 keys ╯
```

`alt+space` talks. `esc` stops. `ctrl+o` opens settings. `tab` allows every
action for this run. Claude Code, Grok, OpenCode, Cursor Agent, Gemini, Codex,
Copilot, Qwen, Kimi, and Hermes are supported. The agent keeps its own login.

## Install

omanta needs Omarchy or Arch Linux, and one coding-agent CLI. Add the
omanta pacman repository, then install:

```sh
printf '\n[omanta]\nSigLevel = Optional TrustAll\nServer = https://github.com/omnium8/omanta/releases/latest/download\n' | sudo tee -a /etc/pacman.conf
sudo pacman -Syu omanta
```

Then open **Omanta** from the launcher. On the first start, omanta downloads
the Kokoro voice (about 340 MB) and starts the voice service.

The packages are not signed yet. Other ways to install (prebuilt package,
AUR, from a clone) and how to uninstall: [INSTALL.md](INSTALL.md).

Keys, agents, settings, and troubleshooting: [MANUAL.md](MANUAL.md).

## Status and feedback

omanta is young software. Report problems and ideas in
[Issues](https://github.com/omnium8/omanta/issues). Tell which agent, which
speech-to-text tool, and which terminal you use.

omanta is a community project. It is not part of Omarchy or Basecamp.

## Credits

The voice is [Kokoro](https://huggingface.co/hexgrad/Kokoro-82M) (Apache 2.0),
through [kokoro-onnx](https://github.com/thewh1teagle/kokoro-onnx). The voice
runs on your machine. Your agent CLI still sends your messages to its own
provider.

## License

MIT. See [LICENSE](LICENSE).
