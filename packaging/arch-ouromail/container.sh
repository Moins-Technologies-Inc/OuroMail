#!/usr/bin/env bash
# SPDX-FileCopyrightText: MailCraft contributors
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Inside the Arch Linux container of .github/workflows/ouromail-package.yml (started by
# runner.sh, never by hand on a workstation). As root: brings the container up to date from Arch's
# repositories and installs the toolchain's needs; then, as an unprivileged user with the runner
# user's ids, runs one job:
#   ci       the packaging checks, then `CI=1 cargo +1.98.1 xtask ci` (all its steps)
#   package  makepkg from packaging/arch-ouromail, the smoke checks, SHA256SUMS in /out
# /src is the checkout (read-only), /cache this branch's cache, /out (package only) the result.
# No GitHub token is in this environment.
set -euo pipefail
export LC_ALL=C

job=${1:?ci or package}
readonly TOOLCHAIN=1.98.1
: "${HOST_UID:?}" "${HOST_GID:?}"
[[ "$HOST_UID" =~ ^[1-9][0-9]*$ && "$HOST_GID" =~ ^[1-9][0-9]*$ ]] || {
	echo "container: HOST_UID and HOST_GID must be ids other than root's" >&2
	exit 2
}

if [ "$(id -u)" = 0 ]; then
	echo "::group::pacman -Syu"
	packages=(rustup git cargo-deny gtk3 libxkbcommon libxkbcommon-x11 libglvnd wayland pkgconf hicolor-icon-theme ttf-dejavu)
	if [ "$job" = ci ]; then
		packages+=(shellcheck actionlint zizmor)
	fi
	pacman -Syu --noconfirm --needed "${packages[@]}"
	echo "::endgroup::"
	pacman -Q cargo-deny rustup
	getent group "$HOST_GID" >/dev/null || groupadd -g "$HOST_GID" builder
	name=$(getent passwd "$HOST_UID" | cut -d: -f1 || true)
	if [ -z "$name" ]; then
		useradd -M -d /home/builder -u "$HOST_UID" -g "$HOST_GID" -s /bin/bash builder
		name=builder
	fi
	install -d -o "$HOST_UID" -g "$HOST_GID" -m 0700 /home/builder /tmp/ouromail
	exec setpriv --reuid="$HOST_UID" --regid="$HOST_GID" --clear-groups \
		env -i PATH=/usr/local/bin:/usr/bin HOME=/home/builder USER="$name" LANG=C.UTF-8 \
		TMPDIR=/tmp/ouromail CI=1 "SHORT_SHA=${SHORT_SHA:-}" HOST_UID="$HOST_UID" HOST_GID="$HOST_GID" \
		GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=safe.directory GIT_CONFIG_VALUE_0=/src \
		bash "$0" "$job"
fi

# From here on: the unprivileged user.
export CARGO_HOME=/cache/cargo-home RUSTUP_HOME=/cache/rustup RUSTUP_AUTO_INSTALL=0
export CARGO_NET_RETRY=10 CARGO_TERM_COLOR=always
mkdir -p "$CARGO_HOME" "$RUSTUP_HOME"
echo "::group::rustup toolchain install $TOOLCHAIN"
rustup toolchain install "$TOOLCHAIN" --profile minimal --component rustfmt,clippy --target wasm32-unknown-unknown
echo "::endgroup::"
rustc "+$TOOLCHAIN" --version
cargo "+$TOOLCHAIN" deny --version
nproc
cd /src

case "$job" in
ci)
	# Every check runs even when one before it fails, so that one run shows every failure; the
	# job fails at the end if any did.
	failed=()
	# step NAME COMMAND...: runs the command and notes its failure.
	step() {
		local name=$1
		shift
		echo "::group::$name"
		"$@" || failed+=("$name")
		echo "::endgroup::"
	}
	step shellcheck shellcheck packaging/arch-ouromail/*.sh
	step "ouromail-update tests" bash packaging/arch-ouromail/test-ouromail-update.sh
	step "runner tests" bash packaging/arch-ouromail/test-runner.sh
	step "smoke tests" bash packaging/arch-ouromail/test-smoke.sh
	step "workflow and pkgver tests" bash packaging/arch-ouromail/test-workflow-and-pkgver.sh
	step actionlint actionlint -no-color .github/workflows/ouromail-package.yml
	step zizmor zizmor --offline --no-progress .github/workflows/ouromail-package.yml
	export CARGO_TARGET_DIR=/cache/target-ci
	# Exactly as a contributor runs it: every step, none skipped.
	step "cargo xtask ci" env -u XTASK_CI_SKIP -u XTASK_CI_ONLY -u XTASK_TEST_FILTER cargo "+$TOOLCHAIN" xtask ci
	if [ "${#failed[@]}" -gt 0 ]; then
		printf 'container: failed: %s\n' "${failed[@]}" >&2
		exit 1
	fi
	;;
package)
	[[ "$SHORT_SHA" =~ ^[0-9a-f]{8}$ ]] || {
		echo "container: SHORT_SHA must be 8 hex digits" >&2
		exit 2
	}
	is_shallow=$(git -C /src rev-parse --is-shallow-repository 2>/dev/null || echo invalid)
	[ "$is_shallow" = false ] || {
		echo "container: /src is a shallow clone or not a git repository; full history is required" >&2
		exit 1
	}
	head_sha=$(git -C /src rev-parse --verify HEAD 2>/dev/null || true)
	if ! [[ "$head_sha" =~ ^[0-9a-f]{40}$ ]] || [ "${head_sha:0:8}" != "$SHORT_SHA" ]; then
		echo "container: HEAD '$head_sha' does not match SHORT_SHA '$SHORT_SHA'" >&2
		exit 1
	fi
	revcount=$(git -C /src rev-list --count HEAD 2>/dev/null || true)
	[[ "$revcount" =~ ^[1-9][0-9]*$ ]] || {
		echo "container: invalid git commit count '$revcount'" >&2
		exit 1
	}
	export CARGO_TARGET_DIR=/cache/target-package
	cd packaging/arch-ouromail
	BUILDDIR=/tmp/ouromail/build PKGDEST=/out SRCDEST=/tmp/ouromail/srcdest makepkg
	cd /out
	pkg=$(find . -maxdepth 1 -type f ! -type l -name "ouromail-*.r${revcount}.g${SHORT_SHA}-*-x86_64.pkg.tar.zst" ! -name 'ouromail-debug-*' -printf '%f\n')
	[ "$(printf '%s\n' "$pkg" | grep -c .)" = 1 ] || {
		echo "container: expected one package, found: $pkg" >&2
		exit 1
	}
	# Only the package itself is published; remove any other regular files.
	find . -maxdepth 1 -type f ! -name "$pkg" -delete
	bash /src/packaging/arch-ouromail/smoke.sh "$pkg" "$SHORT_SHA" "$revcount"
	sha256sum "$pkg" >SHA256SUMS
	cat SHA256SUMS
	;;
*)
	echo "container: unknown job '$job'" >&2
	exit 2
	;;
esac
