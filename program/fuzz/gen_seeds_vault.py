#!/usr/bin/env python3
"""Hand-made seeds for the vault target (fuzz_targets/vault.rs): 8-byte records, record 0 is the configuration.
Each seed walks one story end to end so the fuzzer starts from inputs where every instruction succeeds."""
import os

def rec(op, *args):
    b = [op] + list(args)
    return bytes(b + [0] * (8 - len(b)))

DEPOSIT, WITHDRAW, VALUE, MOVE, FUND, QUOTE, TAKE, SETTLE, PRICE, TIME, CRANK, ALL = 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 12
DEPOSIT_ALL, WITHDRAW_ALL, KILL = 13, 14, 15
LIVE = 0x7F          # every sub-account handed over
CRANK_FIRST = 0x80   # deposit: a crank first

def story(cfg, *ops):
    return rec(cfg) + b"".join(ops)

def maker(i):
    """Give sub-account i cash and margin, and have it quote both sides."""
    return [rec(MOVE, 0, i, 100, 1), rec(FUND, i, 80, 0, 1), rec(QUOTE, i, 10, 10, 40, 1)] if i else \
           [rec(FUND, 0, 80, 0, 1), rec(QUOTE, 0, 10, 10, 40, 1)]

SEEDS = {
    # deposits, one maker, a round trip by a taker, settle, value, withdraw
    "one-maker": story(0, rec(DEPOSIT_ALL, 0, 200), rec(DEPOSIT, 1, 100, LIVE), *maker(0),
                       rec(TAKE, 0, 0, 20), rec(SETTLE, 0), rec(TAKE, 1, 1, 20), rec(SETTLE, 0),
                       rec(ALL), rec(WITHDRAW_ALL, 0, 127), rec(WITHDRAW, 1, 255, LIVE)),
    # three makers; the vault is valued a part at a time and a deposit uses remembered parts
    "three-makers-parts": story(0, rec(DEPOSIT_ALL, 0, 250), *maker(0), *maker(1), *maker(2),
                                rec(TAKE, 0, 0, 60), rec(SETTLE, 0), rec(SETTLE, 1), rec(SETTLE, 2),
                                rec(VALUE, 1), rec(VALUE, 2), rec(TIME, 5, 1), rec(DEPOSIT, 1, 50, 1),
                                rec(PRICE, 40), rec(TIME, 3, 1), rec(DEPOSIT, 2, 50, 1 | CRANK_FIRST),
                                rec(ALL), rec(WITHDRAW, 1, 255, 1)),
    # a part goes stale: the deposit that leaves it out must fail, the one that brings the account must not
    "stale-part": story(0, rec(DEPOSIT_ALL, 0, 250), *maker(1), rec(TAKE, 0, 1, 30), rec(SETTLE, 1), rec(VALUE, 1),
                        rec(TIME, 200, 0), rec(PRICE, 0), rec(DEPOSIT, 1, 10, 1), rec(DEPOSIT, 1, 10, LIVE)),
    # performance fee, lockup, daily cap and drawdown stop all on; the price runs away from the makers
    "all-rules": story(15, rec(DEPOSIT_ALL, 0, 250), rec(DEPOSIT_ALL, 1, 250), *maker(0), *maker(1),
                       rec(TAKE, 0, 0, 200), rec(PRICE, 127), rec(PRICE, 127), rec(SETTLE, 0), rec(SETTLE, 1),
                       rec(ALL), rec(TIME, 11, 1), rec(PRICE, 127), rec(WITHDRAW_ALL, 0, 40), rec(CRANK, LIVE),
                       rec(TAKE, 1, 1, 200), rec(PRICE, 129), rec(SETTLE, 0), rec(SETTLE, 1), rec(ALL),
                       rec(WITHDRAW_ALL, 1, 255)),
    # cash moved out of a maker again: the account empties and drops out of the value, then comes back
    "empty-and-back": story(0, rec(DEPOSIT_ALL, 0, 250), rec(MOVE, 0, 2, 50, 0), rec(VALUE, 2), rec(MOVE, 2, 0, 50, 0),
                            rec(VALUE, 2), rec(TIME, 100, 0), rec(PRICE, 0), rec(DEPOSIT, 1, 10, 1),
                            rec(MOVE, 0, 2, 50, 0), rec(DEPOSIT, 1, 10, 1), rec(KILL, 0, 1), rec(KILL, 0, 0)),
}
os.makedirs("seeds_vault", exist_ok=True)
for name, data in SEEDS.items():
    open(f"seeds_vault/{name}", "wb").write(data)
print(len(SEEDS), "seeds")
