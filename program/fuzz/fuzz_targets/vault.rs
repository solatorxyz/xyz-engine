#![no_main]
//! Native target for the liquidity vault (program/src/vault.rs), in the world of `world.rs`.
//!
//! One market with one fast book; a vault with three sub-accounts, each a maker on that book through its own slab; three
//! depositors; two takers. The fuzzer deposits and withdraws, moves cash between the vault's accounts, locks and
//! releases quoting margin, writes pegged quotes, trades against them, settles, moves the price, lets time pass, and
//! values the vault whole or a part at a time, with or without every account in hand.
//!
//! The harness keeps its own account of what the vault is worth, from the raw account bytes and nothing else:
//!
//!     worth(sub) = cash + sum(lots x mark) + margin locked in the book        (fills settled)
//!
//! and after every instruction that succeeds it checks (each assert names its rule):
//!
//!   money      every token in the token vault is someone's: cash, locked margin, unsettled fills, fees, insurance
//!   lots       positions and unsettled fills sum to zero
//!   shares     the depositors' records add up to the vault's share count
//!   counted    a sub-account that holds anything is in the vault's value (its `active` bit is set)
//!   parts      a value the program remembers for a sub-account is what the harness computes for it in that slot
//!   whole      the value a deposit / withdrawal / crank used is exactly: live worth of the accounts it was given, plus
//!              the remembered parts of the others, each at most VAULT_VALUE_MAX_AGE_SLOTS old
//!   price      shares are minted and paid at value / shares, rounded against the one who acts: nobody else's share
//!              is worth less afterwards, and the actor gains less than one unit of rounding
//!   moves      cash moved between sub-accounts changes no one's value: the parts still add up to the same
//!
//! Input: 8-byte records. Record 0 is the configuration (byte 0: 1 performance fee 10%, 2 lockup 10 slots, 4 daily
//! withdrawal cap 20%, 8 drawdown stop 25%); every later record is one operation, `op = byte 0 & 15` (see the loop).
use libfuzzer_sys::fuzz_target;
use perp_program::spec;
use perp_program::state::*;
use pinocchio::account::{AccountView, RuntimeAccount};
use solana_address::Address;
use std::collections::HashMap;
use std::sync::OnceLock;
include!("world.rs");

const VV: i128 = spec::VAULT_VIRTUAL as i128;
const MAX_AGE: u64 = spec::VAULT_VALUE_MAX_AGE_SLOTS;
const N_SUBS: usize = 3;
const PX0: i64 = 1000_00000000; // the feed's price; the market sees 100_000 per lot

struct Holder {
    auth: K,
    record: K,
    wallet: K,
}
struct Taker {
    auth: K,
    user: K,
}
struct Cx {
    market: K,
    ins: K,
    tokens: K,
    pda: K,
    tok: K,
    oracle: K,
    feed: [u8; 32],
    book: K,
    admin: K,
    operator: K,
    vault: K,
    subs: [K; N_SUBS],
    slabs: [K; N_SUBS],
    holders: Vec<Holder>,
    takers: Vec<Taker>,
}

fn mark(m: &Market) -> i128 {
    (if m.mark_px != 0 { m.mark_px } else { m.oracle_px }) as i128
}

/// (margin locked, lots not settled yet, quote not settled yet) of a sub-account in the book.
fn in_book(w: &World, cx: &Cx, sub: &K) -> (i128, i128, i128) {
    let f: &FastBook = w.rf(&cx.book);
    f.makers[..f.n as usize]
        .iter()
        .filter(|e| e.user == *sub)
        .fold((0, 0, 0), |a, e| (a.0 + e.margin as i128, a.1 + e.base as i128, a.2 + e.quote as i128))
}

/// The harness's own valuation of one sub-account, from the account bytes.
fn worth(w: &World, cx: &Cx, i: usize) -> i128 {
    let (u, m): (&User, &Market) = (w.rf(&cx.subs[i]), w.rf(&cx.market));
    let lots: i128 = u.pos.iter().filter(|p| p.market != [0; 32]).map(|p| p.base as i128).sum();
    let (margin, open_lots, open_quote) = in_book(w, cx, &cx.subs[i]);
    u.cash as i128 + (lots + open_lots) * mark(m) + margin + open_quote
}

fn holds_anything(w: &World, cx: &Cx, i: usize) -> bool {
    let u: &User = w.rf(&cx.subs[i]);
    let (margin, lots, quote) = in_book(w, cx, &cx.subs[i]);
    u.cash != 0 || u.pos.iter().any(|p| p.market != [0; 32]) || margin != 0 || lots != 0 || quote != 0
}

fn checks(w: &World, cx: &Cx, tokens: i128, what: &str) {
    let (m, ins, f, v): (&Market, &Insurance, &FastBook, &Vault) =
        (w.rf(&cx.market), w.rf(&cx.ins), w.rf(&cx.book), w.rf(&cx.vault));
    let users = cx.takers.iter().map(|t| &t.user).chain(cx.subs.iter());
    let (mut lots, mut money) = (0i128, 0i128);
    for k in users {
        let u: &User = w.rf(k);
        money += u.cash as i128;
        lots += u.pos.iter().filter(|p| p.market != [0; 32]).map(|p| p.base as i128).sum::<i128>();
    }
    for e in &f.makers[..f.n as usize] {
        lots += e.base as i128;
        money += e.margin as i128 + e.quote as i128;
        assert!(e.margin >= 0 && e.budget >= 0, "{what}: negative maker margin or budget");
    }
    assert_eq!(lots, 0, "{what}: lots: positions and unsettled fills do not sum to zero");
    assert!(tokens >= 0 && ins.balance >= 0, "{what}: money: the token vault or the insurance fund went negative");
    // Funding moves cash between longs and shorts with per-account rounding; the identity is exact until it first accrues.
    if m.funding_index == 0 {
        assert_eq!(
            money + f.fee_accum as i128 + ins.balance as i128 + m.fees as i128,
            tokens + m.bad_debt as i128,
            "{what}: money: cash + maker margin + unsettled quote + fees + insurance != tokens + bad debt"
        );
    }
    let recorded: u64 = cx.holders.iter().map(|h| w.rf::<VaultDeposit>(&h.record).shares).sum();
    assert_eq!(recorded, v.shares, "{what}: shares: depositors' records != the vault's share count");
    for h in &cx.holders {
        let d: &VaultDeposit = w.rf(&h.record);
        assert!(d.shares != 0 || d.basis == 0, "{what}: shares: a record with no shares still carries a cost basis");
    }
    assert!(v.active & 1 == 1, "{what}: counted: the cash account dropped out of the vault's value");
    for i in 1..N_SUBS {
        assert!(
            v.active >> i & 1 == 1 || !holds_anything(w, cx, i),
            "{what}: counted: sub-account {i} holds value but is not part of the vault's value"
        );
    }
}

/// What the program should have used as the vault's value, given which accounts it was handed: their live worth, plus
/// the parts it remembered for the others. `before` is the vault as it was; worth is read from the state after.
fn composed(w: &World, cx: &Cx, before: &Vault, passed: u8, slot: u64, what: &str) -> i128 {
    let mut nav = 0i128;
    for i in 0..N_SUBS {
        if before.active >> i & 1 == 0 {
            continue;
        }
        if passed >> i & 1 == 1 {
            nav += worth(w, cx, i);
        } else {
            assert!(
                before.sub_slot[i] != 0 && slot <= before.sub_slot[i] + MAX_AGE,
                "{what}: whole: used a remembered value older than the window for sub-account {i}"
            );
            nav += before.sub_eq[i] as i128;
        }
    }
    nav
}

/// After an instruction that valued the accounts in `passed`: what it remembers for each is the harness's own figure.
fn parts_match(w: &World, cx: &Cx, before: &Vault, passed: u8, slot: u64, what: &str) {
    let v: &Vault = w.rf(&cx.vault);
    for i in 0..N_SUBS {
        if passed >> i & 1 == 1 && before.active >> i & 1 == 1 {
            assert_eq!(v.sub_eq[i] as i128, worth(w, cx, i), "{what}: parts: remembered value of sub-account {i}");
            assert_eq!(v.sub_slot[i], slot, "{what}: parts: sub-account {i} valued but not stamped with this slot");
        }
    }
}

fuzz_target!(|data: &[u8]| {
    if data.len() < 16 {
        return;
    }
    let cfg = data[0];
    let pid = Address::new_from_array([7u8; 32]);
    let mut w = World { pid, accts: HashMap::new(), slot: 1000, ts: 1_800_000_000, n: 0, okn: 0, ro_refused: 0 };

    // ---- a market, its book and its price
    let admin = w.signer();
    let (market, ins) = (w.prog(MARKET_SIZE), w.prog(INS_SIZE));
    let (mint, feed) = (w.key(), [7u8; 32]);
    let oracle = w.add(&PYTH, &pyth(&feed, PX0, 10_000_000, w.ts));
    let tok = kk(&TOKEN);
    w.accts.insert(tok, Acct::new(&TOKEN, &Address::default(), &[]));
    let (p, bump) = Address::find_program_address(&[b"vault", &market[..]], &w.pid);
    let pda = kk(&p);
    w.accts.insert(pda, Acct::new(&ad(&pda), &Address::default(), &[]));
    let tokens_acct = w.add(&TOKEN, &token_acct(&mint, &pda));
    let mut d = vec![10u8, bump];
    d.extend_from_slice(&feed);
    for x in [100u64, 150, 100, 120, (-8i64) as u64, 1, 1_000_000, 0] {
        d.extend_from_slice(&x.to_le_bytes());
    }
    let a = (admin, true, true);
    assert!(w.call(
        &[a, (market, false, true), (ins, false, true), (tokens_acct, false, false), (oracle, false, false)],
        &d
    ));
    let book = w.prog(FAST_SIZE);
    assert!(w.call(&[a, (market, false, true), (book, false, true)], &le(61, 0)));
    assert!(w.call(&[(market, false, true), (oracle, false, false)], &[4]));

    // ---- the vault: three sub-accounts, each with a slab approved on the book; an operator with no quote rules
    let vault = w.prog(VAULT_SIZE);
    let mut d = vec![50u8];
    let (lockup, day_cap, perf, dd) = (
        if cfg & 2 != 0 { 10u64 } else { 0 },
        if cfg & 4 != 0 { 2000u64 } else { 0 },
        if cfg & 1 != 0 { 1000u64 } else { 0 },
        if cfg & 8 != 0 { 2500u64 } else { 0 },
    );
    for x in [lockup, day_cap, perf, dd] {
        d.extend_from_slice(&x.to_le_bytes());
    }
    assert!(w.call(&[a, (vault, false, true), (market, false, false)], &d));
    let operator = w.signer();
    let mut d = vec![82u8];
    d.extend_from_slice(&operator);
    d.extend_from_slice(&[0u8; 24]);
    assert!(w.call(&[a, (vault, false, true)], &d));
    let (mut subs, mut slabs) = ([[0u8; 32]; N_SUBS], [[0u8; 32]; N_SUBS]);
    for i in 0..N_SUBS {
        subs[i] = w.prog(USER_SIZE);
        slabs[i] = w.prog(SLAB_SIZE);
        assert!(w.call(&[a, (vault, false, true), (subs[i], false, true)], &[51]));
        assert!(w.call(&[a, (vault, false, true), (subs[i], false, false), (slabs[i], false, true)], &[58]));
        let join = [a, (market, false, false), (book, false, true), (slabs[i], false, true), (vault, false, false)];
        assert!(w.call(&join, &[59]));
    }
    let mut cx = Cx {
        market,
        ins,
        tokens: tokens_acct,
        pda,
        tok,
        oracle,
        feed,
        book,
        admin,
        operator,
        vault,
        subs,
        slabs,
        holders: vec![],
        takers: vec![],
    };

    // ---- people: three depositors with wallets, two takers with collateral
    let mut tokens: i128 = 0;
    for _ in 0..3 {
        let auth = w.signer();
        let (record, wallet) = (w.prog(VDEP_SIZE), w.add(&TOKEN, &token_acct(&mint, &auth)));
        cx.holders.push(Holder { auth, record, wallet });
    }
    for _ in 0..2 {
        let auth = w.signer();
        let user = w.prog(USER_SIZE);
        assert!(w.call(&[(auth, true, true), (user, false, true)], &[12]));
        let wallet = w.add(&TOKEN, &token_acct(&mint, &auth));
        let dep = [
            (auth, true, true),
            (user, false, true),
            (market, false, false),
            (cx.tokens, false, true),
            (wallet, false, true),
            (tok, false, false),
        ];
        assert!(w.call(&dep, &le(13, 100_000_000_000)));
        tokens += 100_000_000_000;
        set_vault(&mut w, &cx.tokens, tokens);
        cx.takers.push(Taker { auth, user });
    }
    checks(&w, &cx, tokens, "setup");

    // Accounts an instruction needs in order to value the sub-accounts in `mask` (bit i = subs[i]); subs[0] is always
    // in the instruction's own list.
    let rest = |cx: &Cx, mask: u8| -> Vec<Meta> {
        let mut v = vec![(cx.book, false, false)];
        v.extend((1..N_SUBS).filter(|i| mask >> i & 1 == 1).map(|i| (cx.subs[i], false, true)));
        v
    };
    let crank = |w: &mut World, cx: &Cx, mask: u8| -> bool {
        let mut m = vec![(cx.vault, false, true), (cx.subs[0], false, true), (cx.market, false, false)];
        m.extend(rest(cx, mask));
        w.call(&m, &[56])
    };
    let mut px = PX0;

    for (step, c) in data[8..].chunks_exact(8).enumerate() {
        let what = format!("step {step} op {}", c[0] & 15);
        let slot = w.slot;
        let before: Vault = w.cp(&cx.vault);
        match c[0] & 15 {
            // deposit: holder, amount, which sub-accounts are handed over live; bit 7 of c[3]: a crank first, so the
            // value the deposit must use is known exactly
            0 | 13 => {
                let h = &cx.holders[c[1] as usize % 3];
                let amt = 1 + c[2] as u64 * 40_000_000;
                let mask = if c[0] & 15 == 13 { 0xff } else { c[3] } | 1;
                let known = if c[3] & 0x80 != 0 && crank(&mut w, &cx, mask) {
                    parts_match(&w, &cx, &before, mask, slot, &what);
                    Some(w.rf::<Vault>(&cx.vault).nav as i128)
                } else {
                    None
                };
                let before: Vault = w.cp(&cx.vault);
                let mut m = vec![
                    (h.auth, true, true),
                    (cx.vault, false, true),
                    (h.record, false, true),
                    (cx.subs[0], false, true),
                    (cx.market, false, false),
                    (cx.tokens, false, true),
                    (h.wallet, false, true),
                    (cx.tok, false, false),
                ];
                m.extend(rest(&cx, mask));
                if w.call(&m, &le(53, amt)) {
                    tokens += amt as i128;
                    set_vault(&mut w, &cx.tokens, tokens);
                    let v: &Vault = w.rf(&cx.vault);
                    let nav = v.nav as i128 - amt as i128;
                    assert_eq!(v.nav as i128, composed(&w, &cx, &before, mask, slot, &what), "{what}: whole: deposit");
                    if let Some(k) = known {
                        assert_eq!(nav, k, "{what}: whole: a deposit right after a crank used another value");
                    }
                    parts_match(&w, &cx, &before, mask, slot, &what);
                    let (s0, s1) = (before.shares as i128 + VV, v.shares as i128 + VV);
                    let minted = (v.shares - before.shares) as i128;
                    assert_eq!(minted, amt as i128 * s0 / (nav + VV), "{what}: price: shares minted");
                    let gain = (nav + amt as i128 + VV) * s0 - (nav + VV) * s1;
                    assert!((0..nav + VV).contains(&gain), "{what}: price: a deposit diluted the other holders");
                }
            }
            // withdraw: holder, share of its shares, which sub-accounts are live. A crank in the same slot with the same
            // accounts comes first: it is permissionless, changes nothing a withdrawal reads, and tells the harness the
            // value the withdrawal has to pay at.
            1 | 14 => {
                let h = &cx.holders[c[1] as usize % 3];
                let rec: VaultDeposit = w.cp(&h.record);
                let sh = (rec.shares as u128 * (1 + c[2] as u128) / 256) as u64;
                let mask = if c[0] & 15 == 14 { 0xff } else { c[3] } | 1;
                let cranked = crank(&mut w, &cx, mask);
                let before: Vault = w.cp(&cx.vault);
                let fees0 = w.rf::<Market>(&cx.market).fees as i128;
                let mut m = vec![
                    (h.auth, true, true),
                    (cx.vault, false, true),
                    (h.record, false, true),
                    (cx.subs[0], false, true),
                    (cx.market, false, true),
                    (cx.tokens, false, true),
                    (h.wallet, false, true),
                    (cx.tok, false, false),
                    (cx.pda, false, false),
                ];
                m.extend(rest(&cx, mask));
                if w.call(&m, &le(54, sh)) {
                    assert!(
                        cranked,
                        "{what}: whole: a withdrawal was valued where a crank with the same accounts could not be"
                    );
                    let (v, mk): (&Vault, &Market) = (w.rf(&cx.vault), w.rf(&cx.market));
                    let nav = before.nav as i128;
                    let (s0, s1) = (before.shares as i128 + VV, v.shares as i128 + VV);
                    let gross = sh as i128 * (nav + VV) / s0;
                    let cost = rec.basis as i128 * sh as i128 / rec.shares as i128;
                    let fee = if gross > cost { (gross - cost) * perf as i128 / 10000 } else { 0 };
                    assert_eq!(v.nav as i128, nav - gross, "{what}: price: value paid for the shares");
                    assert_eq!(mk.fees as i128 - fees0, fee, "{what}: price: performance fee");
                    assert_eq!(
                        v.nav as i128,
                        composed(&w, &cx, &before, mask, slot, &what),
                        "{what}: whole: withdrawal"
                    );
                    let gain = (nav - gross + VV) * s0 - (nav + VV) * s1;
                    assert!((0..s0).contains(&gain), "{what}: price: a withdrawal took value from the other holders");
                    assert!(slot >= rec.unlock_slot, "{what}: a locked deposit was withdrawn");
                    tokens -= gross - fee;
                    set_vault(&mut w, &cx.tokens, tokens);
                }
            }
            // value one sub-account
            2 => {
                let i = c[1] as usize % N_SUBS;
                let m = [
                    (cx.vault, false, true),
                    (cx.subs[i], false, true),
                    (cx.market, false, false),
                    (cx.book, false, false),
                ];
                if w.call(&m, &[84]) {
                    let (_, lots, quote) = in_book(&w, &cx, &cx.subs[i]);
                    assert!(lots == 0 && quote == 0, "{what}: parts: valued a sub-account whose fills are not settled");
                    let v: &Vault = w.rf(&cx.vault);
                    assert_eq!(v.sub_eq[i] as i128, worth(&w, &cx, i), "{what}: parts: vault_value");
                    assert_eq!(v.sub_slot[i], slot, "{what}: parts: vault_value did not stamp the slot");
                }
            }
            // move idle cash between two sub-accounts, as the operator or the admin
            3 => {
                let (from, to) = (c[1] as usize % N_SUBS, c[2] as usize % N_SUBS);
                let amt = c[3] as u64 * 30_000_000;
                let by = if c[4] & 1 == 0 { cx.operator } else { cx.admin };
                let m = [
                    (by, true, true),
                    (cx.vault, false, true),
                    (cx.subs[from], false, true),
                    (cx.subs[to], false, true),
                    (cx.market, false, false),
                    (cx.book, false, false),
                ];
                if w.call(&m, &le(83, amt)) {
                    let v: &Vault = w.rf(&cx.vault);
                    let sum = |x: &Vault| (0..N_SUBS).map(|i| x.sub_eq[i] as i128).sum::<i128>();
                    assert_eq!(
                        sum(v),
                        sum(&before),
                        "{what}: moves: the remembered parts no longer add up to the same"
                    );
                    assert!(v.active >> to & 1 == 1, "{what}: counted: cash arrived in an account that is not counted");
                }
            }
            // lock (admin) or release (operator) quoting margin of a sub-account; optionally set the auto-target
            4 => {
                let i = c[1] as usize % N_SUBS;
                let amt = (c[2] as i8) as i64 * 25_000_000;
                let by = if amt > 0 || c[4] & 1 != 0 { cx.admin } else { cx.operator };
                let mut d = vec![80u8];
                d.extend_from_slice(&amt.to_le_bytes());
                if c[3] & 1 != 0 {
                    d.extend_from_slice(&(c[5] as i64 * 25_000_000).to_le_bytes());
                }
                let m = [
                    (by, true, true),
                    (cx.subs[i], false, true),
                    (cx.market, false, false),
                    (cx.book, false, true),
                    (cx.vault, false, true),
                ];
                w.call(&m, &d);
            }
            // the operator quotes a sub-account's slab: pegged ladders, one or two levels a side
            5 => {
                let i = c[1] as usize % N_SUBS;
                let mut d = vec![79u8];
                qitem(&mut d, 0xC000, 0, 1);
                qitem(&mut d, 0xC000 | 1, 0, 1);
                let peg = |off: i64| (off + (1i64 << 31)) as u32;
                let (bid_off, ask_off) = (c[2] as i64 % 64, c[3] as i64 % 64);
                let qty = c[4] as u32 * 500;
                qitem(&mut d, 0, peg(-bid_off), qty);
                qitem(&mut d, 8, peg(ask_off), qty);
                if c[5] & 1 != 0 {
                    qitem(&mut d, 1, peg(-bid_off - 20), qty * 2);
                    qitem(&mut d, 9, peg(ask_off + 20), qty * 2);
                }
                w.call(&[(cx.operator, true, true), (cx.vault, false, false), (cx.slabs[i], false, true)], &d);
            }
            // a taker trades against every slab
            6 | 11 => {
                let t = &cx.takers[c[1] as usize % 2];
                let side = c[2] & 1;
                let limit = if side == 0 { u32::MAX as u64 } else { 1 };
                let qty = 1 + c[3] as u64 * 300;
                let mut d = vec![66u8, side, 0, N_SUBS as u8];
                d.extend_from_slice(&limit.to_le_bytes());
                d.extend_from_slice(&qty.to_le_bytes());
                let mut m = vec![
                    (t.auth, true, true),
                    (t.user, false, true),
                    (cx.market, false, false),
                    (cx.book, false, true),
                ];
                m.extend(cx.slabs.iter().map(|s| (*s, false, false)));
                w.call(&m, &d);
            }
            // settle a sub-account's fills into it
            7 => {
                let i = c[1] as usize % N_SUBS;
                w.call(&[(cx.subs[i], false, true), (cx.market, false, false), (cx.book, false, true)], &[65]);
            }
            // the price moves (at most 2% a step, inside the circuit breaker)
            8 => {
                px = (px + px * (c[1] as i8) as i64 / 6400).clamp(PX0 / 4, PX0 * 4);
                let t = w.ts;
                w.accts.insert(cx.oracle, Acct::new(&ad(&cx.oracle), &PYTH, &pyth(&cx.feed, px, 10_000_000, t)));
                let mut m = vec![(cx.market, false, true), (cx.oracle, false, false)];
                if c[2] & 1 != 0 {
                    m.push((cx.book, false, false));
                    m.extend(cx.slabs.iter().map(|s| (*s, false, false)));
                }
                w.call(&m, &[4]);
            }
            // time passes: usually a few slots, sometimes past the window in which a remembered part may be used
            9 => {
                let n = if c[2] & 3 == 0 { c[1] as u64 } else { 1 + c[1] as u64 % 12 };
                w.slot += n;
                w.ts += (n * 4 / 10) as i64;
            }
            // crank the vault's value with the chosen accounts in hand
            10 => {
                let mask = c[1] | 1;
                if crank(&mut w, &cx, mask) {
                    let v: &Vault = w.rf(&cx.vault);
                    assert_eq!(v.nav as i128, composed(&w, &cx, &before, mask, slot, &what), "{what}: whole: crank");
                    parts_match(&w, &cx, &before, mask, slot, &what);
                }
            }
            // settle and value every part in this slot, then the two ways of valuing the whole must agree with each
            // other and with the harness
            12 => {
                let mut all = true;
                for i in 0..N_SUBS {
                    w.call(&[(cx.subs[i], false, true), (cx.market, false, false), (cx.book, false, true)], &[65]);
                    let m = [
                        (cx.vault, false, true),
                        (cx.subs[i], false, true),
                        (cx.market, false, false),
                        (cx.book, false, false),
                    ];
                    all &= w.call(&m, &[84]);
                }
                if all {
                    let active = w.rf::<Vault>(&cx.vault).active;
                    let mine: i128 = (0..N_SUBS).filter(|i| active >> i & 1 == 1).map(|i| worth(&w, &cx, i)).sum();
                    assert!(
                        crank(&mut w, &cx, 1),
                        "{what}: whole: every part is fresh but the vault cannot be valued from them"
                    );
                    assert_eq!(
                        w.rf::<Vault>(&cx.vault).nav as i128,
                        mine,
                        "{what}: whole: sum of the parts != the harness's value"
                    );
                    assert!(crank(&mut w, &cx, 0xff), "{what}: whole: valued from parts but not from the accounts");
                    assert_eq!(
                        w.rf::<Vault>(&cx.vault).nav as i128,
                        mine,
                        "{what}: whole: live value != sum of the parts in the same slot"
                    );
                }
            }
            // the operator stops or resumes a slab's quotes
            _ => {
                let i = c[1] as usize % N_SUBS;
                w.call(
                    &[(cx.operator, true, true), (cx.vault, false, false), (cx.slabs[i], false, true)],
                    &[81, c[2] & 1],
                );
            }
        }
        checks(&w, &cx, tokens, &what);
    }
    if dbg() {
        eprintln!("ok instructions: {}  refused for writing a read-only account: {}", w.okn, w.ro_refused);
    }
});
