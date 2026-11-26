#!/usr/bin/env bash
# SPDX-FileCopyrightText: MailCraft contributors
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Assertion tests for:
#   1. .github/workflows/ouromail-package.yml package `result` step (rejects symlinks, unexpected
#      and non-regular entries, and non-monotonic names; accepts valid monotonic package + sums).
#   2. .github/workflows/ouromail-package.yml `publish` job `if:` condition (parsed & evaluated
#      against main/non-main branches, tags, and failed needs) + publish step guard + static
#      workflow security invariants.
#   3. packaging/arch-ouromail/PKGBUILD monotonic pkgver (`r<count>.g<sha8>`), real Arch `vercmp`
#      ordering against delivered r85dd1df9/r6b3fafce and synthetic commits, and shallow/dirty/bad
#      history refusals.
# Pure bash/awk/git/vercmp (no third-party packages); safe inside the Arch CI container.
# Usage: bash packaging/arch-ouromail/test-workflow-and-pkgver.sh
set -euo pipefail
export LC_ALL=C

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
wf="$repo/.github/workflows/ouromail-package.yml"
pkgbuild="$here/PKGBUILD"
T=$(mktemp -d "${TMPDIR:-/tmp}/ouromail-wf-test.XXXXXXXX")
trap 'rm -rf -- "$T"' EXIT

fails=0
cases=0
pass() { cases=$((cases + 1)); echo "ok   $1"; }
fail() { cases=$((cases + 1)); echo "FAIL $1${2:+: $2}"; fails=$((fails + 1)); }

# Helper: extract the `run: |` bash body of a step identified by regex MARKER from workflow YAML.
extract_step() {
	local marker=$1 out=$2
	awk -v m="$marker" '
		$0 ~ m { seen = 1; next }
		seen && /^[[:space:]]*run:[[:space:]]*\|[[:space:]]*$/ { in_run = 1; next }
		in_run {
			if (indent == 0 && $0 ~ /[^[:space:]]/) {
				match($0, /^[[:space:]]*/)
				indent = RLENGTH
			}
			if ($0 ~ /^[[:space:]]*$/) { print ""; next }
			match($0, /^[[:space:]]*/)
			if (RLENGTH < indent) exit
			print substr($0, indent + 1)
		}
	' "$wf" >"$out"
	[ -s "$out" ]
}

###############################################################################
# 1. Package `result` step tests (Finding 2)
###############################################################################
extract_step 'id: result' "$T/result-step.sh" || { fail "W0 extract result step"; exit 1; }
good_pkg="ouromail-0.14.0.r3610.g01234567-1-x86_64.pkg.tar.zst"
echo "CANARY-SECRET" >"$T/canary-secret"

run_result() {
	local rt=$1 outlog=$2 ghout=$3
	: >"$ghout"
	set +e
	env -i PATH=/usr/bin RUNNER_TEMP="$rt" GITHUB_OUTPUT="$ghout" bash -e "$T/result-step.sh" >"$outlog" 2>&1
	local rc=$?
	set -e
	return "$rc"
}

# W1: valid regular monotonic package + SHA256SUMS passes.
mkdir -p "$T/r1/ouromail-out"
printf 'regular-pkg-bytes\n' >"$T/r1/ouromail-out/$good_pkg"
(cd "$T/r1/ouromail-out" && sha256sum "$good_pkg" >SHA256SUMS)
if run_result "$T/r1" "$T/r1.log" "$T/r1.gh" && grep -qx "file=$good_pkg" "$T/r1.gh" && grep -qE '^sums=[0-9a-f]{64}$' "$T/r1.gh"; then
	pass "W1 result step accepts valid regular monotonic package and SHA256SUMS"
else
	fail "W1 valid result" "$(cat "$T/r1.log")"
fi

# W2 (V3): symlinked package file rejected.
mkdir -p "$T/r2/ouromail-out"
ln -s "$T/canary-secret" "$T/r2/ouromail-out/$good_pkg"
(cd "$T/r2/ouromail-out" && sha256sum "$good_pkg" >SHA256SUMS)
if run_result "$T/r2" "$T/r2.log" "$T/r2.gh"; then
	fail "W2 V3 symlink package accepted"
elif [ ! -s "$T/r2.gh" ]; then
	pass "W2 V3 result step rejects symlinked package"
else
	fail "W2 V3 emitted output"
fi

# W3: symlinked SHA256SUMS rejected.
mkdir -p "$T/r3/ouromail-out"
printf 'pkg\n' >"$T/r3/ouromail-out/$good_pkg"
(cd "$T/r3/ouromail-out" && sha256sum "$good_pkg" >"$T/r3/real-sums")
ln -s "$T/r3/real-sums" "$T/r3/ouromail-out/SHA256SUMS"
if run_result "$T/r3" "$T/r3.log" "$T/r3.gh"; then fail "W3 symlink SHA256SUMS accepted"; else pass "W3 result step rejects symlinked SHA256SUMS"; fi

# W4: symlinked ouromail-out dir and symlinked RUNNER_TEMP rejected.
mkdir -p "$T/r4/real-out"
printf 'pkg\n' >"$T/r4/real-out/$good_pkg"
(cd "$T/r4/real-out" && sha256sum "$good_pkg" >SHA256SUMS)
ln -s "$T/r4/real-out" "$T/r4/ouromail-out"
if run_result "$T/r4" "$T/r4.log" "$T/r4.gh"; then fail "W4a symlink ouromail-out accepted"; else pass "W4a result step rejects symlinked ouromail-out"; fi
ln -s "$T/r1" "$T/r4-rt-link"
if run_result "$T/r4-rt-link" "$T/r4b.log" "$T/r4b.gh"; then fail "W4b symlink RUNNER_TEMP accepted"; else pass "W4b result step rejects symlinked RUNNER_TEMP"; fi

# W5: unexpected extra file, nested dir, FIFO, and non-monotonic hash-only name rejected.
mkdir -p "$T/r5a/ouromail-out"
printf 'pkg\n' >"$T/r5a/ouromail-out/$good_pkg"
(cd "$T/r5a/ouromail-out" && sha256sum "$good_pkg" >SHA256SUMS)
printf 'extra\n' >"$T/r5a/ouromail-out/extra.txt"
if run_result "$T/r5a" "$T/r5a.log" "$T/r5a.gh"; then fail "W5a extra file accepted"; else pass "W5a result step rejects unexpected extra file (sorting before pkg)"; fi

mkdir -p "$T/r5e/ouromail-out"
printf 'pkg\n' >"$T/r5e/ouromail-out/$good_pkg"
(cd "$T/r5e/ouromail-out" && sha256sum "$good_pkg" >SHA256SUMS)
printf 'extra\n' >"$T/r5e/ouromail-out/zzz-extra.txt"
if run_result "$T/r5e" "$T/r5e.log" "$T/r5e.gh"; then fail "W5e trailing extra file accepted"; else pass "W5e result step rejects unexpected extra file (sorting after pkg)"; fi

mkdir -p "$T/r5b/ouromail-out/sub"
printf 'pkg\n' >"$T/r5b/ouromail-out/$good_pkg"
(cd "$T/r5b/ouromail-out" && sha256sum "$good_pkg" >SHA256SUMS)
if run_result "$T/r5b" "$T/r5b.log" "$T/r5b.gh"; then fail "W5b subdir accepted"; else pass "W5b result step rejects unexpected subdir"; fi

mkdir -p "$T/r5c/ouromail-out"
printf 'pkg\n' >"$T/r5c/ouromail-out/$good_pkg"
(cd "$T/r5c/ouromail-out" && sha256sum "$good_pkg" >SHA256SUMS)
mkfifo "$T/r5c/ouromail-out/pipe"
if run_result "$T/r5c" "$T/r5c.log" "$T/r5c.gh"; then fail "W5c FIFO accepted"; else pass "W5c result step rejects non-regular FIFO"; fi

mkdir -p "$T/r5d/ouromail-out"
old_pkg="ouromail-0.14.0.r01234567-1-x86_64.pkg.tar.zst"
printf 'pkg\n' >"$T/r5d/ouromail-out/$old_pkg"
(cd "$T/r5d/ouromail-out" && sha256sum "$old_pkg" >SHA256SUMS)
if run_result "$T/r5d" "$T/r5d.log" "$T/r5d.gh"; then fail "W5d hash-only pkgver accepted"; else pass "W5d result step rejects non-monotonic hash-only package name"; fi

mkdir -p "$T/r5f/ouromail-out"
printf 'pkg\n' >"$T/r5f/ouromail-out/$good_pkg"
(cd "$T/r5f/ouromail-out" && sha256sum "$good_pkg" >SHA256SUMS)
printf 'trailing_no_nl ' >>"$T/r5f/ouromail-out/SHA256SUMS"
if run_result "$T/r5f" "$T/r5f.log" "$T/r5f.gh"; then fail "W5f P2.12 trailing non-newline line accepted"; else pass "W5f P2.12 result step rejects trailing non-newline garbage in SHA256SUMS"; fi

mkdir -p "$T/r5g/ouromail-out"
printf 'pkg\n' >"$T/r5g/ouromail-out/$good_pkg"
(cd "$T/r5g/ouromail-out" && sha256sum "$good_pkg" | tr -d '\n' >SHA256SUMS)
if run_result "$T/r5g" "$T/r5g.log" "$T/r5g.gh"; then fail "W5g non-newline-terminated SHA256SUMS accepted"; else pass "W5g result step rejects non-newline-terminated SHA256SUMS"; fi

###############################################################################
# 2. `publish` job `if:` expression evaluator + publish guard step (Finding 4)
###############################################################################
pub_if=$(awk '
	/^  publish:$/ { in_pub = 1; next }
	in_pub && /^  [a-z]/ { exit }
	in_pub && /^    if:/ { sub(/^    if:[[:space:]]*/, ""); print; exit }
' "$wf")

# Pure awk recursive-descent evaluator for GitHub Actions boolean expressions.
eval_publish_if() {
	local expr=$1 success_val=$2 ref_val=$3
	awk -v raw="$expr" -v ctx_success="$success_val" -v ctx_ref="$ref_val" '
	function tokenize(s,   i, ch, rem, tok) {
		n = 0
		# Strip optional ${{ ... }}
		gsub(/^[[:space:]]*\$\{\{[[:space:]]*/, "", s)
		gsub(/[[:space:]]*\}\}[[:space:]]*$/, "", s)
		# Implicit success() && (...) when no status check function is present
		if (s !~ /(success|failure|always|cancelled)\(\)/) {
			s = "success() && (" s ")"
		}
		i = 1
		while (i <= length(s)) {
			rem = substr(s, i)
			if (match(rem, /^[[:space:]]+/)) { i += RLENGTH; continue }
			if (match(rem, /^(&&|\|\||==|!=)/)) { toks[++n] = substr(rem, 1, RLENGTH); i += RLENGTH; continue }
			if (match(rem, /^[()!]/)) { toks[++n] = substr(rem, 1, 1); i += 1; continue }
			if (match(rem, /^\x27[^\x27]*\x27/)) { toks[++n] = substr(rem, 1, RLENGTH); i += RLENGTH; continue }
			if (match(rem, /^[A-Za-z_][A-Za-z0-9_.]*(\(\))?/)) { toks[++n] = substr(rem, 1, RLENGTH); i += RLENGTH; continue }
			print "UNSUPPORTED_TOKEN:" substr(rem, 1, 10) > "/dev/stderr"
			exit 3
		}
	}
	function peek() { return (pos <= n) ? toks[pos] : "" }
	function take(   t) { t = toks[pos]; pos++; return t }
	function parse_or(   v, r) {
		v = parse_and()
		while (peek() == "||") { take(); r = parse_and(); v = (v || r) ? 1 : 0 }
		return v
	}
	function parse_and(   v, r) {
		v = parse_eq()
		while (peek() == "&&") { take(); r = parse_eq(); v = (v && r) ? 1 : 0 }
		return v
	}
	function parse_eq(   v, op, r) {
		v = parse_unary()
		while (peek() == "==" || peek() == "!=") {
			op = take(); r = parse_unary()
			v = (op == "==") ? (v == r ? 1 : 0) : (v != r ? 1 : 0)
		}
		return v
	}
	function parse_unary(   t, v) {
		if (peek() == "!") { take(); return parse_unary() ? 0 : 1 }
		if (peek() == "(") {
			take(); v = parse_or()
			if (take() != ")") exit 3
			return v
		}
		t = take()
		if (t ~ /^\x27.*\x27$/) return substr(t, 2, length(t) - 2)
		if (t == "success()") return (ctx_success == 1) ? 1 : 0
		if (t == "always()") return 1
		if (t == "failure()") return (ctx_success == 1) ? 0 : 1
		if (t == "cancelled()") return 0
		if (t == "github.ref") return ctx_ref
		print "UNKNOWN_IDENT:" t > "/dev/stderr"
		exit 3
	}
	BEGIN {
		if (raw == "") exit 2
		tokenize(raw)
		pos = 1
		res = parse_or()
		if (pos != n + 1) exit 3
		exit (res ? 0 : 1)
	}'
}

if [ -z "$pub_if" ]; then
	fail "W6 publish job missing if: condition"
else
	if eval_publish_if "$pub_if" 1 "refs/heads/ouromail/main"; then
		pass "W6a publish if: allows refs/heads/ouromail/main when package succeeded"
	else
		fail "W6a publish if: rejected refs/heads/ouromail/main" "$pub_if"
	fi
	for bad_case in \
		"0 refs/heads/ouromail/main" \
		"1 refs/heads/main" \
		"1 refs/heads/ouromail/ci" \
		"1 refs/heads/ouromail/main-wip" \
		"1 refs/tags/ouromail/main" \
		"1 refs/tags/ouromail-latest"; do
		s=${bad_case%% *}
		r=${bad_case#* }
		if eval_publish_if "$pub_if" "$s" "$r"; then
			fail "W6b publish if: wrongly allowed success=$s ref=$r" "$pub_if"
		else
			pass "W6b publish if: refused success=$s ref=$r"
		fi
	done
fi

# W7: Execute the extracted publish digest+ref check step directly.
extract_step 'name: Check the files against the package job' "$T/publish-check.sh" || { fail "W7 extract publish check"; exit 1; }
mkdir -p "$T/pub1/ouromail"
printf 'pkg-bytes\n' >"$T/pub1/ouromail/$good_pkg"
(cd "$T/pub1/ouromail" && sha256sum "$good_pkg" >SHA256SUMS)
pub_sums=$(sha256sum "$T/pub1/ouromail/SHA256SUMS" | cut -d' ' -f1)
if env -i PATH=/usr/bin RUNNER_TEMP="$T/pub1" FILE="$good_pkg" SUMS="$pub_sums" REF="refs/heads/ouromail/main" \
	bash -e "$T/publish-check.sh" >"$T/pub1.log" 2>&1; then
	pass "W7a publish check step accepts refs/heads/ouromail/main with valid digest"
else
	fail "W7a publish check" "$(cat "$T/pub1.log")"
fi
if env -i PATH=/usr/bin RUNNER_TEMP="$T/pub1" FILE="$good_pkg" SUMS="$pub_sums" REF="refs/heads/ouromail/ci" \
	bash -e "$T/publish-check.sh" >"$T/pub2.log" 2>&1; then
	fail "W7b publish check accepted non-main ref"
else
	pass "W7b publish check step refuses non-main ref"
fi
mkdir -p "$T/pub3/ouromail"
printf 'pkg-bytes\n' >"$T/pub3/ouromail/$good_pkg"
(cd "$T/pub3/ouromail" && sha256sum "$good_pkg" >SHA256SUMS)
printf 'trailing_no_nl ' >>"$T/pub3/ouromail/SHA256SUMS"
pub3_sums=$(sha256sum "$T/pub3/ouromail/SHA256SUMS" | cut -d' ' -f1)
if env -i PATH=/usr/bin RUNNER_TEMP="$T/pub3" FILE="$good_pkg" SUMS="$pub3_sums" REF="refs/heads/ouromail/main" \
	bash -e "$T/publish-check.sh" >"$T/pub3.log" 2>&1; then
	fail "W7c P2.12 publish check accepted trailing non-newline SHA256SUMS"
else
	pass "W7c P2.12 publish check step refuses trailing non-newline garbage in SHA256SUMS"
fi

# W8: Static workflow security invariants.
if awk '
	/^on:$/ { in_on = 1; next }
	in_on && /^[^[:space:]]/ { in_on = 0 }
	in_on && /pull_request|workflow_run|issues|issue_comment|schedule/ { err = err "bad_trigger " }
	/^permissions: \{\}$/ { has_perm0 = 1 }
	/^  publish:$/ { in_pub = 1; next }
	in_pub && /^  [a-z]/ { in_pub = 0 }
	in_pub && /^    runs-on: ubuntu-24\.04$/ { pub_ubuntu = 1 }
	in_pub && /actions\/checkout|packaging\// { err = err "pub_repo_code " }
	/uses: / {
		u = $0; sub(/.*uses:[[:space:]]*/, "", u); sub(/[[:space:]].*/, "", u)
		if (u !~ /^[^@]+@[0-9a-f]{40}$/) err = err "unpinned:" u " "
	}
	END {
		if (!has_perm0) err = err "no_top_perm_empty "
		if (!pub_ubuntu) err = err "pub_not_ubuntu "
		if (err != "") { print err; exit 1 }
	}
' "$wf" \
	&& grep -q 'fetch-depth: 0' "$wf" \
	&& grep -q 'include-hidden-files: false' "$wf" \
	&& grep -q -- '--signer-workflow [$]GH_REPO/\.github/workflows/ouromail-package\.yml --source-ref refs/heads/ouromail/main' "$wf"; then
	pass "W8 workflow static security invariants (triggers, permissions, ubuntu-24.04, SHA pins, fetch-depth, attestation pins)"
else
	fail "W8 workflow static security invariants"
fi

###############################################################################
# 3. Monotonic pkgver + real Arch vercmp + PKGBUILD prepare() validation (Finding 5)
###############################################################################
if command -v vercmp >/dev/null 2>&1; then
	ver_ok=1
	for pair in \
		"0.14.0.r85dd1df9-1 0.14.0.r3610.g6b3fafce-1 -1" \
		"0.14.0.r6b3fafce-1 0.14.0.r3610.g6b3fafce-1 -1" \
		"0.14.0.r4abb19bb-1 0.14.0.r3610.g6b3fafce-1 -1" \
		"0.14.0.r3610.g6b3fafce-1 0.14.0.r3611.g00000000-1 -1" \
		"0.14.0.r3610.gffffffff-1 0.14.0.r3611.g00000000-1 -1" \
		"0.14.0.r9999.gffffffff-1 0.14.0.r10000.g00000000-1 -1" \
		"0.14.0.r3610.g6b3fafce-1 0.14.1.r3611.g00000000-1 -1"; do
		read -r a b want <<<"$pair"
		got=$(vercmp "$a" "$b")
		if [ "$got" != "$want" ]; then
			fail "V1 vercmp $a vs $b" "got=$got want=$want"
			ver_ok=0
		fi
	done
	if [ "$ver_ok" = 1 ]; then pass "V1 real vercmp proves r<count>.g<sha8> > r85dd1df9/r6b3fafce/r4abb19bb and strictly monotonic"; fi
else
	fail "V1 vercmp missing"
fi

# Exercise PKGBUILD sourcing + prepare() validation on synthetic repos (stubbing cargo/error/check_option).
run_pkgbuild_prepare() {
	local dir=$1
	bash -s "$dir" "$T" <<'EOF'
set -e
startdir="$1/packaging/arch-ouromail"
srcdir="$2/srcdir"
error() { printf 'ERROR: '; printf "$@"; echo; } >&2
check_option() { return 1; }
cargo() { echo "cargo-stub $*"; return 0; }
source "$startdir/PKGBUILD"
prepare
printf 'PKGVER=%s\n' "$pkgver"
EOF
}

# Build synthetic full-history repo with 2 commits, plus shallow clone and non-git dir.
mk_syn_repo() {
	local d=$1
	mkdir -p "$d/packaging/arch-ouromail"
	printf '[workspace.package]\nversion = "0.14.0"\n' >"$d/Cargo.toml"
	cp "$pkgbuild" "$d/packaging/arch-ouromail/PKGBUILD"
	git -C "$d" init -q
	git -C "$d" config user.name test
	git -C "$d" config user.email test@example.org
	git -C "$d" add Cargo.toml packaging/arch-ouromail/PKGBUILD
	git -C "$d" commit -q -m "c1"
	echo "# c2" >>"$d/Cargo.toml"
	git -C "$d" commit -q -am "c2"
}

mk_syn_repo "$T/syn-full"
sha2=$(git -C "$T/syn-full" rev-parse --short=8 HEAD)
if out=$(run_pkgbuild_prepare "$T/syn-full" 2>&1) && grep -qx "PKGVER=0.14.0.r2.g$sha2" <<<"$out"; then
	pass "V2a PKGBUILD clean full-history repo -> 0.14.0.r2.g$sha2"
else
	fail "V2a PKGBUILD clean full repo" "$out"
fi

# Add 3rd commit and prove vercmp(c2, c3) == -1.
echo "# c3" >>"$T/syn-full/Cargo.toml"
git -C "$T/syn-full" commit -q -am "c3"
sha3=$(git -C "$T/syn-full" rev-parse --short=8 HEAD)
if out3=$(run_pkgbuild_prepare "$T/syn-full" 2>&1) \
	&& grep -qx "PKGVER=0.14.0.r3.g$sha3" <<<"$out3" \
	&& [ "$(vercmp "0.14.0.r2.g$sha2-1" "0.14.0.r3.g$sha3-1")" = -1 ]; then
	pass "V2b successive synthetic commit c3 (0.14.0.r3.g$sha3) strictly newer than c2"
else
	fail "V2b successive synthetic commit" "$out3"
fi

# Shallow clone must be refused by PKGBUILD prepare().
git clone -q --depth 1 "file://$T/syn-full" "$T/syn-shallow"
if out=$(run_pkgbuild_prepare "$T/syn-shallow" 2>&1); then
	fail "V2c PKGBUILD accepted shallow clone" "$out"
elif grep -q 'shallow clone' <<<"$out"; then
	pass "V2c PKGBUILD refuses shallow clone"
else
	fail "V2c PKGBUILD shallow error message" "$out"
fi

# Non-git directory must be refused by PKGBUILD prepare().
mkdir -p "$T/syn-nogit/packaging/arch-ouromail"
printf '[workspace.package]\nversion = "0.14.0"\n' >"$T/syn-nogit/Cargo.toml"
cp "$pkgbuild" "$T/syn-nogit/packaging/arch-ouromail/PKGBUILD"
if out=$(run_pkgbuild_prepare "$T/syn-nogit" 2>&1); then
	fail "V2d PKGBUILD accepted non-git directory" "$out"
else
	pass "V2d PKGBUILD refuses non-git directory"
fi

# Dirty tracked tree refused without OUROMAIL_ALLOW_DIRTY=1.
echo "# dirty" >>"$T/syn-full/Cargo.toml"
if out=$(run_pkgbuild_prepare "$T/syn-full" 2>&1); then
	fail "V2e PKGBUILD accepted dirty tree" "$out"
else
	pass "V2e PKGBUILD refuses dirty tracked tree"
fi

# PKGBUILD cheap wins: options=(!debug) and runtime deps.
if grep -qE '^options=\(.*!debug.*\)' "$pkgbuild" \
	&& grep -q 'libxkbcommon' "$pkgbuild" \
	&& grep -q 'libxkbcommon-x11' "$pkgbuild" \
	&& grep -q 'libglvnd' "$pkgbuild"; then
	pass "V3 PKGBUILD declares options=(!debug) and runtime libxkbcommon/libxkbcommon-x11/libglvnd"
else
	fail "V3 PKGBUILD options/depends"
fi

echo "test-workflow-and-pkgver: $cases cases, $fails failure(s)"
[ "$fails" = 0 ]
