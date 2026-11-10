#!/usr/bin/env python3
"""Hand-written seeds for the `lanes` target (8-byte records; record 0 = configuration). Run: python3 gen_seeds_lanes.py
Each scenario is written for several configurations (single / split books, long / short staleness, signer on / off)."""
import os, sys
OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "seeds_lanes")
os.makedirs(OUT, exist_ok=True)
def rec(op, ui=0, c2=0, c3=0, c4=0, c5=0, c6=0, c7=0): return bytes([op, ui, c2 & 255, c3 & 255, c4 & 255, c5 & 255, c6 & 255, c7 & 255])
def cfg(bits=0): return bytes([bits, 0, 0, 0, 0, 0, 0, 0])
PLAIN, SPLIT, SHORT, NOSIGN, FUND, NEARLIQ, NOQUOTES = 1, 2, 4, 8, 16, 32, 64
def price(p): v = p - 98000; return v & 255, v >> 8
def place(ui, side, p, qty, flags=1, c7=0): c2, c3 = price(p); return rec(0, ui, c2, c3, qty - 1, flags, side, c7)
def oracle(step=0, ds=1, mode=0, conf=0): return rec(4, 0, ds - 1, mode, conf, 0, 0, 128 + step)   # step in 0.01% units of 1000.00, +-127
def advance(ds): return rec(5, 0, ds - 1)
def deposit(ui, k): return rec(6, ui, k - 1)
def withdraw(ui, mode=0, c3=0, qty=1, acc=0): return rec(7, ui, mode, c3, qty - 1, acc)
def iso(ui, on): return rec(8, ui, c7=on)
def isot(ui, mode=0, out=False, qty=1, c3=0): return rec(9, ui, mode, c3, qty - 1, 0, 0, 2 if out else 0)
def quote(si, ask, off=0, n=1, step=0, qty=1, clear=True, peg=False, tol=False, inv=False, fresh=False, through=False):
    return rec(10, si, (off + (32 if through else 4)) & 63, qty - 1, n - 1, (step // 5) | (0x80 if clear else 0), 1 if ask else 0,
               (1 if peg else 0) | (2 if tol else 0) | (4 if inv else 0) | (8 if fresh else 0) | (0x40 if through else 0))
def take(ui, buy, qty, lim=None, reduce=False, shards=False, signed=False, sel=0, spx=7):
    return rec(11, ui, 0 if lim is None else lim, sel, qty - 1, (0x10 if reduce else 0) | (0x20 if shards else 0) | (0x40 if signed else 0), 0 if buy else 1, (0x40 if lim is None else 0) | spx)
def tbatch(ui, buy, qty, n3=False, c2=40, c7=0): return rec(12, ui, c2, 0, qty - 1, 1 if n3 else 0, 0 if buy else 1, c7)
def settle(ui, book=0): return rec(13, ui, book)
def fund(ui, mode=0, mag=0, book=0, tgt=None): return rec(14, ui, mode | book << 4, mag, 0 if tgt is None else 1, 0 if tgt is None else tgt)
def kill(si, on): return rec(15, si, c7=on)
def push(book=0, spx=7): return rec(16, 0, book, 0, 0, 0, 0, spx)
def liqfast(ui, keeper=False, maxq=True, qty=1, sel=0): return rec(17, ui, 0, sel, qty - 1, (1 if keeper else 0) | (2 if maxq else 0))
def join(fundit=True): return rec(18, 0, c7=1 if fundit else 0)
def liquidate(ui, qty=40): return rec(19, ui, c4=qty - 1)
def liqbook(ui, qty=40, keeper=False): return rec(20, ui, c4=qty - 1, c5=1 if keeper else 0)
def adlrank(ui): return rec(21, ui)
def adl(ui): return rec(22, ui)
def liqcancel(ui): return rec(23, ui)
def misc(k, ui=0, c3=0): return rec(24, ui, k, c3)
def settrig(ui, slot, kind, side, p, qty): c2, c3 = price(p); return rec(25, ui, kind - 1 + 3 * 0, c3, qty - 1, 0, side, slot)
def exectrig(ui, slot): return rec(26, ui, c7=slot)
def single(who, tag=0, op=1, val=1): return rec(27, who, tag, val, 0, 0, 0, op)
def quorum(k, ui=0, c3=0, c7=0): return rec(28, ui, k, c3, 0, 0, 0, c7)
def warp(k=0, c3=0, c4=0): return rec(29, 0, k, c3, c4)
seeds = {}
def add(name, bits, body):
    for b in (bits if isinstance(bits, (list, tuple)) else [bits]): seeds[f"{name}-{b:02x}"] = cfg(b) + b"".join(body)
ALL = [0, SPLIT, SHORT, NOSIGN, PLAIN, SPLIT | SHORT, PLAIN | SPLIT | NOSIGN]
add("takes-settle-withdraw", ALL, [take(1, True, 20), take(2, False, 15), take(6, True, 40, lim=30), oracle(5), settle(4), settle(5), settle(4, 1), settle(5, 1),
    withdraw(1, 2), withdraw(2, 3), withdraw(4, 1, 100), withdraw(5, 2), oracle(-9, mode=3), take(1, False, 20, reduce=True), withdraw(1, 2), misc(2), withdraw(1, 2)])
add("requote-ladders", ALL, [quote(0, True, 2, 4, 5, 20), quote(0, False, 2, 4, 5, 20), quote(1, True, 0, 3, 0, 10, peg=True), quote(1, False, 0, 3, 0, 10, peg=True),
    take(1, True, 35), take(2, False, 35), oracle(3, mode=7), take(3, True, 12, signed=True), quote(0, True, 1, 2, 5, 9, tol=True), quote(0, False, 1, 2, 5, 9, inv=True),
    take(1, True, 30, signed=True), take(2, False, 30), quote(1, True, 1, 2, 5, 9, fresh=True, peg=True), push(0), take(3, True, 5, signed=True), take(3, True, 5), settle(4), settle(5)])
add("through-the-price", ALL, [quote(0, True, -20, 3, 5, 20, through=True), quote(0, False, -20, 3, 5, 20, through=True), take(1, True, 20), take(2, False, 20),
    oracle(40), take(1, False, 30), oracle(-80), take(2, True, 30), quote(1, True, -12, 2, 5, 9, through=True, tol=True), take(1, True, 9)])
# finding: slab_fund(0) on a settled entry refills the whole budget (maker 4 left with ~0.56M cash, short 100 lots: below IM, above MM)
add("fund-refill", [0, NOSIGN, SHORT], [kill(1, 1), withdraw(4, 1, 251), take(6, True, 40), take(6, True, 40), take(6, True, 20), settle(4), fund(4, 0), take(6, True, 40), settle(4)])
# the same finding carried through: after the refill maker 4 sells 399 more lots and ends 779 lots short on ~5.98M of collateral
# (initial requirement 7.80M) although every instruction was accepted and the price never moved
def round_(n_last): return [quote(0, True, 2, 4, 5, 60)] + [take(6, True, 40)] * 6 + [quote(0, True, 2, 4, 5, 60)] + [take(6, True, 40)] * 3 + [take(6, True, n_last)]
add("fund-refill-overleverage", [0], [kill(1, 1), withdraw(4, 1, 241)] + round_(20) + [settle(4), fund(4, 0)] + round_(39) + [settle(4)])
add("fund-modes", ALL, [fund(4, 1, 50), fund(4, 2, 20), take(1, True, 10), fund(4, 0), fund(4, 1, 5), fund(4, 2, 5), settle(4), fund(4, 3), fund(5, 1, 10, tgt=40), take(2, False, 30), settle(5), settle(5, 1), fund(5, 2, 200), fund(5, 0, tgt=0), withdraw(5, 2), withdraw(4, 2)])
# finding: liquidate_fast fills maker quotes although the oracle is 96 slots old (take_fast does not)
add("liq-aged", [0, SPLIT, NOSIGN], [take(0, True, 18)] + [oracle(-127)] * 6 + [advance(48), advance(48), quote(0, False, 0, 3, 5, 40), take(6, False, 5), liqfast(0)])
add("liq-fast-fresh", ALL, [take(0, True, 18)] + [oracle(-127)] * 6 + [quote(0, False, 0, 3, 5, 40), quote(1, False, 2, 3, 5, 40), liqfast(0, keeper=True), liqfast(0), liqfast(0, qty=3, maxq=False), settle(4), settle(5), withdraw(0, 2), liquidate(0), adlrank(6), adl(0)])
add("near-liq", [NEARLIQ, NEARLIQ | SPLIT, NEARLIQ | SHORT], [liqcancel(0), liqfast(0), liqbook(0), liquidate(0), oracle(-127, mode=1), oracle(-127, mode=3), liqfast(0, keeper=True), adlrank(1), adlrank(2), adl(0), misc(0, 0, 0), misc(0, 0, 1), withdraw(2, 2), withdraw(1, 2)])
# finding (fund mode): a resting ask placed long before it fills collects funding for the time it was not short
add("funding-resting", [FUND, FUND | SPLIT, FUND | PLAIN], [place(2, 1, 100050, 20), warp(0, 2), oracle(0), warp(0, 2), oracle(0), warp(0, 2), oracle(0), place(1, 0, 100050, 20, flags=4)])
add("funding-fast", [FUND, FUND | SPLIT], [warp(0, 2), oracle(0), quote(0, True, 2, 2, 5, 40), take(1, True, 30), warp(0, 2), oracle(0), warp(0, 2), oracle(0), settle(4), settle(5)])
add("timelock", [FUND, FUND | PLAIN], [warp(1, 0, 40), warp(2), warp(3), oracle(0), take(1, True, 10), warp(1, 1, 100), warp(2), warp(3), oracle(0), warp(1, 2, 3), warp(2), warp(3), oracle(0), take(2, False, 10), settle(4), withdraw(1, 2)])
add("adl", [NEARLIQ, NEARLIQ | SPLIT, NEARLIQ | SPLIT | 128], [oracle(-127)] * 6 + [misc(0, 0, 2), adlrank(1), adlrank(2), adl(0), adl(0), liqbook(0), oracle(-127, mode=1), adl(0), misc(2), withdraw(1, 2), withdraw(2, 2)])
add("bids-only-book", [SPLIT | 128, SPLIT | 128 | SHORT], [take(1, False, 18), take(0, False, 18)] + [oracle(127)] * 6 + [liqbook(0), place(2, 1, 107500, 10), liqbook(0, keeper=True), liqfast(0), liquidate(0), liqbook(1), adlrank(4), adl(0)])
add("isolated", ALL, [iso(1, 1), isot(1, 0, False, 40), take(1, True, 10), oracle(60), isot(1, 2, True), isot(1, 3, True), withdraw(1, 2), oracle(-120), isot(1, 2, True), take(1, False, 10, reduce=True), isot(1, 2, True), iso(1, 0), withdraw(1, 2),
    iso(4, 1), iso(2, 1), isot(2, 0, False, 40), place(2, 0, 99950, 5), place(3, 1, 99950, 5, flags=4), isot(2, 2, True), isot(2, 0, True, 1)])
add("book-and-fast", ALL, [place(1, 1, 100020, 10), place(2, 0, 99980, 10), take(3, True, 30, shards=True), take(3, False, 30, shards=True), oracle(2, mode=3), withdraw(1, 2, acc=3), withdraw(1, 2, acc=2), withdraw(1, 2), withdraw(2, 2, acc=4), misc(0, 0, 2), misc(0, 0, 0), withdraw(2, 2)])
add("batch", ALL, [tbatch(1, True, 10), tbatch(2, False, 10, n3=True), tbatch(4, True, 5, c7=3), settle(4), settle(5), tbatch(1, False, 10, c7=0), withdraw(1, 2), withdraw(2, 2), withdraw(3, 2)])
add("pause", [0, SPLIT, PLAIN], [take(1, True, 10), single(0), take(2, True, 10), take(1, False, 10, reduce=True), deposit(1, 5), withdraw(1, 2), single(1, 0, 1, 0), single(3, 0, 1, 1), single(0, 0, 6, 3), single(2, 2, 4, 2), single(0, 3), single(1, 4), single(2, 5),
    quorum(0, c7=0), take(2, True, 10), quorum(1, c3=5, c7=1), take(2, True, 40), quorum(3, 1, c7=1), take(6, True, 10), settle(4), quorum(2, c3=0), quorum(4, c7=1), deposit(1, 200)])
add("triggers", ALL, [take(1, True, 10), settrig(1, 0, 2, 1, 99500, 10), oracle(-100), exectrig(1, 0), settrig(1, 1, 1, 1, 100200, 5), oracle(127), oracle(127), exectrig(1, 1), settrig(2, 2, 3, 0, 100000, 6), exectrig(2, 2), advance(3), oracle(0), exectrig(2, 2)])
add("late-join", ALL, [join(True), quote(2, True, 0, 2, 5, 10), quote(2, False, 0, 2, 5, 10), take(1, True, 40), take(3, True, 5), settle(3), kill(2, 1), take(1, True, 40), kill(2, 0), fund(3, 2, 30), withdraw(3, 2)])
add("stale-quotes", [0, SPLIT], [advance(31), take(1, True, 5), quote(0, True, 0, 1, 0, 5), take(1, True, 5), advance(48), advance(30), take(1, True, 5), oracle(0), take(1, True, 5), advance(48), advance(48), advance(48), advance(10), take(1, True, 5), withdraw(1, 2)])
add("jump", [0, SHORT], [take(1, True, 10), rec(4, 0, 0, 0, 0, 0xEE, 0, 0), single(0), rec(4, 0, 0, 0, 0, 0xEE, 0, 0), oracle(0), quorum(0, c7=0), take(1, False, 10)])
for k, v in seeds.items(): open(os.path.join(OUT, k), "wb").write(v)
print(len(seeds), "seeds ->", OUT)
