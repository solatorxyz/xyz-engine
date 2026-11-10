#!/bin/bash
# Run every Kani harness of the program, one at a time, each under a time limit (default 600 s).
# Fails when any harness is refuted, or when one of the REQUIRED harnesses (known to verify) fails or times out.
# usage: deploy/ops/kani_all.sh [name-filter-regex]      env: KANI_LIMIT=seconds
cd "$(dirname "$0")/../../program" || exit 2
REQUIRED="np_value_is_min_of_value_and_cost mulf_wide_path_reduced np_value_never_credits_a_gain_reduced
cost_after_same_side_accumulates_exactly cost_after_flip_rebases_with_the_right_sign cost_after_flip_no_overflow
liq_needed_at_least_one_lot liq_needed_products_fit band_bps_capped funding_rate_clamped ladder_room_tolerance_cannot_wrap
ladder_room_inventory_cap ladder_room_fresh_only fast_guard_window_cannot_wrap qpx_pegged_price_exact_or_unusable
maker_budget_never_negative reduce_ok_bounds_the_worst_fill_reduced off_band_matches_wide_formula
native_paths_cannot_wrap native_paths_match_wide_small"
fail=0
for h in $(grep -h -A2 'kani::proof' src/proofs.rs src/lib.rs | grep -oE 'fn [a-z0-9_]+' | sed 's/fn //' | grep -E "${1:-.}"); do
  t0=$(date +%s)
  out=$(timeout "${KANI_LIMIT:-600}" cargo kani --harness "$h" --output-format terse 2>&1); rc=$?
  dt=$(( $(date +%s) - t0 ))
  req=0; for r in $REQUIRED; do [ "$r" = "$h" ] && req=1; done
  if grep -q 'VERIFICATION:- SUCCESSFUL' <<<"$out"; then echo "ok       ${dt}s $h"
  elif [ $rc = 124 ]; then echo "timeout  ${dt}s $h$([ $req = 1 ] && echo '  (required)')"; [ $req = 1 ] && fail=1
  else echo "REFUTED  ${dt}s $h"; grep -E 'Failed Checks|VERIFICATION' <<<"$out" | head -5; fail=1; fi
done
exit $fail
