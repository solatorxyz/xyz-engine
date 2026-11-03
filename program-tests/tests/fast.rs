// Fast quote path: real SBF program against margin, conservation and abuse cases.
#[path = "../../program/src/state.rs"]
#[allow(dead_code)]
mod state;
include!("common.rs");
const PX: u64 = ORACLE;
struct Mk { kp: Keypair, u: Address, slab: Address }
fn fast_new(e: &mut Env, idx: u64) -> Address {
    let k = Address::new_unique(); e.prog_acct(k, FAST_SIZE);
    let mut d = vec![61u8]; d.extend_from_slice(&idx.to_le_bytes());
    let (a, m) = (e.admin.pubkey(), e.market);
    e.send("", vec![AccountMeta::new(a, true), r(&m), w(&k)], d, &[], true); k
}
fn maker(e: &mut Env, f: &Address, dep: u64, fund: i64) -> Mk {
    let (kp, u) = e.user(dep);
    let slab = Address::new_unique(); e.prog_acct(slab, SLAB_SIZE);
    e.send("", vec![AccountMeta::new(kp.pubkey(), true), r(&u), w(&slab)], vec![60], &[&kp], true);
    let (a, m) = (e.admin.pubkey(), e.market);
    e.send("", vec![AccountMeta::new(a, true), r(&m), w(f), r(&slab)], vec![62], &[], true);
    let mk = Mk { kp, u, slab };
    if fund != 0 { fund_m(e, f, &mk, fund, true); }
    mk
}
fn fund_m(e: &mut Env, f: &Address, k: &Mk, amt: i64, ok: bool) {
    let mut d = vec![64u8]; d.extend_from_slice(&amt.to_le_bytes());
    let m = e.market;
    e.send("", vec![AccountMeta::new(k.kp.pubkey(), true), w(&k.u), r(&m), w(f)], d, &[&k.kp], ok);
}
fn items(v: &[(u16, u32, u32)]) -> Vec<u8> { let mut d = vec![63u8]; for (i, p, q) in v { d.extend_from_slice(&i.to_le_bytes()); d.extend_from_slice(&p.to_le_bytes()); d.extend_from_slice(&q.to_le_bytes()); } d }
fn quote(e: &mut Env, k: &Mk, v: &[(u16, u32, u32)], ok: bool) -> Option<u64> { e.send("", vec![AccountMeta::new(k.kp.pubkey(), true), w(&k.slab)], items(v), &[&k.kp], ok) }
fn take_ix(side: u8, fl: u8, ns: u8, limit: u64, qty: u64) -> Vec<u8> { let mut d = vec![66u8, side, fl, ns]; d.extend_from_slice(&limit.to_le_bytes()); d.extend_from_slice(&qty.to_le_bytes()); d }
fn tk(e: &mut Env, kp: &Keypair, u: &Address, f: &Address, slabs: &[Address], rest: &[Address], side: u8, limit: u64, qty: u64, ok: bool) -> Option<u64> {
    let m = e.market;
    let mut a = vec![AccountMeta::new(kp.pubkey(), true), w(u), r(&m), w(f)]; a.extend(slabs.iter().map(r)); a.extend(rest.iter().map(w));
    e.send("", a, take_ix(side, 0, slabs.len() as u8, limit, qty), &[kp], ok)
}
fn fb(e: &Env, f: &Address) -> Box<FastBook> { let d = e.data(f); let mut b: Box<std::mem::MaybeUninit<FastBook>> = Box::new_uninit(); unsafe { std::ptr::copy_nonoverlapping(d.as_ptr(), b.as_mut_ptr() as *mut u8, FAST_SIZE); b.assume_init() } }
fn settle(e: &mut Env, f: &Address, k: &Mk) { let m = e.market; e.send("", vec![w(&k.u), r(&m), w(f)], vec![65], &[], true); }
const ASK: u16 = 8; // idx 0, side ask, level 0
fn setup(e: &mut Env) -> (Address, Mk) { e.oracle_update("", 1000_00000000, None); let f = fast_new(e, 0); let k = maker(e, &f, 10_000_000_000, 1_000_000_000); (f, k) }

#[test] fn quote_take_settle_conserves() {
    let mut e = Env::new(); let (f, k) = setup(&mut e);
    let (tp, tu) = e.user(10_000_000_000);
    quote(&mut e, &k, &[(ASK, 100_010, 100), (ASK + 1, 100_020, 100), (0, 99_990, 100)], true);
    tk(&mut e, &tp, &tu, &f, &[k.slab], &[], 0, PX + 100, 130, true);
    let t = e.user_state(&tu); assert_eq!(t.pos[0].base, 130);
    let fbk = fb(&e, &f); assert_eq!(fbk.makers[0].base, -130);
    let nt = 100_010i64 * 100 + 100_020 * 30; assert_eq!(fbk.makers[0].quote, nt);
    let fee = fbk.fee_accum; assert_eq!(fee, (100_010i64 * 100 + 1999) / 2000 + (100_020i64 * 30 + 1999) / 2000);
    assert_eq!(t.cash, 10_000_000_000 - nt - fee);
    // the same quote can't be spent twice
    tk(&mut e, &tp, &tu, &f, &[k.slab], &[], 0, PX + 100, 1000, true);
    let t2 = e.user_state(&tu); assert_eq!(t2.pos[0].base, 200, "only 200 lots were quoted");
    settle(&mut e, &f, &k);
    let u = e.user_state(&k.u); assert_eq!(u.pos[0].base, -200);
    let b2 = fb(&e, &f); assert_eq!((b2.makers[0].base, b2.makers[0].quote), (0, 0));
    assert_eq!(b2.makers[0].budget, 10_000_000_000);
    let sum = u.cash as i128 + t2.cash as i128 + b2.makers[0].margin as i128 + b2.fee_accum as i128;
    assert_eq!(sum, e.vault_amount() as i128, "cash + locked margin + fees == vault");
    assert_eq!(u.pos[0].base + t2.pos[0].base, 0);
    let (m, ins) = (e.market, e.ins); let _ = ins;
    e.send("", vec![w(&m), w(&f)], vec![16], &[], true);
    let ms = e.market_state(); assert_eq!(ms.oi_long, 200); assert_eq!(ms.fees, b2.fee_accum);
}
#[test] fn abuse_is_rejected() {
    let mut e = Env::new(); let (f, k) = setup(&mut e);
    let (tp, tu) = e.user(10_000_000_000);
    let (xp, _) = e.user(1000);
    // someone else can't write a maker's slab
    e.send("", vec![AccountMeta::new(xp.pubkey(), true), w(&k.slab)], items(&[(ASK, 1, 1)]), &[&xp], false);
    quote(&mut e, &k, &[(4096, 100_010, 1)], false);
    e.send("", vec![AccountMeta::new(k.kp.pubkey(), true), w(&k.slab)], vec![63u8, 1, 2, 3], &[&k.kp], false);
    quote(&mut e, &k, &[(ASK, 100_010, 100)], true);
    // a slab the admin never approved, and a slab-shaped account owned by someone else
    let (other_kp, other_u) = e.user(1_000_000); let rogue = Address::new_unique(); e.prog_acct(rogue, SLAB_SIZE);
    e.send("", vec![AccountMeta::new(other_kp.pubkey(), true), r(&other_u), w(&rogue)], vec![60], &[&other_kp], true);
    tk(&mut e, &tp, &tu, &f, &[rogue], &[], 0, PX + 100, 10, false);
    let fake = Address::new_unique(); let d = e.data(&k.slab);
    e.svm.set_account(fake, Account { lamports: 1_000_000_000, data: d, owner: Address::new_unique(), executable: false, rent_epoch: 0 }).unwrap();
    tk(&mut e, &tp, &tu, &f, &[fake], &[], 0, PX + 100, 10, false);
    // only the admin approves makers
    let (a, m) = (e.admin.pubkey(), e.market); let _ = a;
    e.send("", vec![AccountMeta::new(xp.pubkey(), true), r(&m), w(&f), r(&rogue)], vec![62], &[&xp], false);
    // a user that doesn't own the account can't init a slab for it
    let s2 = Address::new_unique(); e.prog_acct(s2, SLAB_SIZE);
    e.send("", vec![AccountMeta::new(xp.pubkey(), true), r(&k.u), w(&s2)], vec![60], &[&xp], false);
    // can't reuse a fresh account twice
    let (ap, m2) = (e.admin.pubkey(), e.market);
    let mut d = vec![61u8]; d.extend_from_slice(&0u64.to_le_bytes());
    e.send("", vec![AccountMeta::new(ap, true), r(&m2), w(&f)], d, &[], false);
    // idx out of range
    let k3 = Address::new_unique(); e.prog_acct(k3, FAST_SIZE);
    let mut d = vec![61u8]; d.extend_from_slice(&128u64.to_le_bytes());
    e.send("", vec![AccountMeta::new(ap, true), r(&m2), w(&k3)], d, &[], false);
    // can't lock more margin than cash, can't pull margin with unsettled fills
    fund_m(&mut e, &f, &k, 100_000_000_000, false);
    tk(&mut e, &tp, &tu, &f, &[k.slab], &[], 0, PX + 100, 10, true);
    fund_m(&mut e, &f, &k, -1, false);
    settle(&mut e, &f, &k);
    fund_m(&mut e, &f, &k, -1, true);
    fund_m(&mut e, &f, &k, -2_000_000_000, false);
}
#[test] fn stale_off_band_killed_and_selftrade_quotes_are_skipped() {
    let mut e = Env::new(); let (f, k) = setup(&mut e);
    let (tp, tu) = e.user(10_000_000_000);
    quote(&mut e, &k, &[(ASK, 100_010, 100), (ASK + 1, 200_000, 100), (ASK + 2, 1, 100)], true);
    let s = e.slot;
    e.set_time(s + 2); e.oracle_update("", 1000_00000000, None);
    tk(&mut e, &tp, &tu, &f, &[k.slab], &[], 0, u64::MAX >> 2, 150, true);
    assert_eq!(e.user_state(&tu).pos[0].base, 100, "off-band levels never fill");
    quote(&mut e, &k, &[(ASK, 100_010, 100)], true);
    let s = e.slot; e.set_time(s + 31); e.oracle_update("", 1000_00000000, None);
    tk(&mut e, &tp, &tu, &f, &[k.slab], &[], 0, PX + 100, 10, true);
    assert_eq!(e.user_state(&tu).pos[0].base, 100, "aged quote never fills");
    quote(&mut e, &k, &[(ASK, 100_010, 100)], true);
    let mut d = vec![67u8, 1];
    e.send("", vec![AccountMeta::new(k.kp.pubkey(), true), w(&k.slab)], std::mem::take(&mut d), &[&k.kp], true);
    tk(&mut e, &tp, &tu, &f, &[k.slab], &[], 0, PX + 100, 10, true);
    assert_eq!(e.user_state(&tu).pos[0].base, 100, "killed maker never fills");
    e.send("", vec![AccountMeta::new(k.kp.pubkey(), true), w(&k.slab)], vec![67u8, 0], &[&k.kp], true);
    let (kp2, u2) = (&k.kp, k.u);
    tk(&mut e, kp2, &u2, &f, &[k.slab], &[], 0, PX + 100, 10, true);
    assert_eq!(e.user_state(&u2).pos[0].base, 0, "no self-trade");
    // stale oracle blocks new risk
    let s = e.slot; e.set_time(s + 10);
    tk(&mut e, &tp, &tu, &f, &[k.slab], &[], 0, PX + 100, 10, false);
}
#[test] fn budget_caps_maker_exposure_and_taker_margin_holds() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None); let f = fast_new(&mut e, 0);
    let k = maker(&mut e, &f, 10_000_000_000, 10_001_000); // 100.01M notional at 10% IMR: 1000 lots at 100_010
    let (tp, tu) = e.user(100_000_000_000);
    quote(&mut e, &k, &[(ASK, 100_010, 100_000)], true);
    tk(&mut e, &tp, &tu, &f, &[k.slab], &[], 0, PX + 100, 5000, true);
    assert_eq!(e.user_state(&tu).pos[0].base, 1000, "fills stop at the maker budget, the taker is not failed");
    let (sp, su) = e.user(1_000_000);
    settle(&mut e, &f, &k);
    quote(&mut e, &k, &[(ASK, 100_010, 100_000)], true);
    tk(&mut e, &sp, &su, &f, &[k.slab], &[], 0, PX + 100, 1000, false); // 100M notional on 1M equity
}
#[test] fn best_price_wins_across_makers_and_book() {
    let mut e = Env::new(); let (f, k) = setup(&mut e); let k2 = maker(&mut e, &f, 10_000_000_000, 1_000_000_000);
    let a0 = e.shard(1, 100_000); let (rm, ru) = e.user(10_000_000_000);
    let (tp, tu) = e.user(10_000_000_000);
    let m = e.market;
    let mut d = vec![1u8]; d.extend_from_slice(&place_item(1, 1, 100_015, 50, 0, 0));
    e.send("", trade_accts(&rm, &ru, &m, &[a0]), d, &[&rm], true);
    quote(&mut e, &k, &[(ASK, 100_020, 40)], true);
    quote(&mut e, &k2, &[(ASK, 100_010, 30)], true);
    tk(&mut e, &tp, &tu, &f, &[k.slab, k2.slab], &[a0], 0, PX + 100, 100, true);
    let b = fb(&e, &f); assert_eq!((b.makers[0].base, b.makers[1].base), (-20, -30), "100_010 then book 100_015 then 100_020");
    assert_eq!(e.shard_state(&a0).n, 0);
    let t = e.user_state(&tu); assert_eq!(t.pos[0].base, 100);
    assert_eq!(t.pos[0].cost, 100_010 * 30 + 100_015 * 50 + 100_020 * 20);
}
#[test] fn time_priority_breaks_price_ties() {
    let mut e = Env::new(); let (f, k) = setup(&mut e); let k2 = maker(&mut e, &f, 10_000_000_000, 1_000_000_000);
    let (tp, tu) = e.user(10_000_000_000);
    quote(&mut e, &k2, &[(ASK, 100_010, 30)], true);
    let s = e.slot; e.set_time(s + 1); e.oracle_update("", 1000_00000000, None);
    quote(&mut e, &k, &[(ASK, 100_010, 30)], true);
    tk(&mut e, &tp, &tu, &f, &[k.slab, k2.slab], &[], 0, PX + 100, 20, true);
    let b = fb(&e, &f); assert_eq!((b.makers[0].base, b.makers[1].base), (0, -20));
}
#[test] fn underwater_maker_can_be_liquidated_after_settle() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None); let f = fast_new(&mut e, 0);
    let k = maker(&mut e, &f, 2_100_000_000, 2_000_000_000);
    let (tp, tu) = e.user(5_000_000_000_000);
    quote(&mut e, &k, &[(ASK, 100_010, 190_000)], true);
    tk(&mut e, &tp, &tu, &f, &[k.slab], &[], 0, PX + 100, 190_000, true);
    e.oracle_update("", 1049_00000000, None); e.oracle_update("", 1060_00000000, None);
    settle(&mut e, &f, &k);
    let b = fb(&e, &f); let u = e.user_state(&k.u);
    assert_eq!(b.makers[0].margin, 0, "margin returned, quoting stopped");
    assert_eq!(b.makers[0].budget, 0); assert_eq!(u.pos[0].base, -190_000);
    let (lp, lu) = e.user(5_000_000_000_000);
    let (m, ins) = (e.market, e.ins);
    let mut d = vec![5u8]; d.extend_from_slice(&u64::MAX.to_le_bytes());
    e.send("", vec![AccountMeta::new(lp.pubkey(), true), w(&lu), w(&k.u), r(&m), w(&ins)], d, &[&lp], true);
    assert_eq!(e.user_state(&k.u).pos[0].base, 0);
}
#[test] fn compute_units() {
    let mut e = Env::new(); let (f, k) = setup(&mut e); let k2 = maker(&mut e, &f, 10_000_000_000, 1_000_000_000);
    let (tp, tu) = e.user(10_000_000_000);
    let v: Vec<(u16, u32, u32)> = (0..95).map(|i| (i as u16, if (i & 8) != 0 { 100_010 + i as u32 } else { 99_990 - i as u32 }, 1000)).collect();
    println!("slab_quote 1: {:?}", quote(&mut e, &k, &v[..1], true));
    println!("slab_quote 95: {:?}", quote(&mut e, &k, &v, true));
    println!("slab_quote 95 #2: {:?}", quote(&mut e, &k2, &v, true));
    println!("take 1 fill 1 slab: {:?}", tk(&mut e, &tp, &tu, &f, &[k.slab], &[], 0, PX + 100, 10, true));
    println!("take 1 fill 2 slabs: {:?}", tk(&mut e, &tp, &tu, &f, &[k.slab, k2.slab], &[], 0, PX + 100, 10, true));
    println!("take 5 fills 2 slabs: {:?}", tk(&mut e, &tp, &tu, &f, &[k.slab, k2.slab], &[], 0, PX + 100, 4000, true));
    let m = e.market;
    println!("settle: {:?}", e.send("", vec![w(&k.u), r(&m), w(&f)], vec![65], &[], true));
}
fn fund_t(e: &mut Env, f: &Address, k: &Mk, amt: i64, target: i64, ok: bool) {
    let mut d = vec![64u8]; d.extend_from_slice(&amt.to_le_bytes()); d.extend_from_slice(&target.to_le_bytes());
    let m = e.market;
    e.send("", vec![AccountMeta::new(k.kp.pubkey(), true), w(&k.u), r(&m), w(f)], d, &[&k.kp], ok);
}
#[test] fn auto_margin_follows_target_from_idle_cash() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None); let f = fast_new(&mut e, 0);
    let k = maker(&mut e, &f, 10_000_000_000, 0);
    fund_t(&mut e, &f, &k, 0, -1, false);
    fund_t(&mut e, &f, &k, 0, 2_000_000_000, true);
    settle(&mut e, &f, &k);
    let b = fb(&e, &f); assert_eq!((b.makers[0].margin, b.makers[0].budget), (2_000_000_000, 20_000_000_000), "settle tops margin up to target");
    assert_eq!(e.user_state(&k.u).cash, 8_000_000_000);
    // fills spend the budget; the next settle refills it from cash without the maker signing anything
    let (tp, tu) = e.user(100_000_000_000);
    quote(&mut e, &k, &[(ASK, 100_010, 100_000)], true);
    tk(&mut e, &tp, &tu, &f, &[k.slab], &[], 0, PX + 100, 100_000, true);
    assert!(fb(&e, &f).makers[0].budget < 20_000_000_000);
    settle(&mut e, &f, &k);
    assert_eq!(fb(&e, &f).makers[0].budget, 20_000_000_000);
    // a target above the account's free collateral stops there
    fund_t(&mut e, &f, &k, 0, 1_000_000_000_000, true);
    settle(&mut e, &f, &k);
    let m1 = fb(&e, &f).makers[0].margin; assert!(m1 > 2_000_000_000);
    settle(&mut e, &f, &k);
    assert_eq!(fb(&e, &f).makers[0].margin, m1, "it stops where the account would fall below initial margin");
    let (m, _) = (e.market, 0); let _ = m;
    tk(&mut e, &tp, &tu, &f, &[k.slab], &[], 0, PX + 100, 1, true);
    // lowering the target hands the excess back
    fund_t(&mut e, &f, &k, 0, 1_000_000_000, true);
    settle(&mut e, &f, &k);
    assert_eq!(fb(&e, &f).makers[0].margin, 1_000_000_000);
    // target 0 = manual: settle leaves margin alone
    fund_t(&mut e, &f, &k, 0, 0, true); fund_m(&mut e, &f, &k, 500_000_000, true);
    settle(&mut e, &f, &k);
    assert_eq!(fb(&e, &f).makers[0].margin, 1_500_000_000);
}
#[test] fn auto_margin_goes_back_when_the_account_is_short() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None); let f = fast_new(&mut e, 0);
    let k = maker(&mut e, &f, 10_000_000_000, 0);
    fund_t(&mut e, &f, &k, 6_500_000_000, 6_500_000_000, true);
    let (tp, tu) = e.user(100_000_000_000);
    quote(&mut e, &k, &[(ASK, 100_010, 500_000)], true);
    tk(&mut e, &tp, &tu, &f, &[k.slab], &[], 0, PX + 100, 500_000, true);
    let q = fb(&e, &f).makers[0].quote;
    settle(&mut e, &f, &k);
    let (u, b) = (e.user_state(&k.u), fb(&e, &f));
    assert_eq!(u.pos[0].base, -500_000);
    assert_eq!(u.cash as i128 + b.makers[0].margin as i128, 10_000_000_000i128 + q as i128, "nothing created or lost");
    assert!(b.makers[0].margin > 0 && b.makers[0].margin < 6_500_000_000, "margin {} handed back to cover initial margin", b.makers[0].margin);
    // the account now meets initial margin again: a taker order that needs exactly the released cash still clears
    assert_eq!(b.makers[0].budget, b.makers[0].margin as i64 * 10);
}
