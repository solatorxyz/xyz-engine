#[path = "../../program/src/state.rs"]
#[allow(dead_code)]
mod state;
// Fix-review round 6 (HEAD 3b145fd) proofs of concept. Each test performs the scenario against the built SBF program and asserts
// what the CURRENT code does: a PASSING test means the issue is PRESENT.
include!("common.rs");

const IOC: u8 = 4;
fn single(item: Vec<u8>) -> Vec<u8> { let mut d = vec![1u8]; d.extend_from_slice(&item); d }
fn amt(tag: u8, v: u64) -> Vec<u8> { let mut d = vec![tag]; d.extend_from_slice(&v.to_le_bytes()); d }
fn code(r: Result<(), String>) -> Option<u32> {
    match r { Ok(()) => None, Err(s) => Some(s.split("Custom(").nth(1).and_then(|x| x.split(')').next()).and_then(|x| x.parse().ok()).unwrap_or(u32::MAX)) }
}
fn try_send(e: &mut Env, accounts: Vec<AccountMeta>, data: Vec<u8>, signers: &[&Keypair]) -> Option<u32> {
    let ix = Instruction { program_id: e.pid, accounts, data };
    let mut s: Vec<&Keypair> = vec![&e.admin]; s.extend_from_slice(signers);
    let tx = Transaction::new_signed_with_payer(&[ix], Some(&e.admin.pubkey()), &s, e.svm.latest_blockhash());
    let r = e.svm.send_transaction(tx); e.svm.expire_blockhash();
    code(match r { Ok(_) => Ok(()), Err(f) => Err(format!("{:?}\n{}", f.err, f.meta.logs.join("\n"))) })
}
fn liq_fast_data() -> Vec<u8> { let mut d = vec![70u8, 1]; d.extend_from_slice(&u64::MAX.to_le_bytes()); d }

// ---------- two markets sharing one collateral pool (same as review3.rs) ----------
struct Mkt { market: Address, ins: Address, vault: Address, vault_pda: Address }
fn switch(e: &mut Env, m: &Mkt) { e.market = m.market; e.ins = m.ins; e.vault = m.vault; e.vault_pda = m.vault_pda; }
fn pool2(e: &mut Env) -> (Mkt, Mkt, Keypair) {
    let admin = Keypair::new(); e.svm.airdrop(&admin.pubkey(), 10_000_000_000).unwrap();
    let pool = Address::new_unique(); e.prog_acct(pool, POOL_SIZE);
    let (vault_pda, bump) = Address::find_program_address(&[b"vault", pool.as_ref()], &e.pid);
    let mint = Address::new_unique(); let mut md = vec![0u8; 82]; md[44] = 6; md[45] = 1;
    e.svm.set_account(mint, Account { lamports: 1_000_000_000, data: md, owner: TOKEN, executable: false, rent_epoch: 0 }).unwrap();
    let vault = Address::new_unique();
    e.svm.set_account(vault, Account { lamports: 1_000_000_000, data: token_acct(&mint, &vault_pda, 0), owner: TOKEN, executable: false, rent_epoch: 0 }).unwrap();
    let mut d = vec![27, bump]; d.extend_from_slice(&0u64.to_le_bytes());
    e.send("", vec![AccountMeta::new(admin.pubkey(), true), w(&pool), r(&vault)], d, &[&admin], true);
    let mut mk = |e: &mut Env| {
        let market = Address::new_unique(); let ins = Address::new_unique();
        e.prog_acct(market, MARKET_SIZE); e.prog_acct(ins, INS_SIZE);
        let mut d = vec![32u8]; d.extend_from_slice(market.as_ref());
        e.send("", vec![w(&pool), AccountMeta::new_readonly(admin.pubkey(), true)], d, &[&admin], true);
        let s = e.slot + 216_000; e.set_time(s);
        let mut d = vec![10u8, 0]; d.extend_from_slice(&e.feed);
        for v in [BW, 3, 100, 10, (-8i64) as u64, 1, 1_000_000, 1_000_000_000] { d.extend_from_slice(&v.to_le_bytes()); }
        let (o, a) = (e.oracle, e.admin.pubkey());
        e.send("", vec![AccountMeta::new(a, true), w(&market), w(&ins), r(&vault), r(&o), w(&pool), AccountMeta::new_readonly(admin.pubkey(), true)], d, &[&admin], true);
        Mkt { market, ins, vault, vault_pda }
    };
    let ma = mk(e); let mb = mk(e);
    (ma, mb, admin)
}
fn upd(e: &mut Env, ms: &[&Mkt], px: i64) { for m in ms { switch(e, m); e.oracle_update("", px, None); } }
fn base_in(e: &Env, u: &Address, m: &Address) -> i64 { e.user_state(u).pos.iter().find(|p| p.market == m.to_bytes()).map_or(0, |p| p.base) }
fn iso_in(e: &Env, u: &Address, m: &Address) -> i64 { e.user_state(u).pos.iter().find(|p| p.market == m.to_bytes()).map_or(0, |p| p.iso_margin) }
fn ranked(e: &Env, side: usize, u: &Address) -> bool {
    let r = e.rank_state(); let n = if side == 0 { r.n_long } else { r.n_short } as usize;
    r.entries[side * RANK_N..side * RANK_N + n].iter().any(|x| x.user == u.to_bytes())
}

// ---------- fast book helpers (same as review3.rs) ----------
struct Mk { kp: Keypair, slab: Address }
fn fast_new(e: &mut Env, idx: u64) -> Address {
    let k = Address::new_unique(); e.prog_acct(k, FAST_SIZE);
    let mut d = vec![61u8]; d.extend_from_slice(&idx.to_le_bytes());
    let (a, m) = (e.admin.pubkey(), e.market);
    e.send("", vec![AccountMeta::new(a, true), w(&m), w(&k)], d, &[], true); k
}
fn maker(e: &mut Env, f: &Address, dep: u64, fund: i64) -> Mk {
    let (kp, u) = e.user(dep);
    let slab = Address::new_unique(); e.prog_acct(slab, SLAB_SIZE);
    e.send("", vec![AccountMeta::new(kp.pubkey(), true), r(&u), w(&slab)], vec![60], &[&kp], true);
    let (a, m) = (e.admin.pubkey(), e.market);
    e.send("", vec![AccountMeta::new(a, true), r(&m), w(f), w(&slab), AccountMeta::new_readonly(kp.pubkey(), true)], vec![62], &[&kp], true);
    if fund != 0 { let mut d = vec![64u8]; d.extend_from_slice(&fund.to_le_bytes());
        e.send("", vec![AccountMeta::new(kp.pubkey(), true), w(&u), r(&m), w(f)], d, &[&kp], true); }
    Mk { kp, slab }
}
fn quote(e: &mut Env, k: &Mk, v: &[(u16, u32, u32)]) {
    let mut d = vec![63u8]; for (i, p, q) in v { d.extend_from_slice(&i.to_le_bytes()); d.extend_from_slice(&p.to_le_bytes()); d.extend_from_slice(&q.to_le_bytes()); }
    e.send("", vec![AccountMeta::new(k.kp.pubkey(), true), w(&k.slab)], d, &[&k.kp], true);
}

// =====================================================================================================================
// H-1 (MEDIUM, G-4 fix bypassed): HEAD makes liquidate_book refuse a side with a registered FastBook "so that maker quotes are
// taken too", but liquidate_fast fills the opposite SHARDS FIRST (take() up to mark -/+ QUOTE_BAND_BPS = 5%) and only then
// looks at maker quotes, with no price comparison. The G-4 attack is unchanged: the liquidator's own bid 4.9% under the mark
// takes the whole chunk while makers bid 0.1% under it. The shipped crank now always uses liquidate_fast in such markets, so
// even honest liquidations are routed into the worst shard order first.
// Control: same scenario without the predator bid fills at the maker's 91_900.
// =====================================================================================================================
fn h1_setup(predator: bool) -> (Option<u32>, i64, i64) {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let (a0, bl) = (e.shard(1, 100_000), e.shard(0, 87_500)); let m = e.market;
    let f = fast_new(&mut e, 0);
    let k = maker(&mut e, &f, 10_000_000_000, 1_000_000_000);
    let (lp, lpu) = e.user(50_000_000_000); let (t, tu) = e.user(1_500_000); let (x, xu) = e.user(50_000_000_000);
    e.send("", trade_accts(&lp, &lpu, &m, &[a0]), single(place_item(1, 1, 100_000, 200, 0, 0)), &[&lp], true);
    e.send("", trade_accts(&t, &tu, &m, &[a0]), single(place_item(0, IOC, 100_000, 130, 0, 0)), &[&t], true);
    e.set_time(e.slot + 1); e.oracle_update("", 920_00000000, None);
    if predator { e.send("", trade_accts(&x, &xu, &m, &[bl]), single(place_item(0, 1, 87_500, 100, 0, 0)), &[&x], true); }
    quote(&mut e, &k, &[(0, 91_900, 1000)]);
    e.set_time(e.slot + 1); e.oracle_update("", 920_00000000, None); quote(&mut e, &k, &[(0, 91_900, 1000)]);
    let ins = e.ins;
    let cash0 = e.user_state(&tu).cash;
    let res = try_send(&mut e, vec![w(&tu), r(&m), w(&ins), w(&f), r(&k.slab), w(&bl), w(&a0)], liq_fast_data(), &[]);
    let (sold, cash1) = (130 - e.user_state(&tu).pos[0].base, e.user_state(&tu).cash);
    (res, sold, if sold > 0 { (cash1 - cash0) / sold } else { 0 })
}
#[test] fn h1_liquidate_fast_still_routes_to_far_shard_bid() {
    let ctl = h1_setup(false);
    let atk = h1_setup(true);
    println!("H-1 control (no predator bid): liquidate_fast {:?}, sold {} lots, avg proceeds/lot {} (fee incl.)", ctl.0, ctl.1, ctl.2);
    println!("H-1 attack (predator bid 87_500, makers 91_900): liquidate_fast {:?}, sold {} lots, avg proceeds/lot {}", atk.0, atk.1, atk.2);
    assert_eq!((ctl.0, atk.0), (None, None));
    assert!(ctl.2 > 91_000, "control fills at the maker quote");
    assert!(atk.2 > 91_000, "fixed: the maker bid (91_900) fills before the 87_500 shard bid (avg {})", atk.2);
}

// =====================================================================================================================
// H-2 (MEDIUM, liveness regression of HEAD): a hybrid market side (registered FastBook) whose only liquidity is a resting shard
// order between 5% (liquidate_fast's reach, QUOTE_BAND_BPS) and 20% (the empty-book proof's reach, LIQ_BOOK_BAND_BPS << 2) from
// the mark. liquidate_book is now refused (BadAccount), liquidate_fast can't reach the order and its empty-book proof counts it
// as live (NoFill), so the position is neither liquidated nor ADL-eligible; it sits below maintenance until bankrupt. Makers
// pulling quotes in a crash is the normal case; anyone can also park such an order on purpose (it only fills at 7.6% off).
// Control: the same market without a registered book liquidates into the 85_000 bid (band step 2).
// =====================================================================================================================
fn h2_setup(book: bool) -> (Option<u32>, Option<u32>, i64, u64) {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let (a0, bf) = (e.shard(1, 100_000), e.shard(0, 85_000)); let m = e.market;
    let fk = if book { let f = fast_new(&mut e, 0); let k = maker(&mut e, &f, 10_000_000_000, 1_000_000_000); Some((f, k)) } else { None };
    let (lp, lpu) = e.user(50_000_000_000); let (t, tu) = e.user(1_500_000); let (x, xu) = e.user(50_000_000_000);
    e.send("", trade_accts(&lp, &lpu, &m, &[a0]), single(place_item(1, 1, 100_000, 200, 0, 0)), &[&lp], true);
    e.send("", trade_accts(&t, &tu, &m, &[a0]), single(place_item(0, IOC, 100_000, 130, 0, 0)), &[&t], true);
    e.send("", trade_accts(&x, &xu, &m, &[bf]), single(place_item(0, 1, 85_000, 100, 0, 0)), &[&x], true);
    e.set_time(e.slot + 1); e.oracle_update("", 920_00000000, None);
    let ins = e.ins;
    let r_book = try_send(&mut e, vec![w(&tu), r(&m), w(&ins), w(&bf), w(&a0)], amt(34, u64::MAX), &[]);
    let r_fast = match &fk { Some((f, k)) => try_send(&mut e, vec![w(&tu), r(&m), w(&ins), w(f), r(&k.slab), w(&bf), w(&a0)], liq_fast_data(), &[]), None => Some(u32::MAX) };
    let p = e.user_state(&tu).pos[0];
    (r_book, r_fast, p.base, p.flags & 2)
}
#[test] fn h2_hybrid_side_with_far_shard_liquidity_cannot_be_liquidated() {
    let ctl = h2_setup(false);
    println!("H-2 control (no FastBook): liquidate_book {:?}, base after {}", ctl.0, ctl.2);
    assert_eq!(ctl.0, None); assert!(ctl.2 < 130, "control: liquidated into the 85_000 bid");
    let atk = h2_setup(true);
    println!("H-2 (FastBook registered, makers not quoting): liquidate_book {:?}, liquidate_fast {:?}, base {}, ADL-eligible flag {}", atk.0, atk.1, atk.2, atk.3);
    assert_eq!((atk.0, atk.1), (Some(1), None), "fixed: liquidate_fast widens the shard band (10%, 20%) before NoFill"); assert!(atk.2 < 130);
}

// =====================================================================================================================
// H-3 (MEDIUM, bypass of HEAD's "the top counterparty must be evaluated"): `unev` holds at most 8 keys. A caller first passes
// 8 un-evaluable users (one junk account repeated 8 times is enough), then the top-ranked candidate X with one of its markets
// left out. X is neither re-scored nor removed nor recorded in `unev`, so ADL runs against X's STALE rank entry. If X's
// position in the market is isolated (iso_health needs no other market) the step goes through without re-checking side,
// profit or score: here X's stale entry is from when it was short; X is now LONG, and ADL makes it buy the bankrupt long's
// lots (X's long doubles) and charges it the deficit share from its isolated margin.
// Control: without the junk accounts the same call is refused (Rank).
// =====================================================================================================================
fn h3_setup(junk: usize) -> (Option<u32>, i64, i64, i64, i64) {
    let mut e = Env::new();
    let (ma, mb, pa) = pool2(&mut e);
    upd(&mut e, &[&ma, &mb], 1000_00000000);
    switch(&mut e, &ma);
    let (a0, b0, ah) = (e.shard_by(1, 100_000, &pa), e.shard_by(0, 100_000, &pa), e.shard_by(1, 85_000, &pa));
    let rk = Address::new_unique(); e.prog_acct(rk, RANK_SIZE);
    e.send("", vec![AccountMeta::new(pa.pubkey(), true), w(&ma.market), w(&rk)], vec![26], &[&pa], true); e.rank = rk;
    let (lp, lpu) = e.user(50_000_000_000); let (t, tu) = e.user(1_500_000); let (x, xu) = e.user(60_000_000); let (j, ju) = e.user(10_000_000);
    // T: bankrupt-to-be long 130
    e.send("", trade_accts(&lp, &lpu, &ma.market, &[a0]), single(place_item(1, 1, 100_000, 130, 0, 0)), &[&lp], true);
    e.send("", trade_accts(&t, &tu, &ma.market, &[a0]), single(place_item(0, IOC, 100_000, 130, 0, 0)), &[&t], true);
    // X: isolated short 50 in A
    e.send("", vec![AccountMeta::new(x.pubkey(), true), w(&xu), r(&ma.market)], vec![30, 1], &[&x], true);
    e.send("", vec![AccountMeta::new(x.pubkey(), true), w(&xu), r(&ma.market)], amt(31, 20_000_000), &[&x], true);
    e.send("", trade_accts(&lp, &lpu, &ma.market, &[b0, a0]), single(place_item(0, 1, 100_000, 50, 0, 0)), &[&lp], true);
    e.send("", trade_accts(&x, &xu, &ma.market, &[b0]), single(place_item(1, IOC, 100_000, 50, 0, 0)), &[&x], true);
    // X and J also hold a slot in market B (a tiny resting bid)
    switch(&mut e, &mb);
    let bb = e.shard_by(0, 50_000, &pa);
    let mut acc = trade_accts(&x, &xu, &mb.market, &[bb]); acc.push(r(&ma.market));
    e.send("", acc, single(place_item(0, 1, 50_000, 1, 0, 0)), &[&x], true);
    e.send("", trade_accts(&j, &ju, &mb.market, &[bb]), single(place_item(0, 1, 50_000, 1, 0, 0)), &[&j], true);
    for p in [950, 900, 850] { e.set_time(e.slot + 1); upd(&mut e, &[&ma, &mb], p * 100_000_000); }
    switch(&mut e, &ma);
    // X is ranked (top short) while short and profitable
    e.send("", vec![r(&ma.market), w(&rk), w(&xu), r(&mb.market)], vec![25], &[], true);
    assert!(ranked(&e, 1, &xu));
    // X flips to LONG 50 (buys 100 at 85_000); its short-side rank entry is now stale
    e.send("", trade_accts(&lp, &lpu, &ma.market, &[ah]), single(place_item(1, 1, 85_000, 100, 0, 0)), &[&lp], true);
    let mut acc = trade_accts(&x, &xu, &ma.market, &[ah]); acc.push(r(&mb.market));
    e.send("", acc, single(place_item(0, IOC, 85_000, 100, 0, 0)), &[&x], true);
    assert_eq!(base_in(&e, &xu, &ma.market), 50); assert!(ranked(&e, 1, &xu), "stale short-side entry");
    let iso0 = iso_in(&e, &xu, &ma.market);
    let mut acc = vec![w(&tu), w(&ma.market), w(&ma.ins), w(&rk)];
    for _ in 0..junk { acc.push(w(&ju)); }
    acc.extend([w(&xu), w(&a0), w(&b0), w(&ah)]);
    let res = try_send(&mut e, acc, vec![6], &[]);
    (res, base_in(&e, &xu, &ma.market), base_in(&e, &tu, &ma.market), iso0, iso_in(&e, &xu, &ma.market))
}
#[test] fn h3_adl_unev_overflow_runs_on_unscored_top() {
    let ctl = h3_setup(0);
    println!("H-3 control (X alone, its market B left out): adl {:?}, X base {}, T base {}", ctl.0, ctl.1, ctl.2);
    assert_eq!(ctl.0, Some(20), "control: the un-evaluable top is refused");
    let atk = h3_setup(8);
    println!("H-3 attack (8 x junk first): adl {:?}, X base {} (was 50, LONG like the target), T base {}, X iso margin {} -> {}", atk.0, atk.1, atk.2, atk.3, atk.4);
    assert_eq!(atk.0, Some(20), "fixed: the top entry must be re-scored here and hold the opposite side");
}

// =====================================================================================================================
// H-4 (MEDIUM, keeper/crank.py vs program): the program only uses the TOP rank entry and needs it passed; entries never passed
// keep their (possibly stale) place. The crank passes the 4 largest same-side positions by |base| (adl_candidates) and never
// sends adl_rank (IX 25). A ranked counterparty that has since closed its position (or a small, highly leveraged one) is never
// passed, so every ADL on that side fails with Rank until someone re-scores that account by hand.
// Program-side demonstration: C1 (top) closes; ADL with the crank's candidate set (C2) -> Rank; with C1 also passed -> ok.
// =====================================================================================================================
#[test] fn h4_stale_top_entry_not_passed_by_crank_blocks_adl() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let (a0, ah) = (e.shard(1, 100_000), e.shard(1, 85_000)); let m = e.market;
    let (t, tu) = e.user(1_500_000);
    let (c1, c1u) = e.user(50_000_000); let (c2, c2u) = e.user(50_000_000); let (lp, lpu) = e.user(50_000_000_000);
    e.send("", trade_accts(&c1, &c1u, &m, &[a0]), single(place_item(1, 1, 100_000, 100, 0, 0)), &[&c1], true);
    e.send("", trade_accts(&c2, &c2u, &m, &[a0]), single(place_item(1, 1, 100_000, 30, 0, 0)), &[&c2], true);
    e.send("", trade_accts(&t, &tu, &m, &[a0]), single(place_item(0, IOC, 100_000, 130, 0, 0)), &[&t], true);
    e.send("", vec![w(&a0), w(&c1u), w(&c2u)], vec![24], &[], true);
    for p in [950, 900, 850] { e.set_time(e.slot + 1); e.oracle_update("", p * 100_000_000, None); }
    e.rank_user(&c1u); e.rank_user(&c2u);
    assert_eq!(e.rank_state().entries[RANK_N].user, c1u.to_bytes());
    // C1 closes its short (buys back 100 at 85_000)
    e.send("", trade_accts(&lp, &lpu, &m, &[ah]), single(place_item(1, 1, 85_000, 100, 0, 0)), &[&lp], true);
    e.send("", trade_accts(&c1, &c1u, &m, &[ah]), single(place_item(0, IOC, 85_000, 100, 0, 0)), &[&c1], true);
    assert_eq!(e.user_state(&c1u).pos.iter().find(|p| p.market == m.to_bytes()).map_or(0, |p| p.base), 0);
    let (ins, rk) = (e.ins, e.rank);
    let crank = try_send(&mut e, vec![w(&tu), w(&m), w(&ins), w(&rk), w(&c2u), w(&a0), w(&ah)], vec![6], &[]);
    let full = try_send(&mut e, vec![w(&tu), w(&m), w(&ins), w(&rk), w(&c2u), w(&c1u), w(&a0), w(&ah)], vec![6], &[]);
    println!("H-4: ADL with the crank's candidates (C2 only) -> {:?}; with the stale top C1 passed too -> {:?}", crank, full);
    // program rule (unchanged): the top entry must be passed; the crank now passes the rank's top entries (keeper/crank.py)
    assert_eq!((crank, full), (Some(20), None));
}

// =====================================================================================================================
// H-5 (LOW, pusher cadence vs program): the mark EMA moves only in an update that carries the complete book, by
// min(ds, MARK_EMA_SLOTS) / MARK_EMA_SLOTS with ds = slots since the LAST update of any kind (funding_slot). The HEAD pusher sends
// the whole-book sample right after the round's price updates, so it usually lands in the same or the next slot as that
// market's price update: ds ~ 0-1 and the EMA (and with it the book's weight in the mark between samples) barely moves.
// Shown: identical book (mid 100_600 vs oracle 100_000); a sample in the same slot as the price update leaves the EMA at 0,
// one 5 slots later moves it.
// =====================================================================================================================
fn h5_ema(gap: u64) -> i64 {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let (b, a) = (e.shard(0, 100_400), e.shard(1, 100_800)); let m = e.market;
    let (x, xu) = e.user(50_000_000_000);
    e.send("", trade_accts(&x, &xu, &m, &[b, a]), batch_data(&[], &[place_item(0, 1, 100_400, 20, 0, 0), place_item(1, 1, 100_800, 20, 0, 0)]), &[&x], true);
    e.set_time(e.slot + 5); e.oracle_update("", 1000_00000000, None); // the pusher's price-only update
    if gap > 0 { e.set_time(e.slot + gap); }
    e.oracle_update_set("", 1000_00000000, &[b, a]); // the separate whole-book sample
    // fixed: the EMA weighs the slots since the previous *sample*: a second sample 10 slots later moves it whatever the price
    // updates did in between
    e.set_time(e.slot + 5); e.oracle_update("", 1000_00000000, None);
    e.set_time(e.slot + 5); e.oracle_update("", 1000_00000000, None);
    e.oracle_update_set("", 1000_00000000, &[b, a]);
    e.market_state().mark_ema
}
#[test] fn h5_sample_right_after_price_update_does_not_move_ema() {
    let (same, later) = (h5_ema(0), h5_ema(5));
    println!("H-5: mark_ema after a whole-book sample in the price update's slot = {}; 5 slots later = {}", same, later);
    assert!(same > 0 && later > 0, "fixed: samples move the EMA by the gap between samples ({same}, {later})");
}
