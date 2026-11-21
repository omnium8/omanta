# Install

omanta is a package for Arch Linux and [Omarchy](https://omarchy.org). Use
one of these ways:

| Way | Updates | Builds on your machine |
| --- | --- | --- |
| [pacman repository](#pacman-repository) (recommended) | with `pacman -Syu` | no |
| [Prebuilt package](#prebuilt-package) | download again | no |
| [AUR](#aur) | with your AUR helper | yes |
| [From a clone](#from-a-clone) | `git pull`, then `bin/install` | yes |

The prebuilt package is for x86_64. On aarch64, use the AUR or a clone.

## Needs

- Arch Linux or Omarchy
- one coding-agent CLI on `PATH` (Claude Code, Grok, OpenCode, …)

Optional:

- Voxtype (Omarchy's dictation) or Superwhisper, for the talk key
- `wtype`, so read-aloud can get a selection that is not in the primary
  clipboard

pacman installs the other dependencies.

## pacman repository

Each GitHub release contains a small pacman repository. pacman then updates
omanta together with the rest of the system.

1. Add the repository to `/etc/pacman.conf`:

   ```sh
   printf '\n[omanta]\nSigLevel = Optional TrustAll\nServer = https://github.com/omnium8/omanta/releases/latest/download\n' | sudo tee -a /etc/pacman.conf
   ```

2. Install omanta:

   ```sh
   sudo pacman -Syu omanta
   ```

The packages are not signed yet. `SigLevel = Optional TrustAll` tells pacman
to accept them. Only the HTTPS connection to GitHub protects the download.

## Prebuilt package

1. Download `omanta-<version>-1-x86_64.pkg.tar.zst` from the
   [latest release](https://github.com/omnium8/omanta/releases/latest).
2. Install it:

   ```sh
   sudo pacman -U ./omanta-*-x86_64.pkg.tar.zst
   ```

To update, download the new package and install it the same way.

## AUR

omanta is not on the AUR yet. Until then, build the AUR recipe yourself. It
downloads the source of the release from GitHub and builds it:

```sh
git clone https://github.com/omnium8/omanta.git
cd omanta/packaging/aur
makepkg -si
```

## From a clone

This way builds the code that you cloned. Use it to try changes.

```sh
git clone https://github.com/omnium8/omanta.git
cd omanta
bin/install
```

`bin/install` runs `makepkg -fsi` in `packaging/`, then `omanta setup`, then
enables and restarts the user service `omanta.service`.

To update:

```sh
git pull
bin/install
```

`bin/install` restarts the voice service, so the old daemon does not
continue to run.

## First start

Open **Omanta** from the launcher, or run `omanta`.

On the first start, omanta prepares the voice by itself:

1. The chat starts the voice service, `omanta.service`, and enables it for
   every login.
2. The service downloads the Kokoro model (about 340 MB) to
   `~/.local/share/omanta/models`.
3. The service builds a uv-managed Python 3.14 environment in
   `~/.local/share/omanta/venv`. A later `pacman` or mise Python update does
   not break the voice.

The chat shows "downloading the voice…" and then "voice ready". To see the
download progress in a terminal, run `omanta setup`.

Log in to the agent once: in the chat, `ctrl+o`, then **Log in**. The agent
keeps its own credentials. omanta does not store API keys.

## After an upgrade

pacman replaces the program, but the voice service that runs continues to
use the old one. Restart it:

```sh
systemctl --user restart omanta.service
```

`bin/install` does this for you.

## Hyprland keys (optional)

Add keys to `~/.config/hypr/bindings.lua` to talk and read aloud from any
window. See [MANUAL.md](MANUAL.md#keys-in-any-window).

## Uninstall

```sh
systemctl --user disable --now omanta.service
sudo pacman -R omanta
```

If you added the pacman repository, remove the `[omanta]` section from
`/etc/pacman.conf`.

`pacman -R` removes the binary, the desktop entry, the service unit, and
`/usr/share/omanta`. It keeps your settings and the voice model:

| Path | What it is |
| --- | --- |
| `~/.config/omanta/` | Settings |
| `~/.local/share/omanta/` | Voice model and Python environment |
| `~/.local/state/omanta/` | Last conversation of each agent |

To remove all of omanta, delete these folders too.

## Develop without installing

```sh
bin/build     # target/release/omanta
bin/test
target/release/omanta daemon   # finds sidecar/ beside Cargo.toml
target/release/omanta
```

To make a release, see [RELEASING.md](RELEASING.md).
