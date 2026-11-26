# OuroMail builds

> **Last reviewed:** 2026-11-23 · **Last updated:** 2026-11-22 · **Change:** minor · **Status:** OuroMail only (the branch `ouromail/main` of github.com/Moins-Technologies-Inc/OuroMail); upstream MailCraft's releases are made by `release.yml` and `package.yml` ([`releasing.md`](releasing.md)).

OuroMail is MailCraft with its own name, theme, icons and Microsoft Entra application. It is
built for Arch Linux x86_64 as one package, `ouromail`, that serves every such computer.

## What starts a build

`.github/workflows/ouromail-package.yml` runs on:

- a push to `ouromail/main`;
- a run by hand: Actions › OuroMail package › Run workflow (or
  `gh workflow run ouromail-package.yml --ref ouromail/main`).

Nothing else starts it: no pull request, no schedule, no other workflow, no issue or comment. The
two jobs that build run on the organisation's own **self-hosted** runner, and the repository is
public; a self-hosted runner must never run code from somebody who cannot push to the repository,
and both of these triggers need push access. Keep them the only ones.

A manual run on another branch runs `ci` and `package` (uploading the workflow artifact) and skips
`publish`, so a non-main branch can never replace `ouromail-latest` or move its tag.

A newer push to the same branch cancels the run in progress.

## What a build does

1. **`ci`** (self-hosted runner, in a throwaway Arch Linux container): checks the packaging
   scripts (`shellcheck`, the committed assertion tests for `ouromail-update`, `runner.sh`,
   `smoke.sh`, the workflow and `PKGBUILD` versioning, plus `actionlint` and `zizmor` on this
   workflow), then runs `CI=1 cargo +1.98.1 xtask ci`, every step of it, exactly as a contributor
   does before a commit. `cargo deny` there reads a fresh copy of the RustSec advisory database.
2. **`package`** (self-hosted runner, in the same kind of container, as an unprivileged user, with
   full git history `fetch-depth: 0`): `makepkg` from
   [`packaging/arch-ouromail/PKGBUILD`](../packaging/arch-ouromail/PKGBUILD) (`options=(!debug)`),
   built from the checked-out commit, then the smoke checks of
   [`packaging/arch-ouromail/smoke.sh`](../packaging/arch-ouromail/smoke.sh) (the package's file
   list, monotonic `.PKGINFO` version, the version shows the commit in both binaries, the Entra
   application id in both binaries, the desktop `Exec=` entry, commands and settings, and no test
   hook in `ouromail-update`), then `SHA256SUMS`. After the container exits, the host result step
   accepts only the expected regular, non-symlink package and `SHA256SUMS` before uploading.
3. **`publish`** (GitHub's own runner; runs only when `github.ref == 'refs/heads/ouromail/main'`;
   runs no code from the repository): checks the package against the digest the `package` job gave,
   records its build provenance attestation (Sigstore, so the repository, workflow and commit of each
   build are in a public transparency log), moves the `ouromail-latest` tag to the built commit and
   replaces the files of the release `ouromail-latest`.

The containers see the checkout read-only, the branch's own cache directory on the runner (with
branch metadata kept outside the container mount under `/srv/ci-cache/ouromail-v1/keys/`) and, for
`package`, an output directory. They are given no GitHub token, and nothing of the runner's home.
Only `publish` holds `contents: write` (to update the release, its files, and force-move the
`ouromail-latest` tag) alongside `id-token: write` and `attestations: write`, and it runs only on
GitHub's own runner for `refs/heads/ouromail/main`.

The package version is the workspace's version plus full git commit count and short commit hash:
`0.14.0.r<commit count>.g<first 8 characters of the commit>` (monotonically increasing with every
commit, and strictly newer than the earlier delivered hash-only builds (`r85dd1df9`, `r6b3fafce`,
`r4abb19bb`) under Arch `vercmp`), while Cargo's version and `ouromail --version` keep
`0.14.0 (<short sha>)`.

A first build (empty cache) takes about the time of a full `cargo xtask ci` plus a release build
on the runner's 16 cores; later builds reuse the cache (`/srv/ci-cache/ouromail-v1/` on the runner:
one directory per branch and toolchain, at most 70 GB together, the least recently used branch
removed first).

## Where the package appears

The release
[`ouromail-latest`](https://github.com/Moins-Technologies-Inc/OuroMail/releases/tag/ouromail-latest)
holds two files, replaced by every build of `ouromail/main`:

- `ouromail-0.14.0.r<count>.g<commit>-1-x86_64.pkg.tar.zst`
- `SHA256SUMS`

It is a prerelease, so it is never shown as the repository's latest release, and its notes name the
commit, the build run and the exact verification commands (including `--signer-workflow` and
`--source-ref`).

## Updating: `ouromail-update`

The package installs `/usr/bin/ouromail-update`. Run it yourself (it asks for `sudo`):

```
ouromail-update            # install the newest build if it is not the one installed
ouromail-update --check    # only say what is installed and what is offered
ouromail-update --force    # reinstall the offered build
```

It reads the release's `SHA256SUMS` over HTTPS (no GitHub token needed), says what is installed
and what is offered and stops when they are the same (in `--check` mode, `--force` is ignored so
matching versions still exit `0`; exit codes are `0` up to date/installed/help, `1` error, `2`
unknown option, `10` `--check` sees a different version). Otherwise it downloads the package and
`SHA256SUMS` over HTTPS into a private temporary directory, checks the SHA-256, checks the build
provenance with `gh attestation verify --repo Moins-Technologies-Inc/OuroMail` (pinned to
`--signer-workflow Moins-Technologies-Inc/OuroMail/.github/workflows/ouromail-package.yml` and
`--source-ref refs/heads/ouromail/main`), refuses while OuroMail is running, backs up
`~/.config/ouromail`, and runs `sudo pacman -U` on the checked file. Nothing it downloads is run.

### What the backup includes and how much space it uses

`~/.config/ouromail` holds your mailbox (`mailbox.mailcraft`), its content folder
(`mailbox.mailcraft.blobs/`), and your settings (`ui.json`, etc.), alongside disposable
`pictures.cache/` and `logs/`. Before installing, `ouromail-update`:

- refuses if `~/.config/ouromail` is a symbolic link or contains symbolic links in the backed-up
  entries (fail-closed);
- selects everything in `~/.config/ouromail` **except** the disposable top-level `pictures.cache`
  and `logs` directories (mailbox and content are always included and never skipped to save space);
- measures the selected size and available free space on the destination filesystem, reports both,
  and stops without installing if there is not enough free space;
- copies into a private temporary staging directory (`cp -a --reflink=auto`), removes only that
  staging directory if copying fails (leaving existing backups and `~/.config/ouromail` untouched),
  moves it into `~/.config/ouromail.bak-ouromail-update-<date>` once complete, and only then
  rotates older backups to keep the last three complete ones.

On copy-on-write filesystems that support reflinks (such as **btrfs** and modern **XFS**),
`--reflink=auto` clones unchanged file extents so backups finish quickly and use minimal extra
space. On **ext4** (which does not support reflinks), each backup is a full copy on disk, so up to
three backups plus the live mailbox (about 4× the mailbox size) can be stored.

Without `gh` (package `github-cli`, signed in with `gh auth login`) it refuses, since it could not
tell who built the package; `--no-attestation` installs with the SHA-256 check alone, which only
proves that the download is the file the release lists.

## The first install on a new computer

`ouromail-update` comes with the package, so the first install is by hand:

```
dir=$(mktemp -d) && cd "$dir"
gh release download ouromail-latest --repo Moins-Technologies-Inc/OuroMail
sha256sum -c SHA256SUMS
gh attestation verify ouromail-*-x86_64.pkg.tar.zst --repo Moins-Technologies-Inc/OuroMail \
  --signer-workflow Moins-Technologies-Inc/OuroMail/.github/workflows/ouromail-package.yml \
  --source-ref refs/heads/ouromail/main
sudo pacman -U ouromail-*-x86_64.pkg.tar.zst
cd && rm -rf "$dir"
```

Then start `ouromail` normally once (not `--sample`): it copies `~/.config/mailcraft` to
`~/.config/ouromail` the first time and asks you to sign in once to its own Entra application.
Afterwards, `ouromail-update` keeps it current.

## The runner

The jobs that build run on the organisation's self-hosted runner (labels `self-hosted`, `Linux`,
`X64`, `podman`, `ouromail`), with rootless podman as the user who owns `/srv/ci-cache`
(`packaging/arch-ouromail/runner.sh`; when the runner service runs a job as root, the script runs
podman as that user). The image is `docker.io/library/archlinux:base-devel`, invoked with
`--pull=missing` and brought up to date by `pacman -Syu` in every job; the base image itself only
changes when someone runs `podman pull docker.io/library/archlinux:base-devel` on the runner by
hand. A container left by a cancelled run is removed at the start of the next job, and podman
stops any container after four hours.

## Revision history

| Date | Change | What |
|---|---|---|
