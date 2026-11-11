#!/bin/bash
# The same tests on the program built at other optimisation levels. The program hands out account references without the
# borrow checker (see `load`); if any behaviour depended on what the optimiser assumes about them, a different build would
# answer differently. Compute-unit assertions are left out (they are about the release build). Run from the repo root.
set -e
export PATH=$HOME/.local/share/solana/install/active_release/bin:$PATH
T=$(ls program-tests/tests/*.rs | xargs -n1 basename | sed 's/\.rs$//' | grep -v '^common$\|^cu$' | sed 's/^/--test /' | tr '\n' ' ')
for lvl in 1 z; do
  (cd program && CARGO_PROFILE_RELEASE_OPT_LEVEL=$lvl CARGO_PROFILE_RELEASE_LTO=off cargo-build-sbf >/dev/null 2>&1)
  echo "== opt-level $lvl: $(sha256sum program/target/deploy/perp_program.so | cut -c1-16)"
  (cd program-tests && cargo test --release --no-fail-fast $T 2>&1 | grep -E "test result|FAILED" | awk '/FAILED/ {print; bad=1} /test result/ {p+=$4; f+=$6} END {print "passed", p, "failed", f; exit (f>0||bad)}')
done
(cd program && cargo-build-sbf >/dev/null 2>&1) && echo "release build restored: $(sha256sum program/target/deploy/perp_program.so | cut -c1-16)"
