#!/usr/bin/env bash
# shellcheck disable=SC2016
# SPDX-FileCopyrightText: MailCraft contributors
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Tests of ouromail-update.sh using external fake curl, pacman, pgrep, gh, sudo, df and cp
# executables on PATH (no production test-dir bypass hook). Nothing is downloaded over the network
# and nothing is installed: PATH holds only the stubs and links to the real coreutils needed.
# Usage: bash packaging/arch-ouromail/test-ouromail-update.sh
# (SC2016: the stubs' bodies are in single quotes on purpose; they expand when a stub runs.)
set -euo pipefail
export LC_ALL=C

here=$(cd "$(dirname "$0")" && pwd)
updater="$here/ouromail-update.sh"
root=$(mktemp -d "${TMPDIR:-/tmp}/ouromail-update-test.XXXXXXXX")
trap 'rm -rf -- "$root"' EXIT

real_bash=$(command -v bash)
real_cp=$(command -v cp)
real_df=$(command -v df)
bin="$root/bin"
mkdir -p "$bin"
for tool in awk cat cut date du find grep mkdir mktemp mv rm sha256sum sort stat tail tr wc; do
	ln -s "$(command -v "$tool")" "$bin/$tool"
done

# The stubs record what they were asked into $root/calls.
stub() {
	printf '#!%s\nprintf "%%s %%s\\n" %q "$*" >> %q\n%s\n' "$real_bash" "$1" "$root/calls" "$2" >"$bin/$1"
	chmod +x "$bin/$1"
}

# Fake curl: asserts HTTPS-only flags, --max-filesize, and exact release URL, then serves from FAKE_RELEASE_DIR.
stub curl '
has_proto=0 has_redir=0 has_max=0 max_val="" out="" url=""
while [ $# -gt 0 ]; do
	case "$1" in
	--proto) [ "${2:-}" = "=https" ] && has_proto=1; shift 2 ;;
	--proto-redir) [ "${2:-}" = "=https" ] && has_redir=1; shift 2 ;;
	--max-filesize) has_max=1; max_val="${2:-}"; shift 2 ;;
	-o) out="${2:-}"; shift 2 ;;
	--) shift; url="${1:-}"; break ;;
	*) shift ;;
	esac
done
[ "$has_proto" = 1 ] || { echo "fake curl: missing --proto =https" >&2; exit 22; }
[ "$has_redir" = 1 ] || { echo "fake curl: missing --proto-redir =https" >&2; exit 22; }
[ "$has_max" = 1 ] && [[ "$max_val" =~ ^[1-9][0-9]*$ ]] || { echo "fake curl: missing --max-filesize" >&2; exit 22; }
prefix="https://github.com/Moins-Technologies-Inc/OuroMail/releases/download/ouromail-latest/"
case "$url" in
"${prefix}"*) name=${url#"$prefix"} ;;
*) echo "fake curl: unexpected url $url" >&2; exit 22 ;;
esac
[ -n "${FAKE_RELEASE_DIR:-}" ] && [ -f "$FAKE_RELEASE_DIR/$name" ] || { echo "fake curl: 404 $name" >&2; exit 22; }
if [ "${FAKE_CURL_IGNORE_MAXSIZE:-0}" != 1 ]; then
	sz=$(wc -c <"$FAKE_RELEASE_DIR/$name")
	[ "$sz" -le "$max_val" ] || { echo "fake curl: max-filesize exceeded ($sz > $max_val)" >&2; exit 63; }
fi
'"$real_cp"' -- "$FAKE_RELEASE_DIR/$name" "$out"
'

stub pacman 'if [ "$1" = -Q ]; then [ -n "${FAKE_INSTALLED:-}" ] || exit 1; echo "ouromail $FAKE_INSTALLED"; fi'
stub pgrep 'if [ -n "${FAKE_RUNNING:-}" ]; then echo 4242; exit 0; fi; exit 1'
stub sudo 'exit "${FAKE_SUDO_RC:-0}"'

# Optional fault-injecting wrappers for df and cp.
cat >"$bin/df" <<EOF
#!$real_bash
if [ -n "\${FAKE_DF_AVAIL:-}" ]; then
	printf 'Avail\n%s\n' "\$FAKE_DF_AVAIL"
	exit 0
fi
exec "$real_df" "\$@"
EOF
chmod +x "$bin/df"

cat >"$bin/cp" <<EOF
#!$real_bash
if [ "\${FAKE_CP_FAIL:-0}" = 1 ]; then
	# Simulate a partial copy before failing (e.g. ENOSPC mid-copy).
	dest="\${!#}"
	mkdir -p "\$dest"
	echo "partial-data" >"\$dest/partial.tmp"
	echo "cp: error writing '\$dest/mailbox.mailcraft': No space left on device" >&2
	exit 1
fi
exec "$real_cp" "\$@"
EOF
chmod +x "$bin/cp"

with_gh() { stub gh 'if [ -n "${FAKE_UNATTESTED:-}" ]; then echo "no attestations found" >&2; exit 1; fi; exit 0'; }
without_gh() { rm -f "$bin/gh"; }

release="$root/release"
# make_release VERSION: a package file of random bytes and SHA256SUMS naming it.
make_release() {
	rm -rf "$release"
	mkdir -p "$release"
	local f="ouromail-$1-1-x86_64.pkg.tar.zst"
	head -c 4096 /dev/urandom >"$release/$f"
	(cd "$release" && sha256sum "$f" >SHA256SUMS)
	printf '%s\n' "$f"
}

fails=0
cases=0

# seed_home: creates a realistic ~/.config/ouromail with mailbox, blobs, settings, pictures.cache and logs.
seed_home() {
	rm -rf "${root:?}/home" "${root:?}/tmp"
	mkdir -p "$root/home/.config/ouromail/mailbox.mailcraft.blobs/3f" \
		"$root/home/.config/ouromail/nested/logs" \
		"$root/home/.config/ouromail/pictures.cache/aa" \
		"$root/home/.config/ouromail/logs" \
		"$root/tmp"
	printf 'mailbox-index-v3\n' >"$root/home/.config/ouromail/mailbox.mailcraft"
	printf 'attachment-payload\n' >"$root/home/.config/ouromail/mailbox.mailcraft.blobs/3f/blob01"
	printf '{"accent":"ouro"}\n' >"$root/home/.config/ouromail/ui.json"
	printf 'nested-not-top-logs\n' >"$root/home/.config/ouromail/nested/logs/keep.txt"
	printf 'disposable-picture\n' >"$root/home/.config/ouromail/pictures.cache/aa/pic01"
	printf 'disposable-log\n' >"$root/home/.config/ouromail/logs/mailcraft.log"
}

# run NAME WANT_EXIT ARGS...: runs the updater with a fresh HOME unless KEEP_HOME=1; output in $root/out.
run() {
	local name=$1 want=$2
	shift 2
	cases=$((cases + 1))
	: >"$root/calls"
	if [ "${KEEP_HOME:-0}" != 1 ]; then
		seed_home
	else
		rm -rf "${root:?}/tmp"
		mkdir -p "$root/tmp"
	fi
	set +e
	env -i PATH="$bin" HOME="$root/home" TMPDIR="$root/tmp" \
		FAKE_RELEASE_DIR="${FAKE_RELEASE_DIR:-$release}" \
		FAKE_INSTALLED="${FAKE_INSTALLED:-}" \
		FAKE_RUNNING="${FAKE_RUNNING:-}" \
		FAKE_UNATTESTED="${FAKE_UNATTESTED:-}" \
		FAKE_SUDO_RC="${FAKE_SUDO_RC:-0}" \
		FAKE_DF_AVAIL="${FAKE_DF_AVAIL:-}" \
		FAKE_CP_FAIL="${FAKE_CP_FAIL:-0}" \
		FAKE_CURL_IGNORE_MAXSIZE="${FAKE_CURL_IGNORE_MAXSIZE:-0}" \
		OUROMAIL_UPDATE_TEST_RELEASE_DIR="${POISON_TEST_DIR:-}" \
		"$real_bash" "$updater" "$@" >"$root/out" 2>&1
	got=$?
	set -e
	if [ "$got" != "$want" ]; then
		echo "FAIL $name: exit $got, want $want"
		sed 's/^/    /' "$root/out"
		fails=$((fails + 1))
		return
	fi
	if [ -n "$(ls -A "$root/tmp")" ]; then
		echo "FAIL $name: temporary files left behind"
		fails=$((fails + 1))
		return
	fi
	echo "ok   $name"
}

expect() {
	if ! grep -qE -- "$1" "$2"; then
		echo "FAIL ${3}: '$1' not in $2"
		sed 's/^/    /' "$2"
		fails=$((fails + 1))
	fi
}
refute() {
	if grep -qE -- "$1" "$2"; then
		echo "FAIL ${3}: '$1' in $2"
		sed 's/^/    /' "$2"
		fails=$((fails + 1))
	fi
}

with_gh
make_release 0.14.0.r3610.gaaaaaaaa >/dev/null

# Verify no production test-dir bypass hook exists or is honoured.
cases=$((cases + 1))
if ! grep -qF 'OUROMAIL_UPDATE_TEST_RELEASE_DIR' "$updater"; then
	echo "ok   no OUROMAIL_UPDATE_TEST_RELEASE_DIR in installed updater"
else
	echo "FAIL OUROMAIL_UPDATE_TEST_RELEASE_DIR still present in updater"
	fails=$((fails + 1))
fi

POISON_TEST_DIR="$release" FAKE_RELEASE_DIR="$root/missing-release" FAKE_INSTALLED=0.14.0.r3610.gaaaaaaaa-1 \
	run "OUROMAIL_UPDATE_TEST_RELEASE_DIR ignored" 1 --check
expect '^curl ' "$root/calls" "OUROMAIL_UPDATE_TEST_RELEASE_DIR ignored"

FAKE_INSTALLED=0.14.0.r3610.gaaaaaaaa-1 run "same version" 0
expect 'up to date' "$root/out" "same version"
refute '^sudo' "$root/calls" "same version"

FAKE_INSTALLED=0.14.0.r3610.gaaaaaaaa-1 run "same version --check" 0 --check
expect 'up to date' "$root/out" "same version --check"

FAKE_INSTALLED=0.14.0.r3610.gaaaaaaaa-1 run "same version --check --force" 0 --check --force
expect 'up to date' "$root/out" "same version --check --force"
refute '^(sudo|gh)' "$root/calls" "same version --check --force"

FAKE_INSTALLED=0.14.0.r3610.gaaaaaaaa-1 run "same version --force" 0 --force
expect '^sudo pacman -U -- .*/ouromail-0\.14\.0\.r3610\.gaaaaaaaa-1-x86_64\.pkg\.tar\.zst$' "$root/calls" "same version --force"

pkg=$(make_release 0.14.0.r3611.gbbbbbbbb)
FAKE_INSTALLED=0.14.0.r3610.gaaaaaaaa-1 run "newer version --check" 10 --check
expect 'installed: 0\.14\.0\.r3610\.gaaaaaaaa-1' "$root/out" "newer version --check"
expect 'offered:   0\.14\.0\.r3611\.gbbbbbbbb-1' "$root/out" "newer version --check"
refute '^(sudo|gh)' "$root/calls" "newer version --check"

FAKE_INSTALLED=0.14.0.r3610.gaaaaaaaa-1 run "newer version + backup exclusions/inclusions" 0
expect "^gh attestation verify .*/$pkg --repo Moins-Technologies-Inc/OuroMail --signer-workflow Moins-Technologies-Inc/OuroMail/\\.github/workflows/ouromail-package\\.yml --source-ref refs/heads/ouromail/main\$" "$root/calls" "newer version"
expect "^sudo pacman -U -- .*/$pkg\$" "$root/calls" "newer version"
expect 'includes mailbox and content; excludes pictures\.cache and logs' "$root/out" "newer version"
expect 'settings and mailbox backed up' "$root/out" "newer version"
bak=$(find "$root/home/.config" -maxdepth 1 -type d -name 'ouromail.bak-ouromail-update-*')
cases=$((cases + 1))
if [ -n "$bak" ] \
	&& [ -f "$bak/mailbox.mailcraft" ] \
	&& [ -f "$bak/mailbox.mailcraft.blobs/3f/blob01" ] \
	&& [ -f "$bak/ui.json" ] \
	&& [ -f "$bak/nested/logs/keep.txt" ] \
	&& [ ! -e "$bak/pictures.cache" ] \
	&& [ ! -e "$bak/logs" ] \
	&& [ -z "$(find "$root/home/.config" -maxdepth 1 -name '.ouromail-update-backup.*')" ]; then
	echo "ok   backup includes mailbox+blobs+ui.json+nested/logs and excludes top-level pictures.cache+logs"
else
	echo "FAIL backup contents wrong: bak=$bak"
	fails=$((fails + 1))
fi

# First install with no settings dir: no backup and nothing created in HOME.
rm -rf "${root:?}/home"
mkdir -p "$root/home"
KEEP_HOME=1 FAKE_INSTALLED='' run "not installed yet, no settings dir" 0
expect 'installed: \(not installed\)' "$root/out" "not installed yet"
expect '^sudo pacman -U' "$root/calls" "not installed yet"
cases=$((cases + 1))
if [ -z "$(ls -A "$root/home")" ]; then
	echo "ok   no settings dir leaves HOME clean"
else
	echo "FAIL unexpected entries in HOME: $(ls -A "$root/home")"
	fails=$((fails + 1))
fi

# Retention: four older backups of ours and one that isn't -> newest 3 of ours kept after success.
seed_home
for d in 20260101-000001 20260101-000002 20260101-000003 20260101-000004; do
	mkdir "$root/home/.config/ouromail.bak-ouromail-update-$d"
done
mkdir "$root/home/.config/ouromail.bak-mine"
KEEP_HOME=1 FAKE_INSTALLED=0.14.0.r3610.gaaaaaaaa-1 run "backup retention" 0
kept=$(cd "$root/home/.config" && find . -maxdepth 1 -name 'ouromail.bak-*' | sort | tr '\n' ' ')
cases=$((cases + 1))
case "$kept" in
"./ouromail.bak-mine ./ouromail.bak-ouromail-update-20260101-000003 ./ouromail.bak-ouromail-update-20260101-000004 ./ouromail.bak-ouromail-update-2"???????-??????" ")
	echo "ok   keeps the last three complete backups" ;;
*)
	echo "FAIL keeps the last three complete backups: $kept"
	fails=$((fails + 1))
	;;
esac

# Insufficient free space: refuses before copying or rotating old backups or installing; never skips mailbox.
seed_home
mkdir "$root/home/.config/ouromail.bak-ouromail-update-20260101-000001"
KEEP_HOME=1 FAKE_DF_AVAIL=1 FAKE_INSTALLED=0.14.0.r3610.gaaaaaaaa-1 run "insufficient space" 1
expect 'not enough free space' "$root/out" "insufficient space"
refute '^sudo' "$root/calls" "insufficient space"
cases=$((cases + 1))
if [ -d "$root/home/.config/ouromail.bak-ouromail-update-20260101-000001" ] \
	&& [ -z "$(find "$root/home/.config" -maxdepth 1 -name '.ouromail-update-backup.*')" ] \
	&& [ "$(find "$root/home/.config" -maxdepth 1 -name 'ouromail.bak-ouromail-update-*' | wc -l)" = 1 ]; then
	echo "ok   insufficient space leaves old backups untouched and creates no partial backup"
else
	echo "FAIL insufficient space disturbed backups"
	fails=$((fails + 1))
fi

# Copy failure mid-backup: removes ONLY partial staging dir; valid old backups and source untouched; no rotation; no install.
seed_home
for d in 20260101-000001 20260101-000002 20260101-000003 20260101-000004; do
	mkdir "$root/home/.config/ouromail.bak-ouromail-update-$d"
	echo "keep-$d" >"$root/home/.config/ouromail.bak-ouromail-update-$d/marker"
done
KEEP_HOME=1 FAKE_CP_FAIL=1 FAKE_INSTALLED=0.14.0.r3610.gaaaaaaaa-1 run "copy failure cleanup" 1
expect 'backup copy .* failed' "$root/out" "copy failure cleanup"
refute '^sudo' "$root/calls" "copy failure cleanup"
cases=$((cases + 1))
if [ -z "$(find "$root/home/.config" -maxdepth 1 -name '.ouromail-update-backup.*')" ] \
	&& [ "$(find "$root/home/.config" -maxdepth 1 -name 'ouromail.bak-ouromail-update-*' | wc -l)" = 4 ] \
	&& [ -f "$root/home/.config/ouromail.bak-ouromail-update-20260101-000001/marker" ] \
	&& [ -f "$root/home/.config/ouromail/mailbox.mailcraft" ]; then
	echo "ok   copy failure removes only partial stage, preserves all old backups and source"
else
	echo "FAIL copy failure left partial stage or damaged old backups/source"
	fails=$((fails + 1))
fi

# Symlink top-level ~/.config/ouromail: fail closed, no backup, no install.
rm -rf "${root:?}/home" "$root/real-cfg"
mkdir -p "$root/home/.config" "$root/real-cfg"
echo "secret" >"$root/real-cfg/mailbox.mailcraft"
ln -s "$root/real-cfg" "$root/home/.config/ouromail"
KEEP_HOME=1 FAKE_INSTALLED=0.14.0.r3610.gaaaaaaaa-1 run "symlink settings dir refused" 1
expect 'symbolic link' "$root/out" "symlink settings dir refused"
refute '^sudo' "$root/calls" "symlink settings dir refused"

# Symlink inside selected backup entries: fail closed, no backup, no install.
seed_home
echo "outside" >"$root/outside-file"
ln -s "$root/outside-file" "$root/home/.config/ouromail/sneaky-link"
KEEP_HOME=1 FAKE_INSTALLED=0.14.0.r3610.gaaaaaaaa-1 run "internal symlink refused" 1
expect 'symbolic link' "$root/out" "internal symlink refused"
refute '^sudo' "$root/calls" "internal symlink refused"

# Symlink at excluded pictures.cache/logs is ignored without following.
seed_home
rm -rf "$root/home/.config/ouromail/pictures.cache"
ln -s "$root/outside-file" "$root/home/.config/ouromail/pictures.cache"
KEEP_HOME=1 FAKE_INSTALLED=0.14.0.r3610.gaaaaaaaa-1 run "symlink at excluded pictures.cache ignored" 0
expect '^sudo pacman -U' "$root/calls" "symlink at excluded pictures.cache ignored"

# Declined install (sudo pacman -U exits 1): exits 1, no "installed", tmp clean, complete backup intact.
FAKE_SUDO_RC=1 FAKE_INSTALLED=0.14.0.r3610.gaaaaaaaa-1 run "declined install" 1
refute 'ouromail-update: installed ' "$root/out" "declined install"
cases=$((cases + 1))
if [ -z "$(find "$root/home/.config" -maxdepth 1 -name '.ouromail-update-backup.*')" ] \
	&& [ "$(find "$root/home/.config" -maxdepth 1 -name 'ouromail.bak-ouromail-update-*' | wc -l)" = 1 ] \
	&& [ -f "$root/home/.config/ouromail/mailbox.mailcraft" ]; then
	echo "ok   declined install leaves complete backup and source intact, no partial stage"
else
	echo "FAIL declined install backup state wrong"
	fails=$((fails + 1))
fi

# Tampered package bytes: SHA-256 mismatch stops before gh and sudo.
echo tampered >>"$release/$pkg"
FAKE_INSTALLED=0.14.0.r3610.gaaaaaaaa-1 run "bad checksum" 1
expect 'SHA-256 mismatch' "$root/out" "bad checksum"
refute '^(gh|sudo)' "$root/calls" "bad checksum"
pkg=$(make_release 0.14.0.r3611.gbbbbbbbb)

# Oversize SHA256SUMS caught by curl --max-filesize AND by post-download wc -c cap.
head -c 70000 /dev/zero | tr '\0' '#' >>"$release/SHA256SUMS"
FAKE_INSTALLED=0.14.0.r3610.gaaaaaaaa-1 run "oversize SHA256SUMS (curl cap)" 1 --check
FAKE_CURL_IGNORE_MAXSIZE=1 FAKE_INSTALLED=0.14.0.r3610.gaaaaaaaa-1 run "oversize SHA256SUMS (wc cap)" 1 --check
expect 'larger than 65536' "$root/out" "oversize SHA256SUMS (wc cap)"
pkg=$(make_release 0.14.0.r3611.gbbbbbbbb)

FAKE_INSTALLED=0.14.0.r3610.gaaaaaaaa-1 FAKE_RUNNING=1 run "running program" 1
expect 'ouromail is running' "$root/out" "running program"
refute '^sudo' "$root/calls" "running program"

FAKE_INSTALLED=0.14.0.r3610.gaaaaaaaa-1 FAKE_UNATTESTED=1 run "missing attestation" 1
expect 'could not be verified' "$root/out" "missing attestation"
refute '^sudo' "$root/calls" "missing attestation"

without_gh
FAKE_INSTALLED=0.14.0.r3610.gaaaaaaaa-1 run "no gh" 1
expect 'gh \(package github-cli\) is not installed' "$root/out" "no gh"
refute '^sudo' "$root/calls" "no gh"

FAKE_INSTALLED=0.14.0.r3610.gaaaaaaaa-1 run "no gh --no-attestation" 0 --no-attestation
expect 'NOT checked' "$root/out" "no gh --no-attestation"
expect "^sudo pacman -U -- .*/$pkg\$" "$root/calls" "no gh --no-attestation"
with_gh

# SHA256SUMS that names two packages, a path, or CRLF.
f2="ouromail-0.14.0.r3612.gcccccccc-1-x86_64.pkg.tar.zst"
cp "$release/$pkg" "$release/$f2"
(cd "$release" && sha256sum "$f2" >>SHA256SUMS)
FAKE_INSTALLED=0.14.0.r3610.gaaaaaaaa-1 run "two packages listed" 1
expect 'exactly one package' "$root/out" "two packages listed"
printf '%064d  ../ouromail-0.14.0.r3613.gdddddddd-1-x86_64.pkg.tar.zst\n' 0 >"$release/SHA256SUMS"
FAKE_INSTALLED=0.14.0.r3610.gaaaaaaaa-1 run "path in SHA256SUMS" 1
expect 'exactly one package' "$root/out" "path in SHA256SUMS"
refute '^sudo' "$root/calls" "path in SHA256SUMS"
printf '%064d  ouromail-0.14.0.r3613.gdddddddd-1-x86_64.pkg.tar.zst\r\n' 0 >"$release/SHA256SUMS"
FAKE_INSTALLED=0.14.0.r3610.gaaaaaaaa-1 run "CRLF SHA256SUMS" 1
expect 'exactly one package' "$root/out" "CRLF SHA256SUMS"

run "unknown option" 2 --bogus
run "help" 0 --help
expect 'no-attestation' "$root/out" "help"
expect 'Exit codes:' "$root/out" "help"
expect '  1   ' "$root/out" "help"
expect '  2   ' "$root/out" "help"
expect '  10  ' "$root/out" "help"

echo "$cases cases, $fails failure(s)"
[ "$fails" = 0 ]
