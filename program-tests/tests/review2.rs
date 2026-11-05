#[path = "../../program/src/state.rs"]
#[allow(dead_code)]
mod state;
// Fix-review round 4 (commits 2dded03 + 569be1b) proofs of concept. Each test performs the scenario against the built SBF
// program and asserts what the CURRENT code does: a PASSING test means the issue is PRESENT.
include!("common.rs");

const IOC: u8 = 4;
fn single(item: Vec<u8>) -> Vec<u8> { let mut d = vec![1u8]; d.extend_from_slice(&item); d }
fn amt(tag: u8, v: u64) -> Vec<u8> { let mut d = vec![tag]; d.extend_from_slice(&v.to_le_bytes()); d }
fn code(r: Result<(), String>) -> Option<u32> {
    match r { Ok(()) => None, Err(s) => Some(s.split("Custom(").nth(1).and_then(|x| x.split(')').next()).and_then(|x| x.parse().ok()).unwrap_or(u32::MAX)) }
}
fn send_ixs(e: &mut Env, ixs: Vec<(Vec<AccountMeta>, Vec<u8>)>, signers: &[&Keypair]) -> Result<(), String> {
    let ixs: Vec<Instruction> = ixs.into_iter().map(|(accounts, data)| Instruction { program_id: e.pid, accounts, data }).collect();
    let mut s: Vec<&Keypair> = vec![&e.admin]; s.extend_from_slice(signers);
    let tx = Transaction::new_signed_with_payer(&ixs, Some(&e.admin.pubkey()), &s, e.svm.latest_blockhash());
    let r = e.svm.send_transaction(tx); e.svm.expire_blockhash();
    match r { Ok(_) => Ok(()), Err(f) => Err(format!("{:?}\n{}", f.err, f.meta.logs.join("\n"))) }
}
fn try_send(e: &mut Env, accounts: Vec<AccountMeta>, data: Vec<u8>, signers: &[&Keypair]) -> Option<u32> {
    code(send_ixs(e, vec![(accounts, data)], signers))
}
fn ins_of(e: &Env, k: &Address) -> i64 { let d = e.data(k); i64::from_le_bytes(d[40..48].try_into().unwrap()) }
/// cross equity at the stored mark of each market (no funding), unclaimed fills not included
fn cross_eq(e: &Env, u: &Address, mkts: &[Address]) -> i128 {
    let s = e.user_state(u); let mut eq = s.cash as i128;
    for p in s.pos.iter() {
        if p.market == [0; 32] || p.flags & 1 != 0 { continue; }
        let mk = mkts.iter().find(|m| m.to_bytes() == p.market).unwrap();
        let d = e.data(mk); let ms: Market = unsafe { std::ptr::read_unaligned(d.as_ptr() as *const Market) };
        eq += p.base as i128 * (if ms.mark_px != 0 { ms.mark_px } else { ms.oracle_px }) as i128;
    }
    eq
}

// ---------- two markets sharing one collateral pool (same as review.rs) ----------
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

// ---------- fast book helpers ----------
struct Mk { kp: Keypair, u: Address, slab: Address }
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
    let mk = Mk { kp, u, slab };
    if fund != 0 { let mut d = vec![64u8]; d.extend_from_slice(&fund.to_le_bytes()); let m = e.market;
        e.send("", vec![AccountMeta::new(mk.kp.pubkey(), true), w(&mk.u), r(&m), w(f)], d, &[&mk.kp], true); }
    mk
}
fn items(v: &[(u16, u32, u32)]) -> Vec<u8> { let mut d = vec![63u8]; for (i, p, q) in v { d.extend_from_slice(&i.to_le_bytes()); d.extend_from_slice(&p.to_le_bytes()); d.extend_from_slice(&q.to_le_bytes()); } d }
fn quote(e: &mut Env, k: &Mk, v: &[(u16, u32, u32)]) { e.send("", vec![AccountMeta::new(k.kp.pubkey(), true), w(&k.slab)], items(v), &[&k.kp], true); }
const ASK: u16 = 8; const BID: u16 = 0;

// =====================================================================================================================
// F-1 (HIGH, regression of 569be1b): liquidations / ADL no longer require a complete view (complete_view removed), but they
// still judge the WHOLE cross account. The caller simply leaves out the shards of another market where the target holds an
// unclaimed profitable maker fill: the account looks bankrupt, ADL runs, insurance pays a "deficit" that does not exist and it
// is CREDITED to the target, which then claims its hidden profit. The shipped crank does exactly this (it passes only the
// liquidated market's shards). Same scenario as review.rs R-5, where passing B's shards gives NotBankrupt.
// =====================================================================================================================
#[test] fn f1_adl_without_other_market_shards_pays_phantom_deficit_from_insurance() {
    let mut e = Env::new();
    let (ma, mb, pa) = pool2(&mut e);
    upd(&mut e, &[&ma, &mb], 1000_00000000);
    { let mut acct = e.svm.get_account(&ma.ins).unwrap(); acct.data[40..48].copy_from_slice(&5_000_000i64.to_le_bytes()); e.svm.set_account(ma.ins, acct).unwrap(); }
    switch(&mut e, &ma);
    let a0 = e.shard_by(1, 100_000, &pa);
    let (lp, lpu) = e.user(50_000_000_000);
    let (t, tu) = e.user(1_620_000);
    let (c, cu) = e.user(50_000_000);
    let rk = Address::new_unique(); e.prog_acct(rk, RANK_SIZE);
    e.send("", vec![AccountMeta::new(pa.pubkey(), true), w(&ma.market), w(&rk)], vec![26], &[&pa], true); e.rank = rk;
    e.send("", trade_accts(&c, &cu, &ma.market, &[a0]), single(place_item(1, 1, 100_000, 130, 0, 0)), &[&c], true);
    e.send("", trade_accts(&t, &tu, &ma.market, &[a0]), single(place_item(0, IOC, 100_000, 130, 0, 0)), &[&t], true);
    e.send("", trade_accts(&c, &cu, &ma.market, &[a0]), vec![3, 0, 0, 0, 0], &[&c], true);
    switch(&mut e, &mb);
    let (b0, a1) = (e.shard_by(0, 70_000, &pa), e.shard_by(1, 100_000, &pa));
    let mut acc = trade_accts(&t, &tu, &mb.market, &[b0]); acc.push(r(&ma.market));
    e.send("", acc, single(place_item(0, 1, 70_000, 30, 0, 0)), &[&t], true);
    e.send("", trade_accts(&lp, &lpu, &mb.market, &[b0]), single(place_item(1, IOC, 70_000, 30, 0, 0)), &[&lp], true);
    e.set_time(e.slot + 1); upd(&mut e, &[&ma, &mb], 850_00000000); switch(&mut e, &ma);
    e.rank_user(&cu);
    let ins0 = ins_of(&e, &ma.ins);
    // crank-shaped ADL (keeper/crank.py ix_adl): target, market, insurance, rank, candidates, other markets, THIS market's shards
    let acc = vec![w(&tu), w(&ma.market), w(&ma.ins), w(&rk), w(&cu), r(&mb.market), w(&a0)];
    let res = try_send(&mut e, acc, vec![6], &[]);
    let ins1 = ins_of(&e, &ma.ins);
    // afterwards anyone (or T itself) claims the B fill
    e.send("", vec![w(&b0), w(&tu)], vec![24], &[], true);
    let eq_after = cross_eq(&e, &tu, &[ma.market, mb.market]);
    println!("F-1: adl -> {:?}; insurance {} -> {}; target equity after claiming B: {}", res, ins0, ins1, eq_after);
    assert_eq!(res, Some(19), "fixed: resting orders elsewhere must be cleared (liq_cancel) before ADL judges the account"); return;
    assert!(ins1 < ins0, "insurance paid the phantom deficit");
    assert!(eq_after > 0, "target is solvent once its fill is claimed: it kept the insurance money");
}

// =====================================================================================================================
// F-2 (HIGH, incomplete fix in 569be1b): the new `adverse` margin skips spline ASKS (`k != KIND_SPLINE`), although a spline
// ask's first level is exactly its anchor price (eff = price + step * level). A spline ask far below the mark rests with
// initial margin charged at the mark only; one fill makes the account bankrupt (bad debt to insurance / ADL). A limit ask at
// the same price is refused with Margin.
// =====================================================================================================================
#[test] fn f2_spline_ask_below_mark_skips_adverse_margin() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let a5 = e.shard(1, 50_000); let m = e.market;
    let (x, xu) = e.user(1_100_000);
    // control: a LIMIT ask of 100 lots at 50_000 (mark 100_000) is refused
    let lim = try_send(&mut e, trade_accts(&x, &xu, &m, &[a5]), single(place_item(1, 0, 50_000, 100, 0, 0)), &[&x]);
    assert_eq!(lim, Some(3), "limit ask below mark: adverse loss margined");
    // same 100 lots as a 2-level spline ask (50 + 50 at 50_000 / 50_001): accepted
    let spl = try_send(&mut e, trade_accts(&x, &xu, &m, &[a5]), single(place_ex(1, 0, 2, 50_000, 50, 0, 0, 0, 1, 2, 0)), &[&x]);
    println!("F-2: limit -> {:?}, spline -> {:?}", lim, spl);
    assert_eq!(spl, Some(3), "fixed: a spline ask below the mark margins its first level"); return;
    // a colluding taker lifts it
    let (y, yu) = e.user(2_000_000);
    e.send("", trade_accts(&y, &yu, &m, &[a5]), single(place_item(0, IOC, 50_001, 100, 0, 0)), &[&y], true);
    e.send("", vec![w(&a5), w(&xu)], vec![24], &[], true);
    let eq = cross_eq(&e, &xu, &[m]);
    println!("F-2: maker equity after the fill {}", eq);
    assert!(eq < -3_000_000, "account bankrupt by its own resting order");
}

// =====================================================================================================================
// F-3 (MEDIUM, regression): batch charges `adverse` against the CROSS check even when the order's position is isolated. An
// isolated-only trader (all collateral in the isolated bucket) can no longer rest a bid even 0.5% above the mark, although
// its isolated margin covers the loss many times over.
// =====================================================================================================================
#[test] fn f3_isolated_position_adverse_charged_to_cross() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let (b0, b1) = (e.shard(0, 100_000), e.shard(0, 100_500)); let m = e.market;
    let (x, xu) = e.user(10_000_000);
    e.send("", vec![AccountMeta::new(x.pubkey(), true), w(&xu), r(&m)], vec![30, 1], &[&x], true);
    e.send("", vec![AccountMeta::new(x.pubkey(), true), w(&xu), r(&m)], amt(31, 10_000_000), &[&x], true);
    let at_mark = try_send(&mut e, trade_accts(&x, &xu, &m, &[b0]), single(place_item(0, 1, 100_000, 10, 0, 0)), &[&x]);
    let above = try_send(&mut e, trade_accts(&x, &xu, &m, &[b1]), single(place_item(0, 1, 100_500, 10, 0, 0)), &[&x]);
    println!("F-3: bid at mark -> {:?}; bid 0.5% above (adverse 5_000 vs 10M isolated margin) -> {:?}", at_mark, above);
    assert_eq!((at_mark, above), (None, None), "fixed: the isolated bucket alone carries its adverse loss");
}

// =====================================================================================================================
// F-4 (MEDIUM): hybrid empty-book proof (liquidate_fast) refuses whenever ANY opposite shard order sits in the band, including
// orders that can never fill (off its own oracle band, or expired). liquidate_book would sweep them, but in a hybrid market it
// must end in NoFill (book registered) and reverts, so the sweep is undone. One 1-lot bid with band_bps = 1 keeps a below-
// maintenance position out of the ADL window (control: review.rs R-7 succeeds without it).
// =====================================================================================================================
#[test] fn f4_unfillable_shard_order_blocks_hybrid_adl_window() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let (a0, b0, bx) = (e.shard(1, 100_000), e.shard(0, 50_000), e.shard(0, 90_000)); let m = e.market;
    let f = Address::new_unique(); e.prog_acct(f, FAST_SIZE);
    let a = e.admin.pubkey();
    e.send("", vec![AccountMeta::new(a, true), w(&m), w(&f)], amt(61, 0), &[], true);
    let (mk, mu) = e.user(1_000_000_000);
    let slab = Address::new_unique(); e.prog_acct(slab, SLAB_SIZE);
    e.send("", vec![AccountMeta::new(mk.pubkey(), true), r(&mu), w(&slab)], vec![60], &[&mk], true);
    e.send("", vec![AccountMeta::new(a, true), r(&m), w(&f), w(&slab), AccountMeta::new_readonly(mk.pubkey(), true)], vec![62], &[&mk], true);
    let (lp, lpu) = e.user(50_000_000_000); let (t, tu) = e.user(1_500_000);
    e.send("", trade_accts(&lp, &lpu, &m, &[a0]), single(place_item(1, 1, 100_000, 200, 0, 0)), &[&lp], true);
    e.send("", trade_accts(&t, &tu, &m, &[a0]), single(place_item(0, IOC, 100_000, 130, 0, 0)), &[&t], true);
    // griefer: 1 lot bid at 90_000 that is only valid within 1 bp of the oracle (never fills)
    let (g, gu) = e.user(1_000_000);
    e.send("", trade_accts(&g, &gu, &m, &[bx]), single(place_item(0, 1, 90_000, 1, 0, 1)), &[&g], true);
    e.set_time(e.slot + 1); e.oracle_update("", 920_00000000, None);
    let ins = e.ins;
    let r_book = try_send(&mut e, vec![w(&tu), r(&m), w(&ins), w(&b0), w(&bx), w(&a0)], amt(34, u64::MAX), &[]);
    let mut d = vec![70u8, 1]; d.extend_from_slice(&u64::MAX.to_le_bytes());
    let r_fast = try_send(&mut e, vec![w(&tu), r(&m), w(&ins), w(&f), r(&slab), w(&b0), w(&bx), w(&a0)], d, &[]);
    println!("F-4: liquidate_book -> {:?}, liquidate_fast -> {:?}, eligible flag {}", r_book, r_fast, e.user_state(&tu).pos[0].flags & 2);
    assert_eq!((r_book, r_fast), (Some(25), None), "fixed: an order that can never fill does not block the empty-book proof");
    // the sweep through the shards removed the dead order on the way (as a book liquidation would)
}

// =====================================================================================================================
// F-5 (MEDIUM): update_oracle takes the book mid from the COMPLETE shard set but from FastBooks only if the caller passes them.
// In a hybrid market whose real liquidity is maker quotes, anyone (and the shipped crank, which never passes FastBooks) prices
// the mid from the shard book alone; two impact-sized resting orders (FUNDING_IMPACT_NOTIONAL = 1e6 units, ~10 lots here) put
// it where the attacker wants and drag the mark to the 5% clamp, while maker quotes sit at +-1 bp. Control: one update with
// the FastBook and its slab gives mid = 100_000.
// =====================================================================================================================
#[test] fn f5_fastbook_omission_lets_thin_shard_book_set_mark() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let (b0, a0) = (e.shard(0, 91_000), e.shard(1, 100_100)); let m = e.market;
    let f = fast_new(&mut e, 0);
    let k = maker(&mut e, &f, 10_000_000_000, 1_000_000_000);
    let (x, xu) = e.user(100_000_000);
    e.send("", trade_accts(&x, &xu, &m, &[b0, a0]), batch_data(&[], &[place_item(0, 1, 91_000, 20, 0, 0), place_item(1, 1, 100_100, 20, 0, 0)]), &[&x], true);
    let o = e.oracle;
    for i in 0..1000u64 {
        if i % 20 == 0 { quote(&mut e, &k, &[(BID, 99_990, 1000), (ASK, 100_010, 1000)]); }
        let s = e.slot + 1; e.set_time(s); e.set_pyth(1000_00000000, 1000_0000);
        e.send("", vec![w(&m), r(&o), r(&b0), r(&a0)], vec![4], &[], true); // crank-shaped update: all shards, no FastBook
    }
    let m1 = e.market_state();
    println!("F-5: oracle {} mark {} (maker quotes 99_990 / 100_010)", m1.oracle_px, m1.mark_px);
    assert!(m1.mark_px > 99_500, "fixed: without the maker books no sample is taken (mark {})", m1.mark_px); return;
    // the same update WITH the registered book: the maker quotes define the mid
    let s = e.slot + 1; e.set_time(s); e.set_pyth(1000_00000000, 1000_0000);
    let ema0 = e.market_state().mark_ema;
    e.send("", vec![w(&m), r(&o), r(&b0), r(&a0), r(&f), r(&k.slab)], vec![4], &[], true);
    println!("F-5: with the book passed: ema {} -> {}", ema0, e.market_state().mark_ema);
    assert!(e.market_state().mark_ema > ema0, "passing the book pulls the EMA back toward 0");
}

// =====================================================================================================================
// F-6 (MEDIUM, crank/program mismatch, pre-existing in the crank): adl() runs settle_funding_all on EVERY candidate with
// `?`, so every market any candidate holds must be passed. keeper/crank.py ix_adl passes the target's other markets only;
// one candidate with a position elsewhere makes the whole ADL fail (BadAccount), every tick.
// =====================================================================================================================
#[test] fn f6_crank_adl_fails_when_candidate_trades_another_market() {
    let mut e = Env::new();
    let (ma, mb, pa) = pool2(&mut e);
    upd(&mut e, &[&ma, &mb], 1000_00000000);
    switch(&mut e, &ma);
    let (a0, b0) = (e.shard_by(1, 100_000, &pa), e.shard_by(0, 84_900, &pa));
    let (t, tu) = e.user(1_500_000); let (c, cu) = e.user(50_000_000);
    let rk = Address::new_unique(); e.prog_acct(rk, RANK_SIZE);
    e.send("", vec![AccountMeta::new(pa.pubkey(), true), w(&ma.market), w(&rk)], vec![26], &[&pa], true); e.rank = rk;
    e.send("", trade_accts(&c, &cu, &ma.market, &[a0]), single(place_item(1, 1, 100_000, 130, 0, 0)), &[&c], true);
    e.send("", trade_accts(&t, &tu, &ma.market, &[a0]), single(place_item(0, IOC, 100_000, 130, 0, 0)), &[&t], true);
    e.send("", trade_accts(&c, &cu, &ma.market, &[a0]), vec![3, 0, 0, 0, 0], &[&c], true);
    // the candidate also holds a resting order in market B
    switch(&mut e, &mb);
    let bb = e.shard_by(0, 50_000, &pa);
    let mut acc = trade_accts(&c, &cu, &mb.market, &[bb]); acc.push(r(&ma.market));
    e.send("", acc, single(place_item(0, 1, 50_000, 1, 0, 0)), &[&c], true);
    for p in [950, 900, 850] { e.set_time(e.slot + 1); upd(&mut e, &[&ma, &mb], p * 100_000_000); }
    switch(&mut e, &ma);
    let mut ra = vec![r(&ma.market), w(&rk), w(&cu), r(&mb.market)]; let _ = &mut ra;
    e.send("", ra, vec![25], &[], true);
    let crank = vec![w(&tu), w(&ma.market), w(&ma.ins), w(&rk), w(&cu), w(&a0), w(&b0)];
    let r1 = try_send(&mut e, crank, vec![6], &[]);
    let full = vec![w(&tu), w(&ma.market), w(&ma.ins), w(&rk), w(&cu), r(&mb.market), w(&a0), w(&b0)];
    let r2 = try_send(&mut e, full, vec![6], &[]);
    println!("F-6: crank-shaped ADL -> {:?}; with the candidate's market -> {:?}", r1, r2);
    assert_eq!((r1, r2), (Some(20), None), "fixed: a candidate whose markets are missing is skipped, not fatal");
}

// =====================================================================================================================
// F-7 (HIGH, regression of 569be1b): in a hybrid market (shards + registered FastBook for the side) a target that keeps ONE
// resting order in the market escapes both permissionless liquidations while maker quotes could absorb it:
//  - liquidate_book cancels its orders, finds nothing in the shards, and must end in NoFill (book registered) -> reverts,
//    so the cancels revert too;
//  - liquidate_fast never cancels and refuses with MissingShards while n_orders != 0.
// The crank (do_liq) then stops (MissingShards = "reload shards"); only a capitalised takeover or ADL once bankrupt remain.
// Control: same account without the resting order is liquidated by liquidate_fast.
// =====================================================================================================================
fn f7_setup(rest_order: bool) -> (Option<u32>, Option<u32>) {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let (a0, a9, b0) = (e.shard(1, 100_000), e.shard(1, 150_000), e.shard(0, 50_000)); let m = e.market;
    let f = fast_new(&mut e, 0);
    let k = maker(&mut e, &f, 10_000_000_000, 1_000_000_000);
    let (lp, lpu) = e.user(50_000_000_000); let (t, tu) = e.user(1_500_000);
    e.send("", trade_accts(&lp, &lpu, &m, &[a0]), single(place_item(1, 1, 100_000, 200, 0, 0)), &[&lp], true);
    e.send("", trade_accts(&t, &tu, &m, &[a0]), single(place_item(0, IOC, 100_000, 130, 0, 0)), &[&t], true);
    if rest_order { e.send("", trade_accts(&t, &tu, &m, &[a9]), single(place_item(1, 1, 150_000, 1, 0, 0)), &[&t], true); }
    e.set_time(e.slot + 1); e.oracle_update("", 920_00000000, None);
    quote(&mut e, &k, &[(BID, 91_900, 1000)]);
    let ins = e.ins;
    let r_book = try_send(&mut e, vec![w(&tu), r(&m), w(&ins), w(&b0), w(&a0), w(&a9)], amt(34, u64::MAX), &[]);
    let mut d = vec![70u8, 1]; d.extend_from_slice(&u64::MAX.to_le_bytes());
    let r_fast = try_send(&mut e, vec![w(&tu), r(&m), w(&ins), w(&f), r(&k.slab), w(&b0), w(&a0), w(&a9)], d, &[]);
    println!("F-7 (resting order: {}): liquidate_book -> {:?}, liquidate_fast -> {:?}, base {}", rest_order, r_book, r_fast, e.user_state(&tu).pos[0].base);
    (r_book, r_fast)
}
#[test] fn f7_one_resting_order_blocks_hybrid_liquidation() {
    assert_eq!(f7_setup(false), (Some(25), None), "control: maker quotes absorb the liquidation");
    assert_eq!(f7_setup(true), (Some(25), None), "fixed: liquidate_fast cancels the target's orders itself");
}
