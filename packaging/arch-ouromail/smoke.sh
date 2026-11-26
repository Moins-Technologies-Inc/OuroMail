#!/usr/bin/env bash
# shellcheck disable=SC2317
# SPDX-FileCopyrightText: MailCraft contributors
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Smoke checks of a built OuroMail package (those of the hand-built Stage 2 package): its file list,
# no path naming mailcraft, monotonic .PKGINFO version, the version shows the commit, the Entra
# application id in both binaries, the desktop Exec entry, the commands and settings that must be
# in the binaries, and no test bypass hook in ouromail-update. It runs the binaries it unpacks, so
# it belongs in a job that holds no secret.
# Usage: smoke.sh <package.pkg.tar.zst> <8-character commit> [commit-count]
# (SC2317: the checks are run by name through check(), which shellcheck cannot follow.)
set -euo pipefail
export LC_ALL=C

pkg=${1:?package file}
short=${2:?8-character commit}
revcount=${3:-}
[[ "$short" =~ ^[0-9a-f]{8}$ ]] || {
	echo "smoke: '$short' is not an 8-character commit" >&2
	exit 2
}
if [ -n "$revcount" ] && ! [[ "$revcount" =~ ^[1-9][0-9]*$ ]]; then
	echo "smoke: '$revcount' is not a positive commit count" >&2
	exit 2
fi

dir=$(mktemp -d "${TMPDIR:-/tmp}/ouromail-smoke.XXXXXXXX")
trap 'rm -rf -- "$dir"' EXIT
tar --zstd -xf "$pkg" -C "$dir"
bin="$dir/usr/bin"
desktop="$dir/usr/share/applications/tech.moins.ouromail.desktop"

want_files="usr/bin/ouromail
usr/bin/ouromail-cli
usr/bin/ouromail-update
usr/share/applications/tech.moins.ouromail.desktop
usr/share/icons/hicolor/128x128/apps/tech.moins.ouromail.png
usr/share/icons/hicolor/16x16/apps/tech.moins.ouromail.png
usr/share/icons/hicolor/24x24/apps/tech.moins.ouromail.png
usr/share/icons/hicolor/256x256/apps/tech.moins.ouromail.png
usr/share/icons/hicolor/32x32/apps/tech.moins.ouromail.png
usr/share/icons/hicolor/48x48/apps/tech.moins.ouromail.png
usr/share/icons/hicolor/512x512/apps/tech.moins.ouromail.png
usr/share/icons/hicolor/64x64/apps/tech.moins.ouromail.png
usr/share/licenses/ouromail/LICENSE-APACHE
usr/share/licenses/ouromail/LICENSE-MIT
usr/share/mime/packages/tech.moins.ouromail.mime.xml"

fails=0
# check NAME COMMAND...: runs the command and reports it.
check() {
	local name=$1
	shift
	if "$@"; then
		echo "ok   $name"
	else
		echo "FAIL $name"
		fails=$((fails + 1))
	fi
}
# hits_in BIN TEXT: how many lines of BIN hold TEXT.
hits_in() { grep -a -c -F -- "$2" "$1" || true; }
present_in() {
	local n
	n=$(hits_in "$1" "$2")
	echo "     $(basename "$1") $2: $n"
	[ "$n" -ge 1 ]
}
absent_in() {
	local n
	n=$(hits_in "$1" "$2")
	echo "     $(basename "$1") $2: $n"
	[ "$n" = 0 ]
}
lists() { awk -v c="$1" '$1 == c { found = 1 } END { exit !found }' <<<"$commands"; }
updater_help() {
	local out
	out=$("$bin/ouromail-update" --help)
	grep -q -- --no-attestation <<<"$out" && grep -q -- '10' <<<"$out"
}
updater_no_test_hook() { ! grep -qF -- 'OUROMAIL_UPDATE_TEST_RELEASE_DIR' "$bin/ouromail-update"; }
desktop_exec() { grep -qE '^Exec=ouromail( %U)?$' "$desktop" && grep -qx 'TryExec=ouromail' "$desktop"; }
pkginfo_ver() {
	[ -f "$dir/.PKGINFO" ] || return 1
	local ver re
	ver=$(sed -n 's/^pkgver = //p' "$dir/.PKGINFO" | head -n 1)
	echo "     .PKGINFO pkgver: $ver"
	if [ -n "$revcount" ]; then
		re="^[0-9]+\\.[0-9]+\\.[0-9]+\\.r${revcount}\\.g${short}-[1-9][0-9]*\$"
	else
		re="^[0-9]+\\.[0-9]+\\.[0-9]+\\.r[1-9][0-9]*\\.g${short}-[1-9][0-9]*\$"
	fi
	[[ "$ver" =~ $re ]]
}

echo "=== PACKAGE FILES ==="
got_files=$(cd "$dir" && find usr -type f | sort)
echo "$got_files"
echo "=== VERSION ==="
version=$("$bin/ouromail" --version)
echo "$version"
cli_version=$("$bin/ouromail-cli" --version)
echo "$cli_version"
echo "=== COMMANDS ==="
commands=$("$bin/ouromail-cli" commands)
grep -E 'calendar\.select|calendar\.paste|signature\.import' <<<"$commands" || true
echo "=== CHECKS ==="
check "file list" [ "$got_files" = "$want_files" ]
named=$(cd "$dir" && find . -iname '*mailcraft*')
check "no path names mailcraft" [ -z "$named" ]
check ".PKGINFO monotonic pkgver" pkginfo_ver
check "ouromail --version shows $short" grep -qF -- "($short)" <<<"$version"
check "ouromail-cli --version shows $short" grep -qF -- "($short)" <<<"$cli_version"
check "ouromail new Entra id present" present_in "$bin/ouromail" 5e47e861-08fe-4949-9d64-d747df40ea76
check "ouromail old Entra id absent" absent_in "$bin/ouromail" 6e83816e-f26c-455a-9253-494b784e61fa
check "ouromail-cli new Entra id present" present_in "$bin/ouromail-cli" 5e47e861-08fe-4949-9d64-d747df40ea76
check "ouromail-cli old Entra id absent" absent_in "$bin/ouromail-cli" 6e83816e-f26c-455a-9253-494b784e61fa
check "app id present" present_in "$bin/ouromail" tech.moins.ouromail
check "tray label present" present_in "$bin/ouromail" "Requires a system tray."
check "ui.reminderPopups present" present_in "$bin/ouromail" ui.reminderPopups
check "ui.accent present" present_in "$bin/ouromail" ui.accent
check "desktop Exec=ouromail" desktop_exec
for c in calendar.select calendar.paste signature.import; do
	check "ouromail-cli lists $c" lists "$c"
done
check "ouromail-update --help" updater_help
check "ouromail-update has no test-dir hook" updater_no_test_hook

echo "=== $fails failure(s) ==="
[ "$fails" = 0 ]
