#[path = "../../program/src/state.rs"]
#[allow(dead_code)]
mod state;
// Order-book behaviour a trader or a market maker leans on without reading the code: what a reduce-only order may cost, what a
// replace does when the market is paused, what a stop does after a partial fill or after its position is gone.
include!("common.rs");

const E_MARGIN: u32 = 3; const E_STALE: u32 = 4; const E_BAD_DATA: u32 = 2; const E_MISSING: u32 = 19;
const POST: u8 = 1; const REDUCE: u8 = 2; const IOC: u8 = 4; const IF_CANCELLED: u8 = 128;
const PX: i64 = 1000_00000000; // oracle 1000.00 = book price 100_000

fn try_send(e: &mut Env, accounts: Vec<AccountMeta>, data: Vec<u8>, signers: &[&Keypair]) -> Option<u32> {
    let ix = Instruction { program_id: e.pid, accounts, data };
    let mut s: Vec<&Keypair> = vec![&e.admin]; s.extend_from_slice(signers);
    let tx = Transaction::new_signed_with_payer(&[ix], Some(&e.admin.pubkey()), &s, e.svm.latest_blockhash());
    let r = e.svm.send_transaction(tx); e.svm.expire_blockhash();
    match r { Ok(_) => None, Err(f) => { let s = format!("{:?}", f.err);
        Some(s.split("Custom(").nth(1).and_then(|x| x.split(')').next()).and_then(|x| x.parse().ok()).unwrap_or(u32::MAX)) } }
}
fn single(item: Vec<u8>) -> Vec<u8> { let mut d = vec![1u8]; d.extend_from_slice(&item); d }
fn place(e: &mut Env, kp: &Keypair, u: &Address, sh: &[Address], side: u8, fl: u8, px: u64, q: u64) -> Option<u32> {
    let m = e.market; try_send(e, trade_accts(kp, u, &m, sh), single(place_item(side, fl, px, q, 0, 0)), &[kp])
}
fn batch(e: &mut Env, kp: &Keypair, u: &Address, sh: &[Address], c: &[Vec<u8>], p: &[Vec<u8>]) -> Option<u32> {
    let m = e.market; try_send(e, trade_accts(kp, u, &m, sh), batch_data(c, p), &[kp])
}
fn trig(e: &mut Env, kp: &Keypair, u: &Address, slot: u8, kind: u8, side: u8, vals: [u64; 6]) {
    let m = e.market; let mut d = vec![20u8, slot, kind, side]; for v in vals { d.extend_from_slice(&v.to_le_bytes()); }
    assert_eq!(try_send(e, vec![AccountMeta::new(kp.pubkey(), true), w(u), r(&m)], d, &[kp]), None, "set_trigger");
}
fn exec(e: &mut Env, kp: &Keypair, ku: &Address, target: &Address, slot: u8, sh: &[Address]) -> Option<u32> {
    let m = e.market; let mut v = vec![AccountMeta::new(kp.pubkey(), true), w(ku), w(target), r(&m)]; v.extend(sh.iter().map(w));
    try_send(e, v, vec![21, slot], &[kp])
}
fn tick(e: &mut Env, px: i64) { e.set_time(e.slot + 1); e.oracle_update("", px, None); }
/// `v` long 10 lots from a maker ask at 100_000. Returns (maker, maker user, victim, victim user, ask shard).
fn long10(e: &mut Env, dep: u64) -> (Keypair, Address, Keypair, Address, Address) {
    e.oracle_update("", PX, None);
    let a0 = e.shard(1, 100_000);
    let (mm, mmu) = e.user(100_000_000_000); let (v, vu) = e.user(dep);
    assert_eq!(place(e, &mm, &mmu, &[a0], 1, POST, 100_000, 10), None);
    assert_eq!(place(e, &v, &vu, &[a0], 0, IOC, 100_000, 10), None);
    assert_eq!(e.user_state(&vu).pos[0].base, 10);
    (mm, mmu, v, vu, a0)
}

// A reduce-only order skipped every margin check: sold far under the mark into a chosen bid it moved the account's whole
// equity, and more, to that bid -- a deficit the insurance fund pays and the other account withdraws.
#[test] fn reduce_only_cannot_leave_bad_debt() {
    let mut e = Env::new(); let (_mm, _mmu, v, vu, _) = long10(&mut e, 150_000);
    let low = e.shard(0, 50_000); let (acc, accu) = e.user(10_000_000_000);
    assert_eq!(place(&mut e, &acc, &accu, &[low], 0, POST, 50_000, 10), None); // the accomplice's bid, 50% under the mark
    // long 10 at 100_000 with ~150,000 of equity: selling at 50_000 loses 500,000
    assert_eq!(place(&mut e, &v, &vu, &[low], 1, IOC | REDUCE, 50_000, 10), Some(E_MARGIN), "reduce-only below zero equity");
    assert_eq!(e.user_state(&vu).pos[0].base, 10);
    // an honest close near the mark still works, with no fresh-margin requirement
    let near = e.shard(0, 99_500);
    assert_eq!(place(&mut e, &acc, &accu, &[near], 0, POST, 99_500, 10), None);
    assert_eq!(place(&mut e, &v, &vu, &[near], 1, IOC | REDUCE, 99_000, 10), None);
    let s = e.user_state(&vu); assert_eq!(s.pos[0].base, 0); assert!(s.cash > 0);
}

// A maker's cancel-replace during a stale oracle (or a pause) used to revert as a whole: the re-quote it could not place kept
// the quotes it wanted pulled on the book.
#[test] fn replace_under_a_stale_oracle_still_cancels() {
    let mut e = Env::new(); e.oracle_update("", PX, None);
    let b0 = e.shard(0, 99_500); let (mm, mmu) = e.user(10_000_000_000);
    assert_eq!(batch(&mut e, &mm, &mmu, &[b0], &[], &[place_ex(0, POST, 0, 99_500, 10, 0, 0, 7, 0, 0, 0)]), None);
    assert_eq!(e.shard_state(&b0).n, 1);
    e.set_time(e.slot + 50); // oracle now stale
    assert_eq!(place(&mut e, &mm, &mmu, &[b0], 0, POST, 99_510, 10), Some(E_STALE), "a lone place still reports it");
    assert_eq!(batch(&mut e, &mm, &mmu, &[b0], &[cancel_item(7, 128, 0)], &[place_item(0, POST, 99_510, 10, 0, 0)]), None);
    assert_eq!(e.shard_state(&b0).n, 0, "the cancel landed, the re-quote was skipped");
    let p = e.user_state(&mmu); assert!(p.pos.iter().all(|p| p.bid_open == 0));
}

// Post-only was judged against orders[0] even when that order could no longer trade: an expired ask at the top made every
// post-only bid at or above it vanish silently.
#[test] fn post_only_is_not_blocked_by_an_expired_order() {
    let mut e = Env::new(); e.oracle_update("", PX, None);
    let (a0, b0) = (e.shard(1, 100_000), e.shard(0, 100_000));
    let (mm, mmu) = e.user(10_000_000_000); let (o, ou) = e.user(10_000_000_000); let m = e.market;
    let exp = e.slot + 1;
    assert_eq!(try_send(&mut e, trade_accts(&o, &ou, &m, &[a0]), single(place_item(1, POST, 100_000, 5, exp, 0)), &[&o]), None);
    assert_eq!(place(&mut e, &o, &ou, &[a0], 1, POST, 100_050, 5), None); // the live ask
    tick(&mut e, PX); tick(&mut e, PX);
    assert_eq!(place(&mut e, &mm, &mmu, &[b0, a0], 0, POST, 100_020, 5), None);
    assert_eq!(e.shard_state(&b0).n, 1, "rests: the only ask under it is dead");
    assert_eq!(place(&mut e, &mm, &mmu, &[b0, a0], 0, POST, 100_050, 5), None);
    assert_eq!(e.shard_state(&b0).n, 1, "a bid at the live ask is still skipped");
    // slide lands one tick under the live ask, not under the dead one
    assert_eq!(place(&mut e, &mm, &mmu, &[b0, a0], 0, POST | 16, 100_090, 5), None);
    let s = e.shard_state(&b0); assert_eq!(s.n, 2); assert_eq!(s.orders[0].price, 100_049);
}

#[test] fn replace_only_requotes_what_was_still_resting() {
    let mut e = Env::new(); e.oracle_update("", PX, None);
    let b0 = e.shard(0, 99_500); let (mm, mmu) = e.user(10_000_000_000); let (t, tu) = e.user(10_000_000_000);
    let q = |px: u64, cid: u64, fl: u8| place_ex(0, fl, 0, px, 10, 0, 0, cid, 0, 0, 0);
    assert_eq!(batch(&mut e, &mm, &mmu, &[b0], &[], &[q(99_550, 7, POST)]), None);
    assert_eq!(place(&mut e, &t, &tu, &[b0], 1, IOC, 99_550, 10), None); // the quote is hit in full
    // the maker's replace arrives after the fill: without the flag it would now carry twice the intended size
    assert_eq!(batch(&mut e, &mm, &mmu, &[b0], &[cancel_item(7, 128, 0)], &[q(99_540, 8, POST | IF_CANCELLED)]), None);
    assert_eq!(e.shard_state(&b0).n, 0, "nothing was cancelled, nothing is placed");
    assert_eq!(batch(&mut e, &mm, &mmu, &[b0], &[], &[q(99_540, 8, POST)]), None);
    assert_eq!(batch(&mut e, &mm, &mmu, &[b0], &[cancel_item(8, 128, 0)], &[q(99_530, 9, POST | IF_CANCELLED)]), None);
    let s = e.shard_state(&b0); assert_eq!(s.n, 1); assert_eq!(s.orders[0].price, 99_530);
    // the flag with no cancel at that index never places
    assert_eq!(batch(&mut e, &mm, &mmu, &[b0], &[], &[q(99_520, 10, POST | IF_CANCELLED)]), None);
    assert_eq!(e.shard_state(&b0).n, 1);
}

#[test] fn cancel_all_clears_both_sides() {
    let mut e = Env::new(); e.oracle_update("", PX, None);
    let (a0, b0) = (e.shard(1, 100_500), e.shard(0, 99_500));
    let (mm, mmu) = e.user(10_000_000_000); let (o, ou) = e.user(10_000_000_000);
    let it = |s: u8, px: u64| place_item(s, POST, px, 10, 0, 0);
    assert_eq!(batch(&mut e, &mm, &mmu, &[a0, b0], &[], &[it(0, 99_550), it(0, 99_540), it(1, 100_500)]), None);
    assert_eq!(place(&mut e, &o, &ou, &[b0], 0, POST, 99_545, 3), None); // someone else's order stays
    assert_eq!(batch(&mut e, &mm, &mmu, &[a0, b0], &[cancel_item(0, 64, 0)], &[]), None);
    assert_eq!(e.shard_state(&a0).n, 0); assert_eq!(e.shard_state(&b0).n, 1);
    let s = e.user_state(&mmu); assert!(s.pos.iter().all(|p| p.bid_open == 0 && p.ask_open == 0 && p.n_orders == 0));
}

#[test] fn post_only_reduce_only_is_refused() {
    let mut e = Env::new(); e.oracle_update("", PX, None);
    let b0 = e.shard(0, 99_000); let (mm, mmu) = e.user(10_000_000_000);
    assert_eq!(place(&mut e, &mm, &mmu, &[b0], 0, POST | REDUCE, 99_000, 10), Some(E_BAD_DATA));
}

// A stop was spent by its first lot: 4 of 10 filled and the other 6 were left with no protection.
#[test] fn stop_survives_a_partial_fill_and_pays_one_tip() {
    let mut e = Env::new(); let (mm, mmu, v, vu, _) = long10(&mut e, 1_000_000);
    let b0 = e.shard(0, 99_000); let (kp, ku) = e.user(1_000);
    assert_eq!(place(&mut e, &mm, &mmu, &[b0], 0, POST, 99_000, 4), None);
    trig(&mut e, &v, &vu, 0, 2, 1, [99_500, 98_000, 10, 1_000, 0, 0]); // SL: sell 10 at oracle <= 99_500, limit 98_000, tip 1000
    tick(&mut e, 995_00000000);
    let k0 = e.user_state(&ku).cash;
    assert_eq!(exec(&mut e, &kp, &ku, &vu, 0, &[b0]), None);
    let s = e.user_state(&vu); assert_eq!(s.pos[0].base, 6);
    assert_ne!(s.triggers[0].kind, 0, "the stop still protects the 6 lots left"); assert_eq!(s.triggers[0].qty, 6);
    assert_eq!(e.user_state(&ku).cash - k0, 400, "4/10 of the tip");
    assert_eq!(place(&mut e, &mm, &mmu, &[b0], 0, POST, 99_000, 20), None);
    assert_eq!(exec(&mut e, &kp, &ku, &vu, 0, &[b0]), None);
    let s = e.user_state(&vu); assert!(s.pos.iter().all(|p| p.base == 0)); assert_eq!(s.triggers[0].kind, 0);
    assert_eq!(e.user_state(&ku).cash - k0, 1_000, "the whole tip, once");
}

// TP and SL on one position: the TP closed it and the SL stayed armed, ready to close whatever the account opened next.
#[test] fn stops_die_with_the_position() {
    let mut e = Env::new(); let (mm, mmu, v, vu, a0) = long10(&mut e, 1_000_000);
    let b0 = e.shard(0, 99_900);
    trig(&mut e, &v, &vu, 0, 1, 1, [110_000, 109_000, 10, 0, 0, 0]); // TP
    trig(&mut e, &v, &vu, 1, 2, 1, [95_000, 94_000, 10, 0, 0, 0]);   // SL
    assert_eq!(place(&mut e, &mm, &mmu, &[b0], 0, POST, 99_900, 10), None);
    assert_eq!(place(&mut e, &v, &vu, &[b0], 1, IOC | REDUCE, 99_900, 10), None); // closed by hand
    let s = e.user_state(&vu); assert_eq!(s.triggers[0].kind, 0); assert_eq!(s.triggers[1].kind, 0);
    // a stop set ahead of the entry waits for the position, then dies with it
    trig(&mut e, &v, &vu, 0, 2, 1, [95_000, 94_000, 10, 0, 0, 0]);
    assert_eq!(place(&mut e, &mm, &mmu, &[a0], 1, POST, 100_000, 10), None);
    assert_eq!(place(&mut e, &v, &vu, &[a0], 0, IOC, 100_000, 10), None);
    let s = e.user_state(&vu); assert_ne!(s.triggers[0].kind, 0, "armed by the entry");
    assert_eq!(place(&mut e, &mm, &mmu, &[b0], 0, POST, 99_900, 10), None);
    assert_eq!(place(&mut e, &v, &vu, &[b0], 1, IOC | REDUCE, 99_900, 10), None);
    assert_eq!(e.user_state(&vu).triggers[0].kind, 0);
}

// The reduce cap is read off the position. With a resting ask already filled but not yet claimed, a keeper that left the ask
// shard out made the stop sell the position a second time.
#[test] fn stop_needs_the_accounts_own_shards() {
    let mut e = Env::new(); let (mm, mmu, v, vu, a0) = long10(&mut e, 5_000_000);
    let b0 = e.shard(0, 99_000); let (kp, ku) = e.user(1_000);
    assert_eq!(place(&mut e, &v, &vu, &[a0], 1, POST, 100_090, 10), None);   // victim's own resting ask
    assert_eq!(place(&mut e, &mm, &mmu, &[a0], 0, IOC, 100_090, 10), None);  // filled, unclaimed: the victim is in fact flat
    assert_eq!(place(&mut e, &mm, &mmu, &[b0], 0, POST, 99_000, 10), None);
    trig(&mut e, &v, &vu, 0, 2, 1, [99_500, 98_000, 10, 0, 0, 0]);
    tick(&mut e, 995_00000000);
    assert_eq!(exec(&mut e, &kp, &ku, &vu, 0, &[b0]), Some(E_MISSING));
    assert_eq!(exec(&mut e, &kp, &ku, &vu, 0, &[b0, a0]), None, "with the full view there is nothing to reduce: cleared");
    let s = e.user_state(&vu); assert!(s.pos.iter().all(|p| p.base == 0), "not flipped short"); assert_eq!(s.triggers[0].kind, 0);
}

#[test] fn stop_with_no_limit_is_a_bounded_market_order() {
    let mut e = Env::new(); e.oracle_update("", PX, None);
    let (a0, b0) = (e.shard(1, 100_000), e.shard(0, 100_000));
    let (mm, mmu) = e.user(100_000_000_000); let (v, vu) = e.user(5_000_000); let (kp, ku) = e.user(1_000);
    assert_eq!(place(&mut e, &mm, &mmu, &[b0], 0, POST, 100_000, 10), None);
    assert_eq!(place(&mut e, &v, &vu, &[b0], 1, IOC, 100_000, 10), None); // short 10
    trig(&mut e, &v, &vu, 0, 2, 0, [100_400, 0, 10, 0, 0, 0]); // buy stop, no limit: used to be unfillable
    let far = e.shard(1, 120_000); let a1 = e.shard(1, 100_600);
    assert_eq!(place(&mut e, &mm, &mmu, &[far], 1, POST, 120_000, 10), None); // 20% away: outside the bound
    tick(&mut e, 1005_00000000);
    assert!(exec(&mut e, &kp, &ku, &vu, 0, &[a0, a1, far]).is_some(), "nothing inside mark + 10%");
    assert_eq!(place(&mut e, &mm, &mmu, &[a1], 1, POST, 100_600, 10), None);
    assert_eq!(exec(&mut e, &kp, &ku, &vu, 0, &[a0, a1, far]), None);
    assert!(e.user_state(&vu).pos.iter().all(|p| p.base == 0));
    assert_eq!(e.shard_state(&far).n, 1);
}

// A TWAP slice that met a thin book dropped what it missed, and one with fewer lots than slices never traded at all.
#[test] fn twap_carries_its_shortfall() {
    let mut e = Env::new(); e.oracle_update("", PX, None);
    let b0 = e.shard(0, 99_900);
    let (mm, mmu) = e.user(100_000_000_000); let (v, vu) = e.user(50_000_000); let (kp, ku) = e.user(1_000);
    assert_eq!(place(&mut e, &mm, &mmu, &[b0], 0, POST, 99_900, 4), None);
    trig(&mut e, &v, &vu, 0, 3, 1, [0, 99_000, 30, 0, 2, 3]); // sell 30 in 3 slices
    assert_eq!(exec(&mut e, &kp, &ku, &vu, 0, &[b0]), None);
    let t = e.user_state(&vu).triggers[0]; assert_eq!((t.qty, t.slices_left), (26, 2), "slice of 10 found 4: 26 left");
    assert_eq!(place(&mut e, &mm, &mmu, &[b0], 0, POST, 99_900, 100), None);
    e.set_time(e.slot + 2); e.oracle_update("", PX, None);
    assert_eq!(exec(&mut e, &kp, &ku, &vu, 0, &[b0]), None);
    let t = e.user_state(&vu).triggers[0]; assert_eq!((t.qty, t.slices_left), (13, 1));
    e.set_time(e.slot + 2); e.oracle_update("", PX, None);
    assert_eq!(exec(&mut e, &kp, &ku, &vu, 0, &[b0]), None);
    let s = e.user_state(&vu); assert_eq!(s.triggers[0].kind, 0); assert_eq!(s.pos[0].base, -30);
    // 2 lots over 5 slices: one lot a slice until done
    trig(&mut e, &v, &vu, 1, 3, 1, [0, 99_000, 2, 0, 2, 5]);
    for _ in 0..2 { e.set_time(e.slot + 2); e.oracle_update("", PX, None); assert_eq!(exec(&mut e, &kp, &ku, &vu, 1, &[b0]), None); }
    let s = e.user_state(&vu); assert_eq!(s.triggers[1].kind, 0); assert_eq!(s.pos[0].base, -32);
}

// Found by the fuzzer (seed funding-resting-10). A resting order filled long after it was placed was charged funding from the
// account's last settle, not from the fill: an idle account's ask collected funding for a period in which it was not short,
// paid by no one. Two accounts of one owner could print it for ~10 bps of fees.
#[test] fn a_resting_order_earns_funding_only_from_its_fill() {
    let mut e = Env::new(); e.oracle_update("", PX, None);
    let a0 = e.shard(1, 100_000); let (mm, mmu) = e.user(10_000_000_000); let (t, tu) = e.user(10_000_000_000);
    assert_eq!(place(&mut e, &mm, &mmu, &[a0], 1, POST, 100_050, 20), None);
    for _ in 0..3 { e.set_time(e.slot + 27_000); e.oracle_update("", PX, None); }
    let i1 = e.market_state().funding_index; assert_ne!(i1, 0, "the index moved while the order rested");
    let (c0, t0) = (e.user_state(&mmu).cash, e.user_state(&tu).cash);
    assert_eq!(place(&mut e, &t, &tu, &[a0], 0, IOC, 100_050, 20), None);
    assert_eq!(batch(&mut e, &mm, &mmu, &[a0], &[], &[]), None); // the maker claims
    let fee = t0 - e.user_state(&tu).cash - 20 * 100_050;
    assert_eq!(e.user_state(&mmu).cash - c0, 20 * 100_050, "the sale and nothing else: it was not short before the fill");
    // from the fill on, the short side receives exactly what the long side pays
    for _ in 0..2 { e.set_time(e.slot + 27_000); e.oracle_update("", PX, None); }
    let i2 = e.market_state().funding_index; assert_ne!(i2, i1);
    assert_eq!(batch(&mut e, &mm, &mmu, &[a0], &[], &[]), None);
    assert_eq!(batch(&mut e, &t, &tu, &[a0], &[], &[]), None);
    let (dm, dt) = (e.user_state(&mmu).cash - c0 - 20 * 100_050, e.user_state(&tu).cash - t0 + 20 * 100_050 + fee);
    assert_eq!(dm, 20 * (i2 - i1)); assert_eq!(dm + dt, 0, "funding nets to zero");
}
