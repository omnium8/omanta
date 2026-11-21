# Releasing

This guide is for the maintainer. Users read [INSTALL.md](INSTALL.md).

## Make a release

```sh
bin/release 0.3.1
```

The script:

1. sets the version in `Cargo.toml`, `Cargo.lock`, `packaging/PKGBUILD`,
   and `packaging/aur/PKGBUILD` (and `packaging/aur/.SRCINFO`),
2. runs the tests,
3. commits, makes the tag `v0.3.1`, and pushes both.

The tag starts the workflow `.github/workflows/release.yml`. Watch it with
`gh run watch`. The workflow:

1. builds the package in an `archlinux:base-devel` container, with
   `packaging/build-repo`,
2. makes the pacman repository database (`omanta.db`, `omanta.files`),
3. attaches the package and the database to the GitHub release, and marks
   the release as the latest.

If no run starts, or to build an existing tag again, start it by hand:

```sh
gh workflow run release.yml -f tag=v0.3.1
```

## How the pacman repository works

Each release contains a complete repository: the package and the database.
Users point pacman at this URL:

```
https://github.com/omnium8/omanta/releases/latest/download
```

GitHub sends `latest/download/<file>` to the file of the newest release. So
the newest release is always the repository, and no server is necessary.
Do not delete the files from the latest release.

To build the same files on your machine:

```sh
packaging/build-repo dist
```

## Publish to the AUR

The AUR recipe is `packaging/aur/PKGBUILD`. It builds the tagged release
from source. `bin/release` keeps its version and `.SRCINFO` up to date.

The first time:

1. Make an account on <https://aur.archlinux.org>.
2. Add your SSH public key to the account.
3. Clone the empty package and copy the recipe into it:

   ```sh
   git clone ssh://aur@aur.archlinux.org/omanta.git ~/aur/omanta
   cp packaging/aur/PKGBUILD packaging/aur/.SRCINFO ~/aur/omanta/
   cd ~/aur/omanta
   makepkg -si    # test the recipe
   git add PKGBUILD .SRCINFO
   git commit -m "omanta 0.3.0"
   git push
   ```

4. In INSTALL.md, replace the AUR section with `yay -S omanta`.

After each release, copy the two files again, then commit and push.

## Sign the packages

The packages are not signed now, so users must set
`SigLevel = Optional TrustAll`. To sign them:

1. Make a key for the project, without a passphrase:

   ```sh
   gpg --quick-gen-key "omanta packages <EMAIL>" ed25519 sign never
   ```

2. Store the private key as the repository secret `PACKAGER_GPG_KEY`:

   ```sh
   gpg --armor --export-secret-keys "omanta packages" | gh secret set PACKAGER_GPG_KEY
   ```

3. Publish the public key, for example as `omanta.asc` in this repository.
4. Make a release. The workflow signs when the secret exists.
5. In INSTALL.md and README.md, tell users to trust the key and remove
   `TrustAll`:

   ```sh
   sudo pacman-key --add omanta.asc
   sudo pacman-key --lsign-key <KEY-ID>
   ```

   Then `SigLevel = Required` in the `[omanta]` section.

Caution: anyone who has the private key can publish packages that users
install as root. Keep a backup of the key in a safe place, and nowhere else.
