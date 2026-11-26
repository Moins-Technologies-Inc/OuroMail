#!/usr/bin/env bash
# SPDX-FileCopyrightText: MailCraft contributors
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Assertion tests for packaging/arch-ouromail/runner.sh with a fake podman on PATH.
# Verifies host-side cache metadata isolation (V1/V2), symlink/non-regular refusal, bounded LRU
# pruning of verified owned 16-hex children only, container isolation flags, lock timeout message,
# and SIGINT/SIGTERM cancellation. No real podman or network access.
# Usage: bash packaging/arch-ouromail/test-runner.sh
set -euo pipefail
export LC_ALL=C

here=$(cd "$(dirname "$0")" && pwd)
runner="$here/runner.sh"
T=$(mktemp -d "${TMPDIR:-/tmp}/ouromail-runner-test.XXXXXXXX")
trap 'rm -rf -- "$T"' EXIT

real_bash=$(command -v bash)
mkdir -p "$T/bin" "$T/cache" "$T/ws" "$T/rt" "$T/home"

cat >"$T/bin/podman" <<'EOF'
#!/bin/bash
{ printf 'podman'; printf ' [%s]' "$@"; echo; } >>"$PLOG"
case "$1" in
ps) cat "$PSTALE" 2>/dev/null || true ;;
run)
	cache="" out="" prev=""
	for a in "$@"; do
		if [ "$prev" = -v ]; then
			case "$a" in
			*:/cache:rw) cache=${a%:/cache:rw} ;;
			*:/out:rw) out=${a%:/out:rw} ;;
			esac
		fi
		prev=$a
	done
	if [ "${FAKE_PLANT:-}" = key ] && [ -n "$cache" ]; then
		ln -sfn "$VICTIM" "$cache/KEY"
	fi
	sleep "${FAKE_RUN_SECONDS:-0}"
	exit "${FAKE_RUN_EXIT:-0}"
	;;
esac
exit 0
EOF
chmod +x "$T/bin/podman"

fails=0
cases=0
pass() { cases=$((cases + 1)); echo "ok   $1"; }
fail() { cases=$((cases + 1)); echo "FAIL $1${2:+: $2}"; fails=$((fails + 1)); }

cache_key() { printf '%s\n%s\n' "$1" 1.98.1 | sha256sum | cut -c1-16; }

rrun() {
	local job=$1
	shift
	env -i PATH="$T/bin:/usr/bin" HOME="$T/home" PLOG="$T/plog" PSTALE="$T/stale" \
		OUROMAIL_CACHE="${CACHE_OVERRIDE:-$T/cache}" OUROMAIL_IMAGE=img \
		GITHUB_WORKSPACE="$T/ws" GITHUB_REF="${REF:-refs/heads/ouromail/main}" \
		GITHUB_SHA="${SHA_OVERRIDE:-0123456789abcdef0123456789abcdef01234567}" \
		GITHUB_RUN_ID=7 GITHUB_RUN_ATTEMPT=1 RUNNER_TEMP="${RT_OVERRIDE:-$T/rt}" \
		GITHUB_TOKEN=LEAKTOKEN ACTIONS_RUNTIME_TOKEN=LEAKRT \
		VICTIM="$T/home/victim-canary" "$@" \
		"$real_bash" "$runner" "$job" >"$T/out" 2>&1
}

main_key=$(cache_key refs/heads/ouromail/main)
main_cache="$T/cache/ouromail-v1/$main_key"
main_meta="$T/cache/ouromail-v1/keys/$main_key.key"

# R1: package job argv isolation, /src:ro, 3 -v, 3 -e, no token leak, KEY stored outside /cache mount, container stopped before exit.
echo ouromail-6-1-ci >"$T/stale"
mkdir -p "$T/rt/ouromail-out"
echo old >"$T/rt/ouromail-out/leftover"
: >"$T/plog"
if rrun package; then rc=0; else rc=$?; fi
runline=$(grep '^podman \[run\]' "$T/plog" || true)
if [ "$rc" = 0 ] \
	&& grep -qF "[-v] [$T/ws:/src:ro]" <<<"$runline" \
	&& grep -qF "[-v] [$main_cache:/cache:rw]" <<<"$runline" \
	&& grep -qF "[-v] [$T/rt/ouromail-out:/out:rw]" <<<"$runline" \
	&& ! grep -qE 'LEAK|--env-host|--privileged|--init|podman\.sock|docker\.sock' <<<"$runline" \
	&& [ "$(grep -o '\[-e\]' <<<"$runline" | wc -l)" = 3 ] \
	&& [ "$(grep -o '\[-v\]' <<<"$runline" | wc -l)" = 3 ] \
	&& grep -qF 'podman [rm] [-f] [-t] [10] [ouromail-6-1-ci]' "$T/plog" \
	&& grep -qF 'podman [rm] [-f] [-t] [10] [ouromail-7-1-package]' "$T/plog" \
	&& [ "$(stat -c %a "$T/rt/ouromail-out")" = 700 ] \
	&& [ ! -e "$T/rt/ouromail-out/leftover" ] \
	&& [ ! -e "$main_cache/KEY" ] \
	&& [ "$(cat "$main_meta")" = "refs/heads/ouromail/main 1.98.1" ]; then
	pass "R1 package isolation, stale cleanup, host-only metadata outside /cache mount"
else
	fail "R1 package isolation" "rc=$rc runline=$runline out=$(cat "$T/out")"
fi
rm -f "$T/stale"

# R2 (V1): container replaces /cache/KEY with symlink to host canary -> next run leaves canary intact.
echo "CANARY-VICTIM-INTACT" >"$T/home/victim-canary"
if rrun ci FAKE_PLANT=key && rrun ci; then rc=0; else rc=$?; fi
if [ "$rc" = 0 ] && [ "$(cat "$T/home/victim-canary")" = "CANARY-VICTIM-INTACT" ] \
	&& [ "$(cat "$main_meta")" = "refs/heads/ouromail/main 1.98.1" ]; then
	pass "R2 V1 container /cache/KEY symlink cannot overwrite host file"
else
	fail "R2 V1" "rc=$rc victim=$(cat "$T/home/victim-canary")"
fi

# R3 (V2): another branch container plants /cache/KEY -> secret canary; LRU pruning logs host key, never secret.
echo "CANARY-SECRET-DO-NOT-LEAK" >"$T/home/secret-canary"
other_key=$(cache_key refs/heads/other)
if REF=refs/heads/other rrun ci FAKE_PLANT=key VICTIM="$T/home/secret-canary"; then rc=0; else rc=$?; fi
fallocate -l 1500M "$T/cache/ouromail-v1/$other_key/blob"
touch -h -d '2 days ago' "$T/cache/ouromail-v1/$other_key"
if rrun ci OUROMAIL_CACHE_MAX_GB=1; then rc2=0; else rc2=$?; fi
if [ "$rc" = 0 ] && [ "$rc2" = 0 ] \
	&& ! grep -q "CANARY-SECRET-DO-NOT-LEAK" "$T/out" \
	&& grep -qF "refs/heads/other 1.98.1" "$T/out" \
	&& [ ! -e "$T/cache/ouromail-v1/$other_key" ] \
	&& [ ! -e "$T/cache/ouromail-v1/keys/$other_key.key" ] \
	&& [ -d "$main_cache" ]; then
	pass "R3 V2 LRU pruning reads host-only metadata, never container /cache/KEY"
else
	fail "R3 V2" "rc=$rc rc2=$rc2 out=$(cat "$T/out")"
fi

# R4: LRU pruning ignores arbitrary / non-hex / real-newline directories under root and prunes only 16-hex dirs.
nl_dir="$T/cache/ouromail-v1/"$'bad\nname'
mkdir -p "$T/cache/ouromail-v1/0123456789abcdef" "$T/cache/ouromail-v1/not-a-hex-dir" "$nl_dir"
nl_found=$(find "$T/cache/ouromail-v1" -mindepth 1 -maxdepth 1 -type d -name $'bad\nname' -printf 'yes')
fallocate -l 1200M "$T/cache/ouromail-v1/0123456789abcdef/blob"
touch -d '5 days ago' "$T/cache/ouromail-v1/not-a-hex-dir" "$nl_dir"
touch -d '3 days ago' "$T/cache/ouromail-v1/0123456789abcdef"
if rrun ci OUROMAIL_CACHE_MAX_GB=1; then rc=0; else rc=$?; fi
nl_after=$(find "$T/cache/ouromail-v1" -mindepth 1 -maxdepth 1 -type d -name $'bad\nname' -printf 'yes')
if [ "$rc" = 0 ] \
	&& [[ "$nl_dir" == *$'\n'* ]] \
	&& [ "$nl_found" = yes ] \
	&& [ "$nl_after" = yes ] \
	&& [ ! -e "$T/cache/ouromail-v1/0123456789abcdef" ] \
	&& [ -d "$T/cache/ouromail-v1/not-a-hex-dir" ] \
	&& [ -d "$nl_dir" ]; then
	pass "R4 LRU prunes only validated 16-hex branch dirs, ignores arbitrary/real-newline names"
else
	fail "R4 LRU scope" "rc=$rc nl_found=$nl_found nl_after=$nl_after out=$(cat "$T/out")"
fi
rm -rf -- "$T/cache/ouromail-v1/not-a-hex-dir" "$nl_dir"

# R5: over cap with only this branch left clears targets, keeps cargo-home.
mkdir -p "$main_cache/target-ci" "$main_cache/cargo-home"
fallocate -l 1200M "$main_cache/target-ci/blob"
echo keep >"$main_cache/cargo-home/keep"
if rrun ci OUROMAIL_CACHE_MAX_GB=1; then rc=0; else rc=$?; fi
if [ "$rc" = 0 ] && [ ! -e "$main_cache/target-ci" ] && [ -f "$main_cache/cargo-home/keep" ]; then
	pass "R5 over-cap single branch clears targets only"
else
	fail "R5 single branch cap" "rc=$rc"
fi

# R6: symlinked cache root, symlinked cache entry, and symlinked/nonregular metadata all refused fail-closed.
mkdir -p "$T/real-c" "$T/outside"
echo "PRECIOUS" >"$T/outside/precious"
ln -s "$T/real-c" "$T/sym-c"
if CACHE_OVERRIDE="$T/sym-c" rrun ci; then fail "R6a symlinked OUROMAIL_CACHE accepted"; else pass "R6a symlinked OUROMAIL_CACHE refused"; fi

mkdir -p "$T/c-entry/ouromail-v1"
ln -s "$T/outside" "$T/c-entry/ouromail-v1/ffffffffffffffff"
if CACHE_OVERRIDE="$T/c-entry" rrun ci; then
	fail "R6b symlinked cache entry accepted"
elif [ -f "$T/outside/precious" ]; then
	pass "R6b symlinked cache entry refused and target intact"
else
	fail "R6b target damaged"
fi

mkdir -p "$T/c-meta/ouromail-v1/keys"
ln -s "$T/outside/precious" "$T/c-meta/ouromail-v1/keys/$main_key.key"
if CACHE_OVERRIDE="$T/c-meta" rrun ci; then
	fail "R6c symlinked metadata file accepted"
elif [ "$(cat "$T/outside/precious")" = "PRECIOUS" ]; then
	pass "R6c symlinked metadata file refused and target intact"
else
	fail "R6c target damaged"
fi

mkdir -p "$T/c-fifo/ouromail-v1/keys"
mkfifo "$T/c-fifo/ouromail-v1/keys/$main_key.key"
if CACHE_OVERRIDE="$T/c-fifo" rrun ci; then fail "R6d non-regular FIFO metadata accepted"; else pass "R6d non-regular FIFO metadata refused"; fi

# R7: exit code propagation, bad job, bad cap, bad SHA.
if rrun ci FAKE_RUN_EXIT=3; then rc=0; else rc=$?; fi
if [ "$rc" = 3 ]; then pass "R7a container exit code 3 propagated"; else fail "R7a exit code" "rc=$rc"; fi
if rrun deploy; then rc=0; else rc=$?; fi
if [ "$rc" = 2 ]; then pass "R7b unknown job rc 2"; else fail "R7b" "rc=$rc"; fi
if rrun ci OUROMAIL_CACHE_MAX_GB=abc; then rc=0; else rc=$?; fi
if [ "$rc" = 2 ]; then pass "R7c bad cap rc 2"; else fail "R7c" "rc=$rc"; fi
if SHA_OVERRIDE=notasha rrun ci; then rc=0; else rc=$?; fi
if [ "$rc" = 2 ]; then pass "R7d bad GITHUB_SHA rc 2"; else fail "R7d" "rc=$rc"; fi

# R8: SIGTERM -> 143 and SIGINT -> 130 both remove own container.
for sig_case in "TERM 143" "INT 130"; do
	sig=${sig_case%% *}
	want_rc=${sig_case##* }
	: >"$T/plog"
	setsid env --default-signal=INT --default-signal=TERM -i PATH="$T/bin:/usr/bin" HOME="$T/home" PLOG="$T/plog" PSTALE="$T/stale" \
		OUROMAIL_CACHE="$T/cache" OUROMAIL_IMAGE=img GITHUB_WORKSPACE="$T/ws" \
		GITHUB_REF=refs/heads/ouromail/main GITHUB_SHA=0123456789abcdef0123456789abcdef01234567 \
		GITHUB_RUN_ID=7 GITHUB_RUN_ATTEMPT=1 RUNNER_TEMP="$T/rt" FAKE_RUN_SECONDS=30 \
		"$real_bash" "$runner" ci >"$T/sig-$sig.out" 2>&1 &
	pid=$!
	for _ in {1..50}; do if grep -q '^podman \[run\]' "$T/plog" 2>/dev/null; then break; fi; sleep 0.1; done
	sleep 0.2
	kill "-$sig" -- "-$pid" 2>/dev/null || true
	if wait "$pid"; then rc=0; else rc=$?; fi
	if [ "$rc" = "$want_rc" ] && grep -qF 'podman [rm] [-f] [-t] [10] [ouromail-7-1-ci]' "$T/plog"; then
		pass "R8 SIG$sig exits $want_rc and removes own container"
	else
		fail "R8 SIG$sig" "rc=$rc want=$want_rc"
	fi
done

# R9: lock timeout prints a clear error message (simulated via fake flock).
mkdir -p "$T/fbin"
cp "$T/bin/podman" "$T/fbin/podman"
printf '#!%s\nexit 1\n' "$real_bash" >"$T/fbin/flock"
chmod +x "$T/fbin/flock"
if env -i PATH="$T/fbin:/usr/bin" HOME="$T/home" PLOG="$T/plog" PSTALE="$T/stale" \
	OUROMAIL_CACHE="$T/cache" OUROMAIL_IMAGE=img GITHUB_WORKSPACE="$T/ws" \
	GITHUB_REF=refs/heads/ouromail/main GITHUB_SHA=0123456789abcdef0123456789abcdef01234567 \
	GITHUB_RUN_ID=7 GITHUB_RUN_ATTEMPT=1 RUNNER_TEMP="$T/rt" \
	"$real_bash" "$runner" ci >"$T/lock.out" 2>&1; then
	fail "R9 lock timeout should fail"
elif grep -q 'cache lock busy' "$T/lock.out"; then
	pass "R9 lock timeout reports clear error message"
else
	fail "R9 lock timeout missing message" "$(cat "$T/lock.out")"
fi

# R10 (P1.13): control characters and newlines in GITHUB_REF are stripped before logging or storing metadata.
ctrl_ref=$'refs/heads/bad\x1b[31m\nINJECTED'
ctrl_key=$(cache_key "$ctrl_ref")
ctrl_meta="$T/cache/ouromail-v1/keys/$ctrl_key.key"
if REF="$ctrl_ref" rrun ci; then rc=0; else rc=$?; fi
if [ "$rc" = 0 ] \
	&& ! grep -q $'\x1b' "$T/out" \
	&& ! grep -qx 'INJECTED' "$T/out" \
	&& grep -qF 'refs/heads/bad[31mINJECTED' "$T/out" \
	&& [ "$(wc -l <"$ctrl_meta")" = 1 ] \
	&& [ "$(cat "$ctrl_meta")" = "refs/heads/bad[31mINJECTED 1.98.1" ]; then
	pass "R10 P1.13 control characters and newlines in GITHUB_REF sanitized in log and keyfile"
else
	fail "R10 GITHUB_REF sanitization" "rc=$rc out=$(cat "$T/out")"
fi
rm -rf -- "$T/cache/ouromail-v1/$ctrl_key" "$ctrl_meta"

echo "test-runner: $cases cases, $fails failure(s)"
[ "$fails" = 0 ]
