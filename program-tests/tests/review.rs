#[path = "../../program/src/state.rs"]
#[allow(dead_code)]
mod state;
// Fix-review (round 3) proofs of concept. Each test performs the scenario against the built SBF program and asserts the
// behaviour observed on the CURRENT code (a passing test = the issue is present).
include!("common.rs");

const IOC: u8 = 4;
const LEDGER: u32 = 13; const MISSING_SHARDS: u32 = 19;
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
fn tok(e: &mut Env, owner: &Address, amount: u64) -> Address {
    let s = Address::new_unique(); let mint = Address::from(e.market_state().mint);
    e.svm.set_account(s, Account { lamports: 1_000_000_000, data: token_acct(&mint, owner, amount), owner: TOKEN, executable: false, rent_epoch: 0 }).unwrap(); s
}
fn tok_amt(e: &Env, k: &Address) -> u64 { u64::from_le_bytes(e.data(k)[64..72].try_into().unwrap()) }
fn set_trig(e: &mut Env, kp: &Keypair, u: &Address, slot: u8, kind: u8, side: u8, v: [u64; 6]) {
    let m = e.market; let mut d = vec![20u8, slot, kind, side]; for x in v { d.extend_from_slice(&x.to_le_bytes()); }
    e.send("", vec![AccountMeta::new(kp.pubkey(), true), w(u), r(&m)], d, &[kp], true);
}

// ---------------------------------------------------------------------------------------------------------------------
// R-1: TWAP slice on an ISOLATED position. The tip now comes out of cross cash BEFORE the check, and the check became
// phealth() (= the isolated bucket only). Nothing looks at cross cash any more, so every slice moves MAX_KEEPER_TIP from the
// user's cross cash to the keeper, without limit. Attacker = user + keeper; keeper withdraws real vault tokens.
// ---------------------------------------------------------------------------------------------------------------------
#[test] fn r1_twap_tip_on_isolated_position_drains_cross_cash() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let a0 = e.shard(1, 100_000); let m = e.market;
    let (mm, mmu) = e.user(50_000_000_000);
    e.send("", trade_accts(&mm, &mmu, &m, &[a0]), single(place_item(1, 1, 100_000, 1_000, 0, 0)), &[&mm], true);
    let (x, xu) = e.user(1_000_000);            // attacker: 1 USDC
    let (k, ku) = e.user(0);                    // attacker's keeper account: 0
    // isolate the market and move all collateral but 0 into the isolated bucket
    e.send("", vec![AccountMeta::new(x.pubkey(), true), w(&xu), r(&m)], vec![30, 1], &[&x], true);
    e.send("", vec![AccountMeta::new(x.pubkey(), true), w(&xu), r(&m)], amt(31, 1_000_000), &[&x], true);
    assert_eq!(e.user_state(&xu).cash, 0);
    // TWAP buy: 20 slices of 1 lot, tip = MAX_KEEPER_TIP (1_000_000) per slice
    set_trig(&mut e, &x, &xu, 0, 3, 0, [0, 100_000, 20, 1_000_000, 1, 20]);
    let mut refused = false;
    for _ in 0..20 {
        e.set_time(e.slot + 1); e.oracle_update("", 1000_00000000, None);
        let acc = vec![AccountMeta::new(k.pubkey(), true), w(&ku), w(&xu), r(&m), w(&a0)];
        if try_send(&mut e, acc, vec![21, 0], &[&k]).is_some() { refused = true; break; }
    }
    assert!(refused, "fixed: the tip comes from the isolated bucket and is judged there");
    assert_eq!(e.user_state(&xu).cash, 0, "cross cash untouched");
}

// ---------------------------------------------------------------------------------------------------------------------
// R-2: off-market resting order (pre-existing, not touched by the patch; checked because ADL/insurance math depends on it):
// a bid far above the oracle rests with initial margin computed at the MARK, not at its own price.
// ---------------------------------------------------------------------------------------------------------------------
#[test] fn r2_bid_far_above_oracle_rests_with_mark_margin() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let b0 = e.shard(0, 200_000); let m = e.market;
    let (x, xu) = e.user(2_000_000);               // 2 USDC
    // 1 lot at 2x the oracle: IM at the mark is 10_000; the fill would lose 100_000 (5% of... 50x the margin)
    let r = try_send(&mut e, trade_accts(&x, &xu, &m, &[b0]), single(place_item(0, 1, 200_000, 100, 0, 0)), &[&x]);
    println!("R-2: place 100 lots @2x oracle with 2 USDC -> {:?}", r);
    let (y, yu) = e.user(100_000_000);
    if r.is_none() {
        e.send("", trade_accts(&y, &yu, &m, &[b0]), single(place_item(1, IOC, 200_000, 100, 0, 0)), &[&y], true);
        e.send("", vec![w(&b0), w(&xu)], vec![24], &[], true);
        let s = e.user_state(&xu);
        println!("R-2: maker after fill: cash {} base {} equity {}", s.cash, s.pos[0].base, s.cash as i128 + s.pos[0].base as i128 * 100_000);
    }
    assert_eq!(r, Some(3), "fixed: the immediate loss of a bid above the mark is margined");
}

// ---------- two markets sharing one collateral pool ----------
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

// ---------------------------------------------------------------------------------------------------------------------
// R-3: maker_clear() refuses liquidation / ADL of ANY position of an account with maker margin locked anywhere, and the only
// way to unlock it (permissionless slab_settle) returns it only when the CROSS account is below maintenance. An ISOLATED
// position is judged on its own bucket, so a maker with a healthy cross account and an underwater isolated position can never
// be liquidated (book, takeover, fast) or ADL'd: its isolated loss runs into bad debt.
// ---------------------------------------------------------------------------------------------------------------------
#[test] fn r3_maker_isolated_position_cannot_be_liquidated() {
    let mut e = Env::new();
    let (ma, mb, pa) = pool2(&mut e);
    upd(&mut e, &[&ma, &mb], 1000_00000000);
    // market A: fast book; M joins and locks 1 USDC of maker margin
    switch(&mut e, &ma);
    let f = Address::new_unique(); e.prog_acct(f, FAST_SIZE);
    e.send("", vec![AccountMeta::new(pa.pubkey(), true), w(&ma.market), w(&f)], amt(61, 0), &[&pa], true);
    let (mk, mu) = e.user(100_000_000);
    let slab = Address::new_unique(); e.prog_acct(slab, SLAB_SIZE);
    e.send("", vec![AccountMeta::new(mk.pubkey(), true), r(&mu), w(&slab)], vec![60], &[&mk], true);
    e.send("", vec![AccountMeta::new(pa.pubkey(), true), r(&ma.market), w(&f), w(&slab), AccountMeta::new_readonly(mk.pubkey(), true)], vec![62], &[&pa, &mk], true);
    e.send("", vec![AccountMeta::new(mk.pubkey(), true), w(&mu), r(&ma.market), w(&f)], amt(64, 1_000_000), &[&mk], true);
    // market B: isolated long, 13 lots on 150_000 of isolated margin
    switch(&mut e, &mb);
    let a0 = e.shard_by(1, 100_000, &pa); let b0 = e.shard_by(0, 90_000, &pa);
    let (lp, lpu) = e.user(50_000_000_000);
    e.send("", trade_accts(&lp, &lpu, &mb.market, &[a0]), single(place_item(1, 1, 100_000, 100, 0, 0)), &[&lp], true);
    e.send("", vec![AccountMeta::new(mk.pubkey(), true), w(&mu), r(&mb.market)], vec![30, 1], &[&mk], true);
    e.send("", vec![AccountMeta::new(mk.pubkey(), true), w(&mu), r(&mb.market), r(&ma.market), r(&f)], amt(31, 150_000), &[&mk], true);
    let mut acc = trade_accts(&mk, &mu, &mb.market, &[a0]); acc.push(r(&ma.market));
    e.send("", acc, single(place_item(0, IOC, 100_000, 13, 0, 0)), &[&mk], true);
    e.send("", trade_accts(&lp, &lpu, &mb.market, &[b0]), single(place_item(0, 1, 90_000, 100, 0, 0)), &[&lp], true);
    // -8%: isolated equity 150_000 - 13*8_000 = 46_000 < maintenance 13*92_000*5% = 59_800
    e.set_time(e.slot + 1); upd(&mut e, &[&ma, &mb], 920_00000000); switch(&mut e, &mb);
    let s = e.user_state(&mu); let pi = s.pos.iter().position(|p| p.market == mb.market.to_bytes()).unwrap();
    println!("R-3: isolated equity {} vs mm {}; cross cash {}", s.pos[pi].iso_margin as i128 + s.pos[pi].base as i128 * 92_000, 13 * 92_000 / 20, s.cash);
    // permissionless settle of the maker's book first (what the crank does): cross is healthy, margin stays locked
    e.send("", vec![w(&mu), r(&ma.market), w(&f), r(&mb.market)], vec![65], &[], true);
    // book liquidation (every shard of B, the book of A, all markets)
    let liq = vec![w(&mu), r(&mb.market), w(&mb.ins), w(&b0), w(&a0), r(&ma.market), r(&f)];
    assert_eq!(try_send(&mut e, liq, amt(34, u64::MAX), &[]), None, "fixed: locked maker margin doesn't block an isolated position");
}

// ---------------------------------------------------------------------------------------------------------------------
// R-4: complete_view() needs EVERY shard (both sides) of EVERY market where the target has a resting order, before any
// liquidation path runs. The shipped crank (keeper/crank.py ix_liq_book / ix_liq_fast / ix_takeover) passes the shards of the
// liquidated market plus the other markets only, so a target with one resting order elsewhere is never liquidated (the crank
// treats MissingShards as "reload shard list" and retries forever). Accounts needed grow with sum(n_shards) of every market
// the target rests in (up to 16 markets), which a target controls with one cheap far-away order per market.
// ---------------------------------------------------------------------------------------------------------------------
#[test] fn r4_resting_order_in_other_market_blocks_crank_liquidation() {
    let mut e = Env::new();
    let (ma, mb, pa) = pool2(&mut e);
    upd(&mut e, &[&ma, &mb], 1000_00000000);
    switch(&mut e, &ma);
    let (a0, b0) = (e.shard_by(1, 100_000, &pa), e.shard_by(0, 90_000, &pa));
    let (lp, lpu) = e.user(50_000_000_000);
    let (t, tu) = e.user(1_500_000);
    e.send("", trade_accts(&lp, &lpu, &ma.market, &[a0]), single(place_item(1, 1, 100_000, 200, 0, 0)), &[&lp], true);
    e.send("", trade_accts(&t, &tu, &ma.market, &[a0]), single(place_item(0, IOC, 100_000, 130, 0, 0)), &[&t], true);
    e.send("", trade_accts(&lp, &lpu, &ma.market, &[b0]), single(place_item(0, 1, 90_000, 1000, 0, 0)), &[&lp], true);
    // market B has 4 shards; the target parks ONE far-away bid there
    switch(&mut e, &mb);
    let sb: Vec<Address> = vec![e.shard_by(0, 50_000, &pa), e.shard_by(0, 90_000, &pa), e.shard_by(1, 100_000, &pa), e.shard_by(1, 110_000, &pa)];
    let mut acc = trade_accts(&t, &tu, &mb.market, &[sb[0]]); acc.push(r(&ma.market));
    e.send("", acc, single(place_item(0, 1, 50_000, 1, 0, 0)), &[&t], true);
    e.set_time(e.slot + 1); upd(&mut e, &[&ma, &mb], 920_00000000); switch(&mut e, &ma);
    // crank-shaped liquidate_book: user, market, insurance, the market's shards, other markets
    let crank = vec![w(&tu), r(&ma.market), w(&ma.ins), w(&a0), w(&b0), r(&mb.market)];
    // its order elsewhere is one more step, never a dead end: refused until IX_LIQ_CANCEL clears that market (permissionless,
    // only for an account below maintenance), then the liquidation goes through with the liquidated market's shards only
    assert_eq!(try_send(&mut e, crank.clone(), amt(34, u64::MAX), &[]), Some(MISSING_SHARDS));
    switch(&mut e, &mb);
    let mut lc = vec![w(&tu), r(&mb.market)]; lc.extend(sb.iter().map(w)); lc.push(r(&ma.market));
    assert_eq!(try_send(&mut e, lc, vec![77], &[]), None, "liq_cancel in market B");
    switch(&mut e, &ma);
    assert_eq!(try_send(&mut e, crank, amt(34, u64::MAX), &[]), None, "fixed: liquidation after liq_cancel");
}

// ---------------------------------------------------------------------------------------------------------------------
// R-5: adl() now calls complete_view() (all shards of every market with resting/unclaimed orders must be passed) but never
// claim_all(): the passed shards are checked and then ignored. Unclaimed maker fills in another market stay outside the
// equity ADL judges: an account whose true equity is positive is treated as bankrupt and its "deficit" is paid by insurance.
// ---------------------------------------------------------------------------------------------------------------------
#[test] fn r5_adl_ignores_unclaimed_fills_it_requires() {
    let mut e = Env::new();
    let (ma, mb, pa) = pool2(&mut e);
    upd(&mut e, &[&ma, &mb], 1000_00000000);
    // insurance of A holds 5 USDC (fees of earlier liquidations)
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
    e.send("", trade_accts(&c, &cu, &ma.market, &[a0]), vec![3, 0, 0, 0, 0], &[&c], true); // C claims: short 130
    // B: T's bid is hit by T's own second account far below its price -> T has an unclaimed +3 USDC profit
    switch(&mut e, &mb);
    let (b0, a1) = (e.shard_by(0, 70_000, &pa), e.shard_by(1, 100_000, &pa));
    let mut acc = trade_accts(&t, &tu, &mb.market, &[b0]); acc.push(r(&ma.market));
    e.send("", acc, single(place_item(0, 1, 70_000, 30, 0, 0)), &[&t], true);
    e.send("", trade_accts(&lp, &lpu, &mb.market, &[b0]), single(place_item(1, IOC, 70_000, 30, 0, 0)), &[&lp], true);
    // A drops 15%: T's visible equity 1.62M - 130*15_000 - fee = -336_500 (bankrupt); with the unclaimed B fill (+30*15_000 at
    // the new mark 85_000) it is +113_500, solvent.
    e.set_time(e.slot + 1); upd(&mut e, &[&ma, &mb], 850_00000000); switch(&mut e, &ma);
    switch(&mut e, &ma); e.rank_user(&cu);
    let ins0 = { let d = e.data(&ma.ins); i64::from_le_bytes(d[40..48].try_into().unwrap()) };
    // ADL of T in A: complete_view demands B's shards (T has a closed order there) ... and then doesn't claim them
    let acc = vec![w(&tu), w(&ma.market), w(&ma.ins), w(&rk), w(&cu), r(&mb.market), w(&b0), w(&a1)];
    let r = try_send(&mut e, acc, vec![6], &[]);
    let ins1 = { let d = e.data(&ma.ins); i64::from_le_bytes(d[40..48].try_into().unwrap()) };
    println!("R-5: adl -> {:?}; insurance {} -> {}; target base A {}", r, ins0, ins1, e.user_state(&tu).pos[0].base);
    assert_eq!(r, Some(10), "fixed: ADL claims the passed shards first; the account is not bankrupt");
}

// ---------------------------------------------------------------------------------------------------------------------
// R-6: "oracle2 can't be dropped" is only half closed: the caller can still drop the PRIMARY. With an OracleSet primary,
// leaving one member out makes read_primary fail (Err(Oracle)), and update_oracle falls back to oracle2 ALONE (only a 5%
// move check vs the stored price). So with the two sources 3% apart -- where passing both trips the 1% divergence breaker --
// the caller instead lands oracle2's price and the market stays open.
// ---------------------------------------------------------------------------------------------------------------------
fn post(px: i64, ts: i64) -> Vec<u8> { let mut d = vec![41u8]; for v in [px, 1_000_000, ts, 3] { d.extend_from_slice(&v.to_le_bytes()); } d }
fn member(e: &mut Env, k: &Keypair) -> Address {
    let a = Address::new_unique(); e.prog_acct(a, state::AGG_SIZE);
    let mut d = vec![40u8]; d.extend_from_slice(&e.feed); d.extend_from_slice(&(-8i32).to_le_bytes()); d.extend_from_slice(&2000u64.to_le_bytes());
    assert_eq!(try_send(e, vec![AccountMeta::new(k.pubkey(), true), w(&a)], d, &[k]), None);
    a
}
#[test] fn r6_drop_a_set_member_to_price_from_oracle2_alone() {
    let mut e = Env::new(); let o = e.oracle; e.prog_acct(o, state::OSET_SIZE);
    let ks: Vec<Keypair> = (0..3).map(|_| Keypair::new()).collect();
    let ms: Vec<Address> = ks.iter().map(|k| member(&mut e, k)).collect();
    let (a, m) = (e.admin.pubkey(), e.market);
    let mut d = vec![43u8, 2, 3]; for x in &ms { d.extend_from_slice(x.as_ref()); }
    assert_eq!(try_send(&mut e, vec![AccountMeta::new(a, true), w(&o)], d, &[]), None);
    let post_all = |e: &mut Env, px: i64| { let ts = e.ts; for i in 0..3 { let (k, x) = (&ks[i], ms[i]);
        assert_eq!(try_send(e, vec![AccountMeta::new(k.pubkey(), true), w(&x)], post(px, ts), &[k]), None); } };
    post_all(&mut e, 1000_00000000);
    let upd = |e: &mut Env, mem: &[Address], extra: &[Address]| { let mut acc = vec![w(&m), r(&o)]; for x in mem { acc.push(r(x)); } for x in extra { acc.push(r(x)); } try_send(e, acc, vec![4], &[]) };
    assert_eq!(upd(&mut e, &ms, &[]), None);
    // oracle2 (Pyth) configured through the timelock
    let o2 = Address::new_unique(); let feed2 = [9u8; 32];
    let mut d = vec![28u8, 10]; d.extend_from_slice(&0u64.to_le_bytes()); d.extend_from_slice(o2.as_ref()); d.extend_from_slice(&feed2);
    e.send("", vec![AccountMeta::new(a, true), w(&m)], d, &[], true);
    e.set_time(e.slot + 216_000); e.send("", vec![w(&m)], vec![29], &[], true);
    let put2 = |e: &mut Env, px: i64| { let d = pyth_data(&feed2, px, (px / 10000) as u64, e.ts, e.slot);
        e.svm.set_account(o2, Account { lamports: 1_000_000_000, data: d, owner: PYTH, executable: false, rent_epoch: 0 }).unwrap(); };
    e.set_time(e.slot + 1); post_all(&mut e, 1000_00000000); put2(&mut e, 1000_00000000);
    assert_eq!(upd(&mut e, &ms, &[o2]), None);
    // the sources now disagree by 3%
    e.set_time(e.slot + 3); post_all(&mut e, 1000_00000000); put2(&mut e, 970_00000000);
    // attacker: every member but one, plus oracle2 -> oracle2's price lands, nothing trips
    assert_eq!(upd(&mut e, &ms[..2], &[o2]), Some(19), "fixed: a set member left out is refused, no fallback to oracle2 alone");
}

// ---------------------------------------------------------------------------------------------------------------------
// R-7: a market with BOTH resting-book shards and a registered FastBook for the side can no longer open the ADL window:
// liquidate_book now returns NoFill when fast_book[side] is set ("proven by liquidate_fast"), but liquidate_fast refuses to
// mark eligibility whenever m.n_shards != 0. A position below maintenance with an empty band stays open until bankrupt.
// ---------------------------------------------------------------------------------------------------------------------
#[test] fn r7_hybrid_market_has_no_adl_window() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let (a0, b0) = (e.shard(1, 100_000), e.shard(0, 50_000)); let m = e.market;
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
    e.set_time(e.slot + 1); e.oracle_update("", 920_00000000, None);
    let ins = e.ins;
    let r1 = try_send(&mut e, vec![w(&tu), r(&m), w(&ins), w(&b0), w(&a0)], amt(34, u64::MAX), &[]);
    let mut d = vec![70u8, 1]; d.extend_from_slice(&u64::MAX.to_le_bytes());
    let r2 = try_send(&mut e, vec![w(&tu), r(&m), w(&ins), w(&f), r(&slab)], d.clone(), &[]);
    assert_eq!((r1, r2), (Some(1), Some(19)), "book path refused with maker quotes on the side; fast path needs every opposite shard");
    let r3 = try_send(&mut e, vec![w(&tu), r(&m), w(&ins), w(&f), r(&slab), r(&b0)], d, &[]);
    assert_eq!(r3, None, "fixed: every maker and every opposite shard passed, all empty in the band");
    assert_ne!(e.user_state(&tu).pos[0].flags & 2, 0, "ADL-eligible");
}

// ---------------------------------------------------------------------------------------------------------------------
// R-8: all_shards() requires every opposite-side shard, but take() refuses more than MAX_TAKE_SHARDS (16) shards of a side.
// Nothing caps shards per side (init_shard allows up to 2^23), so a market with a 17th shard on a side can never run
// liquidate_book (or exec_trigger) into that side again.
// ---------------------------------------------------------------------------------------------------------------------
#[test] fn r8_seventeen_bid_shards_brick_book_liquidation() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let a0 = e.shard(1, 100_000); let m = e.market;
    let bids: Vec<Address> = (0..16u64).map(|i| e.shard(0, 99_900 - 100 * i)).collect();
    // fixed: a 17th shard on one side is refused at creation (16 = what one take walks)
    let k = Address::new_unique(); e.prog_acct(k, SHARD_SIZE);
    let (adm, mm_) = (e.admin.pubkey(), e.market); let mut d = vec![11u8, 0]; d.extend_from_slice(&98_200u64.to_le_bytes());
    assert_eq!(try_send(&mut e, vec![AccountMeta::new(adm, true), w(&mm_), w(&k)], d, &[]), Some(12));
    return;
    let (lp, lpu) = e.user(50_000_000_000); let (t, tu) = e.user(1_500_000);
    e.send("", trade_accts(&lp, &lpu, &m, &[a0]), single(place_item(1, 1, 100_000, 200, 0, 0)), &[&lp], true);
    e.send("", trade_accts(&t, &tu, &m, &[a0]), single(place_item(0, IOC, 100_000, 130, 0, 0)), &[&t], true);
    e.send("", trade_accts(&lp, &lpu, &m, &[bids[0]]), single(place_item(0, 1, 99_900, 1000, 0, 0)), &[&lp], true);
    e.set_time(e.slot + 1); e.oracle_update("", 960_00000000, None);
    e.set_time(e.slot + 1); e.oracle_update("", 920_00000000, None);
    let ins = e.ins;
    let mut all = vec![w(&tu), r(&m), w(&ins), w(&a0)]; all.extend(bids.iter().map(w));
    let r_all = try_send(&mut e, all, amt(34, u64::MAX), &[]);
    let mut some = vec![w(&tu), r(&m), w(&ins), w(&a0)]; some.extend(bids[..16].iter().map(w));
    let r_16 = try_send(&mut e, some, amt(34, u64::MAX), &[]);
    println!("R-8: all 17 bid shards -> {:?}; 16 -> {:?}", r_all, r_16);
    assert_eq!((r_all, r_16), (Some(2), Some(MISSING_SHARDS)));
}

// ---------------------------------------------------------------------------------------------------------------------
// R-9: adl() now refuses a target with resting orders in the market ("cancelled by a liquidation first"), but the
// liquidation that would cancel them reverts as a whole with UseAdl whenever the deficit exceeds insurance (the cancels
// revert with it). A bankrupt account that keeps one resting order in the market can be neither liquidated nor ADL'd.
// ---------------------------------------------------------------------------------------------------------------------
#[test] fn r9_resting_order_makes_bankrupt_account_unclosable() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let (a0, a9, b0) = (e.shard(1, 100_000), e.shard(1, 150_000), e.shard(0, 84_900)); let m = e.market;
    let (lp, lpu) = e.user(50_000_000_000); let (t, tu) = e.user(1_500_000); let (c, cu) = e.user(50_000_000);
    e.send("", trade_accts(&c, &cu, &m, &[a0]), single(place_item(1, 1, 100_000, 130, 0, 0)), &[&c], true);
    e.send("", trade_accts(&t, &tu, &m, &[a0]), single(place_item(0, IOC, 100_000, 130, 0, 0)), &[&t], true);
    e.send("", trade_accts(&c, &cu, &m, &[a0]), vec![3, 0, 0, 0, 0], &[&c], true);
    // the target keeps one tiny ask far above the market
    e.send("", trade_accts(&t, &tu, &m, &[a9]), single(place_item(1, 1, 150_000, 1, 0, 0)), &[&t], true);
    e.send("", trade_accts(&lp, &lpu, &m, &[b0]), single(place_item(0, 1, 84_900, 1000, 0, 0)), &[&lp], true);
    for p in [950, 900, 850] { e.set_time(e.slot + 1); e.oracle_update("", p * 100_000_000, None); } // bankrupt, insurance 0
    e.rank_user(&cu);
    let ins = e.ins; let rk = e.rank;
    let r_book = try_send(&mut e, vec![w(&tu), r(&m), w(&ins), w(&b0), w(&a0), w(&a9)], amt(34, u64::MAX), &[]);
    let r_adl = try_send(&mut e, vec![w(&tu), w(&m), w(&ins), w(&rk), w(&cu), w(&b0), w(&a0), w(&a9)], vec![6], &[]);
    println!("R-9: liquidate_book -> {:?}, adl -> {:?}, target base {} n_orders {}", r_book, r_adl, e.user_state(&tu).pos[0].base, e.user_state(&tu).pos[0].n_orders);
    assert_eq!((r_book, r_adl), (Some(23), None), "fixed: ADL cancels the target's resting order itself");
}
