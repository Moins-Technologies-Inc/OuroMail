#!/usr/bin/env bash
# SPDX-FileCopyrightText: MailCraft contributors
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Assertion tests for packaging/arch-ouromail/smoke.sh using synthetic packages (stub binaries,
# no real build). Verifies file list, mailcraft path check, monotonic .PKGINFO version, both
# binaries' --version and Entra ids, desktop Exec=, CLI commands, and updater checks.
# Usage: bash packaging/arch-ouromail/test-smoke.sh
set -euo pipefail
export LC_ALL=C

here=$(cd "$(dirname "$0")" && pwd)
smoke="$here/smoke.sh"
updater="$here/ouromail-update.sh"
T=$(mktemp -d "${TMPDIR:-/tmp}/ouromail-smoke-test.XXXXXXXX")
trap 'rm -rf -- "$T"' EXIT

NEW=5e47e861-08fe-4949-9d64-d747df40ea76
OLD=6e83816e-f26c-455a-9253-494b784e61fa
SHA=abcdef12
REV=3610
fails=0
cases=0

mk() {
	local n=$1 r="$T/$1/root"
	shift
	mkdir -p "$r/usr/bin" "$r/usr/share/applications" "$r/usr/share/licenses/ouromail" "$r/usr/share/mime/packages"
	for s in 16 24 32 48 64 128 256 512; do
		mkdir -p "$r/usr/share/icons/hicolor/${s}x${s}/apps"
		: >"$r/usr/share/icons/hicolor/${s}x${s}/apps/tech.moins.ouromail.png"
	done
	printf '[Desktop Entry]\nType=Application\nName=OuroMail\nExec=ouromail %%U\nTryExec=ouromail\n' \
		>"$r/usr/share/applications/tech.moins.ouromail.desktop"
	: >"$r/usr/share/mime/packages/tech.moins.ouromail.mime.xml"
	: >"$r/usr/share/licenses/ouromail/LICENSE-APACHE"
	: >"$r/usr/share/licenses/ouromail/LICENSE-MIT"
	printf 'pkgname = ouromail\npkgver = 0.14.0.r%s.g%s-1\n' "$REV" "$SHA" >"$r/.PKGINFO"
	printf '#!/bin/sh\n# %s tech.moins.ouromail Requires a system tray. ui.reminderPopups ui.accent\necho "OuroMail 0.14.0 (%s)"\n' \
		"$NEW" "$SHA" >"$r/usr/bin/ouromail"
	cat >"$r/usr/bin/ouromail-cli" <<EOF
#!/bin/sh
# $NEW
case "\$1" in
--version) echo "ouromail-cli 0.14.0 ($SHA)" ;;
commands) printf 'calendar.select  Select\ncalendar.paste  Paste\nsignature.import  Import\nmail.flag  Flag\n' ;;
esac
EOF
	cp "$updater" "$r/usr/bin/ouromail-update"
	chmod 0755 "$r/usr/bin/"*
	local t
	for t in "$@"; do
		(cd "$r" && eval "$t")
	done
	tar --zstd -cf "$T/$n.pkg.tar.zst" -C "$r" .PKGINFO usr
}

expect() {
	local n=$1 want=$2 line=${3:-} short=${4:-$SHA} rev=${5-$REV}
	cases=$((cases + 1))
	set +e
	if [ -n "$rev" ]; then
		env -i PATH=/usr/bin TMPDIR="$T" bash "$smoke" "$T/$n.pkg.tar.zst" "$short" "$rev" >"$T/$n.out" 2>&1
	else
		env -i PATH=/usr/bin TMPDIR="$T" bash "$smoke" "$T/$n.pkg.tar.zst" "$short" >"$T/$n.out" 2>&1
	fi
	local rc=$?
	set -e
	if [ "$want" = 0 ] && [ "$rc" != 0 ]; then
		echo "FAIL $n: rc $rc, want 0"
		sed 's/^/    /' "$T/$n.out"
		fails=$((fails + 1))
		return
	fi
	if [ "$want" != 0 ] && [ "$rc" = 0 ]; then
		echo "FAIL $n: rc 0, want failure"
		sed 's/^/    /' "$T/$n.out" | tail -n 25
		fails=$((fails + 1))
		return
	fi
	if [ -n "$line" ] && ! grep -qE -- "$line" "$T/$n.out"; then
		echo "FAIL $n: '$line' not in output"
		sed 's/^/    /' "$T/$n.out"
		fails=$((fails + 1))
		return
	fi
	echo "ok   $n (rc $rc)"
}

mk good
expect good 0 '^=== 0 failure\(s\) ===$'
expect good 0 '^=== 0 failure\(s\) ===$' "$SHA" ""
expect good 1 '^FAIL \.PKGINFO monotonic pkgver$' "$SHA" 9999
mk oldver "sed -i 's/r$REV\\.g$SHA/r$SHA/' .PKGINFO"
expect oldver 1 '^FAIL \.PKGINFO monotonic pkgver$' "$SHA" ""
mk oldid "printf '# $OLD\n' >>usr/bin/ouromail"
expect oldid 1 '^FAIL ouromail old Entra id absent$'
mk cli_oldid "printf '# $OLD\n' >>usr/bin/ouromail-cli"
expect cli_oldid 1 '^FAIL ouromail-cli old Entra id absent$'
mk cli_nonew "sed -i 's/$NEW/00000000-0000-0000-0000-000000000000/' usr/bin/ouromail-cli"
expect cli_nonew 1 '^FAIL ouromail-cli new Entra id present$'
mk bad_desktop "sed -i 's/Exec=ouromail %U/Exec=mailcraft %U/' usr/share/applications/tech.moins.ouromail.desktop"
expect bad_desktop 1 '^FAIL desktop Exec=ouromail$'
mk bad_tryexec "sed -i 's/TryExec=ouromail/TryExec=mailcraft/' usr/share/applications/tech.moins.ouromail.desktop"
expect bad_tryexec 1 '^FAIL desktop Exec=ouromail$'
mk extrafile ": >usr/bin/mailcraft"
expect extrafile 1 '^FAIL file list$'
mk emptydir "mkdir -p usr/share/MailCraft"
expect emptydir 1 '^FAIL no path names mailcraft$'
mk nocmd "sed -i 's/signature.import  Import//' usr/bin/ouromail-cli"
expect nocmd 1 '^FAIL ouromail-cli lists signature\.import$'
mk hookupd "printf '# OUROMAIL_UPDATE_TEST_RELEASE_DIR\n' >>usr/bin/ouromail-update"
expect hookupd 1 '^FAIL ouromail-update has no test-dir hook$'
expect good 2 'is not an 8-character commit' 'ABCDEF12'
expect good 2 'is not a positive commit count' "$SHA" '0'

echo "test-smoke: $cases cases, $fails failure(s)"
[ "$fails" = 0 ]
