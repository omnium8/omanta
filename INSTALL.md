# Install

omanta is an Arch / Omarchy package. Clone this repository, then build and
install from the checkout. The package recipe has no remote source tarball;
`makepkg` compiles the tree you just cloned.

## Needs

On the machine:

- Arch Linux or [Omarchy](https://omarchy.org)
- `cargo` (Rust)
- `uv`
- `pipewire`, `alsa-lib`, `curl`, `wl-clipboard`
- one coding-agent CLI on `PATH` (Claude Code, Grok, OpenCode, …)

Optional:

- Voxtype or Superwhisper, for the talk key
- `wtype`, so read-aloud can grab a selection that is not in the primary
  clipboard

## Install

```sh
git clone https://github.com/omnium8/omanta.git
cd omanta
bin/install
```

`bin/install` runs `makepkg -fsi` in `packaging/`, then `omanta setup`, then
enables and restarts the user service `omanta.service`.

`omanta setup` downloads the Kokoro model files (about 340 MB) to
`~/.local/share/omanta/models` and builds a uv-managed Python 3.14 environment
in `~/.local/share/omanta/venv`. A later `pacman` or mise Python update does
not break the voice.

Open **Omanta** from the launcher, or run `omanta`.

Log in to the agent once: in the chat, `ctrl+o` then **Log in**. The agent
keeps its own credentials. omanta does not store API keys.

## After an upgrade

From the same clone:

```sh
git pull
bin/install
```

`bin/install` restarts the voice service so the old daemon does not keep
running.

## Hyprland keys (optional)

Add to `~/.config/hypr/bindings.lua` if you want talk and read-aloud from any
window. See [MANUAL.md](MANUAL.md#keys-in-any-window).

## Uninstall

```sh
systemctl --user disable --now omanta.service
sudo pacman -R omanta
```

That removes the binary, the desktop entry, the service unit, and
`/usr/share/omanta`. It leaves your settings and the voice model:

| Path | What it is |
| --- | --- |
| `~/.config/omanta/` | Settings |
| `~/.local/share/omanta/` | Voice model and Python environment |
| `~/.local/state/omanta/` | Last conversation of each agent |

Delete those folders if you want a clean slate.

## Develop without installing

```sh
bin/build     # target/release/omanta
bin/test
target/release/omanta daemon   # finds sidecar/ beside Cargo.toml
target/release/omanta
```
