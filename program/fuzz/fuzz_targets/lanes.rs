#![no_main]
//! Second native target (same technique as native.rs: the program's own processor code, compiled for the host, against
//! in-memory accounts). It drives what native.rs does not: the withdrawal lane (withdraw / iso_transfer), the maker fast
//! path (slab_quote, take_fast, take_fast_batch, slab_fund, slab_settle, slab_kill, fast_join, fast_px_push with a signed
//! price, liquidate_fast, triggers through the book), a pooled market whose admin is a 2-of-3 Multisig (single-member
//! emergency pause), and, in "fund" mode, long time warps (funding accrual, timelocked admin changes).
//!
//! Input: 8-byte records. Record 0 is the configuration, every later record is one operation (op = byte 0 & 31).
//! Configuration byte 0: 1 plain admin key (else pooled market with a 2-of-3 multisig admin), 2 one FastBook per side, 4 short
//! staleness (3 slots; else 150), 8 no price signer, 16 fund mode (time warps), 32 start near a liquidation, 64 makers start
//! without quotes, 128 (with 2) only the bids book exists. `gen_seeds_lanes.py` writes the hand-made seeds and documents the ops.
//! Invariants are checked after EVERY successful instruction (see `invariants`, `fill_checks` and the per-op checks in the
//! loop); each assert message names the rule. A transaction that fails is rolled back; a "successful" instruction that wrote
//! to an account passed read-only is treated as failed (the runtime would refuse it).
//!
//! `model_place` is a reference model of the resting-order matcher (`take`), written from the docs: every successful limit
//! order placed through IX 1 is compared with it (taker lots, cash, cost basis, the orders left, the makers' ledgers, fees).
//!
//! Known findings can be switched off with LANES_KNOWN=tag,tag (or `all`) so the fuzzer can go on past them:
//!   fund-refill  slab_fund with amount <= 0 on a settled entry refills the whole budget although initial margin leaves less free
//!   liq-aged     liquidate_fast fills maker quotes while the oracle is older than FAST_GUARD_MAX_AGE_SLOTS (no signed price)
//!   funding      funding is not conserved across lazily settled maker fills (ledger claims / unsettled fast fills)
use libfuzzer_sys::fuzz_target;
use perp_program::spec;
use perp_program::state::*;
use pinocchio::account::{AccountView, RuntimeAccount};
use solana_address::Address;
use std::collections::HashMap;
use std::sync::OnceLock;
include!("world.rs");

const SYSVAR_IX: Address = Address::from_str_const("Sysvar1nstructions1111111111111111111111111");
const ED25519: Address = Address::from_str_const("Ed25519SigVerify111111111111111111111111111");
fn known(tag: &str) -> bool {
    static K_: OnceLock<String> = OnceLock::new();
    let s = K_.get_or_init(|| std::env::var("LANES_KNOWN").unwrap_or_default());
    s == "all" || s.split(',').any(|t| t == tag)
}

fn place(
    side: u8,
    flags: u8,
    kind: u8,
    price: u64,
    qty: u64,
    band: u16,
    cid: u64,
    a1: i64,
    a2: u64,
    a3: u64,
) -> Vec<u8> {
    let mut b = vec![side, flags, kind];
    for v in [price, qty, 0u64] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    b.extend_from_slice(&band.to_le_bytes());
    b.extend_from_slice(&cid.to_le_bytes());
    b.extend_from_slice(&a1.to_le_bytes());
    b.extend_from_slice(&a2.to_le_bytes());
    b.extend_from_slice(&a3.to_le_bytes());
    b
}
/// Instructions sysvar holding [Ed25519 verify of one signed fast price, the current instruction]; current index 1.
fn sysvar_data(signer: &K, market: &K, px: u64, slot: u64, ts_ms: u64) -> Vec<u8> {
    let mut ed = vec![1u8, 0];
    for v in [48u16, 0xffff, 16, 0xffff, 112, 64, 0xffff] {
        ed.extend_from_slice(&v.to_le_bytes());
    }
    ed.extend_from_slice(signer);
    ed.extend_from_slice(&[0u8; 64]);
    ed.extend_from_slice(&spec::FAST_PX_MAGIC);
    ed.extend_from_slice(market);
    for v in [px, slot, ts_ms] {
        ed.extend_from_slice(&v.to_le_bytes());
    }
    let ix = |pid: &K, data: &[u8]| {
        let mut b = vec![0u8, 0];
        b.extend_from_slice(pid);
        b.extend_from_slice(&(data.len() as u16).to_le_bytes());
        b.extend_from_slice(data);
        b
    };
    let (i0, i1) = (ix(&kk(&ED25519), &ed), ix(&[9u8; 32], &[0]));
    let mut d = vec![2u8, 0];
    d.extend_from_slice(&6u16.to_le_bytes());
    d.extend_from_slice(&((6 + i0.len()) as u16).to_le_bytes());
    d.extend_from_slice(&i0);
    d.extend_from_slice(&i1);
    d.extend_from_slice(&1u16.to_le_bytes());
    d
}

struct Usr {
    auth: K,
    u: K,
    src: K,
}
struct Slb {
    slab: K,
    ui: usize,
    joined: bool,
}
struct Cx {
    market: K,
    ins: K,
    rank: K,
    vault: K,
    pda: K,
    tok: K,
    oracle: K,
    feed: [u8; 32],
    sh: Vec<K>,
    books: Vec<K>,
    users: Vec<Usr>,
    slabs: Vec<Slb>,
    msig: K,
    mem: [K; 3],
    stranger: K,
    admin0: K,
    pooled: bool,
    signer: Option<K>,
    sysvar: K,
}
impl Cx {
    /// accounts that authorize an admin action: first the signer the instruction expects at index 0, then (appended) the rest
    fn a0(&self) -> Meta {
        if self.pooled {
            (self.mem[0], true, true)
        } else {
            (self.admin0, true, true)
        }
    }
    fn tail(&self) -> Vec<Meta> {
        if self.pooled {
            vec![(self.msig, false, false), (self.mem[1], true, false)]
        } else {
            vec![]
        }
    }
    fn book(&self, opp: usize) -> K {
        self.books[opp.min(self.books.len() - 1)]
    }
    fn joined(&self) -> Vec<&Slb> {
        self.slabs.iter().filter(|s| s.joined).collect()
    }
}

// ---------- the harness's own model of health (independent of the program's helpers) ----------
fn mark(m: &Market) -> u64 {
    if m.mark_px != 0 {
        m.mark_px
    } else {
        m.oracle_px
    }
}
fn band(m: &Market) -> u64 {
    if m.oracle_px == 0 {
        0
    } else {
        ((m.oracle_conf as u128 * 10000 / m.oracle_px as u128) as u64).min(50)
    }
}
fn up(x: i128, b: u64) -> i128 {
    (x * b as i128 + 9999) / 10000
}
fn is_stale(m: &Market, slot: u64, ts: i64) -> bool {
    m.oracle_px == 0 || slot > m.oracle_slot + m.max_staleness_slots || ts - m.oracle_ts > m.max_age_secs as i64
}
#[derive(Clone, Copy, Default, Debug)]
struct Hl {
    eq: i128,
    im: i128,
    mm: i128,
    cost: i128,
    b: i128,
}
/// (cross bucket, isolated bucket if the market position is isolated) from the booked account only, as the program judges it
fn booked(u: &User, m: &Market) -> (Hl, Option<Hl>) {
    let (px, b) = (mark(m) as i128, band(m));
    let mut c = Hl { eq: u.cash as i128, ..Default::default() };
    let mut iso = None;
    for p in u.pos.iter() {
        if p.market == [0; 32] {
            continue;
        }
        let gross = (p.base as i128 + p.bid_open as i128).abs().max((p.base as i128 - p.ask_open as i128).abs());
        let h = Hl {
            eq: p.base as i128 * px,
            im: up(gross * px, m.imr_bps + b),
            mm: up((p.base as i128).abs() * px, m.mmr_bps + b),
            cost: p.cost as i128,
            b: p.base as i128,
        };
        if p.flags & 1 != 0 {
            iso = Some(Hl { eq: h.eq + p.iso_margin as i128, ..h });
        } else {
            c.eq += h.eq;
            c.im += h.im;
            c.mm += h.mm;
            c.cost += h.cost;
            c.b += h.b;
        }
    }
    (c, iso)
}
/// What a user really holds: booked account + maker fills not yet claimed from shard ledgers + its FastBook entries
/// (locked margin, unsettled fills). Position-only requirements (resting orders ignored), so this is a lower bar than the program's.
fn truth(w: &World, cx: &Cx, ui: usize) -> (Hl, Option<Hl>) {
    let m: &Market = w.rf(&cx.market);
    let u: &User = w.rf(&cx.users[ui].u);
    let uk = cx.users[ui].u;
    let (px, b) = (mark(m) as i128, band(m));
    let (mut pb, mut pq, mut fm) = (0i128, 0i128, 0i128);
    for s in &cx.sh {
        let s: &Shard = w.rf(s);
        for l in &s.ledger[..s.nled as usize] {
            if l.owner == uk {
                pb += l.base as i128;
                pq += l.quote as i128;
            }
        }
    }
    for f in &cx.books {
        let f: &FastBook = w.rf(f);
        for e in &f.makers[..f.n as usize] {
            if e.user == uk {
                pb += e.base as i128;
                pq += e.quote as i128;
                fm += e.margin as i128;
            }
        }
    }
    let p = u.pos.iter().find(|p| p.market == cx.market);
    let (base, iso) = (p.map_or(0, |p| p.base as i128) + pb, p.is_some_and(|p| p.flags & 1 != 0));
    let pos = Hl {
        eq: base * px + pq,
        im: up(base.abs() * px, m.imr_bps + b),
        mm: up(base.abs() * px, m.mmr_bps + b),
        cost: 0,
        b: base,
    };
    if iso {
        (
            Hl { eq: u.cash as i128 + fm, ..Default::default() },
            Some(Hl { eq: pos.eq + p.unwrap().iso_margin as i128, ..pos }),
        )
    } else {
        (Hl { eq: u.cash as i128 + fm + pos.eq, ..pos }, None)
    }
}
fn pending(w: &World, cx: &Cx, ui: usize) -> bool {
    let uk = cx.users[ui].u;
    cx.sh.iter().any(|s| {
        let s: &Shard = w.rf(s);
        s.ledger[..s.nled as usize].iter().any(|l| l.owner == uk)
    }) || cx.books.iter().any(|f| {
        let f: &FastBook = w.rf(f);
        f.makers[..f.n as usize].iter().any(|e| e.user == uk && (e.base != 0 || e.quote != 0))
    })
}

/// The order rules of 2026-11-10 (docs/ORDER_RULES.md), after every successful instruction that exercises one.
fn order_rule_checks(
    w: &World,
    cx: &Cx,
    op: usize,
    ui: usize,
    si: usize,
    pre_users: &[User],
    pre_led: &[i64],
    ro_single: bool,
    repl: Option<(u64, bool)>,
    cancel_all: bool,
    what: &str,
) {
    let m: &Market = w.rf(&cx.market);
    let base = |x: &User| x.pos.iter().find(|p| p.market == cx.market).map_or(0i64, |p| p.base);
    // the bucket the market position is margined in, as the program judges it
    let bucket_eq = |x: &User| {
        let (c, iso) = booked(x, m);
        iso.map_or(c.eq, |i| i.eq)
    };
    let shrunk = |b0: i64, b1: i64| b1.unsigned_abs() <= b0.unsigned_abs() && (b1 == 0 || (b1 > 0) == (b0 > 0));
    // a stop that has protected a position does not outlive it
    let no_stale_stop = |x: &User, who: &str| {
        if base(x) == 0 {
            for t in x.triggers.iter() {
                assert!(
                    t.kind == 0 || t.kind & 0xff == 3 || t.next_slot == 0,
                    "{what}: {who} is flat but still has an armed TP/SL"
                );
            }
        }
    };
    let uk = cx.users[ui].u;
    let now: &User = w.rf(&uk);
    if matches!(op, 0 | 1 | 2 | 3) {
        no_stale_stop(now, "the account");
    }
    if ro_single {
        let b0 = base(&pre_users[ui]) + pre_led[ui];
        let b1 = base(now);
        assert!(shrunk(b0, b1), "{what}: a reduce-only order grew or flipped the position ({b0} -> {b1})");
        if b1 != b0 {
            assert!(bucket_eq(now) >= 0, "{what}: a reduce-only fill left equity {} < 0", bucket_eq(now));
        }
    }
    let mine = |f: &dyn Fn(&Order) -> bool| {
        cx.sh.iter().any(|s| {
            let s: &Shard = w.rf(s);
            s.orders[..s.n as usize].iter().any(|o| o.owner == uk && f(o))
        })
    };
    if let Some((cid, had)) = repl {
        assert!(
            had || !mine(&|o| o.cid == cid),
            "{what}: an IF_CANCELLED place landed though its cancel removed nothing"
        );
    }
    if cancel_all {
        assert!(!mine(&|_| true), "{what}: orders left after a cancel-all");
    }
    if op == 26 {
        let ti = ui % 6;
        let (tk, kp) = (cx.users[ti].u, cx.users[6].u);
        let (t0, t1): (&User, &User) = (&pre_users[ti], w.rf(&tk));
        let (tr0, tr1) = (&pre_users[ti].triggers[si], &t1.triggers[si]);
        let tip = w.rf::<User>(&kp).cash as i128 - pre_users[6].cash as i128;
        assert!(
            tip >= 0 && tip <= tr0.tip.min(spec::MAX_KEEPER_TIP) as i128,
            "{what}: keeper was paid {tip}, the trigger's tip is {}",
            tr0.tip
        );
        if tr0.kind & 0xff == 3 {
            assert!(
                tr1.kind == 0 || (tr1.slices_left + 1 == tr0.slices_left && tr1.qty < tr0.qty),
                "{what}: a TWAP slice ran without using a slice or lots"
            );
        } else {
            let (b0, b1) = (base(t0) + pre_led[ti], base(t1));
            assert!(shrunk(b0, b1), "{what}: a TP/SL grew or flipped the position ({b0} -> {b1})");
            if b1 != b0 {
                assert!(bucket_eq(t1) >= 0, "{what}: a TP/SL left equity {} < 0", bucket_eq(t1));
            }
            assert!(
                tr1.kind == 0 || (tr1.qty < tr0.qty && tr1.tip <= tr0.tip),
                "{what}: a TP/SL filled without its size going down"
            );
            // the tip is paid once over all partial fills: what was paid now came out of what is left
            if tr1.kind != 0 {
                assert_eq!(
                    tr1.tip as i128 + tip,
                    tr0.tip.min(spec::MAX_KEEPER_TIP) as i128,
                    "{what}: a partial TP/SL fill did not take its tip out of the stop"
                );
            }
            // a stop is spent only when its size is done or there is nothing left to reduce: a partial fill leaves it armed
            let cap0 = if tr0.side == 0 && b0 < 0 {
                b0.unsigned_abs()
            } else if tr0.side == 1 && b0 > 0 {
                b0 as u64
            } else {
                0
            };
            if cap0 != 0 && tr1.kind == 0 && b1 != 0 {
                assert!(
                    b0.unsigned_abs() - b1.unsigned_abs() >= tr0.qty,
                    "{what}: a TP/SL was spent by a partial fill ({b0} -> {b1}, size {})",
                    tr0.qty
                );
            }
            no_stale_stop(t1, "the stop's account");
        }
    }
}
/// Global invariants, after every successful instruction.
fn invariants(w: &World, cx: &Cx, vault_amt: i128, exact_money: bool, what: &str) {
    let m: &Market = w.rf(&cx.market);
    let ins: &Insurance = w.rf(&cx.ins);
    let (mut base, mut longs, mut money, mut oi) = (0i128, 0i64, 0i128, 0i64);
    for x in &cx.users {
        let u: &User = w.rf(&x.u);
        money += u.cash as i128;
        let (mut empty, mut have) = (false, false);
        let (mut bo, mut ao, mut no) = (0u64, 0u64, 0u64);
        for s in &cx.sh {
            let s: &Shard = w.rf(s);
            for o in &s.orders[..s.n as usize] {
                if o.owner == x.u {
                    if s.side == 0 {
                        bo += o.qty
                    } else {
                        ao += o.qty
                    }
                    no += 1;
                }
            }
            for l in &s.ledger[..s.nled as usize] {
                if l.owner == x.u {
                    if s.side == 0 {
                        bo += l.open_reduce
                    } else {
                        ao += l.open_reduce
                    }
                    no += l.n_closed;
                }
            }
        }
        let (mut locked, mut unsettled) = (0i128, false);
        for f in &cx.books {
            let f: &FastBook = w.rf(f);
            for e in &f.makers[..f.n as usize] {
                if e.user == x.u {
                    locked += e.margin as i128;
                    unsettled |= e.base != 0 || e.quote != 0;
                }
            }
        }
        for (j, p) in u.pos.iter().enumerate() {
            if p.market == [0; 32] {
                empty = true;
                continue;
            }
            assert!(!empty, "{what}: positions are not packed at the front");
            assert!(p.market == cx.market && !have, "{what}: unknown or duplicate position slot {j}");
            have = true;
            base += p.base as i128;
            longs += p.base.max(0);
            if p.flags & 1 != 0 {
                money += p.iso_margin as i128;
                assert!(p.flags & 4 == 0, "{what}: a maker-pinned position is isolated");
            } else {
                assert_eq!(
                    p.iso_margin as i128, locked,
                    "{what}: maker margin mirror on the position != margin locked in the books"
                );
            }
            if p.base == 0 {
                assert_eq!(p.cost, 0, "{what}: flat position with a cost basis");
            }
            assert_eq!(
                (p.bid_open, p.ask_open, p.n_orders),
                (bo, ao, no),
                "{what}: open-order accounting (bid_open, ask_open, n_orders) != resting orders + unclaimed ledger"
            );
        }
        if !have {
            assert!(bo == 0 && ao == 0 && no == 0 && locked == 0 && !unsettled, "{what}: orders, fills or maker margin without a position slot (bid_open {bo} ask_open {ao} orders {no} locked {locked} unsettled {unsettled})");
        }
        assert!(locked >= 0, "{what}: negative maker margin");
        // complete_view / maker_clear rely on this: unsettled fast fills always sit behind locked margin (so behind a non-zero mirror)
        if unsettled {
            assert!(locked > 0, "{what}: unsettled maker fills with no margin locked");
        }
    }
    for s in &cx.sh {
        let s: &Shard = w.rf(s);
        money += s.fee_accum as i128;
        oi += s.oi_long_delta;
        for l in &s.ledger[..s.nled as usize] {
            base += l.base as i128;
            money += l.quote as i128;
        }
    }
    for (bi, fk) in cx.books.iter().enumerate() {
        let f: &FastBook = w.rf(fk);
        money += f.fee_accum as i128;
        oi += f.oi_long_delta;
        for (mi, e) in f.makers[..f.n as usize].iter().enumerate() {
            base += e.base as i128;
            money += e.margin as i128 + e.quote as i128;
            assert!(e.budget >= 0, "{what}: maker {mi} budget negative on book {bi}");
            let sl: &Slab = w.rf(&e.slab);
            for ul in 0..16 {
                let (us, q) = (&f.used[mi][ul], &sl.quotes[(f.idx as usize) << 4 | ul]);
                if us.gen == q.gen {
                    assert!(us.qty <= q.qty, "{what}: quote generation filled past its size (maker {mi} level {ul})");
                }
            }
        }
    }
    oi += ins.oi_long_delta;
    assert_eq!(
        base, 0,
        "{what}: lots not conserved (sum of positions + unclaimed ledger + unsettled maker fills != 0)"
    );
    assert!(ins.balance >= 0, "{what}: insurance negative");
    assert_eq!(
        (m.oi_long as i64).wrapping_add(oi),
        longs,
        "{what}: open interest (market + pending deltas) != sum of booked longs"
    );
    assert!(vault_amt >= 0, "{what}: more tokens left the vault than entered");
    if exact_money {
        assert_eq!(money + ins.balance as i128 + m.fees as i128, vault_amt + m.bad_debt as i128,
        "{what}: conservation: cash + isolated margin + unclaimed quote + maker margin/quote + insurance + fees != vault + bad_debt");
    }
}

/// Every maker fill an instruction made, found by comparing the books before and after. Returns total lots filled.
fn fill_checks(
    w: &World,
    cx: &Cx,
    pre: &[FastBook],
    settle_or_fund: bool,
    liq: bool,
    taker: Option<K>,
    what: &str,
) -> u64 {
    let m: &Market = w.rf(&cx.market);
    let mut total = 0u64;
    for (bi, fk) in cx.books.iter().enumerate() {
        let f: &FastBook = w.rf(fk);
        let b0 = &pre[bi];
        let signed_fresh = f.fast_px != 0 && w.slot <= f.fast_slot + spec::FAST_PX_MAX_SLOTS;
        let bp = if signed_fresh { f.fast_px } else { m.oracle_px } as i128;
        for mi in 0..f.n as usize {
            let (e, e0) = (&f.makers[mi], &b0.makers[mi]);
            let sl: &Slab = w.rf(&e.slab);
            let (mut lots, mut dbase, mut nt, mut snt, mut nf) = (0u64, 0i128, 0i128, 0i128, 0i128);
            for ul in 0..16usize {
                let (us, u0) = (&f.used[mi][ul], &b0.used[mi][ul]);
                if us.gen == u0.gen {
                    assert!(us.qty >= u0.qty, "{what}: used size went down within one quote generation");
                }
                let fq = if us.gen == u0.gen { us.qty - u0.qty } else { us.qty } as i128;
                if fq == 0 {
                    continue;
                }
                let ask = ul >> 3 == 1;
                let q = &sl.quotes[(f.idx as usize) << 4 | ul];
                let lad = &sl.lad[(f.idx as usize) << 1 | (ul >> 3)];
                assert_eq!(q.gen, us.gen, "{what}: fill booked against a quote generation the slab does not hold");
                assert!(us.qty <= q.qty, "{what}: quote generation filled past its size");
                assert!(
                    f.side == 0 || f.side == (ul >> 3) as u64 + 1,
                    "{what}: fill on a side this book does not serve"
                );
                let peg = lad.flags & spec::LADDER_PEG != 0;
                let p = if peg { bp + q.px as i128 - spec::QUOTE_PEG_BIAS as i128 } else { q.px as i128 };
                assert!(p > 0 && p < (1i128 << 32), "{what}: fill at an unusable price {p}");
                let mk_ = mark(m) as i128;
                assert!(
                    (p - mk_).abs() * 10000 <= mk_ * spec::QUOTE_BAND_BPS as i128,
                    "{what}: maker fill at {p} outside the quote band of mark {mk_}"
                );
                assert!(
                    w.slot as i128 - q.slot as i128 <= spec::QUOTE_MAX_AGE_SLOTS as i128,
                    "{what}: fill of a quote older than QUOTE_MAX_AGE_SLOTS"
                );
                assert!(sl.kill == 0, "{what}: fill of a killed slab");
                assert!(
                    sl.idx_market[f.idx as usize] == cx.market,
                    "{what}: fill of a quote block not registered for this market"
                );
                if let Some(t) = taker {
                    assert!(sl.user != t, "{what}: a taker filled its own quotes");
                }
                if !peg && lad.tol_bps == 0 {
                    let through = if ask { bp - p } else { p - bp };
                    assert!(through * 10000 <= bp * spec::FAST_GUARD_BPS as i128,
                        "{what}: fixed-price un-toleranced level filled {through} through the book price {bp} (> FAST_GUARD_BPS)");
                }
                if lad.tol_bps != 0 {
                    let r = lad.ref_px as i128;
                    assert!(
                        (bp - r).abs() * 10000 <= r * lad.tol_bps as i128,
                        "{what}: fill beyond the ladder's own tolerance"
                    );
                }
                let fr = (lad.flags >> spec::LADDER_FRESH_SHIFT) as u64;
                if fr != 0 {
                    assert!(
                        f.fast_px != 0 && w.slot <= f.fast_slot + fr.min(spec::FAST_PX_MAX_SLOTS),
                        "{what}: fresh-only ladder filled without a fresh signed price"
                    );
                }
                if !(liq && known("liq-aged")) {
                    assert!(signed_fresh || w.slot <= m.oracle_slot + spec::FAST_GUARD_MAX_AGE_SLOTS,
                        "{what}: maker fill while the price is older than FAST_GUARD_MAX_AGE_SLOTS and there is no fresh signed price");
                }
                if lad.inv_max != 0 {
                    let inv = f.ext[mi].inv0 as i128 + e.base as i128;
                    assert!(
                        lad.inv_max as i128 + if ask { inv } else { -inv } >= 0,
                        "{what}: ladder filled past its inventory cap"
                    );
                }
                lots += fq as u64;
                nf += 1;
                nt += p * fq;
                if ask {
                    dbase -= fq;
                    snt += p * fq;
                } else {
                    dbase += fq;
                    snt -= p * fq;
                }
            }
            if settle_or_fund {
                assert_eq!(lots, 0, "{what}: a settle / fund filled quotes");
                continue;
            }
            assert_eq!(
                (f.ext[mi].rebate_bps >> 16) as i128 - (b0.ext[mi].rebate_bps >> 16) as i128,
                lots as i128,
                "{what}: maker volume != lots recorded in its used table"
            );
            assert_eq!(
                e.base as i128 - e0.base as i128,
                dbase,
                "{what}: maker lots != lots recorded in its used table"
            );
            assert_eq!(
                e0.budget as i128 - e.budget as i128,
                nt,
                "{what}: maker budget did not drop by exactly the filled notional"
            );
            let rebate = e.quote as i128 - e0.quote as i128 - snt;
            assert!(
                rebate >= 0 && rebate <= nt * 5 / 10000 + nf,
                "{what}: maker quote != filled notional (+ rebate within the taker fee)"
            );
            assert_eq!(e.margin, e0.margin, "{what}: maker margin changed outside fund / settle");
            total += lots;
        }
    }
    total
}

// ---------- reference model of `take` (resting orders), written from docs/SECURITY.md "Order book" and docs/FAST_PATH.md ----------
// Rules modelled: a taker fills the best opposite price first; at one price the order that arrived in an earlier slot, then the
// earlier shard, then the earlier order; never through its limit; the fill is at the maker's price; the taker pays
// ceil(notional x fee bps), the maker gets its notional (+ floor rebate) in the shard ledger; an order outside its own oracle
// band is removed, not filled; the taker's own orders never fill (self-trade prevention: cancel the resting one, stop, or
// decrement both); the taker's own unclaimed fills are booked first; a cost basis is the net quote paid, 0 when flat, re-based at
// the trade's average price on a flip. Only limit orders (pegs and splines re-price lazily and are left to the invariants).
struct Pred {
    base: i128,
    bucket: i128,
    cost: i128,
    books: Vec<(usize, Vec<(K, u64, u64)>)>,
    ledgers: Vec<(usize, HashMap<K, (i64, i64, u64, u64)>)>,
    fees: i128,
    ioc: bool,
    open: (u64, u64, u64),
}
fn cost_rule(ob: i128, nb: i128, cost: i128, paid: i128) -> i128 {
    if nb == 0 {
        0
    } else if ob != 0 && (ob > 0) != (nb > 0) {
        nb * paid.abs() / (nb - ob).abs()
    } else {
        cost + paid
    }
}
fn model_place(w: &World, cx: &Cx, ui: usize, side: u8, fl: u8, limit: u64, mut qty: u64) -> Option<Pred> {
    let m: &Market = w.rf(&cx.market);
    let u: &User = w.rf(&cx.users[ui].u);
    let uk = cx.users[ui].u;
    let bid = side == 0;
    let opp = 1 - side as u64;
    let (post, reduce, fok) = (fl & 1 != 0, fl & 2 != 0, fl & 8 != 0);
    let ioc = fl & 4 != 0 || fok || reduce;
    let stp = (fl as u64 & 96) >> 5;
    if post {
        return None;
    }
    let p0 = u.pos.iter().find(|p| p.market == cx.market);
    let iso = p0.is_some_and(|p| p.flags & 1 != 0);
    let (mut base, mut cost) = (p0.map_or(0, |p| p.base as i128), p0.map_or(0, |p| p.cost as i128));
    let mut bucket = if iso { p0.unwrap().iso_margin as i128 } else { u.cash as i128 };
    let mut open = p0.map_or((0, 0, 0), |p| (p.bid_open, p.ask_open, p.n_orders));
    // 1) the taker's own unclaimed maker fills, shard by shard in account order
    let mut ledgers: Vec<(usize, HashMap<K, (i64, i64, u64, u64)>)> = vec![];
    let mut books: Vec<(usize, Vec<(K, u64, u64)>)> = vec![];
    let mut all: Vec<(u64, u64, usize, usize, K, u64, u64, u64)> = vec![]; // price, arrival, shard, pos, owner, qty, band, rebate bps
    for (si, sk) in cx.sh.iter().enumerate() {
        let s: &Shard = w.rf(sk);
        let mut led = HashMap::new();
        for l in &s.ledger[..s.nled as usize] {
            if l.owner == uk {
                let ob = base;
                base += l.base as i128;
                bucket += l.quote as i128;
                cost = cost_rule(ob, base, cost, -(l.quote as i128));
                if s.side == 0 {
                    open.0 -= l.open_reduce
                } else {
                    open.1 -= l.open_reduce
                }
                open.2 -= l.n_closed;
            } else {
                led.insert(l.owner, (l.base, l.quote, l.open_reduce, l.n_closed));
            }
        }
        ledgers.push((si, led));
        if s.side != opp {
            continue;
        }
        for (i, o) in s.orders[..s.n as usize].iter().enumerate() {
            if o.meta & 0xff != 0 {
                return None;
            }
            all.push((o.price, o.meta >> 32, si, i, o.owner, o.qty, o.band_bps, (o.meta >> 8) & 0xff));
        }
        books.push((si, vec![]));
    }
    if reduce {
        let cap = if bid && base < 0 {
            (-base) as u64
        } else if !bid && base > 0 {
            base as u64
        } else {
            0
        };
        qty = qty.min(cap);
    }
    // 2) priority: price, arrival slot, shard, position
    all.sort_by(|a, b| {
        (if bid { a.0.cmp(&b.0) } else { b.0.cmp(&a.0) }).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)).then(a.3.cmp(&b.3))
    });
    let off = |px: u64, band: u64| {
        band != 0 && (px.abs_diff(m.oracle_px) as u128) * 10000 > m.oracle_px as u128 * band as u128
    };
    let within = |px: u64| if bid { px <= limit } else { px >= limit };
    if fok {
        let have: u64 = all.iter().filter(|o| o.4 != uk && !off(o.0, o.6) && within(o.0)).map(|o| o.5).sum();
        if have < qty {
            qty = 0;
        }
    }
    let taker_bps = spec::TIERS[(u.tier as usize).min(spec::TIERS.len() - 1)].0;
    let (mut fees, mut stop) = (0i128, false);
    let mut rest: Vec<(u64, u64, usize, usize, K, u64, u64, u64)> = vec![];
    for mut o in all {
        if stop || qty == 0 || !within(o.0) {
            rest.push(o);
            stop = true;
            continue;
        }
        let led = &mut ledgers.iter_mut().find(|x| x.0 == o.2).unwrap().1;
        if o.4 == uk {
            let live = !off(o.0, o.6);
            if stp == 1 && live {
                rest.push(o);
                stop = true;
                continue;
            }
            let d = if stp > 1 && live { qty.min(o.5) } else { o.5 };
            if stp > 1 && live {
                qty -= d;
            }
            if bid {
                open.1 -= d
            } else {
                open.0 -= d
            }
            o.5 -= d;
            if o.5 == 0 {
                open.2 -= 1;
            } else {
                rest.push(o);
            }
        } else if off(o.0, o.6) {
            let e = led.entry(o.4).or_insert((0, 0, 0, 0));
            e.2 += o.5;
            e.3 += 1;
        } else {
            let q = qty.min(o.5);
            let nt = o.0 as i128 * q as i128;
            let (fee, reb) = ((nt * taker_bps as i128 + 9999) / 10000, nt * o.7 as i128 / 10000);
            let e = led.entry(o.4).or_insert((0, 0, 0, 0));
            if bid {
                e.0 -= q as i64;
                e.1 += (nt + reb) as i64;
            } else {
                e.0 += q as i64;
                e.1 += (-nt + reb) as i64;
            }
            e.2 += q;
            o.5 -= q;
            if o.5 == 0 {
                e.3 += 1;
            } else {
                rest.push(o);
            }
            fees += fee - reb;
            qty -= q;
            let paid = if bid { nt } else { -nt };
            bucket -= fee + paid;
            let ob = base;
            base += if bid { q as i128 } else { -(q as i128) };
            cost = cost_rule(ob, base, cost, paid);
        }
    }
    rest.sort_by(|a, b| a.2.cmp(&b.2).then(a.3.cmp(&b.3)));
    for o in rest {
        books.iter_mut().find(|x| x.0 == o.2).unwrap().1.push((o.4, o.0, o.5));
    }
    Some(Pred { base, bucket, cost, books, ledgers, fees, ioc, open })
}
fn check_pred(w: &World, cx: &Cx, ui: usize, pr: &Pred, fees0: i128, what: &str) {
    let u: &User = w.rf(&cx.users[ui].u);
    let uk = cx.users[ui].u;
    let p = u.pos.iter().find(|p| p.market == cx.market);
    let iso = p.is_some_and(|p| p.flags & 1 != 0);
    assert_eq!(p.map_or(0, |p| p.base as i128), pr.base, "{what}: take differs from the reference model: taker lots");
    assert_eq!(
        if iso { p.unwrap().iso_margin as i128 } else { u.cash as i128 },
        pr.bucket,
        "{what}: take differs from the reference model: taker cash (notional + fees)"
    );
    assert_eq!(
        p.map_or(0, |p| p.cost as i128),
        pr.cost,
        "{what}: take differs from the reference model: taker cost basis"
    );
    if pr.ioc {
        assert_eq!(
            p.map_or((0, 0, 0), |p| (p.bid_open, p.ask_open, p.n_orders)),
            pr.open,
            "{what}: take differs from the reference model: taker's own resting orders (self-trade prevention)"
        );
    }
    let mut fees = 0i128;
    for (si, sk) in cx.sh.iter().enumerate() {
        let s: &Shard = w.rf(sk);
        fees += s.fee_accum as i128;
        if let Some((_, want)) = pr.books.iter().find(|x| x.0 == si) {
            let got: Vec<(K, u64, u64)> = s.orders[..s.n as usize].iter().map(|o| (o.owner, o.price, o.qty)).collect();
            assert!(got == *want, "{what}: take differs from the reference model: resting orders left in shard {si}");
            let led: HashMap<K, (i64, i64, u64, u64)> = s.ledger[..s.nled as usize]
                .iter()
                .map(|l| (l.owner, (l.base, l.quote, l.open_reduce, l.n_closed)))
                .collect();
            assert!(
                led == pr.ledgers.iter().find(|x| x.0 == si).unwrap().1,
                "{what}: take differs from the reference model: maker fills in the ledger of shard {si}"
            );
        }
        assert!(
            !s.ledger[..s.nled as usize].iter().any(|l| l.owner == uk),
            "{what}: the taker's own fills were not claimed"
        );
    }
    assert_eq!(fees - fees0, pr.fees, "{what}: take differs from the reference model: fees kept by the shards");
}

fuzz_target!(|data: &[u8]| {
    if data.len() < 16 {
        return;
    }
    let cfg = data[0];
    let (plain, split, short_stale, no_signer, fund, near_liq, no_quotes) =
        (cfg & 1 != 0, cfg & 2 != 0, cfg & 4 != 0, cfg & 8 != 0, cfg & 16 != 0, cfg & 32 != 0, cfg & 64 != 0);
    let pid = Address::new_from_array([7u8; 32]);
    let mut w = World { pid, accts: HashMap::new(), slot: 1000, ts: 1_800_000_000, n: 0, okn: 0, ro_refused: 0 };
    let admin0 = w.signer();
    let mem = [w.signer(), w.signer(), w.signer()];
    let stranger = w.signer();
    let msig = w.prog(MULTISIG_SIZE);
    {
        let mut d = vec![33u8, 2, 3];
        for k in &mem {
            d.extend_from_slice(k);
        }
        assert!(w.call(&[(admin0, true, true), (msig, false, true)], &d));
    }
    let market = w.prog(MARKET_SIZE);
    let ins = w.prog(INS_SIZE);
    let rank = w.prog(RANK_SIZE);
    let mint = w.key();
    let feed = [7u8; 32];
    let oracle = w.add(&PYTH, &pyth(&feed, 1000_00000000, 10_000_000, w.ts));
    let tok = kk(&TOKEN);
    w.accts.insert(tok, Acct::new(&TOKEN, &Address::default(), &[]));
    let (stale_slots, max_age) = if short_stale { (3u64, 10u64) } else { (150, 120) };
    let params = |max_oi: u64| {
        let mut d = vec![];
        for v in [100u64, stale_slots, 100, max_age, (-8i64) as u64, 1, 1_000_000, max_oi] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        d
    };
    let (vault, pda);
    if plain {
        let (p, bump) = Address::find_program_address(&[b"vault", &market[..]], &w.pid);
        pda = kk(&p);
        vault = w.add(&TOKEN, &token_acct(&mint, &pda));
        let mut d = vec![10u8, bump];
        d.extend_from_slice(&feed);
        d.extend(params(0));
        assert!(w.call(
            &[
                (admin0, true, true),
                (market, false, true),
                (ins, false, true),
                (vault, false, false),
                (oracle, false, false)
            ],
            &d
        ));
    } else {
        // pooled market: its admin is pinned to the pool's admin, a 2-of-3 Multisig; admission is timelocked
        let pool = w.prog(POOL_SIZE);
        let (p, bump) = Address::find_program_address(&[b"vault", &pool[..]], &w.pid);
        pda = kk(&p);
        vault = w.add(&TOKEN, &token_acct(&mint, &pda));
        let mut d = vec![27u8, bump];
        d.extend_from_slice(&0u64.to_le_bytes());
        d.extend_from_slice(&msig);
        assert!(w.call(&[(admin0, true, true), (pool, false, true), (vault, false, false), (msig, false, false)], &d));
        let mut d = vec![32u8];
        d.extend_from_slice(&market);
        assert!(
            !w.call(&[(pool, false, true), (msig, false, false), (mem[0], true, false)], &d),
            "one multisig member proposed a pool market"
        );
        assert!(w.call(&[(pool, false, true), (msig, false, false), (mem[0], true, false), (mem[2], true, false)], &d));
        w.slot += spec::TIMELOCK_SLOTS;
        w.ts += 86_400;
        w.accts.insert(oracle, Acct::new(&ad(&oracle), &PYTH, &pyth(&feed, 1000_00000000, 10_000_000, w.ts)));
        let mut d = vec![10u8, 0];
        d.extend_from_slice(&feed);
        d.extend(params(50_000_000));
        assert!(w.call(
            &[
                (admin0, true, true),
                (market, false, true),
                (ins, false, true),
                (vault, false, false),
                (oracle, false, false),
                (pool, false, true),
                (msig, false, false),
                (mem[0], true, false),
                (mem[1], true, false)
            ],
            &d
        ));
    }
    let sysvar = kk(&SYSVAR_IX);
    let mut cx = Cx {
        market,
        ins,
        rank,
        vault,
        pda,
        tok,
        oracle,
        feed,
        sh: vec![],
        books: vec![],
        users: vec![],
        slabs: vec![],
        msig,
        mem,
        stranger,
        admin0,
        pooled: !plain,
        signer: None,
        sysvar,
    };
    w.accts.insert(pda, Acct::new(&ad(&pda), &Address::default(), &[]));
    let adm = |cx: &Cx, mid: &[Meta]| -> Vec<Meta> {
        let mut v = vec![cx.a0()];
        v.extend_from_slice(mid);
        v.extend(cx.tail());
        v
    };
    assert!(w.call(&adm(&cx, &[(market, false, true), (rank, false, true)]), &[26]));
    for (side, lo) in [(0u8, 99_900u64), (0, 99_000), (1, 100_000), (1, 101_000)] {
        let s = w.prog(SHARD_SIZE);
        let mut d = vec![11u8, side];
        d.extend_from_slice(&lo.to_le_bytes());
        assert!(w.call(&adm(&cx, &[(market, false, true), (s, false, true)]), &d));
        cx.sh.push(s);
    }
    // cfg bit 7 with split books: only the bids book exists, so the ask side of the market has no maker quotes and shorts are
    // liquidated by liquidate_book (which refuses a side that has a registered book)
    for side in if split {
        if cfg & 128 != 0 {
            vec![1u8]
        } else {
            vec![1u8, 2]
        }
    } else {
        vec![0u8]
    } {
        let f = w.prog(FAST_SIZE);
        let mut d = le(61, 0);
        d.push(side);
        assert!(w.call(&adm(&cx, &[(market, false, true), (f, false, true)]), &d));
        cx.books.push(f);
    }
    if !no_signer {
        let sk = w.key();
        cx.signer = Some(sk);
        for f in cx.books.clone() {
            let mut d = vec![74u8];
            d.extend_from_slice(&sk);
            assert!(w.call(&adm(&cx, &[(market, false, false), (f, false, true)]), &d));
        }
    }
    let mut px: i64 = 1000_00000000;
    let mut vault_amt: i128 = 0;
    // one oracle step: Pyth account rewritten, then IX 4 or (half the time, with the whole book) the soft IX 78
    let upd = |w: &mut World, cx: &Cx, px: i64, conf: u64, mode: u8| -> bool {
        let t = w.ts;
        w.accts.insert(cx.oracle, Acct::new(&ad(&cx.oracle), &PYTH, &pyth(&cx.feed, px, conf, t)));
        let mut mm = vec![(cx.market, false, true), (cx.oracle, false, false)];
        if mode & 1 != 0 {
            mm.extend(cx.sh.iter().map(|s| (*s, false, false)));
        }
        if mode & 2 != 0 {
            mm.extend(cx.books.iter().map(|s| (*s, false, false)));
            mm.extend(cx.joined().iter().map(|s| (s.slab, false, false)));
        }
        if mode & 4 != 0 {
            // IX 78 (soft): same effect as IX 4 when usable; when not, the market must be byte-identical
            let before = w.bytes(&cx.market);
            assert!(w.call(&mm, &[78]), "IX 78 must never fail");
            let strict = w.call(&mm, &[4]);
            if !strict {
                assert_eq!(w.bytes(&cx.market), before, "soft update changed the market although the strict one fails");
            }
            true
        } else {
            w.call(&mm, &[4])
        }
    };
    assert!(upd(&mut w, &cx, px, 10_000_000, 0));
    let mut dep = vec![0i128; 7];
    let mut wd = vec![0i128; 7];
    for i in 0..7usize {
        let auth = w.signer();
        let u = w.prog(USER_SIZE);
        assert!(w.call(&[(auth, true, true), (u, false, true)], &[12]));
        let src = w.add(&TOKEN, &token_acct(&mint, &auth));
        let amt: u64 = match i {
            0..=3 => 200_000 + 3_000_000 * i as u64,
            4 | 5 => 40_000_000,
            _ => 1_000_000_000,
        };
        assert!(w.call(
            &[
                (auth, true, true),
                (u, false, true),
                (market, false, false),
                (vault, false, true),
                (src, false, true),
                (tok, false, false)
            ],
            &le(13, amt)
        ));
        vault_amt += amt as i128;
        dep[i] += amt as i128;
        set_vault(&mut w, &vault, vault_amt);
        cx.users.push(Usr { auth, u, src });
    }
    // slabs: users 4 and 5 are approved makers from the start; user 3 owns a slab that may join later (op 18)
    for ui in [4usize, 5, 3] {
        let s = w.prog(SLAB_SIZE);
        let x = &cx.users[ui];
        assert!(w.call(&[(x.auth, true, true), (x.u, false, false), (s, false, true)], &[60]));
        cx.slabs.push(Slb { slab: s, ui, joined: false });
    }
    let join = |w: &mut World, cx: &mut Cx, si: usize| -> bool {
        let (slab, auth) = (cx.slabs[si].slab, cx.users[cx.slabs[si].ui].auth);
        let mut ok = true;
        for f in cx.books.clone() {
            ok &= w.call(
                &adm(cx, &[(cx.market, false, false), (f, false, true), (slab, false, true), (auth, true, false)]),
                &[62],
            );
        }
        if ok {
            cx.slabs[si].joined = true;
        }
        ok
    };
    assert!(join(&mut w, &mut cx, 0));
    assert!(join(&mut w, &mut cx, 1));
    let pu = |px: i64| (px / 1_000_000) as u64;
    // a quote call for one ladder: settings (always written, so ladders move between modes) then `n` levels
    let quote = |w: &mut World,
                 cx: &Cx,
                 si: usize,
                 ask: bool,
                 peg: bool,
                 tol: u16,
                 refpx: u32,
                 inv: u32,
                 fresh: u16,
                 levels: &[(u32, u32)],
                 clear: bool|
     -> bool {
        let lad = ask as u16;
        let mut d = vec![63u8];
        qitem(&mut d, 0x8000 | lad, refpx, tol as u32);
        qitem(&mut d, 0xC000 | lad, inv, (peg as u32) | (fresh as u32) << 8);
        for l in 0..8usize {
            if l < levels.len() {
                qitem(&mut d, lad << 3 | l as u16, levels[l].0, levels[l].1);
            } else if clear {
                qitem(&mut d, lad << 3 | l as u16, 0, 0);
            }
        }
        let x = &cx.slabs[si];
        w.call(&[(cx.users[x.ui].auth, true, true), (x.slab, false, true)], &d)
    };
    let fundm = |w: &mut World, cx: &Cx, ui: usize, book: K, amt: i64, tgt: Option<i64>| -> bool {
        let x = &cx.users[ui];
        let mut d = le(64, amt as u64);
        if let Some(t) = tgt {
            d.extend_from_slice(&t.to_le_bytes());
        }
        w.call(&[(x.auth, true, true), (x.u, false, true), (cx.market, false, false), (book, false, true)], &d)
    };
    for ui in [4usize, 5] {
        for f in cx.books.clone() {
            assert!(fundm(&mut w, &cx, ui, f, 4_000_000, None));
        }
    }
    if !no_quotes {
        let p = pu(px) as u32;
        assert!(quote(&mut w, &cx, 0, true, false, 0, 0, 0, 0, &[(p + 10, 30), (p + 20, 40), (p + 40, 60)], true));
        assert!(quote(&mut w, &cx, 0, false, false, 0, 0, 0, 0, &[(p - 10, 30), (p - 20, 40), (p - 40, 60)], true));
        assert!(quote(&mut w, &cx, 1, true, false, 0, 0, 0, 0, &[(p + 15, 25), (p + 30, 50)], true));
        assert!(quote(&mut w, &cx, 1, false, false, 0, 0, 0, 0, &[(p - 15, 25), (p - 30, 50)], true));
    }
    let trade = |cx: &Cx, ui: usize| -> Vec<Meta> {
        let x = &cx.users[ui];
        let mut m = vec![(x.auth, true, true), (x.u, false, true), (cx.market, false, false)];
        m.extend(cx.sh.iter().map(|s| (*s, false, true)));
        m
    };
    if near_liq {
        // user0 long 18 lots on 200k, bids resting near the top, oracle stepped to 935: user0 just below maintenance
        let mut d = vec![1u8];
        d.extend(place(1, 1, 0, 100_000, 40, 0, 0, 0, 0, 0));
        w.call(&trade(&cx, 1), &d);
        let mut d = vec![1u8];
        d.extend(place(0, spec::FLAG_IOC as u8, 0, 100_000, 18, 0, 0, 0, 0, 0));
        w.call(&trade(&cx, 0), &d);
        for (p, q) in [(99_900u64, 6u64), (99_050, 6), (99_000, 12)] {
            let mut d = vec![1u8];
            d.extend(place(0, 1, 0, p, q, 0, 0, 0, 0, 0));
            w.call(&trade(&cx, 2), &d);
        }
        for p2 in [970_00000000i64, 950_00000000, 935_00000000] {
            w.slot += 1;
            w.ts += 1;
            upd(&mut w, &cx, p2, (p2 / 10000) as u64, 1);
            px = p2;
        }
    }
    let mut ts_ms: u64 = 1000;
    let mut slots_used: u64 = 0;
    let mut idx_moved = false;
    invariants(&w, &cx, vault_amt, true, "setup");
    let mut okc = [0u32; 32];
    let mut tot = [0u32; 32];
    let mut modelled = 0u32;
    let mut mark_cid = 1000u64;
    let all_shards_w = |cx: &Cx| -> Vec<Meta> { cx.sh.iter().map(|s| (*s, false, true)).collect() };
    let slabs_for = |cx: &Cx, w: &World, book: &K, sel: u8| -> Vec<Meta> {
        // approved slabs in the book's own order; `sel` can drop one, repeat one or reverse them (the program must cope)
        let f: &FastBook = w.rf(book);
        let mut v: Vec<Meta> = f.makers[..f.n as usize].iter().map(|e| (e.slab, false, false)).collect();
        match sel & 7 {
            5 => {
                v.pop();
            }
            6 => {
                if let Some(x) = v.first().cloned() {
                    v.push(x);
                }
            }
            7 => v.reverse(),
            _ => {}
        }
        v
    };
    for (step, c) in data[8..].chunks_exact(8).enumerate() {
        let op = (c[0] & 31) as usize;
        let ui = (c[1] % 7) as usize;
        let (auth, u, src) = (cx.users[ui].auth, cx.users[ui].u, cx.users[ui].src);
        let price = 98_000 + (u16::from_le_bytes([c[2], c[3]]) % 4_000) as u64 / 10 * 10;
        let qty = 1 + c[4] as u64 % 40;
        let side = c[6] & 1;
        let ok0 = w.okn;
        tot[op] += 1;
        let pre_books: Vec<FastBook> = cx.books.iter().map(|f| w.cp::<FastBook>(f)).collect();
        let pre_users: Vec<User> = cx.users.iter().map(|x| w.cp::<User>(&x.u)).collect();
        let pre_true: Vec<(Hl, Option<Hl>)> = (0..7).map(|i| truth(&w, &cx, i)).collect();
        let pre_m: Market = w.cp(&market);
        let pre_led: Vec<i64> = cx
            .users
            .iter()
            .map(|x| {
                cx.sh
                    .iter()
                    .map(|s| {
                        let s: &Shard = w.rf(s);
                        s.ledger[..s.nled as usize].iter().filter(|l| l.owner == x.u).map(|l| l.base).sum::<i64>()
                    })
                    .sum()
            })
            .collect();
        let (mut ro_single, mut repl, mut cancel_all) = (false, None::<(u64, bool)>, false);
        let (mut settle_or_fund, mut liq, mut taker) = (false, false, None::<K>);
        let what = format!("step {step} op {op} user {ui}");
        if dbg() {
            eprintln!("{what} {:?} slot {} px {}", &c[2..], w.slot, pu(px));
        }
        match op {
            0 | 1 => {
                let k = if c[7] & 0x30 == 0x10 {
                    1
                } else if c[7] & 0x30 == 0x20 {
                    2
                } else {
                    0
                };
                let (a1, a2, a3) = match k {
                    1 => ((c[2] as i64) - 128, 0, 0),
                    2 => (1 + (c[2] % 7) as i64, 1 + (c[3] % 8) as u64, (c[7] % 3) as u64),
                    _ => (0, 0, 0),
                };
                let mut d = vec![1u8];
                d.extend(place(
                    side,
                    c[5] & 0x7f,
                    k,
                    if k == 1 { 0 } else { price },
                    qty,
                    if c[7] & 1 == 1 { 300 } else { 0 },
                    (c[7] % 4) as u64,
                    a1,
                    a2,
                    a3,
                ));
                // differential: a limit order's take against the reference model (every shard is passed, funding has not moved)
                let pred =
                    if k == 0 && !idx_moved { model_place(&w, &cx, ui, side, c[5] & 0x7f, price, qty) } else { None };
                let fees0: i128 = cx.sh.iter().map(|s| w.rf::<Shard>(s).fee_accum as i128).sum();
                ro_single = k == 0 && c[5] & 2 != 0;
                if w.call(&trade(&cx, ui), &d) {
                    if let Some(pr) = pred {
                        check_pred(&w, &cx, ui, &pr, fees0, &what);
                        modelled += 1;
                    }
                }
            }
            2 => {
                let cid = (c[7] % 4) as u64;
                let mut d = vec![3u8];
                d.extend_from_slice(&0u16.to_le_bytes());
                match c[5] & 3 {
                    // cancel everything, place nothing
                    2 => {
                        cancel_all = true;
                        d.push(1);
                        d.push(0);
                        d.extend_from_slice(&0u64.to_le_bytes());
                        d.push(64);
                        d.extend_from_slice(&0u64.to_le_bytes());
                    }
                    // replace: the first place carries FLAG_IF_CANCELLED and a cid no other order has
                    1 => {
                        mark_cid += 1;
                        let had = cid != 0
                            && cx.sh.iter().any(|s| {
                                let s: &Shard = w.rf(s);
                                s.side == side as u64
                                    && s.orders[..s.n as usize].iter().any(|o| o.owner == u && o.cid == cid)
                            });
                        repl = Some((mark_cid, had));
                        d.push(1);
                        d.push(2);
                        d.extend_from_slice(&cid.to_le_bytes());
                        d.push(side | 128);
                        d.extend_from_slice(&0u64.to_le_bytes());
                        d.extend(place(side, 1 | 128, 0, price, qty, 0, mark_cid, 0, 0, 0));
                        d.extend(place(1 - side, 1, 0, price + 200, qty, 0, 0, 0, 0, 0));
                    }
                    _ => {
                        d.push(1);
                        d.push(2);
                        d.extend_from_slice(&cid.to_le_bytes());
                        d.push(side | 128);
                        d.extend_from_slice(&0u64.to_le_bytes());
                        d.extend(place(side, 1, 0, price, qty, 0, cid, 0, 0, 0));
                        d.extend(place(1 - side, 1, 0, price + 200, qty, 0, 0, 0, 0, 0));
                    }
                }
                w.call(&trade(&cx, ui), &d);
            }
            3 => {
                let mut d = vec![2u8];
                d.extend_from_slice(&0u16.to_le_bytes());
                d.push(1);
                d.push(0);
                let id = ((c[2] % 4) as u64) << 40 | (c[7] as u64 % 16);
                d.extend_from_slice(&id.to_le_bytes());
                d.push(side);
                d.extend_from_slice(&price.to_le_bytes());
                w.call(&trade(&cx, ui), &d);
            }
            4 => {
                // oracle step: <= 1.3% a step, or (c[5] == 0xEE) a 6% jump that must be held until confirmed
                let d = if c[5] == 0xEE {
                    px / 100 * 6 * if c[7] & 1 == 1 { 1 } else { -1 }
                } else {
                    (c[7] as i64 - 128) * 1_0000000
                };
                px = (px + d).clamp(500_00000000, 1500_00000000);
                let ds = 1 + (c[2] % 5) as u64;
                w.slot += ds;
                w.ts += 1;
                slots_used += ds;
                let conf = if c[4] < 200 { (px / 10000) as u64 } else { (px / 10000) as u64 * (c[4] as u64 - 195) };
                upd(&mut w, &cx, px, conf, c[3]);
            }
            5 => {
                let ds = 1 + (c[2] % 48) as u64;
                if fund || slots_used + ds < 8000 {
                    w.slot += ds;
                    w.ts += (ds * 4 / 10) as i64;
                    slots_used += ds;
                }
            }
            6 => {
                let amt = (1 + c[2] as u64) * 10_000;
                if w.call(
                    &[
                        (auth, true, true),
                        (u, false, true),
                        (market, false, false),
                        (vault, false, true),
                        (src, false, true),
                        (tok, false, false),
                    ],
                    &le(13, amt),
                ) {
                    vault_amt += amt as i128;
                    dep[ui] += amt as i128;
                    set_vault(&mut w, &vault, vault_amt);
                }
            }
            7 => {
                // withdraw: a fixed amount, a share of cash, or the lane's own boundary (the most the harness model allows, or 1 more)
                let (h, _) = booked(&pre_users[ui], &pre_m);
                let lane_max = (h.eq.min(pre_users[ui].cash as i128 + h.cost) - h.im).max(0);
                let amt = match c[2] & 3 {
                    0 => qty * 10_000,
                    1 => (pre_users[ui].cash.max(0) as u64 / 256) * (c[3] as u64 + 1),
                    2 => lane_max as u64,
                    _ => lane_max as u64 + 1 + (c[3] as u64) * 997,
                };
                let mut mm = vec![
                    (auth, true, true),
                    (u, false, true),
                    (market, false, false),
                    (vault, false, true),
                    (src, false, true),
                    (tok, false, false),
                    (pda, false, false),
                ];
                match c[5] & 3 {
                    0 | 1 => mm.extend(all_shards_w(&cx)),
                    2 => mm.extend(cx.sh[..2].iter().map(|s| (*s, false, true))),
                    _ => {}
                }
                if c[5] & 4 == 0 {
                    mm.extend(cx.books.iter().map(|f| (*f, false, false)));
                }
                if w.call(&mm, &le(14, amt)) {
                    vault_amt -= amt as i128;
                    wd[ui] += amt as i128;
                    set_vault(&mut w, &vault, vault_amt);
                    let (t, _) = truth(&w, &cx, ui);
                    let us: &User = w.rf(&u);
                    let (bk, _) = booked(us, w.rf(&market));
                    assert!(
                        !pending(&w, &cx, ui),
                        "{what}: withdraw succeeded with unclaimed / unsettled fills outstanding"
                    );
                    assert!(
                        t.eq >= t.im && t.eq >= 0,
                        "{what}: after a withdraw of {amt} true equity {} < initial requirement {}",
                        t.eq,
                        t.im
                    );
                    // realized = cash + cost basis of what is open - net deposits; total withdrawn <= deposits + realized gains
                    let realized = us.cash as i128 + bk.cost - (dep[ui] - wd[ui]);
                    assert!(
                        wd[ui] <= dep[ui] + realized,
                        "{what}: withdrawn {} > deposits {} + realized {}",
                        wd[ui],
                        dep[ui],
                        realized
                    );
                    // differential on the lane itself (the harness's own arithmetic on the account as the instruction left it)
                    assert!(bk.eq.min(us.cash as i128 + bk.cost) >= bk.im, "{what}: withdraw of {amt} passed although equity without positive PnL {} < initial requirement {}", bk.eq.min(us.cash as i128 + bk.cost), bk.im);
                }
            }
            8 => {
                w.call(&[(auth, true, true), (u, false, true), (market, false, false)], &[30, c[7] & 1]);
            }
            9 => {
                let (_, iso) = booked(&pre_users[ui], &pre_m);
                let p0 = &pre_users[ui].pos[0];
                let iso_max = iso.map_or(1, |h| (h.eq.min(p0.iso_margin as i128 + h.cost) - h.im).max(0) as i64);
                let v = match c[2] & 3 {
                    0 | 1 => (qty * 5_000) as i64,
                    2 => iso_max,
                    _ => iso_max + 1 + c[3] as i64 * 997,
                };
                let amt = if c[7] & 2 == 0 { v } else { -v };
                let mut mm = vec![(auth, true, true), (u, false, true), (market, false, false)];
                if c[5] & 1 == 0 {
                    mm.extend(all_shards_w(&cx));
                }
                if w.call(&mm, &le(31, amt as u64)) {
                    let (tc, ti) = truth(&w, &cx, ui);
                    let us: &User = w.rf(&u);
                    if amt > 0 {
                        assert!(
                            us.cash >= 0 && tc.eq >= tc.im,
                            "{what}: cash -> isolated left the cross account at {} < {}",
                            tc.eq,
                            tc.im
                        );
                    } else if amt < 0 {
                        let ti = ti.expect("isolated position");
                        assert!(!pending(&w, &cx, ui), "{what}: isolated margin left with unclaimed fills outstanding");
                        assert!(
                            ti.eq >= ti.im,
                            "{what}: isolated -> cash left the isolated position at {} < {}",
                            ti.eq,
                            ti.im
                        );
                    }
                }
            }
            10 | 31 => {
                // maker quote
                let si = (c[1] % 3) as usize;
                let ask = side == 1;
                let p = pu(px) as i64;
                let off = (c[2] % 64) as i64 - if c[7] & 0x40 != 0 { 32 } else { 4 }; // mostly on the passive side, sometimes through the price
                let n = 1 + (c[4] % 4) as usize;
                let stepv = (c[5] % 4) as i64 * 5;
                let q = 1 + (c[3] % 60) as u32;
                let peg = c[7] & 1 != 0;
                let (tol, refpx) =
                    if c[7] & 2 != 0 { (5 + (c[7] >> 4) as u16, (p + (c[3] as i64 % 7) - 3) as u32) } else { (0, 0) };
                let inv = if c[7] & 4 != 0 { 1 + (c[3] % 50) as u32 } else { 0 };
                let fresh = if c[7] & 8 != 0 { 1 + (c[5] >> 6) as u16 } else { 0 };
                let mut lv = vec![];
                for l in 0..n as i64 {
                    let d = off + l * stepv;
                    let v = if ask { p + d } else { p - d };
                    let d2 = if ask { d } else { -d };
                    lv.push((
                        if peg { (spec::QUOTE_PEG_BIAS as i64 + d2) as u32 } else { v.max(1) as u32 },
                        q + l as u32,
                    ));
                }
                if c[7] & 0x80 != 0 && c[5] & 0x30 == 0x30 {
                    lv.reverse();
                } // a worst-first ladder must be refused
                quote(&mut w, &cx, si, ask, peg, tol, refpx, inv, fresh, &lv, c[5] & 0x80 != 0);
            }
            11 | 30 => {
                // take_fast
                let bid = side == 0;
                let opp = if bid { 1 } else { 0 };
                let book = cx.book(opp);
                let p = pu(px) as i64;
                let lim = if c[7] & 0x40 != 0 {
                    if bid {
                        u32::MAX as u64
                    } else {
                        1
                    }
                } else {
                    (if bid { p + (c[2] % 64) as i64 } else { p - (c[2] % 64) as i64 }) as u64
                };
                let sl = slabs_for(&cx, &w, &book, c[3]);
                let mut d =
                    vec![66u8, side, if c[5] & 0x10 != 0 { spec::FLAG_REDUCE_ONLY as u8 } else { 0 }, sl.len() as u8];
                d.extend_from_slice(&lim.to_le_bytes());
                d.extend_from_slice(&qty.to_le_bytes());
                let mut mm = vec![(auth, true, true), (u, false, true), (market, false, false), (book, false, true)];
                mm.extend(sl);
                if c[5] & 0x20 != 0 {
                    mm.extend(all_shards_w(&cx));
                }
                if let (Some(sk), true) = (cx.signer, c[5] & 0x40 != 0) {
                    ts_ms += if c[7] & 0x20 != 0 { 0 } else { 1 };
                    let spx = (p + (c[7] % 16) as i64 * if c[7] & 0x10 != 0 { 100 } else { 3 } - 20) as u64; // sometimes > 100 bps away: must be ignored
                    w.accts.insert(
                        sysvar,
                        Acct::new(
                            &SYSVAR_IX,
                            &Address::default(),
                            &sysvar_data(&sk, &market, spx, w.slot.saturating_sub((c[3] >> 6) as u64), ts_ms),
                        ),
                    );
                    mm.push((sysvar, false, false));
                }
                taker = Some(u);
                w.call(&mm, &d);
            }
            12 => {
                // take_fast_batch: two or three takers, one book
                let bid = side == 0;
                let opp = if bid { 1 } else { 0 };
                let book = cx.book(opp);
                let p = pu(px) as i64;
                let sl = slabs_for(&cx, &w, &book, c[3]);
                let n = 2 + (c[5] & 1) as usize;
                let mut d = vec![72u8, 0, sl.len() as u8, n as u8];
                let mut mm = vec![(market, false, false), (book, false, true)];
                mm.extend(sl);
                for k in 0..n {
                    let x = &cx.users[(ui + k * (1 + (c[2] % 5) as usize)) % 7];
                    let s_ = if split { side } else { side ^ ((c[7] >> k) & 1) };
                    let b_ = s_ == 0;
                    let lim = (if b_ { p + (c[2] % 64) as i64 } else { p - (c[2] % 64) as i64 }) as u64;
                    d.push(s_);
                    d.push(if c[5] >> (4 + k) & 1 != 0 { spec::FLAG_REDUCE_ONLY as u8 } else { 0 });
                    d.extend_from_slice(&lim.to_le_bytes());
                    d.extend_from_slice(&(qty + k as u64).to_le_bytes());
                    mm.push((x.u, false, true));
                    mm.push((x.auth, true, false));
                    mm.push((tok, false, false));
                }
                if c[5] & 0x80 != 0 {
                    mm.extend(all_shards_w(&cx));
                }
                w.call(&mm, &d);
            }
            13 => {
                settle_or_fund = true;
                let book = cx.books[(c[2] as usize) % cx.books.len()];
                if w.call(&[(u, false, true), (market, false, false), (book, false, true)], &[65]) {
                    // differential: the refilled budget is exactly margin that initial margin leaves free, over IMR
                    let f: &FastBook = w.rf(&book);
                    let m: &Market = w.rf(&market);
                    let e = f.makers[..f.n as usize].iter().find(|e| e.user == u).expect("settled a maker");
                    let (h, _) = booked(w.rf(&u), m);
                    let p0 = &w.rf::<User>(&u).pos[0]; // a stale price only matters to an account that has exposure
                    let free = if is_stale(m, w.slot, w.ts) && (p0.base != 0 || p0.bid_open != 0 || p0.ask_open != 0) {
                        0
                    } else {
                        (e.margin as i128 + h.eq - h.im).clamp(0, e.margin.max(0) as i128)
                    };
                    assert_eq!(
                        e.budget as i128,
                        free * 10000 / m.imr_bps as i128,
                        "{what}: settle: budget != (margin that initial margin leaves free) / IMR"
                    );
                    assert!(e.base == 0 && e.quote == 0, "{what}: settle left fills behind");
                }
            }
            14 => {
                settle_or_fund = true;
                let book = cx.books[(c[2] as usize >> 4) % cx.books.len()];
                let amt = match c[2] & 3 {
                    0 => 0,
                    1 => (c[3] as i64 + 1) * 20_000,
                    2 => -((c[3] as i64 + 1) * 20_000),
                    _ => -1,
                };
                let tgt = if c[4] & 1 != 0 { Some(c[5] as i64 * 50_000) } else { None };
                let bi = cx.books.iter().position(|b| *b == book).unwrap();
                if fundm(&mut w, &cx, ui, book, amt, tgt) {
                    let f: &FastBook = w.rf(&book);
                    let m: &Market = w.rf(&market);
                    let mi = f.makers[..f.n as usize].iter().position(|e| e.user == u).unwrap();
                    let (e, e0) = (&f.makers[mi], &pre_books[bi].makers[mi]);
                    let mb = |x: i128| x.max(0) * 10000 / m.imr_bps as i128;
                    let (h, _) = booked(w.rf(&u), m);
                    let free = (e.margin as i128 + h.eq - h.im).clamp(0, e.margin.max(0) as i128);
                    if e0.base != 0 || e0.quote != 0 {
                        assert!(
                            e.budget as i128
                                <= e0.budget as i128 + (mb(e.margin as i128) - mb(e0.margin as i128)).max(0),
                            "{what}: fund with unsettled fills refilled the budget"
                        );
                    } else if !known("fund-refill") {
                        assert!(e.budget as i128 <= (e0.budget as i128).max(mb(free)),
                        "{what}: fund (amount {amt}) raised the budget {} -> {} above what initial margin leaves free ({}): margin {}, equity {}, IM {}", e0.budget, e.budget, mb(free), e.margin, h.eq, h.im);
                    }
                    if amt > 0 {
                        assert!(
                            h.eq >= h.im && w.rf::<User>(&u).cash >= 0,
                            "{what}: margin locked from an account below initial margin"
                        );
                    }
                }
            }
            15 => {
                let si = (c[1] % 3) as usize;
                let x = &cx.slabs[si];
                w.call(&[(cx.users[x.ui].auth, true, true), (x.slab, false, true)], &[67, c[7] & 1]);
            }
            16 => {
                if let Some(sk) = cx.signer {
                    ts_ms += 1;
                    let book = cx.books[(c[2] as usize) % cx.books.len()];
                    let spx = (pu(px) as i64 + (c[7] % 16) as i64 * if c[7] & 0x10 != 0 { 100 } else { 3 } - 20) as u64;
                    w.accts.insert(
                        sysvar,
                        Acct::new(
                            &SYSVAR_IX,
                            &Address::default(),
                            &sysvar_data(&sk, &market, spx, w.slot.saturating_sub((c[3] >> 6) as u64), ts_ms),
                        ),
                    );
                    w.call(&[(book, false, true), (market, false, false), (sysvar, false, false)], &[75]);
                }
            }
            17 => {
                liq = true;
                taker = Some(u); // liquidate_fast: the side's book, every approved slab (or a wrong set), all shards, optional keeper
                let b = pre_users[ui].pos[0].base;
                let opp = if b < 0 { 1 } else { 0 };
                let book = cx.book(opp);
                let sl = slabs_for(&cx, &w, &book, c[3]);
                let mut d = vec![70u8, sl.len() as u8];
                d.extend_from_slice(&(if c[5] & 2 != 0 { u64::MAX } else { qty }).to_le_bytes());
                let mut mm = vec![(u, false, true), (market, false, false), (ins, false, true), (book, false, true)];
                mm.extend(sl);
                if c[5] & 4 == 0 {
                    mm.extend(all_shards_w(&cx));
                }
                if c[5] & 1 != 0 && ui != 6 {
                    mm.push((cx.users[6].u, false, true));
                }
                w.call(&mm, &d);
            }
            18 => {
                settle_or_fund = true;
                if !cx.slabs[2].joined {
                    join(&mut w, &mut cx, 2);
                    if cx.slabs[2].joined && c[7] & 1 != 0 {
                        for f in cx.books.clone() {
                            fundm(&mut w, &cx, 3, f, 500_000, None);
                        }
                    }
                }
            }
            19 => {
                liq = true;
                let (lq, tg) = (&cx.users[6], &cx.users[ui % 6]);
                let mut mm = vec![
                    (lq.auth, true, true),
                    (lq.u, false, true),
                    (tg.u, false, true),
                    (market, false, false),
                    (ins, false, true),
                ];
                match c[5] & 3 {
                    3 => mm.extend(cx.sh[..2].iter().map(|s| (*s, false, true))),
                    _ => mm.extend(all_shards_w(&cx)),
                }
                w.call(&mm, &le(5, qty));
            }
            20 => {
                liq = true;
                let mut mm = vec![(u, false, true), (market, false, false), (ins, false, true)];
                mm.extend(all_shards_w(&cx));
                if c[5] & 1 != 0 && ui != 6 {
                    mm.push((cx.users[6].u, false, true));
                }
                w.call(&mm, &le(34, qty));
            }
            21 => {
                w.call(&[(market, false, false), (rank, false, true), (u, false, true)], &[25]);
            }
            22 => {
                liq = true;
                let mut mm = vec![(u, false, true), (market, false, true), (ins, false, true), (rank, false, true)];
                mm.extend(cx.users.iter().map(|x| (x.u, false, true)));
                if c[5] & 1 == 0 {
                    mm.extend(all_shards_w(&cx));
                }
                w.call(&mm, &[6]);
            }
            23 => {
                let mut mm = vec![(u, false, true), (market, false, false)];
                mm.extend(all_shards_w(&cx));
                w.call(&mm, &[77]);
            }
            24 => match c[2] % 3 {
                0 => {
                    let s = cx.sh[(c[3] % 4) as usize];
                    let mut mm = vec![(s, false, true)];
                    mm.extend(cx.users.iter().map(|x| (x.u, false, true)));
                    w.call(&mm, &[24]);
                }
                1 => {
                    w.call(&[(u, false, true), (market, false, false)], &[15]);
                }
                _ => {
                    let mut mm = vec![(market, false, true), (ins, false, true)];
                    mm.extend(all_shards_w(&cx));
                    mm.extend(cx.books.iter().map(|f| (*f, false, true)));
                    w.call(&mm, &[16]);
                }
            },
            25 => {
                let mut d = vec![20u8, c[7] % 4, (1 + c[2] % 3) | (c[3] & 0xc0), side];
                for v in [
                    price,
                    if c[3] & 0x20 != 0 {
                        0
                    } else if side == 0 {
                        price + 2000
                    } else {
                        price.saturating_sub(2000)
                    },
                    qty,
                    3,
                    1,
                    2,
                ] {
                    d.extend_from_slice(&v.to_le_bytes());
                }
                w.call(&[(auth, true, true), (u, false, true), (market, false, false)], &d);
            }
            26 => {
                // keeper (user 6) runs a trigger of `ui`: through the side's registered book with every slab, and all shards
                let tr = pre_users[ui].triggers[(c[7] % 4) as usize];
                let opp = if tr.side == 0 { 1 } else { 0 };
                let book = cx.book(opp);
                let (kp, tg) = (&cx.users[6], &cx.users[ui % 6]);
                let mut mm = vec![
                    (kp.auth, true, true),
                    (kp.u, false, true),
                    (tg.u, false, true),
                    (market, false, false),
                    (book, false, true),
                ];
                mm.extend(slabs_for(&cx, &w, &book, c[3]));
                mm.extend(all_shards_w(&cx));
                taker = Some(tg.u);
                w.call(&mm, &[21, c[7] % 4]);
            }
            27 => {
                // ONE signer that is not a full authority: a multisig member, a stranger, or (plain market) anyone but the admin
                let who = match c[1] % 5 {
                    0 => mem[0],
                    1 => mem[1],
                    2 => mem[2],
                    3 => stranger,
                    _ => auth,
                };
                let val = match c[3] % 6 {
                    0 => 0u64,
                    1 => 1,
                    2 => 2,
                    3 => 3,
                    4 => c[4] as u64 * 1000,
                    _ => u64::MAX,
                };
                let key32 = [c[5]; 32];
                let (tag, d): (u8, Vec<u8>) = match c[2] % 6 {
                    0 | 1 => (22, {
                        let mut d = vec![22u8, c[7] % 16];
                        d.extend_from_slice(&val.to_le_bytes());
                        d
                    }),
                    2 => (28, {
                        let mut d = vec![28u8, c[7] % 16];
                        d.extend_from_slice(&val.to_le_bytes());
                        d.extend_from_slice(&key32);
                        d
                    }),
                    3 => (68, vec![68u8, c[7] & 1]),
                    4 => (74, {
                        let mut d = vec![74u8];
                        d.extend_from_slice(&if c[7] & 1 == 0 { [0u8; 32] } else { key32 });
                        d
                    }),
                    _ => (23, vec![23u8, c[7] & 1]),
                };
                let mid: Vec<Meta> = match tag {
                    74 => vec![(market, false, false), (cx.books[0], false, true)],
                    23 => vec![(market, false, false), (u, false, true)],
                    _ => vec![(market, false, true)],
                };
                let mut mm = vec![(who, true, true)];
                mm.extend(mid);
                mm.push((msig, c[6] & 2 != 0, false)); // the multisig account flagged as signer counts for nothing
                let before: Vec<Vec<u8>> = [market, cx.books[0], u].iter().map(|k| w.bytes(k)).collect();
                let ok = w.call(&mm, &d);
                let after: Vec<Vec<u8>> = [market, cx.books[0], u].iter().map(|k| w.bytes(k)).collect();
                if ok && !(plain && who == admin0) {
                    let is_member = cx.pooled && mem.contains(&who);
                    assert!(
                        tag == 22 && d[1] as u64 == spec::ADMIN_PAUSE && val == 1 && is_member,
                        "{what}: admin instruction {tag} op {} succeeded for a single signer that is not the admin",
                        d[1]
                    );
                    let off = core::mem::offset_of!(Market, paused);
                    let mut exp = before[0].clone();
                    if exp[off..off + 8] == 0u64.to_le_bytes() {
                        exp[off..off + 8].copy_from_slice(&1u64.to_le_bytes());
                    } // a stronger pause (breaker) is kept
                    assert!(
                        after[0] == exp && after[1] == before[1] && after[2] == before[2],
                        "{what}: single-member emergency pause changed more than paused = 1"
                    );
                }
            }
            28 => {
                // full authority: immediate admin actions and tiers
                let (tag, mid, d): (u8, Vec<Meta>, Vec<u8>) = match c[2] % 5 {
                    0 => (22, vec![(market, false, true)], {
                        let mut d = vec![22u8, 1];
                        d.extend_from_slice(&((c[7] & 1) as u64).to_le_bytes());
                        d
                    }),
                    1 => (22, vec![(market, false, true)], {
                        let mut d = vec![22u8, 6];
                        d.extend_from_slice(&(if c[7] & 1 == 0 { 0 } else { 20 + c[3] as u64 }).to_le_bytes());
                        d
                    }),
                    2 => (22, vec![(market, false, true)], {
                        let mut d = vec![22u8, 3];
                        d.extend_from_slice(&(50 + c[3] as u64 * 1000).to_le_bytes());
                        d
                    }),
                    3 => (23, vec![(market, false, false), (u, false, true)], vec![23u8, c[7] & 1]),
                    _ => (22, vec![(market, false, true)], {
                        let mut d = vec![22u8, 2];
                        d.extend_from_slice(
                            &(if c[7] & 1 == 0 { 0 } else { vault_amt as u64 + c[3] as u64 * 100_000 }).to_le_bytes(),
                        );
                        d
                    }),
                };
                let _ = tag;
                w.call(&adm(&cx, &mid), &d);
            }
            29 => {
                if fund {
                    match c[2] % 4 {
                        // long idle periods and timelocked changes
                        0 => {
                            w.slot += 9000 * (1 + (c[3] % 3) as u64);
                            w.ts += 60;
                        }
                        1 => {
                            let (opc, val) = match c[3] % 4 {
                                0 => (spec::ADMIN_IMR, 800 + c[4] as u64 * 5),
                                1 => (spec::ADMIN_MMR, 300 + c[4] as u64 * 2),
                                2 => (spec::ADMIN_FUNDING_PERIOD, 50 + c[4] as u64 * 40),
                                _ => (spec::ADMIN_IMPACT_NOTIONAL, 1 + c[4] as u64 * 10_000),
                            };
                            let mut d = vec![28u8, opc as u8];
                            d.extend_from_slice(&val.to_le_bytes());
                            w.call(&adm(&cx, &[(market, false, true)]), &d);
                        }
                        2 => {
                            w.slot += spec::TIMELOCK_SLOTS;
                            w.ts += 60;
                        }
                        _ => {
                            w.call(&[(market, false, true)], &[29]);
                        }
                    }
                }
            }
            _ => {}
        }
        let changed = w.okn > ok0;
        if changed {
            okc[op] += 1;
            let m: &Market = w.rf(&market);
            if m.funding_index != 0 {
                idx_moved = true;
            }
            invariants(&w, &cx, vault_amt, !idx_moved, &what);
            order_rule_checks(
                &w,
                &cx,
                op,
                ui,
                (c[7] % 4) as usize,
                &pre_users,
                &pre_led,
                ro_single,
                repl,
                cancel_all,
                &what,
            );
            let lots = fill_checks(&w, &cx, &pre_books, settle_or_fund, liq, taker, &what);
            if op != 13 && op != 14 && op != 18 {
                for (bi, fk) in cx.books.iter().enumerate() {
                    let f: &FastBook = w.rf(fk);
                    for mi in 0..pre_books[bi].n as usize {
                        assert!(
                            f.makers[mi].budget <= pre_books[bi].makers[mi].budget,
                            "{what}: a maker budget rose outside fund / settle"
                        );
                    }
                }
            }
            // maker fills come only from takes, triggers and liquidate_fast
            if lots > 0 {
                assert!(
                    matches!(op, 11 | 30 | 12 | 17 | 26),
                    "{what}: maker quotes filled by an instruction that is not a take, trigger or liquidate_fast"
                );
            }
            // a cost basis only moves with the position or its cash
            for i in 0..7 {
                let (a, b) = (w.rf::<User>(&cx.users[i].u), &pre_users[i]);
                if a.cash == b.cash
                    && a.pos[0].base == b.pos[0].base
                    && a.pos[0].iso_margin == b.pos[0].iso_margin
                    && a.pos[0].market == b.pos[0].market
                {
                    assert_eq!(a.pos[0].cost, b.pos[0].cost, "{what}: cost basis of user {i} moved without a fill");
                }
            }
            // a liquidation / ADL only ever hits an account that really is below maintenance (everything it holds counted), and only shrinks its position
            if liq && !idx_moved {
                let ti = if op == 19 { ui % 6 } else { ui };
                let (a, b) = (w.rf::<User>(&cx.users[ti].u), &pre_users[ti]);
                if a.pos[0].base != b.pos[0].base || a.cash != b.cash {
                    let (tc, tiso) = pre_true[ti];
                    let h = tiso.unwrap_or(tc);
                    assert!(
                        h.eq < h.mm,
                        "{what}: user {ti} was liquidated / deleveraged with true equity {} >= maintenance {}",
                        h.eq,
                        h.mm
                    );
                    let nb = truth(&w, &cx, ti);
                    let nb = nb.1.unwrap_or(nb.0).b;
                    assert!(
                        nb.abs() <= h.b.abs() && (nb == 0 || (nb > 0) == (h.b > 0)),
                        "{what}: liquidation grew or flipped the position ({} -> {nb})",
                        h.b
                    );
                }
            }
        }
    }
    if std::env::var("NATIVE_STATS").is_ok() {
        eprintln!(
            "ok/total per op: {:?} ro_refused {} modelled {}",
            okc.iter().zip(tot.iter()).enumerate().collect::<Vec<_>>(),
            w.ro_refused,
            modelled
        );
    }
    // settle + claim everything at one index, aggregate, then the exact end-of-run conservation (also after funding moved)
    w.slot += 1;
    w.ts += 1;
    upd(&mut w, &cx, px, (px / 10000) as u64, 0);
    for i in 0..7 {
        let x = &cx.users[i];
        let (a, u) = (x.auth, x.u);
        for f in cx.books.clone() {
            w.call(&[(u, false, true), (market, false, false), (f, false, true)], &[65]);
        }
        let mut d = vec![3u8];
        d.extend_from_slice(&0u16.to_le_bytes());
        d.push(0);
        d.push(0);
        w.call(&trade(&cx, i), &d);
        w.call(&[(u, false, true), (market, false, false)], &[15]);
        let _ = a;
    }
    let mut mm = vec![(market, false, true), (ins, false, true)];
    mm.extend(all_shards_w(&cx));
    mm.extend(cx.books.iter().map(|f| (*f, false, true)));
    w.call(&mm, &[16]);
    let m: &Market = w.rf(&market);
    let settled = (0..7).all(|i| {
        !pending(&w, &cx, i)
            && w.rf::<User>(&cx.users[i].u).pos.iter().all(|p| p.market == [0; 32] || p.funding_snap == m.funding_index)
    });
    if !settled && dbg() {
        eprintln!("final: not every account could be settled (stale / paused): exact conservation skipped");
    }
    if dbg() {
        for i in 0..7 {
            let (t, ti) = truth(&w, &cx, i);
            eprintln!(
                "final user {i}: true equity {} position {} lots, initial requirement {}, maintenance {}{}",
                t.eq,
                t.b,
                t.im,
                t.mm,
                ti.map_or(String::new(), |h| format!(" | isolated: equity {} lots {} IM {}", h.eq, h.b, h.im))
            );
        }
    }
    if m.funding_index != 0 {
        idx_moved = true;
    } // the closing update may be the one that moves the index
    let exact = if idx_moved { settled && !known("funding") } else { true };
    invariants(&w, &cx, vault_amt, exact, "final (everything claimed and settled)");
});
