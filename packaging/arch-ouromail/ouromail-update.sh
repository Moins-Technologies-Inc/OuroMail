#!/usr/bin/env bash
# SPDX-FileCopyrightText: MailCraft contributors
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# ouromail-update: install the newest OuroMail package from the rolling GitHub release
# `ouromail-latest` of github.com/Moins-Technologies-Inc/OuroMail (docs/ouromail-builds.md).
# Installed by the Arch package as /usr/bin/ouromail-update. Nothing fetched is ever run: the
# package goes to `sudo pacman -U` only after its SHA-256 and its build provenance are checked.
set -euo pipefail
umask 077
export LC_ALL=C

readonly REPO="Moins-Technologies-Inc/OuroMail"
readonly TAG="ouromail-latest"
readonly WORKFLOW="$REPO/.github/workflows/ouromail-package.yml"
readonly SOURCE_REF="refs/heads/ouromail/main"
readonly BASE_URL="https://github.com/$REPO/releases/download/$TAG"
readonly PKGNAME="ouromail"
readonly KEEP_BACKUPS=3

usage() {
	cat <<EOF
Usage: ouromail-update [--check] [--force] [--no-attestation] [--help]

Installs the newest OuroMail package from the rolling release '$TAG' of
https://github.com/$REPO (no GitHub token needed).

  1. Reads the release's SHA256SUMS and says what is installed and what is offered;
     stops when they are the same.
  2. Downloads the package and SHA256SUMS over HTTPS into a private temporary directory and
     checks the package's SHA-256.
  3. Checks the package's build provenance attestation with
       gh attestation verify --repo $REPO
     (signed by $WORKFLOW
      for $SOURCE_REF). Without 'gh' (package github-cli, signed in)
     the package is refused unless --no-attestation is given.
  4. Refuses while 'ouromail' is running.
  5. Backs up ~/.config/ouromail (mailbox, its content folder and settings; excludes
     disposable pictures.cache and logs) after checking free space, staged privately,
     retaining the last $KEEP_BACKUPS complete backups it made.
  6. Runs 'sudo pacman -U' on the verified file, then removes the temporary directory.

Options:
  --check            only report what is installed and what is offered
                     (--force is ignored in --check mode)
  --force            install even when the offered version is the one installed
  --no-attestation   install without checking the build provenance attestation
                     (the SHA-256 is still checked; it only proves the download is the
                     file the release lists, not who built it)
  -h, --help         show this help

Exit codes:
  0   up to date, installed, or help shown
  1   error (download, verification, running app, backup, or install failed)
  2   unknown option
  10  --check found a different version on offer
EOF
}

say() { printf 'ouromail-update: %s\n' "$*"; }
die() {
	printf 'ouromail-update: %s\n' "$*" >&2
	exit 1
}

human_bytes() {
	awk -v b="$1" 'BEGIN {
		if (b >= 1073741824) printf "%.1f GiB", b / 1073741824;
		else if (b >= 1048576) printf "%.1f MiB", b / 1048576;
		else if (b >= 1024) printf "%.1f KiB", b / 1024;
		else printf "%d B", b;
	}'
}

check_only=0
force=0
attest=1
for arg in "$@"; do
	case "$arg" in
	--check) check_only=1 ;;
	--force) force=1 ;;
	--no-attestation) attest=0 ;;
	-h | --help)
		usage
		exit 0
		;;
	*)
		usage >&2
		exit 2
		;;
	esac
done

tmp=$(mktemp -d "${TMPDIR:-/tmp}/ouromail-update.XXXXXXXX")
backup_stage=""
backup_root=""
# shellcheck disable=SC2329 # called by the EXIT trap below
cleanup() {
	rm -rf -- "$tmp"
	if [ -n "$backup_stage" ] && [ -n "$backup_root" ] && [[ "$backup_stage" == "$backup_root"/.ouromail-update-backup.* ]]; then
		rm -rf -- "$backup_stage"
	fi
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# fetch NAME MAX_BYTES OUT: one file of the release over HTTPS only, at most MAX_BYTES.
fetch() {
	local name=$1 max=$2 out=$3
	curl --proto =https --proto-redir =https -fsSL --retry 3 --connect-timeout 20 \
		--max-time 1800 --max-filesize "$max" -o "$out" -- "$BASE_URL/$name" ||
		die "cannot download $BASE_URL/$name"
	local size
	size=$(wc -c <"$out")
	[ "$size" -le "$max" ] || die "$name is larger than $max bytes"
}

fetch SHA256SUMS 65536 "$tmp/SHA256SUMS"

# Exactly one line naming a package of ours: 64 hex digits, two spaces, a plain file name.
pattern="^[0-9a-f]{64}  ${PKGNAME}-[0-9A-Za-z._+]+-[0-9]+-x86_64\\.pkg\\.tar\\.zst\$"
lines=$(grep -cE "$pattern" "$tmp/SHA256SUMS" || true)
[ "$lines" = 1 ] || die "the release's SHA256SUMS does not name exactly one package (found $lines)"
line=$(grep -E "$pattern" "$tmp/SHA256SUMS")
want_sum=${line%% *}
file=${line##* }
offered=${file#"$PKGNAME"-}
offered=${offered%-x86_64.pkg.tar.zst}

installed=$(pacman -Q "$PKGNAME" 2>/dev/null | cut -d' ' -f2 || true)
say "installed: ${installed:-(not installed)}"
say "offered:   $offered ($TAG)"

if [ "$installed" = "$offered" ] && { [ "$check_only" = 1 ] || [ "$force" = 0 ]; }; then
	say "up to date (--force reinstalls it)"
	exit 0
fi
if [ "$check_only" = 1 ]; then
	say "another version is offered; run ouromail-update to install it"
	exit 10
fi

say "downloading $file"
fetch "$file" 1073741824 "$tmp/$file"
got_sum=$(sha256sum -- "$tmp/$file" | cut -d' ' -f1)
[ "$got_sum" = "$want_sum" ] || die "SHA-256 mismatch for $file: got $got_sum, the release lists $want_sum; nothing installed"
say "SHA-256 ok: $got_sum"

if [ "$attest" = 1 ]; then
	command -v gh >/dev/null 2>&1 ||
		die "gh (package github-cli) is not installed, so the build provenance cannot be checked; install it and sign in (gh auth login), or pass --no-attestation; nothing installed"
	gh attestation verify "$tmp/$file" --repo "$REPO" --signer-workflow "$WORKFLOW" --source-ref "$SOURCE_REF" ||
		die "the build provenance attestation of $file could not be verified; nothing installed"
	say "build provenance verified"
else
	say "build provenance NOT checked (--no-attestation)"
fi

if pgrep -x ouromail >/dev/null 2>&1; then
	die "ouromail is running: close it (File > Exit) and run ouromail-update again; nothing installed"
fi

config="${XDG_CONFIG_HOME:-$HOME/.config}"
src="$config/ouromail"
if [ -L "$src" ]; then
	die "$src is a symbolic link; refusing to back up through a symlink; nothing installed"
fi
if [ -e "$src" ]; then
	[ -d "$src" ] || die "$src is not a directory; nothing installed"
	if [ -L "$config" ] || [ ! -d "$config" ]; then
		die "$config must be a real directory; nothing installed"
	fi
	# Fail closed if any selected entry inside $src is a symlink (excluding disposable pictures.cache and logs).
	sym=$(find "$src" -mindepth 1 \( -path "$src/pictures.cache" -o -path "$src/logs" \) -prune -o -type l -print -quit)
	if [ -n "$sym" ]; then
		die "$src contains a symbolic link ($sym); refusing to back up; nothing installed"
	fi

	# Select every top-level entry except disposable pictures.cache and logs; mailbox and its
	# content folder are always included and never skipped to save space.
	backup_items=()
	while IFS= read -r -d '' item; do
		case "$item" in
		"$src/pictures.cache" | "$src/logs") continue ;;
		*) backup_items+=("$item") ;;
		esac
	done < <(find "$src" -mindepth 1 -maxdepth 1 -print0 | LC_ALL=C sort -z)

	selected_bytes=0
	if [ "${#backup_items[@]}" -gt 0 ]; then
		disk_bytes=$(du -P -s --block-size=1 -- "${backup_items[@]}" | awk '{sum += $1} END {printf "%.0f\n", sum + 0}')
		apparent_bytes=$(du -P -sb -- "${backup_items[@]}" | awk '{sum += $1} END {printf "%.0f\n", sum + 0}')
		[[ "$disk_bytes" =~ ^[0-9]+$ && "$apparent_bytes" =~ ^[0-9]+$ ]] ||
			die "could not measure backup size for $src; nothing installed"
		if [ "$apparent_bytes" -gt "$disk_bytes" ]; then
			selected_bytes=$apparent_bytes
		else
			selected_bytes=$disk_bytes
		fi
	fi
	avail_bytes=$(df --block-size=1 --output=avail -- "$config" | tail -n 1 | tr -dc '0-9')
	[[ "$avail_bytes" =~ ^[0-9]+$ ]] || die "could not measure free space on $config; nothing installed"

	need_human=$(human_bytes "$selected_bytes")
	avail_human=$(human_bytes "$avail_bytes")
	say "backup of $src (includes mailbox and content; excludes pictures.cache and logs): $need_human selected, $avail_human free"
	if [ "$avail_bytes" -lt "$selected_bytes" ]; then
		die "not enough free space in $config for backup (need $need_human, only $avail_human free; will not skip mailbox or content); nothing installed"
	fi

	backup_root="$config"
	backup_stage=$(mktemp -d "$config/.ouromail-update-backup.XXXXXXXX")
	if [ "${#backup_items[@]}" -gt 0 ]; then
		if ! cp -a --reflink=auto -- "${backup_items[@]}" "$backup_stage/"; then
			die "backup copy of $src failed (partial backup removed; existing backups and source untouched); nothing installed"
		fi
	fi
	backup="$config/ouromail.bak-ouromail-update-$(date +%Y%m%d-%H%M%S)"
	if [ -e "$backup" ] || [ -L "$backup" ]; then
		die "backup $backup already exists; nothing installed"
	fi
	if ! mv -T -- "$backup_stage" "$backup"; then
		die "could not finalize backup $backup; nothing installed"
	fi
	backup_stage=""
	say "settings and mailbox backed up to $backup ($need_human; pictures.cache and logs excluded)"

	# Rotate only after the new backup is complete; keep the newest $KEEP_BACKUPS complete backups.
	old=()
	keep_seen=0
	while IFS= read -r -d '' b; do
		keep_seen=$((keep_seen + 1))
		if [ "$keep_seen" -gt "$KEEP_BACKUPS" ]; then
			old+=("$b")
		fi
	done < <(find "$config" -mindepth 1 -maxdepth 1 -type d ! -type l -name 'ouromail.bak-ouromail-update-*' -printf '%f\0' | LC_ALL=C sort -z -r)
	for b in "${old[@]}"; do
		case "$b" in
		ouromail.bak-ouromail-update-*)
			if [ -d "$config/$b" ] && [ ! -L "$config/$b" ]; then
				rm -rf -- "$config/$b"
				say "removed old backup $config/$b"
			fi
			;;
		esac
	done
fi

say "running: sudo pacman -U $tmp/$file"
sudo pacman -U -- "$tmp/$file"
say "installed $offered"
