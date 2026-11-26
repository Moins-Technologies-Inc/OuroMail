#!/usr/bin/env bash
# SPDX-FileCopyrightText: MailCraft contributors
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Host side of one job of .github/workflows/ouromail-package.yml, on the organisation's
# self-hosted runner (rootless podman, as the runner's user): picks this branch's cache, removes
# what a cancelled run may have left, starts the Arch container with container.sh, and keeps the
# cache under its cap.
# Usage: runner.sh ci|package
#
# The container sees the checkout (read-only), this branch's cache directory and, for `package`,
# an output directory under $RUNNER_TEMP; never the runner's home, its credentials, the host-only
# branch metadata directory ($root/keys), or another branch's cache, and no GitHub token (only the
# variables named below are passed in).
set -euo pipefail
export LC_ALL=C

job=${1:?ci or package}
case "$job" in ci | package) ;; *)
	echo "runner: unknown job '$job'" >&2
	exit 2
	;;
esac
: "${OUROMAIL_CACHE:?}" "${OUROMAIL_IMAGE:?}" "${GITHUB_WORKSPACE:?}" "${GITHUB_REF:?}" "${GITHUB_SHA:?}"
: "${GITHUB_RUN_ID:?}" "${GITHUB_RUN_ATTEMPT:?}" "${RUNNER_TEMP:?}"
[[ "$GITHUB_SHA" =~ ^[0-9a-f]{40}$ ]] || {
	echo "runner: GITHUB_SHA must be 40 lowercase hex digits" >&2
	exit 2
}
readonly TOOLCHAIN=1.98.1
# What the cache may hold, all branches together; the least recently used branch goes first.
readonly CACHE_MAX_GB=${OUROMAIL_CACHE_MAX_GB:-70}
[[ "$CACHE_MAX_GB" =~ ^[1-9][0-9]*$ ]] || {
	echo "runner: OUROMAIL_CACHE_MAX_GB must be a number of GB" >&2
	exit 2
}
# A job that runs longer than this is stopped by podman itself, whatever happens to the runner.
readonly CONTAINER_TIMEOUT=$((4 * 3600))

require_real_dir() {
	local p=$1 label=$2
	if [ -L "$p" ] || [ ! -d "$p" ]; then
		echo "runner: $label ($p) must be a real directory, not a symlink" >&2
		exit 1
	fi
}

require_not_symlink() {
	local p=$1 label=$2
	if [ -L "$p" ]; then
		echo "runner: refusing symlink for $label ($p)" >&2
		exit 1
	fi
}

require_real_dir "$OUROMAIL_CACHE" "OUROMAIL_CACHE"
require_real_dir "$RUNNER_TEMP" "RUNNER_TEMP"

# The package's output, read by the workflow's later steps.
out="$RUNNER_TEMP/ouromail-out"

# The runner service may run its jobs as root (docs/ouromail-builds.md). Podman is set up rootless
# for the user who owns the cache, so the rest of this script runs as that user, with only the
# variables it needs.
if [ "$(id -u)" = 0 ]; then
	user=$(stat -c %U "$OUROMAIL_CACHE")
	[ "$user" != root ] || {
		echo "runner: $OUROMAIL_CACHE must belong to the user podman runs as, not root" >&2
		exit 2
	}
	if [ "$job" = package ]; then
		require_not_symlink "$out" "package output dir"
		rm -rf -- "$out"
		install -d -m 0700 -o "$user" -g "$(id -gn "$user")" "$out"
	fi
	echo "runner: the job runs as root; podman runs as $user"
	exec runuser -u "$user" -- env -i PATH=/usr/local/bin:/usr/bin:/bin LANG=C.UTF-8 \
		HOME="$(getent passwd "$user" | cut -d: -f6)" USER="$user" LOGNAME="$user" \
		XDG_RUNTIME_DIR="/run/user/$(id -u "$user")" OUROMAIL_OUT_READY=1 \
		OUROMAIL_CACHE="$OUROMAIL_CACHE" OUROMAIL_IMAGE="$OUROMAIL_IMAGE" \
		OUROMAIL_CACHE_MAX_GB="${OUROMAIL_CACHE_MAX_GB:-}" GITHUB_WORKSPACE="$GITHUB_WORKSPACE" \
		GITHUB_REF="$GITHUB_REF" GITHUB_SHA="$GITHUB_SHA" GITHUB_RUN_ID="$GITHUB_RUN_ID" \
		GITHUB_RUN_ATTEMPT="$GITHUB_RUN_ATTEMPT" RUNNER_TEMP="$RUNNER_TEMP" \
		bash "$0" "$job"
fi

# One directory per branch (and toolchain), so that a manual run on another branch never writes
# into what ouromail/main builds with. Branch KEY metadata lives outside the container mount under
# $root/keys/<key>.key so container-writable /cache can never influence host file reads or writes.
key=$(printf '%s\n%s\n' "$GITHUB_REF" "$TOOLCHAIN" | sha256sum | cut -c1-16)
[[ "$key" =~ ^[0-9a-f]{16}$ ]] || {
	echo "runner: invalid cache key '$key'" >&2
	exit 1
}
root="$OUROMAIL_CACHE/ouromail-v1"
meta_dir="$root/keys"
cache="$root/$key"
lock_file="$root/.lock"
me=$(id -u)

check_cache_tree() {
	require_real_dir "$OUROMAIL_CACHE" "OUROMAIL_CACHE"
	require_not_symlink "$root" "cache root"
	if [ -e "$root" ]; then
		require_real_dir "$root" "cache root"
		local sym_entry
		sym_entry=$(find "$root" -mindepth 1 -maxdepth 1 -type l -print -quit)
		if [ -n "$sym_entry" ]; then
			echo "runner: refusing symlinked cache entry $sym_entry" >&2
			exit 1
		fi
	fi
	require_not_symlink "$meta_dir" "cache metadata dir"
	if [ -e "$meta_dir" ]; then
		require_real_dir "$meta_dir" "cache metadata dir"
		local bad_meta
		bad_meta=$(find "$meta_dir" -mindepth 1 -maxdepth 1 \( -type l -o ! -type f \) -print -quit)
		if [ -n "$bad_meta" ]; then
			echo "runner: refusing non-regular or symlinked cache metadata $bad_meta" >&2
			exit 1
		fi
	fi
	require_not_symlink "$cache" "branch cache"
	if [ -e "$cache" ]; then
		require_real_dir "$cache" "branch cache"
	fi
	require_not_symlink "$lock_file" "cache lock"
	if [ -e "$lock_file" ] && [ ! -f "$lock_file" ]; then
		echo "runner: refusing non-regular cache lock $lock_file" >&2
		exit 1
	fi
}

check_cache_tree
mkdir -p "$root" "$meta_dir" "$cache"
check_cache_tree

# One job at a time uses the cache (the runner runs one job at a time anyway).
exec 9>"$lock_file"
flock -w 3600 9 || {
	echo "runner: cache lock busy for 1 h" >&2
	exit 1
}
check_cache_tree

meta_file="$meta_dir/$key.key"
if [ -L "$meta_file" ] || { [ -e "$meta_file" ] && [ ! -f "$meta_file" ]; }; then
	echo "runner: refusing non-regular cache metadata $meta_file" >&2
	exit 1
fi
tmp_meta=$(mktemp "$meta_dir/.key.XXXXXXXX")
chmod 0600 "$tmp_meta"
safe_ref=$(printf '%s' "${GITHUB_REF:0:200}" | tr -cd '[:print:]')
printf '%s %s\n' "$safe_ref" "$TOOLCHAIN" >"$tmp_meta"
mv -f -- "$tmp_meta" "$meta_file"
touch "$cache"
echo "cache: $cache ($(du -Psh "$cache" | cut -f1)) for $safe_ref, toolchain $TOOLCHAIN"

# A container left by a cancelled or killed run of this workflow is removed before anything else.
mapfile -t stale < <(podman ps -a --filter 'name=^ouromail-' --format '{{.Names}}')
for c in "${stale[@]}"; do
	echo "runner: removing container $c left by an earlier run"
	podman rm -f -t 10 "$c" >/dev/null
done
[ "${#stale[@]}" -gt 0 ] || echo "runner: no container left by an earlier run"

name="ouromail-$GITHUB_RUN_ID-$GITHUB_RUN_ATTEMPT-$job"
# shellcheck disable=SC2329 # called by the EXIT trap below
cleanup() { podman rm -f -t 10 "$name" >/dev/null 2>&1 || true; }
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# No --init: the runner's podman has no init binary (catatonit). bash is the container's
# first process; a cancellation reaches it through cleanup(), whose `podman rm -f -t 10`
# kills what is still running after ten seconds, and --timeout bounds the job anyway.
args=(run --rm --name "$name" --timeout "$CONTAINER_TIMEOUT"
	--userns=keep-id --user 0:0 --pull=missing
	-v "$GITHUB_WORKSPACE:/src:ro"
	-v "$cache:/cache:rw"
	-e "HOST_UID=$(id -u)" -e "HOST_GID=$(id -g)" -e "SHORT_SHA=${GITHUB_SHA:0:8}"
	-w /src)
if [ "$job" = package ]; then
	if [ -z "${OUROMAIL_OUT_READY:-}" ]; then
		require_real_dir "$RUNNER_TEMP" "RUNNER_TEMP"
		require_not_symlink "$out" "package output dir"
		rm -rf -- "$out"
		mkdir -m 0700 "$out"
	else
		require_real_dir "$out" "package output dir"
	fi
	args+=(-v "$out:/out:rw")
fi

# In the background, so that a cancellation reaches the trap at once.
podman "${args[@]}" "$OUROMAIL_IMAGE" bash /src/packaging/arch-ouromail/container.sh "$job" &
rc=0
wait $! || rc=$?
# Ensure the container is fully stopped and removed before host-side cache pruning or result collection.
podman rm -f -t 10 "$name" >/dev/null 2>&1 || true

# Keep the cache under its cap: whole branch directories, least recently used first, never this one.
check_cache_tree
while :; do
	used=$(du -Ps --block-size=1G "$root" | cut -f1)
	[ "$used" -gt "$CACHE_MAX_GB" ] || break
	entry=
	IFS= read -r -d '' entry < <(
		find "$root" -mindepth 1 -maxdepth 1 -type d ! -type l ! -name "$key" \
			-regextype posix-extended -regex '.*/[0-9a-f]{16}' \
			-printf '%T@ %f\0' | LC_ALL=C sort -z -n
	) || true
	if [ -z "$entry" ]; then
		echo "runner: cache at ${used}G over its ${CACHE_MAX_GB}G cap with only this branch left; clearing its targets"
		require_real_dir "$cache" "branch cache"
		rm -rf -- "$cache/target-ci" "$cache/target-package"
		break
	fi
	oldest_name=${entry#* }
	if ! [[ "$oldest_name" =~ ^[0-9a-f]{16}$ ]] || [ "$oldest_name" = "$key" ]; then
		echo "runner: refusing unexpected cache entry '$oldest_name'" >&2
		exit 1
	fi
	oldest="$root/$oldest_name"
	if [ -L "$oldest" ] || [ ! -d "$oldest" ] || [ "$(stat -c %u "$oldest")" != "$me" ]; then
		echo "runner: refusing to prune unverified cache entry $oldest" >&2
		exit 1
	fi
	label='?'
	oldest_meta="$meta_dir/$oldest_name.key"
	if [ -e "$oldest_meta" ] || [ -L "$oldest_meta" ]; then
		if [ -L "$oldest_meta" ] || [ ! -f "$oldest_meta" ] || [ "$(stat -c %u "$oldest_meta")" != "$me" ]; then
			echo "runner: refusing non-regular or unowned cache metadata $oldest_meta" >&2
			exit 1
		fi
		raw_label=$(head -c 200 "$oldest_meta" | tr -cd '[:print:]')
		[ -n "$raw_label" ] && label="$raw_label"
	fi
	echo "runner: cache at ${used}G, over ${CACHE_MAX_GB}G: removing $oldest ($label)"
	rm -rf -- "$oldest"
	if [ -f "$oldest_meta" ] && [ ! -L "$oldest_meta" ]; then
		rm -f -- "$oldest_meta"
	fi
done
echo "cache after: $(du -Psh "$cache" | cut -f1) here, $(du -Psh "$root" | cut -f1) in all"
exit "$rc"
