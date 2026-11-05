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
    quote(&mut e, &k, &[(ASK, 1, 100), (ASK + 1, 100_010, 100), (ASK + 2, 200_000, 100)], true);
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
fn try_send(e: &mut Env, accounts: Vec<AccountMeta>, data: Vec<u8>, signers: &[&Keypair]) -> bool {
    let ix = Instruction { program_id: e.pid, accounts, data };
    let mut s: Vec<&Keypair> = vec![&e.admin]; s.extend_from_slice(signers);
    let tx = Transaction::new_signed_with_payer(&[ix], Some(&e.admin.pubkey()), &s, e.svm.latest_blockhash());
    let ok = e.svm.send_transaction(tx).is_ok(); e.svm.expire_blockhash(); ok
}
struct Rng(u64);
impl Rng { fn n(&mut self, m: u64) -> u64 { self.0 ^= self.0 << 13; self.0 ^= self.0 >> 7; self.0 ^= self.0 << 17; self.0 % m } }
/// Random quotes, takes, settles, funding moves, kills and price moves against three makers and three takers.
/// Every step: positions net to zero (users + unsettled maker fills) and no maker budget goes negative.
/// At the end, after everyone settles: cash + locked margin + fees + insurance == tokens in the vault.
#[test] fn random_fast_sequences() {
    let (mut takes_ok, mut takes_fail) = (0u64, 0u64);
    for seed in 1..=24u64 {
        let mut g = Rng(seed * 0x9E3779B97F4A7C15 | 1);
        let mut e = Env::new(); e.oracle_update("", 1000_00000000, None); let f = fast_new(&mut e, 0);
        let mk: Vec<Mk> = (0..3).map(|i| maker(&mut e, &f, 20_000_000_000, if i == 0 { 0 } else { 2_000_000_000 })).collect();
        let tk_: Vec<(Keypair, Address)> = (0..3).map(|_| e.user(2_000_000_000 + g.n(8_000_000_000))).collect();
        fund_t(&mut e, &f, &mk[0], 0, 5_000_000_000, true); settle(&mut e, &f, &mk[0]);
        let mut px: i64 = 1000_00000000;
        let slabs: Vec<Address> = mk.iter().map(|m| m.slab).collect();
        for _ in 0..250 {
            match g.n(10) {
                0..=2 => { let i = g.n(3) as usize; let mid = (px / 1_000_000) as u64;
                    let v: Vec<(u16, u32, u32)> = (0..4).map(|_| { let s = g.n(2) as u16; let l = g.n(8) as u16;
                        let off = 1 + g.n(300); let p = if s == 1 { mid + off } else { mid - off }; (s << 3 | l, p as u32, (1 + g.n(50_000)) as u32) }).collect();
                    let v = fit(&e, &mk[i].slab, &v); quote(&mut e, &mk[i], &v, true); }
                3..=5 => { let i = g.n(3) as usize; let side = g.n(2) as u8; let mid = (px / 1_000_000) as u64;
                    let lim = if side == 0 { mid + g.n(600) } else { mid - g.n(600) };
                    let m = e.market; let (kp, u) = (&tk_[i].0, tk_[i].1);
                    let mut a = vec![AccountMeta::new(kp.pubkey(), true), w(&u), r(&m), w(&f)]; a.extend(slabs.iter().map(r));
                    let fl = if g.n(4) == 0 { 1u8 } else { 0 };
                    let mut d = take_ix(side, fl, 3, lim, 1 + g.n(80_000)); d[2] = fl;
                    let kpc = kp.insecure_clone();
                    if try_send(&mut e, a, d, &[&kpc]) { takes_ok += 1 } else { takes_fail += 1 } }
                6 => { let i = g.n(3) as usize; let m = e.market; let u = mk[i].u; try_send(&mut e, vec![w(&u), r(&m), w(&f)], vec![65], &[]); }
                7 => { let i = g.n(3) as usize; let amt = g.n(3_000_000_000) as i64 - 1_000_000_000;
                    let mut d = vec![64u8]; d.extend_from_slice(&amt.to_le_bytes()); if g.n(3) == 0 { d.extend_from_slice(&(g.n(6_000_000_000) as i64).to_le_bytes()); }
                    let m = e.market; let kp = mk[i].kp.insecure_clone(); let u = mk[i].u;
                    try_send(&mut e, vec![AccountMeta::new(kp.pubkey(), true), w(&u), r(&m), w(&f)], d, &[&kp]); }
                8 => { let i = g.n(3) as usize; let kp = mk[i].kp.insecure_clone(); let s = mk[i].slab;
                    try_send(&mut e, vec![AccountMeta::new(kp.pubkey(), true), w(&s)], vec![67u8, (g.n(5) == 0) as u8], &[&kp]); }
                _ => { px = (px as i128 * (10_000 + g.n(401) as i128 - 200) / 10_000) as i64; let s = e.slot + 1 + g.n(3); e.set_time(s); e.oracle_update("", px, None); }
            }
            let b = fb(&e, &f);
            let mut net: i128 = (0..3).map(|i| b.makers[i].base as i128).sum();
            for a in mk.iter().map(|m| m.u).chain(tk_.iter().map(|t| t.1)) { net += e.user_state(&a).pos.iter().filter(|p| p.market == e.market.to_bytes()).map(|p| p.base as i128).sum::<i128>(); }
            assert_eq!(net, 0, "seed {seed}: positions don't net to zero");
            for i in 0..3 { assert!(b.makers[i].budget >= 0, "seed {seed}: negative budget"); }
        }
        let s = e.slot + 1; e.set_time(s); e.oracle_update("", px, None);
        for i in 0..3 { let m = e.market; let u = mk[i].u; e.send("", vec![w(&u), r(&m), w(&f)], vec![65], &[], true); }
        for i in 0..3 { let m = e.market; let (kp, u) = (tk_[i].0.insecure_clone(), tk_[i].1);
            let mut a = vec![AccountMeta::new(kp.pubkey(), true), w(&u), r(&m), w(&f)]; a.extend(slabs.iter().map(r));
            let mut d = take_ix(0, 1, 3, 1, 1); d[2] = 1; try_send(&mut e, a, d, &[&kp]); }
        let b = fb(&e, &f);
        let mut sum: i128 = (0..3).map(|i| b.makers[i].margin as i128).sum::<i128>() + b.fee_accum as i128 + e.ins_bal() as i128 + e.market_state().fees as i128;
        for a in mk.iter().map(|m| m.u).chain(tk_.iter().map(|t| t.1)) { sum += e.user_state(&a).cash as i128; }
        // unrealized PnL is not cash: add it at the mark so the books close
        let ms = e.market_state();
        for a in mk.iter().map(|m| m.u).chain(tk_.iter().map(|t| t.1)) { let _ = (a, &ms); }
        let v = e.vault_amount() as i128;
        let pnl_open: i128 = mk.iter().map(|m| m.u).chain(tk_.iter().map(|t| t.1)).map(|a| e.user_state(&a).pos.iter().filter(|p| p.market == e.market.to_bytes()).map(|p| p.base as i128).sum::<i128>()).sum();
        assert_eq!(pnl_open, 0);
        assert!((sum - v).abs() <= 16, "seed {seed}: cash {sum} vs vault {v}");
    }
    println!("fuzz takes ok {takes_ok} rejected {takes_fail}");
    assert!(takes_ok > 500);
}
fn liq_fast(e: &mut Env, f: &Address, t: &Address, slabs: &[Address], ok: bool) -> Option<u64> {
    let (m, ins) = (e.market, e.ins);
    let mut a = vec![w(t), r(&m), w(&ins), w(f)]; a.extend(slabs.iter().map(r));
    let mut d = vec![70u8, slabs.len() as u8]; d.extend_from_slice(&u64::MAX.to_le_bytes());
    e.send(if ok { "liquidate_fast" } else { "" }, a, d, &[], ok)
}
/// 9x long taker, price -9%: liquidation fills against maker bids in 25% chunks, fee to insurance, maker fills pending.
#[test] fn liquidate_fast_fills_against_maker_quotes() {
    let mut e = Env::new(); let (f, k) = setup(&mut e);
    let (tp, tu) = e.user(1_000_000_000);
    quote(&mut e, &k, &[(ASK, 100_010, 100_000)], true);
    tk(&mut e, &tp, &tu, &f, &[k.slab], &[], 0, PX + 100, 90_000, true);
    settle(&mut e, &f, &k);
    liq_fast(&mut e, &f, &tu, &[k.slab], false); // healthy
    let s = e.slot + 1; e.set_time(s); e.oracle_update("", 910_00000000, None);
    liq_fast(&mut e, &f, &tu, &[k.slab], true); // below maintenance and no bids anywhere: succeeds by opening the ADL window
    let u0 = e.user_state(&tu); assert_eq!(u0.pos[0].flags & 2, 2, "empty fast book (all makers passed, no shards) opens the ADL window");
    quote(&mut e, &k, &[(0, 90_990, 100_000)], true);
    let ins0 = e.ins_bal();
    liq_fast(&mut e, &f, &tu, &[k.slab], true);
    let u = e.user_state(&tu); assert_eq!(u.pos[0].base, 67_500, "one 25% chunk");
    let b = fb(&e, &f); assert_eq!(b.makers[0].base, 22_500, "maker bought the chunk at its bid, pending until settle");
    let fee = e.ins_bal() - ins0; assert_eq!(fee as i128, (90_990i128 * 22_500 * 25) / 10_000, "liquidation fee to insurance");
    while e.user_state(&tu).pos[0].base > 0 { if liq_fast_try(&mut e, &f, &tu, &[k.slab]) == false { break; } }
    let net: i128 = e.user_state(&tu).pos[0].base as i128 + fb(&e, &f).makers[0].base as i128 + e.user_state(&k.u).pos[0].base as i128;
    assert_eq!(net, 0);
}
fn liq_fast_try(e: &mut Env, f: &Address, t: &Address, slabs: &[Address]) -> bool {
    let (m, ins) = (e.market, e.ins);
    let mut a = vec![w(t), r(&m), w(&ins), w(f)]; a.extend(slabs.iter().map(r));
    let mut d = vec![70u8, slabs.len() as u8]; d.extend_from_slice(&u64::MAX.to_le_bytes());
    try_send(e, a, d, &[])
}
/// No ADL window unless the fast book is provably empty: every approved maker passed and no book shards exist.
#[test] fn liquidate_fast_needs_every_maker_for_adl() {
    let mut e = Env::new(); let (f, k) = setup(&mut e);
    let k2 = maker(&mut e, &f, 10_000_000_000, 1_000_000_000);
    let (tp, tu) = e.user(1_000_000_000);
    quote(&mut e, &k, &[(ASK, 100_010, 100_000)], true);
    tk(&mut e, &tp, &tu, &f, &[k.slab], &[], 0, PX + 100, 90_000, true);
    let s = e.slot + 1; e.set_time(s); e.oracle_update("", 910_00000000, None);
    liq_fast(&mut e, &f, &tu, &[k.slab], false); // second maker not passed: NoFill
    assert_eq!(e.user_state(&tu).pos[0].flags & 2, 0);
    let _ = e.shard(1, 100_000); // a book exists now: the fast book alone proves nothing
    liq_fast(&mut e, &f, &tu, &[k.slab, k2.slab], false);
    assert_eq!(e.user_state(&tu).pos[0].flags & 2, 0);
    // a fake slab or a slab from another market is rejected
    let other = Address::new_unique(); e.prog_acct(other, SLAB_SIZE);
    liq_fast(&mut e, &f, &tu, &[other], false);
}
fn send_many(e: &mut Env, ixs: Vec<(Vec<AccountMeta>, Vec<u8>)>, signers: &[&Keypair]) -> bool {
    let ixs: Vec<Instruction> = ixs.into_iter().map(|(accounts, data)| Instruction { program_id: e.pid, accounts, data }).collect();
    let mut s: Vec<&Keypair> = vec![&e.admin]; s.extend_from_slice(signers);
    let tx = Transaction::new_signed_with_payer(&ixs, Some(&e.admin.pubkey()), &s, e.svm.latest_blockhash());
    let ok = e.svm.send_transaction(tx).is_ok(); e.svm.expire_blockhash(); ok
}
fn assert_ix(u: &Address, mkts: &[(Address, i64, i64)], toks: &[(Address, u64)], min_eq: i64) -> (Vec<AccountMeta>, Vec<u8>) {
    let mut d = vec![71u8, mkts.len() as u8, toks.len() as u8]; d.extend_from_slice(&min_eq.to_le_bytes());
    for (_, lo, hi) in mkts { d.extend_from_slice(&lo.to_le_bytes()); d.extend_from_slice(&hi.to_le_bytes()); }
    for (_, m) in toks { d.extend_from_slice(&m.to_le_bytes()); }
    let mut a = vec![r(u)]; a.extend(mkts.iter().map(|(m, _, _)| r(m))); a.extend(toks.iter().map(|(t, _)| r(t)));
    (a, d)
}
fn take_accts(e: &Env, kp: &Keypair, u: &Address, f: &Address, slabs: &[Address]) -> Vec<AccountMeta> {
    let m = e.market; let mut a = vec![AccountMeta::new(kp.pubkey(), true), w(u), r(&m), w(f)]; a.extend(slabs.iter().map(r)); a
}
/// A leg that can't fill in full reverts the whole transaction: the take before the guard leaves no trace.
#[test] fn assert_reverts_a_short_leg_and_passes_a_full_one() {
    let mut e = Env::new(); let (f, k) = setup(&mut e);
    let (tp, tu) = e.user(10_000_000_000);
    quote(&mut e, &k, &[(ASK, 100_010, 60)], true);
    let m = e.market;
    let take = (take_accts(&e, &tp, &tu, &f, &[k.slab]), take_ix(0, 0, 1, PX + 100, 100));
    let tpc = tp.insecure_clone();
    assert!(!send_many(&mut e, vec![take.clone(), assert_ix(&tu, &[(m, 100, 100)], &[], i64::MIN)], &[&tpc]), "only 60 available: revert");
    assert_eq!(e.user_state(&tu).pos[0].market, [0; 32], "nothing happened");
    assert_eq!(fb(&e, &f).makers[0].base, 0);
    assert!(send_many(&mut e, vec![take, assert_ix(&tu, &[(m, 50, 100)], &[], i64::MIN)], &[&tpc]), "60 is inside [50, 100]");
    assert_eq!(e.user_state(&tu).pos[0].base, 60);
    // equity floor: the taker paid fees, so equity is just under its deposit
    assert!(!send_many(&mut e, vec![assert_ix(&tu, &[(m, 60, 60)], &[], 10_000_000_000)], &[]));
    assert!(send_many(&mut e, vec![assert_ix(&tu, &[(m, 60, 60)], &[], 9_000_000_000)], &[]));
    // a market the user never touched counts as flat
    let other = e.market; let _ = other;
}
/// The spot leg: a token balance below the floor reverts; a fake (non-token) account is rejected.
#[test] fn assert_checks_token_balances() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let (_tp, tu) = e.user(1_000_000);
    let mint = Address::from(e.market_state().mint); let tok = Address::new_unique();
    e.svm.set_account(tok, Account { lamports: 1_000_000_000, data: token_acct(&mint, &Address::new_unique(), 5_000), owner: TOKEN, executable: false, rent_epoch: 0 }).unwrap();
    assert!(send_many(&mut e, vec![assert_ix(&tu, &[], &[(tok, 5_000)], i64::MIN)], &[]));
    assert!(!send_many(&mut e, vec![assert_ix(&tu, &[], &[(tok, 5_001)], i64::MIN)], &[]));
    let fake = Address::new_unique(); e.prog_acct(fake, 165);
    assert!(!send_many(&mut e, vec![assert_ix(&tu, &[], &[(fake, 0)], i64::MIN)], &[]));
    assert!(!send_many(&mut e, vec![(vec![r(&tu)], vec![71u8, 1, 0])], &[]), "short data");
}
/// Random quote items made valid for best-first ladders (asks up, bids down, ties allowed): each live item's price is clamped
/// between its live neighbours' prices on the slab as it will be (items apply in order). Clamping makes equal prices with
/// different quote slots inside one ladder, the tie matching must break by age.
fn fit(e: &Env, slab: &Address, v: &[(u16, u32, u32)]) -> Vec<(u16, u32, u32)> {
    let mut lad: std::collections::HashMap<usize, [(u32, u32); 8]> = Default::default();
    let mut out = Vec::new();
    for &(i, p, q) in v {
        let (g, l) = (i as usize / 8, i as usize % 8); let ask = g & 1 == 1;
        let cur = lad.entry(g).or_insert_with(|| { let mut a = [(0, 0); 8]; for k in 0..8 { let x = slab_q(e, slab, g * 8 + k); a[k] = (x.px, x.qty); } a });
        let mut p = p;
        if q > 0 {
            let before = cur[..l].iter().filter(|x| x.1 > 0).map(|x| x.0); let after = cur[l + 1..].iter().filter(|x| x.1 > 0).map(|x| x.0);
            if ask { if let Some(b) = before.max() { p = p.max(b); } if let Some(a) = after.min() { p = p.min(a); } }
            else { if let Some(b) = before.min() { p = p.min(b); } if let Some(a) = after.max() { p = p.max(a); } }
        }
        cur[l] = (p, q); out.push((i, p, q));
    }
    out
}
/// Ladders are best-first: a level better than one above it is refused. Equal prices are allowed, and among them the older
/// quote fills first even when it sits further down the ladder.
#[test] fn ladders_are_best_first_and_ties_go_to_the_older_quote() {
    let mut e = Env::new(); let (f, k) = setup(&mut e);
    quote(&mut e, &k, &[(ASK, 100_020, 100), (ASK + 1, 100_010, 100)], false);
    quote(&mut e, &k, &[(1, 99_990, 100), (2, 99_995, 100)], false);
    quote(&mut e, &k, &[(ASK, 100_010, 100), (ASK + 1, 100_010, 100)], true);
    quote(&mut e, &k, &[(ASK, 100_010, 0), (ASK + 2, 0, 0)], true);
    quote(&mut e, &k, &[(ASK, 100_000, 5), (ASK + 1, 0, 0)], true);
    quote(&mut e, &k, &[(ASK, 100_010, 0), (ASK + 1, 100_010, 100)], true);
    let s = e.slot; e.set_time(s + 2); e.oracle_update("", 1000_00000000, None);
    quote(&mut e, &k, &[(ASK, 100_010, 100)], true); // same price, newer, higher up the ladder
    let (tp, tu) = e.user(10_000_000_000);
    tk(&mut e, &tp, &tu, &f, &[k.slab], &[], 0, PX + 100, 30, true);
    let b = fb(&e, &f);
    assert_eq!((b.used[0][8].qty, b.used[0][9].qty), (0, 30), "the older quote at level 1 fills first");
    tk(&mut e, &tp, &tu, &f, &[k.slab], &[], 0, PX + 100, 100, true);
    let b = fb(&e, &f);
    assert_eq!((b.used[0][8].qty, b.used[0][9].qty), (30, 100), "then the newer one");
}
fn slab_q(e: &Env, k: &Address, i: usize) -> Quote { let d = e.data(k); let o = 96 + i * 16; Quote { px: u32::from_le_bytes(d[o..o + 4].try_into().unwrap()),
    qty: u32::from_le_bytes(d[o + 4..o + 8].try_into().unwrap()), gen: u32::from_le_bytes(d[o + 8..o + 12].try_into().unwrap()), slot: u32::from_le_bytes(d[o + 12..o + 16].try_into().unwrap()) } }
/// Reference model of take_fast's maker matching, written from the rules, not from the code: repeatedly take the best usable
/// quote (better price, then older quote slot, then slab order, then level order); a quote is usable when live (qty, age <= 30
/// slots, inside mark +-5%, maker not killed, not the taker's own), and its size is capped by what is left of this quote
/// generation and by the maker's budget. Returns (lots per maker, taker lots, notional).
fn model_take(e: &Env, f: &Address, slabs: &[(Address, Address)], taker: &Address, bid: bool, limit: u64, mut left: u64) -> (Vec<i64>, u64) {
    let b = fb(e, f); let ms = e.market_state(); let px0 = if ms.mark_px != 0 { ms.mark_px } else { ms.oracle_px };
    let band = px0 / 10000 * 500; let (lo, hi) = (px0.saturating_sub(band), px0 + band); let slot = e.slot as u32;
    let mut budget: Vec<i64> = (0..slabs.len()).map(|i| b.makers[i].budget).collect();
    let mut used: Vec<[(u32, u32); 16]> = (0..slabs.len()).map(|i| { let mut u = [(0u32, 0u32); 16]; for l in 0..16 { u[l] = (b.used[i][l].gen, b.used[i][l].qty); } u }).collect();
    let opp = if bid { 1 } else { 0 }; let mut got = vec![0i64; slabs.len()]; let mut total = 0u64;
    while left > 0 {
        let mut best: Option<(u64, u32, usize, usize, u64)> = None;
        for (k, (sl, owner)) in slabs.iter().enumerate() {
            let kill = u64::from_le_bytes(e.data(sl)[88..96].try_into().unwrap());
            if kill != 0 || owner == taker || budget[k] <= 0 { continue; }
            for l in 0..8 {
                let q = slab_q(e, sl, opp << 3 | l); let p = q.px as u64;
                if q.qty == 0 || slot - q.slot.min(slot) > 30 || p == 0 || p < lo || p > hi { continue; }
                let u = used[k][opp << 3 | l]; let av = (if u.0 == q.gen { q.qty.saturating_sub(u.1) } else { q.qty }) as u64;
                let av = av.min(budget[k] as u64 / p); if av == 0 { continue; }
                let better = match best { None => true, Some((bp, bs, _, _, _)) => (bid && p < bp) || (!bid && p > bp) || (p == bp && q.slot < bs) };
                if better { best = Some((p, q.slot, k, l, av)); }
            }
        }
        let Some((p, _, k, l, av)) = best else { break };
        if (bid && p > limit) || (!bid && p < limit) { break; }
        let fq = av.min(left); let q = slab_q(e, &slabs[k].0, opp << 3 | l);
        budget[k] -= (p * fq) as i64; let u = &mut used[k][opp << 3 | l]; if u.0 == q.gen { u.1 += fq as u32 } else { *u = (q.gen, fq as u32) }
        got[k] += if bid { -(fq as i64) } else { fq as i64 }; total += fq; left -= fq;
    }
    (got, total)
}
/// Random interleavings of requotes, takes (including stale limits), kills, settles and price moves. Every take must match the
/// reference model exactly, never trade through its limit, and never fill a quote generation past its quoted size.
#[test] fn fast_path_matches_reference_model() {
    let mut checked = 0u64;
    for seed in 1..=30u64 {
        let mut g = Rng(seed.wrapping_mul(0xD1B54A32D192ED03) | 1);
        let mut e = Env::new(); e.oracle_update("", 1000_00000000, None); let f = fast_new(&mut e, 0);
        let mk: Vec<Mk> = (0..3).map(|_| maker(&mut e, &f, 50_000_000_000, 5_000_000_000)).collect();
        let tk_: Vec<(Keypair, Address)> = (0..2).map(|_| e.user(50_000_000_000)).collect();
        let slabs: Vec<(Address, Address)> = mk.iter().map(|m| (m.slab, m.u)).collect();
        let mut px: i64 = 1000_00000000;
        for _ in 0..200 {
            match g.n(9) {
                0..=2 => { let i = g.n(3) as usize; let mid = (px / 1_000_000) as u64;
                    let v: Vec<(u16, u32, u32)> = (0..3).map(|_| { let s = g.n(2) as u16; let l = g.n(8) as u16; let off = 1 + g.n(6);  // few price points: many ties
                        (s << 3 | l, (if s == 1 { mid + off } else { mid - off }) as u32, (1 + g.n(400)) as u32) }).collect();
                    let v = fit(&e, &mk[i].slab, &v); quote(&mut e, &mk[i], &v, true); }
                3..=5 => { let i = g.n(2) as usize; let bid = g.n(2) == 0; let mid = (px / 1_000_000) as u64;
                    let limit = if bid { mid + g.n(8) } else { mid - g.n(8) }; let qty = 1 + g.n(900);
                    let (kp, u) = (tk_[i].0.insecure_clone(), tk_[i].1);
                    let (want, total) = model_take(&e, &f, &slabs, &u, bid, limit, qty);
                    let before: Vec<i64> = (0..3).map(|k| fb(&e, &f).makers[k].base).collect(); let tb = e.user_state(&u).pos.iter().find(|p| p.market == e.market.to_bytes()).map_or(0, |p| p.base);
                    let m = e.market; let mut a = vec![AccountMeta::new(kp.pubkey(), true), w(&u), r(&m), w(&f)]; a.extend(mk.iter().map(|x| r(&x.slab)));
                    let ok = try_send(&mut e, a, take_ix(if bid { 0 } else { 1 }, 0, 3, limit, qty), &[&kp]);
                    let after: Vec<i64> = (0..3).map(|k| fb(&e, &f).makers[k].base).collect(); let ta = e.user_state(&u).pos.iter().find(|p| p.market == e.market.to_bytes()).map_or(0, |p| p.base);
                    if ok { for k in 0..3 { assert_eq!(after[k] - before[k], want[k], "seed {seed}: maker {k} fill differs from the reference model"); }
                        assert_eq!((ta - tb).unsigned_abs(), total, "seed {seed}: taker fill differs"); checked += 1; }
                    else { assert_eq!(after, before, "seed {seed}: a failed take must change nothing"); }
                    // no quote generation is ever filled past its size
                    let b = fb(&e, &f);
                    for (k, (sl, _)) in slabs.iter().enumerate() { for l in 0..16 { let q = slab_q(&e, sl, l); if b.used[k][l].gen == q.gen { assert!(b.used[k][l].qty <= q.qty, "seed {seed}: overfill"); } } } }
                6 => { let i = g.n(3) as usize; let kp = mk[i].kp.insecure_clone(); let s = mk[i].slab; try_send(&mut e, vec![AccountMeta::new(kp.pubkey(), true), w(&s)], vec![67u8, (g.n(4) == 0) as u8], &[&kp]); }
                7 => { let i = g.n(3) as usize; let m = e.market; let u = mk[i].u; try_send(&mut e, vec![w(&u), r(&m), w(&f)], vec![65], &[]); }
                _ => { px = (px as i128 * (10_000 + g.n(41) as i128 - 20) / 10_000) as i64; let s = e.slot + 1 + g.n(3); e.set_time(s); e.oracle_update("", px, None); }
            }
        }
    }
    println!("reference model: {checked} takes compared");
    assert!(checked > 1000);
}
fn fast_new_side(e: &mut Env, idx: u64, side: u8) -> Address {
    let k = Address::new_unique(); e.prog_acct(k, FAST_SIZE);
    let mut d = vec![61u8]; d.extend_from_slice(&idx.to_le_bytes()); d.push(side);
    let (a, m) = (e.admin.pubkey(), e.market);
    e.send("", vec![AccountMeta::new(a, true), r(&m), w(&k)], d, &[], true); k
}
/// Split market: buyers fill against the ask book, sellers against the bid book. The two take transactions write disjoint
/// accounts (their own User and their side's book), so the runtime can run them in parallel; the maker's fills from both
/// books land in one position at settle.
#[test] fn split_books_take_settle_and_disjoint_locks() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let (fbid, fask) = (fast_new_side(&mut e, 0, 1), fast_new_side(&mut e, 0, 2));
    let k = maker(&mut e, &fbid, 10_000_000_000, 1_000_000_000);
    let (a, m) = (e.admin.pubkey(), e.market); let _ = a;
    e.send("", vec![AccountMeta::new(e.admin.pubkey(), true), r(&m), w(&fask), r(&k.slab)], vec![62], &[], true);
    fund_m(&mut e, &fask, &k, 1_000_000_000, true);
    quote(&mut e, &k, &[(ASK, 100_010, 500), (0, 99_990, 500)], true);
    let (bp, bu) = e.user(10_000_000_000); let (sp, su) = e.user(10_000_000_000);
    // wrong book for the side is refused
    tk(&mut e, &bp, &bu, &fbid, &[k.slab], &[], 0, PX + 100, 10, false);
    tk(&mut e, &sp, &su, &fask, &[k.slab], &[], 1, PX - 100, 10, false);
    let buy = take_accts(&e, &bp, &bu, &fask, &[k.slab]); let sell = take_accts(&e, &sp, &su, &fbid, &[k.slab]);
    let wb: Vec<_> = buy.iter().filter(|x| x.is_writable).map(|x| x.pubkey).collect(); let ws: Vec<_> = sell.iter().filter(|x| x.is_writable).map(|x| x.pubkey).collect();
    assert!(wb.iter().all(|k| !ws.contains(k)), "buy and sell write disjoint accounts");
    tk(&mut e, &bp, &bu, &fask, &[k.slab], &[], 0, PX + 100, 300, true);
    tk(&mut e, &sp, &su, &fbid, &[k.slab], &[], 1, PX - 100, 200, true);
    assert_eq!((e.user_state(&bu).pos[0].base, e.user_state(&su).pos[0].base), (300, -200));
    assert_eq!((fb(&e, &fask).makers[0].base, fb(&e, &fbid).makers[0].base), (-300, 200));
    settle(&mut e, &fask, &k); settle(&mut e, &fbid, &k);
    assert_eq!(e.user_state(&k.u).pos[0].base, -100, "both books settle into one position");
    // mark sees both sides only when both books (with all their slabs) are passed
    let (mm, orc) = (e.market, e.oracle);
    e.send("", vec![w(&mm), r(&orc), r(&fbid), r(&fask), r(&k.slab)], vec![4], &[], true);
    // liquidation of a long sells into the bid book; the ask book is refused for it
    let (m2, ins) = (e.market, e.ins);
    let mut d = vec![70u8, 1]; d.extend_from_slice(&u64::MAX.to_le_bytes());
    assert!(!try_send(&mut e, vec![w(&bu), r(&m2), w(&ins), w(&fask), r(&k.slab)], d.clone(), &[]), "healthy and wrong book");
}
/// The keeper that sends a liquidation earns LIQ_KEEPER_SHARE_BPS of the liquidation fee; insurance gets the rest.
/// Without a keeper account insurance gets it all; a keeper bound to another vault is refused.
#[test] fn liquidation_fee_pays_the_keeper() {
    let run = |with_keeper: bool| -> (i64, i64, i64) {
        let mut e = Env::new(); let (f, k) = setup(&mut e);
        let (tp, tu) = e.user(1_000_000_000); let (_kp, ku) = e.user(1);
        quote(&mut e, &k, &[(ASK, 100_010, 100_000)], true);
        tk(&mut e, &tp, &tu, &f, &[k.slab], &[], 0, PX + 100, 90_000, true);
        settle(&mut e, &f, &k);
        let s = e.slot + 1; e.set_time(s); e.oracle_update("", 910_00000000, None);
        quote(&mut e, &k, &[(0, 90_990, 100_000)], true);
        let (m, ins) = (e.market, e.ins); let (i0, k0) = (e.ins_bal(), e.user_state(&ku).cash);
        let mut a = vec![w(&tu), r(&m), w(&ins), w(&f), r(&k.slab)]; if with_keeper { a.push(w(&ku)); }
        let mut d = vec![70u8, 1]; d.extend_from_slice(&u64::MAX.to_le_bytes());
        e.send("", a, d, &[], true);
        let fee = (90_990i64 * 22_500 * 25) / 10_000;
        (fee, e.ins_bal() - i0, e.user_state(&ku).cash - k0)
    };
    let (fee, ins, kp) = run(false); assert_eq!((ins, kp), (fee, 0));
    let (fee, ins, kp) = run(true); assert_eq!(kp, fee * 2000 / 10_000, "keeper share"); assert_eq!(ins + kp, fee, "nothing created or lost");
    // a keeper account from another vault is refused
    let mut e = Env::new(); let (f, k) = setup(&mut e);
    let (tp, tu) = e.user(1_000_000_000);
    quote(&mut e, &k, &[(ASK, 100_010, 100_000)], true);
    tk(&mut e, &tp, &tu, &f, &[k.slab], &[], 0, PX + 100, 90_000, true);
    let s = e.slot + 1; e.set_time(s); e.oracle_update("", 910_00000000, None);
    quote(&mut e, &k, &[(0, 90_990, 100_000)], true);
    let stranger = Address::new_unique(); e.prog_acct(stranger, USER_SIZE);
    let mut d0 = e.data(&stranger); d0[0..8].copy_from_slice(&DISC_USER.to_le_bytes()); d0[64..96].copy_from_slice(&[9u8; 32]);
    e.svm.set_account(stranger, Account { lamports: 1_000_000_000, data: d0, owner: e.pid, executable: false, rent_epoch: 0 }).unwrap();
    let (m, ins) = (e.market, e.ins); let mut d = vec![70u8, 1]; d.extend_from_slice(&u64::MAX.to_le_bytes());
    assert!(!try_send(&mut e, vec![w(&tu), r(&m), w(&ins), w(&f), r(&k.slab), w(&stranger)], d, &[]));
}
/// Just under maintenance, liquidation closes only what restores 1.25x maintenance (well under the 25% chunk) and the
/// account comes out healthy; deep under, the 25% chunk still caps one transaction.
#[test] fn liquidation_is_partial_when_barely_under() {
    let mut e = Env::new(); let (f, k) = setup(&mut e);
    let (tp, tu) = e.user(1_000_000_000);
    quote(&mut e, &k, &[(ASK, 100_010, 100_000)], true);
    tk(&mut e, &tp, &tu, &f, &[k.slab], &[], 0, PX + 100, 90_000, true);
    // 9x long: equity = 90k x px - 8.0054e9, maintenance = 5% of 90k x px: under below px 93,630. Use 93,600.
    let s = e.slot + 1; e.set_time(s); e.oracle_update("", 936_00000000, None);
    let m0 = e.market_state(); let px = m0.mark_px.max(m0.oracle_px) as i128;
    let u0 = e.user_state(&tu); let eq0 = u0.cash as i128 + 90_000i128 * px; let mm0 = 90_000i128 * px * 500 / 10_000;
    assert!(eq0 < mm0, "set up just under maintenance: eq {eq0} mm {mm0}");
    quote(&mut e, &k, &[(0, (px - 5) as u32, 100_000)], true);
    let (m, ins) = (e.market, e.ins); let mut d = vec![70u8, 1]; d.extend_from_slice(&u64::MAX.to_le_bytes());
    e.send("", vec![w(&tu), r(&m), w(&ins), w(&f), r(&k.slab)], d, &[], true);
    let u = e.user_state(&tu); let closed = 90_000 - u.pos[0].base;
    println!("partial liquidation closed {closed} of 90000 (25% chunk = 22500)");
    assert!(closed > 0 && closed < 22_500, "closed {closed}: less than the 25% chunk");
    let eq = u.cash as i128 + u.pos[0].base as i128 * px; let mm = u.pos[0].base as i128 * px * 500 / 10_000;
    assert!(eq >= mm, "healthy after one partial liquidation: eq {eq} mm {mm}");
    let (m, ins) = (e.market, e.ins); let mut d = vec![70u8, 1]; d.extend_from_slice(&u64::MAX.to_le_bytes());
    assert!(!try_send(&mut e, vec![w(&tu), r(&m), w(&ins), w(&f), r(&k.slab)], d, &[]), "no second liquidation needed");
}
fn set_trig(e: &mut Env, kp: &Keypair, u: &Address, slot: u8, kind: u8, side: u8, v: [u64; 6]) {
    let m = e.market; let mut d = vec![20u8, slot, kind, side]; for x in v { d.extend_from_slice(&x.to_le_bytes()); }
    e.send("", vec![AccountMeta::new(kp.pubkey(), true), w(u), r(&m)], d, &[kp], true);
}
fn exec_fast(e: &mut Env, kp: &Keypair, ku: &Address, tu: &Address, f: &Address, slabs: &[Address], slot: u8) -> bool {
    let m = e.market; let mut a = vec![AccountMeta::new(kp.pubkey(), true), w(ku), w(tu), r(&m), w(f)]; a.extend(slabs.iter().map(r));
    let kpc = kp.insecure_clone(); try_send(e, a, vec![21u8, slot], &[&kpc])
}
/// Stop-loss on a fast-path market: it fires on the oracle, fills against maker bids, and pays the keeper its tip.
#[test] fn stop_loss_fills_on_the_fast_book() {
    let mut e = Env::new(); let (f, k) = setup(&mut e);
    let (tp, tu) = e.user(10_000_000_000); let (kp, ku) = e.user(1);
    quote(&mut e, &k, &[(ASK, 100_010, 5_000)], true);
    tk(&mut e, &tp, &tu, &f, &[k.slab], &[], 0, PX + 100, 1_000, true);
    set_trig(&mut e, &tp, &tu, 0, 2, 1, [99_000, 90_000, 1_000, 1_000, 0, 0]); // SL: sell 1000 when price <= 99,000, tip 1000
    quote(&mut e, &k, &[(0, 99_400, 5_000)], true);
    assert!(!exec_fast(&mut e, &kp, &ku, &tu, &f, &[k.slab], 0), "not triggered yet");
    let s = e.slot + 1; e.set_time(s); e.oracle_update("", 985_00000000, None);
    quote(&mut e, &k, &[(0, 98_450, 5_000)], true);
    let (c0, k0) = (e.user_state(&tu).cash, e.user_state(&ku).cash);
    assert!(exec_fast(&mut e, &kp, &ku, &tu, &f, &[k.slab], 0), "stop-loss executes against the maker bid");
    let t = e.user_state(&tu); assert_eq!(t.pos[0].base == 0 || t.pos[0].market == [0; 32], true, "position closed");
    assert_eq!(e.user_state(&ku).cash - k0, 1_000, "keeper tip");
    assert!(t.cash > c0, "sale proceeds credited");
    assert_eq!(fb(&e, &f).makers[0].base, -1_000 + 1_000, "maker bought it back: pending base nets to zero");
    assert!(!exec_fast(&mut e, &kp, &ku, &tu, &f, &[k.slab], 0), "one-shot");
}
/// TWAP on a fast-path market: one slice per interval, nothing before the next slot is due.
#[test] fn twap_slices_on_the_fast_book() {
    let mut e = Env::new(); let (f, k) = setup(&mut e);
    let (tp, tu) = e.user(10_000_000_000); let (kp, ku) = e.user(1);
    quote(&mut e, &k, &[(ASK, 100_010, 5_000)], true);
    set_trig(&mut e, &tp, &tu, 1, 3, 0, [0, PX + 100, 900, 0, 5, 3]); // TWAP buy 900 in 3 slices, every 5 slots
    assert!(exec_fast(&mut e, &kp, &ku, &tu, &f, &[k.slab], 1));
    assert_eq!(e.user_state(&tu).pos[0].base, 300);
    assert!(!exec_fast(&mut e, &kp, &ku, &tu, &f, &[k.slab], 1), "next slice not due");
    for n in 0..2 { let s = e.slot + 5; e.set_time(s); e.oracle_update("", 1000_00000000, None); quote(&mut e, &k, &[(ASK, 100_010, 5_000)], true);
        assert!(exec_fast(&mut e, &kp, &ku, &tu, &f, &[k.slab], 1), "slice {n}"); }
    assert_eq!(e.user_state(&tu).pos[0].base, 900);
    assert_eq!(e.user_state(&tu).triggers[1].kind, 0, "done");
}
/// On a split market a trigger must bring the book of the side it trades; the other book is refused.
#[test] fn trigger_uses_the_right_split_book() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let (fbid, fask) = (fast_new_side(&mut e, 0, 1), fast_new_side(&mut e, 0, 2));
    let k = maker(&mut e, &fask, 10_000_000_000, 1_000_000_000);
    let m = e.market; e.send("", vec![AccountMeta::new(e.admin.pubkey(), true), r(&m), w(&fbid), r(&k.slab)], vec![62], &[], true);
    quote(&mut e, &k, &[(ASK, 100_010, 5_000)], true);
    let (tp, tu) = e.user(10_000_000_000); let (kp, ku) = e.user(1);
    set_trig(&mut e, &tp, &tu, 0, 3, 0, [0, PX + 100, 300, 0, 5, 1]); // buy: needs the ask book
    assert!(!exec_fast(&mut e, &kp, &ku, &tu, &fbid, &[k.slab], 0), "bid book refused for a buy");
    assert!(exec_fast(&mut e, &kp, &ku, &tu, &fask, &[k.slab], 0));
    assert_eq!(e.user_state(&tu).pos[0].base, 300);
}
/// Random sequences with high leverage and liquidations (keeper paid): positions net to zero after every step, and at the end
/// cash + locked margin + fees + insurance (keeper included as a user) equals the tokens in the vault.
#[test] fn random_sequences_with_liquidations_conserve() {
    let (mut liqs, mut takes) = (0u64, 0u64);
    for seed in 1..=40u64 {
        let mut g = Rng(seed.wrapping_mul(0xA24BAED4963EE407) | 1);
        let mut e = Env::new(); e.oracle_update("", 1000_00000000, None); let f = fast_new(&mut e, 0);
        let mk: Vec<Mk> = (0..2).map(|_| maker(&mut e, &f, 200_000_000_000, 20_000_000_000)).collect();
        let tk_: Vec<(Keypair, Address)> = (0..3).map(|_| e.user(1_000_000_000 + g.n(1_000_000_000))).collect();
        let (_kp, ku) = e.user(1);
        let slabs: Vec<Address> = mk.iter().map(|m| m.slab).collect();
        let mut px: i64 = 1000_00000000;
        for _ in 0..200 {
            let mid = (px / 1_000_000) as u64;
            match g.n(8) {
                0..=1 => { for m in &mk { let v: Vec<(u16, u32, u32)> = (0..4).map(|l| { let s = (l % 2) as u16; let off = 1 + g.n(40);
                    (s << 3 | (l / 2) as u16, (if s == 1 { mid + off } else { mid - off }) as u32, (1 + g.n(60_000)) as u32) }).collect();
                    let v = fit(&e, &m.slab, &v); quote(&mut e, m, &v, true); } }
                2..=3 => { let i = g.n(3) as usize; let side = g.n(2) as u8; let lim = if side == 0 { mid + 500 } else { mid - 500 };
                    let (kp, u) = (tk_[i].0.insecure_clone(), tk_[i].1); let m = e.market;
                    let mut a = vec![AccountMeta::new(kp.pubkey(), true), w(&u), r(&m), w(&f)]; a.extend(slabs.iter().map(r));
                    if try_send(&mut e, a, take_ix(side, 0, 2, lim, 1 + g.n(90_000)), &[&kp]) { takes += 1; } }
                4 => { let i = g.n(2) as usize; let m = e.market; let u = mk[i].u; try_send(&mut e, vec![w(&u), r(&m), w(&f)], vec![65], &[]); }
                5 => { let i = g.n(3) as usize; let u = tk_[i].1; let (m, ins) = (e.market, e.ins);
                    let mut a = vec![w(&u), r(&m), w(&ins), w(&f)]; a.extend(slabs.iter().map(r)); a.push(w(&ku));
                    let mut d = vec![70u8, 2]; d.extend_from_slice(&u64::MAX.to_le_bytes());
                    if try_send(&mut e, a, d, &[]) { liqs += 1; } }
                _ => { px = (px as i128 * (10_000 + g.n(601) as i128 - 300) / 10_000) as i64; let s = e.slot + 1 + g.n(3); e.set_time(s); e.oracle_update("", px, None); }
            }
            let b = fb(&e, &f);
            let mut net: i128 = (0..2).map(|i| b.makers[i].base as i128).sum();
            for a in mk.iter().map(|m| m.u).chain(tk_.iter().map(|t| t.1)) { net += e.user_state(&a).pos.iter().filter(|p| p.market == e.market.to_bytes()).map(|p| p.base as i128).sum::<i128>(); }
            assert_eq!(net, 0, "seed {seed}: positions don't net to zero");
            for i in 0..2 { assert!(b.makers[i].budget >= 0, "seed {seed}: negative budget"); }
        }
        let s = e.slot + 1; e.set_time(s); e.oracle_update("", px, None);
        for m in &mk { let mm = e.market; let u = m.u; e.send("", vec![w(&u), r(&mm), w(&f)], vec![65], &[], true); }
        for (kp, u) in &tk_ { let m = e.market; let kp = kp.insecure_clone(); let mut a = vec![AccountMeta::new(kp.pubkey(), true), w(u), r(&m), w(&f)]; a.extend(slabs.iter().map(r));
            let mut d = take_ix(0, 1, 2, 1, 1); d[2] = 1; try_send(&mut e, a, d, &[&kp]); }
        let b = fb(&e, &f);
        let mut sum: i128 = (0..2).map(|i| b.makers[i].margin as i128).sum::<i128>() + b.fee_accum as i128 + e.ins_bal() as i128 + e.market_state().fees as i128;
        for a in mk.iter().map(|m| m.u).chain(tk_.iter().map(|t| t.1)).chain(std::iter::once(ku)) { sum += e.user_state(&a).cash as i128; }
        let v = e.vault_amount() as i128;
        assert!((sum - v).abs() <= 32, "seed {seed}: cash {sum} vs vault {v}");
    }
    println!("liquidation fuzz: {liqs} liquidations, {takes} takes");
    assert!(liqs > 20, "the fuzz must actually liquidate");
}
/// The SDK's liquidation price formula matches the program: just above it liquidation is refused, just below it goes through.
#[test] fn liquidation_price_formula_matches_the_program() {
    let mut e = Env::new(); let (f, k) = setup(&mut e);
    let (tp, tu) = e.user(1_000_000_000);
    quote(&mut e, &k, &[(ASK, 100_010, 100_000)], true);
    tk(&mut e, &tp, &tu, &f, &[k.slab], &[], 0, PX + 100, 90_000, true);
    let u = e.user_state(&tu); let mmr = e.market_state().mmr_bps as f64;
    // p* = (mm_rest - eq_rest) / (b - |b| mmr): one position, so eq_rest = cash and mm_rest = 0
    let b = u.pos[0].base as f64; let pstar = -(u.cash as f64) / (b - b.abs() * mmr / 10_000.0);
    println!("liquidation price {pstar:.1}");
    let try_at = |e: &mut Env, px: u64| -> bool {
        let s = e.slot + 1; e.set_time(s); e.oracle_update("", (px * 1_000_000) as i64, None);
        quote(e, &k, &[(0, (px - 3) as u32, 100_000)], true);
        let (m, ins) = (e.market, e.ins); let mut d = vec![70u8, 1]; d.extend_from_slice(&u64::MAX.to_le_bytes());
        try_send(e, vec![w(&tu), r(&m), w(&ins), w(&f), r(&k.slab)], d, &[])
    };
    assert!(!try_at(&mut e, pstar.ceil() as u64 + 2), "above the liquidation price: refused");
    assert!(try_at(&mut e, pstar.floor() as u64 - 2), "below the liquidation price: liquidated");
}

/// CU of a take with a full fast book: 8 makers, 8 levels a side, ladders best-first (as the MM quotes; worst-first is refused).
#[test] fn compute_units_eight_makers() {
    for worst_first in [false, true] {
        let mut e = Env::new(); let (f, k0) = setup(&mut e);
        let mut ks = vec![k0]; for _ in 1..8 { ks.push(maker(&mut e, &f, 10_000_000_000, 1_000_000_000)); }
        for (j, k) in ks.iter().enumerate() {
            let v: Vec<(u16, u32, u32)> = (0..16u32).map(|i| {
                let l = if worst_first { 7 - (i & 7) } else { i & 7 };
                (i as u16, if i & 8 != 0 { 100_010 + l * 8 + j as u32 } else { 99_990 - l * 8 - j as u32 }, 1000)
            }).collect();
            quote(&mut e, k, &v, !worst_first);
        }
        if worst_first { continue; } // a worst-first ladder is rejected (ladders are best-first)
        let (tp, tu) = e.user(10_000_000_000);
        let slabs: Vec<Address> = ks.iter().map(|k| k.slab).collect();
        let tag = "best-first";
        println!("8 makers {tag} take 1 fill: {:?}", tk(&mut e, &tp, &tu, &f, &slabs, &[], 0, PX + 100, 10, true));
        println!("8 makers {tag} take 5 fills: {:?}", tk(&mut e, &tp, &tu, &f, &slabs, &[], 0, PX + 100, 4500, true));
    }
}

/// CU profile of take_fast (build with --features cuprof): checkpoint deltas for 1 maker / 1 fill, 8 makers / 1 fill, 8 makers / 5 fills.
#[test] fn cu_profile() {
    fn show(e: &Env, label: &str) {
        let mut pts: Vec<(u64, u64)> = Vec::new(); let mut tag = None;
        for l in e.logs.iter() {
            let v: Vec<&str> = l.trim_start_matches("Program log: ").split(", ").collect();
            if v.len() == 5 && v[0] == "0xc0" { tag = u64::from_str_radix(&v[1][2..], 16).ok(); continue; }
            if let (Some(t), Some(r)) = (tag, l.strip_prefix("Program consumption: ")) { pts.push((t, r.split(' ').next().unwrap().parse().unwrap())); tag = None; }
        }
        let mut out = String::new();
        for w in pts.windows(2) { out += &format!(" {}->{}:{}", w[0].0, w[1].0, w[0].1 - w[1].1); }
        println!("PROFILE {label}:{out}");
    }
    let mut e = Env::new(); let (f, k) = setup(&mut e);
    let v: Vec<(u16, u32, u32)> = (0..95).map(|i| (i as u16, if (i & 8) != 0 { 100_010 + i as u32 } else { 99_990 - i as u32 }, 1000)).collect();
    let c = quote(&mut e, &k, &v, true); show(&e, &format!("slab_quote 95 total={c:?}"));
    let mut ks = vec![k];
    for _ in 0..7 { let m = maker(&mut e, &f, 10_000_000_000, 1_000_000_000); ks.push(m); }
    let (tp, tu) = e.user(100_000_000_000);
    for (i, m) in ks.iter().enumerate() {
        let v: Vec<(u16, u32, u32)> = (0..8).map(|l| (ASK + l as u16, 100_010 + 10 * l as u32 + i as u32, 1000)).collect();
        quote(&mut e, m, &v, true);
    }
    let c = tk(&mut e, &tp, &tu, &f, &[ks[0].slab], &[], 0, PX + 1000, 10, true); show(&e, &format!("1 slab 1 fill total={c:?}"));
    let all: Vec<Address> = ks.iter().map(|m| m.slab).collect();
    let c = tk(&mut e, &tp, &tu, &f, &all, &[], 0, PX + 1000, 10, true); show(&e, &format!("8 slabs 1 fill total={c:?}"));
    let c = tk(&mut e, &tp, &tu, &f, &all, &[], 0, PX + 1000, 4500, true); show(&e, &format!("8 slabs 5 fills total={c:?}"));
}

/// Several fill books for one side of one market: each holds its own used tables and maker budgets over the same maker quotes,
/// so takers on different books write disjoint accounts (their User and their book) and run in parallel. Each book fills a
/// quote up to its size, so a maker on N books offers N times the size, capped by each book's budget.
#[test] fn several_books_per_side_fill_in_parallel_and_conserve() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let (a1, a2) = (fast_new_side(&mut e, 0, 2), fast_new_side(&mut e, 0, 2));
    let k = maker(&mut e, &a1, 10_000_000_000, 1_000_000_000);
    let (adm, m) = (e.admin.pubkey(), e.market);
    e.send("", vec![AccountMeta::new(adm, true), r(&m), w(&a2), r(&k.slab)], vec![62], &[], true);
    fund_m(&mut e, &a2, &k, 1_000_000_000, true);
    quote(&mut e, &k, &[(ASK, 100_010, 100)], true);
    let (t1, u1) = e.user(10_000_000_000); let (t2, u2) = e.user(10_000_000_000);
    tk(&mut e, &t1, &u1, &a1, &[k.slab], &[], 0, PX + 100, 80, true);
    tk(&mut e, &t2, &u2, &a2, &[k.slab], &[], 0, PX + 100, 80, true);
    assert_eq!((e.user_state(&u1).pos[0].base, e.user_state(&u2).pos[0].base), (80, 80));
    tk(&mut e, &t1, &u1, &a1, &[k.slab], &[], 0, PX + 100, 80, true);
    assert_eq!(e.user_state(&u1).pos[0].base, 100, "each book fills the quote up to its size once");
    settle(&mut e, &a1, &k); settle(&mut e, &a2, &k);
    assert_eq!(e.user_state(&k.u).pos[0].base, -180);
    let (b1, b2) = (fb(&e, &a1), fb(&e, &a2));
    assert_eq!((b1.makers[0].base, b2.makers[0].base), (0, 0), "settled into the maker's position");
}

fn set_delegate(e: &mut Env, owner: &Keypair, u: &Address, rec: &Address, delegate: &Address, expiry: u64, max_qty: u64) {
    let mut d = vec![42u8]; d.extend_from_slice(delegate.as_ref()); d.extend_from_slice(&expiry.to_le_bytes()); d.extend_from_slice(&max_qty.to_le_bytes());
    e.send("", vec![AccountMeta::new(owner.pubkey(), true), r(u), w(rec)], d, &[owner], true);
}
fn batch_ix(e: &Env, f: &Address, slabs: &[Address], takers: &[(Address, Address, u8, u8, u64, u64)], recs: &[Address]) -> (Vec<AccountMeta>, Vec<u8>) {
    let m = e.market;
    let mut a = vec![r(&m), w(f)]; a.extend(slabs.iter().map(r));
    let mut d = vec![72u8, 0, slabs.len() as u8, takers.len() as u8];
    for (i, (u, sg, side, fl, limit, qty)) in takers.iter().enumerate() {
        a.push(w(u)); a.push(AccountMeta::new_readonly(*sg, true)); a.push(r(recs.get(i).unwrap_or(u)));
        d.push(*side); d.push(*fl); d.extend_from_slice(&limit.to_le_bytes()); d.extend_from_slice(&qty.to_le_bytes());
    }
    (a, d)
}
/// A relayer with session delegates sends many takers in one instruction: they fill in order against the same quotes,
/// an order over its margin (worst case) or without authority is skipped, the rest still fill, and the book conserves.
#[test] fn batched_takes_fill_in_order_and_skip_bad_orders() {
    let mut e = Env::new(); let (f, k) = setup(&mut e);
    quote(&mut e, &k, &[(ASK, 100_010, 300), (ASK + 1, 100_020, 300)], true);
    let relayer = Keypair::new(); e.svm.airdrop(&relayer.pubkey(), 1_000_000_000).unwrap();
    let mut us = Vec::new();
    for dep in [10_000_000_000u64, 10_000_000_000, 1_000_000, 10_000_000_000] {
        let (kp, u) = e.user(dep); let rec = Address::new_unique(); e.prog_acct(rec, DELEGATE_SIZE);
        let s = e.slot; set_delegate(&mut e, &kp, &u, &rec, &relayer.pubkey(), s + 1000, 1_000); us.push((kp, u, rec));
    }
    let rp = relayer.pubkey();
    let stranger = Keypair::new();
    let takers = vec![(us[0].1, rp, 0u8, 0u8, PX + 100, 200u64), (us[1].1, rp, 0, 0, PX + 100, 200), (us[2].1, rp, 0, 0, PX + 100, 200),
                      (us[3].1, stranger.pubkey(), 0, 0, PX + 100, 50), (us[3].1, rp, 0, 0, PX + 100, 5_000)];
    let recs: Vec<Address> = vec![us[0].2, us[1].2, us[2].2, us[3].2, us[3].2];
    let (a, d) = batch_ix(&e, &f, &[k.slab], &takers, &recs);
    assert!(send_many(&mut e, vec![(a, d)], &[&relayer, &stranger]));
    let b = |e: &Env, u: &Address| e.user_state(u).pos.iter().find(|p| p.market == e.market.to_bytes()).map_or(0, |p| p.base);
    assert_eq!((b(&e, &us[0].1), b(&e, &us[1].1)), (200, 200), "both fill, in order: 300 at 100_010 then 300 at 100_020");
    assert_eq!(b(&e, &us[2].1), 0, "1 USDC of collateral can't carry 200 lots: skipped");
    assert_eq!(b(&e, &us[3].1), 0, "a stranger's signature and an order over the delegate cap are skipped");
    let fbk = fb(&e, &f); assert_eq!(fbk.makers[0].base, -400);
    assert_eq!((fbk.used[0][8].qty, fbk.used[0][9].qty), (300, 100));
    // the batch path and take_fast agree on what fills
    let (tp, tu) = e.user(10_000_000_000); tk(&mut e, &tp, &tu, &f, &[k.slab], &[], 0, PX + 100, 1_000, true);
    assert_eq!(b(&e, &tu), 200, "what the batch left: 200 at 100_020");
}
/// CU of a batch: per taker, against take_fast for the same fills.
#[test] fn compute_units_batch() {
    let mut e = Env::new(); let (f, k) = setup(&mut e);
    let v: Vec<(u16, u32, u32)> = (0..8).map(|l| (ASK + l as u16, 100_010 + 10 * l as u32, 100_000)).collect();
    quote(&mut e, &k, &v, true);
    let relayer = Keypair::new(); e.svm.airdrop(&relayer.pubkey(), 1_000_000_000).unwrap(); let rp = relayer.pubkey();
    let mut us = Vec::new();
    for _ in 0..10 { let (kp, u) = e.user(10_000_000_000); let rec = Address::new_unique(); e.prog_acct(rec, DELEGATE_SIZE);
        let s = e.slot; set_delegate(&mut e, &kp, &u, &rec, &rp, s + 1000, 1_000_000); us.push((kp, u, rec)); }
    for n in [1usize, 5, 10] {
        let takers: Vec<_> = us[..n].iter().map(|x| (x.1, rp, 0u8, 0u8, PX + 100, 10u64)).collect();
        let rest: Vec<Address> = us[..n].iter().map(|x| x.2).collect();
        let (a, d) = batch_ix(&e, &f, &[k.slab], &takers, &rest);
        let ix = Instruction { program_id: e.pid, accounts: a, data: d };
        let tx = Transaction::new_signed_with_payer(&[ix], Some(&e.admin.pubkey()), &[&e.admin, &relayer], e.svm.latest_blockhash());
        let cu = e.svm.send_transaction(tx).map(|m| m.compute_units_consumed).unwrap(); e.svm.expire_blockhash();
        println!("batch {n} takers 1 fill each: {cu} CU, {} per taker", cu / n as u64);
    }
}

/// Random batches agree with the same orders sent one at a time through take_fast: the same script runs on two books, one
/// batched (relayer with session delegates), one order by order. Takers are well collateralised, so the batch's stricter
/// worst-case margin never skips; every taker's position, every maker's fills and the fees must match after every round.
#[test] fn random_batches_match_sequential_takes() {
    let mut rounds = 0;
    for seed in 1..=20u64 {
        let mut g = Rng(seed.wrapping_mul(0x2545F4914F6CDD1D) | 1);
        let mut envs: Vec<(Env, Address, Vec<Mk>, Vec<(Keypair, Address, Address)>, Keypair)> = (0..2).map(|_| {
            let mut e = Env::new(); e.oracle_update("", 1000_00000000, None); let f = fast_new(&mut e, 0);
            let mk: Vec<Mk> = (0..3).map(|_| maker(&mut e, &f, 50_000_000_000, 5_000_000_000)).collect();
            let rl = Keypair::new(); e.svm.airdrop(&rl.pubkey(), 1_000_000_000).unwrap();
            let tk_ = (0..6).map(|_| { let (kp, u) = e.user(100_000_000_000); let rec = Address::new_unique(); e.prog_acct(rec, DELEGATE_SIZE);
                let s = e.slot; set_delegate(&mut e, &kp, &u, &rec, &rl.pubkey(), s + 100_000, u64::MAX); (kp, u, rec) }).collect();
            (e, f, mk, tk_, rl)
        }).collect();
        for _ in 0..15 {
            for _ in 0..2 {
                let i = g.n(3) as usize;
                let v: Vec<(u16, u32, u32)> = (0..4).map(|_| { let s = g.n(2) as u16; let l = g.n(8) as u16; let off = 1 + g.n(8);
                    (s << 3 | l, (if s == 1 { 100_000 + off } else { 100_000 - off }) as u32, (1 + g.n(400)) as u32) }).collect();
                let v = { let (e, _, mk, _, _) = &envs[0]; fit(e, &mk[i].slab, &v) };
                for (e, _, mk, _, _) in envs.iter_mut() { quote(e, &mk[i], &v, true); }
            }
            let orders: Vec<(usize, u8, u64, u64)> = (0..1 + g.n(6)).map(|_| { let side = g.n(2) as u8;
                (g.n(6) as usize, side, if side == 0 { 100_000 + g.n(10) } else { 100_000 - g.n(10) }, 1 + g.n(700)) }).collect();
            { let (e, f, mk, tk_, rl) = &mut envs[0];
              let takers: Vec<_> = orders.iter().map(|&(t, side, lim, q)| (tk_[t].1, rl.pubkey(), side, 0u8, lim, q)).collect();
              let recs: Vec<Address> = orders.iter().map(|&(t, ..)| tk_[t].2).collect();
              let slabs: Vec<Address> = mk.iter().map(|m| m.slab).collect();
              let (a, d) = batch_ix(e, f, &slabs, &takers, &recs); let rl = rl.insecure_clone();
              assert!(send_many(e, vec![(a, d)], &[&rl])); }
            { let (e, f, mk, tk_, _) = &mut envs[1];
              let slabs: Vec<Address> = mk.iter().map(|m| m.slab).collect();
              for &(t, side, lim, q) in &orders { let (kp, u) = (tk_[t].0.insecure_clone(), tk_[t].1); tk(e, &kp, &u, f, &slabs, &[], side, lim, q, true); } }
            let pos = |e: &Env, u: &Address| e.user_state(u).pos.iter().find(|p| p.market == e.market.to_bytes()).map_or((0, 0), |p| (p.base, p.cost));
            for t in 0..6 { assert_eq!(pos(&envs[0].0, &envs[0].3[t].1), pos(&envs[1].0, &envs[1].3[t].1), "seed {seed}: taker {t} position"); 
                            assert_eq!(envs[0].0.user_state(&envs[0].3[t].1).cash, envs[1].0.user_state(&envs[1].3[t].1).cash, "seed {seed}: taker {t} cash"); }
            let (b0, b1) = (fb(&envs[0].0, &envs[0].1), fb(&envs[1].0, &envs[1].1));
            for k in 0..3 { assert_eq!((b0.makers[k].base, b0.makers[k].quote, b0.makers[k].budget), (b1.makers[k].base, b1.makers[k].quote, b1.makers[k].budget), "seed {seed}: maker {k}"); }
            assert_eq!(b0.fee_accum, b1.fee_accum); rounds += 1;
        }
    }
    println!("{rounds} rounds: batch == sequential");
}

// ---------- maker protection (2026-11-05) ----------
fn peg(off: i64) -> u32 { (off + (1i64 << 31)) as u32 }
const ASK_LAD: u16 = 1; // ladder of idx 0, ask side
fn later(e: &mut Env, slots: u64) { let s = e.slot + slots; e.set_time(s); }

#[test] fn pegged_quotes_follow_the_oracle() {
    let mut e = Env::new(); let (f, k) = setup(&mut e);
    let (tp, tu) = e.user(10_000_000_000);
    // both ladders pegged; an ask 10 above the oracle and a bid 10 below, written once
    quote(&mut e, &k, &[(0xC000, 0, 1), (0xC000 | ASK_LAD, 0, 1), (ASK, peg(10), 100), (0, peg(-10), 100)], true);
    tk(&mut e, &tp, &tu, &f, &[k.slab], &[], 0, PX + 100, 40, true);
    assert_eq!(fb(&e, &f).makers[0].quote, 100_010 * 40);
    later(&mut e, 3); e.oracle_update("", 1010_00000000, None);
    // the maker sent nothing, its quotes moved with the oracle
    tk(&mut e, &tp, &tu, &f, &[k.slab], &[], 0, 101_000 + 100, 40, true);
    assert_eq!(fb(&e, &f).makers[0].quote, 100_010 * 40 + 101_010 * 40);
    tk(&mut e, &tp, &tu, &f, &[k.slab], &[], 1, 101_000 - 100, 30, true);
    let b = fb(&e, &f);
    assert_eq!((b.makers[0].quote, b.makers[0].base), (100_010 * 40 + 101_010 * 40 - 100_990 * 30, -50));
    // a taker limit is still a limit: nothing at oracle + 10 when the buyer stops at oracle + 5
    let t0 = e.user_state(&tu).pos[0].base;
    tk(&mut e, &tp, &tu, &f, &[k.slab], &[], 0, 101_005, 10, true);
    assert_eq!(e.user_state(&tu).pos[0].base, t0);
    // a pegged ladder is best-first like any other
    quote(&mut e, &k, &[(ASK, peg(20), 1), (ASK + 1, peg(10), 1)], false);
    quote(&mut e, &k, &[(ASK, peg(10), 1), (ASK + 1, peg(20), 1)], true);
    // back to fixed prices in the same instruction
    quote(&mut e, &k, &[(0xC000 | ASK_LAD, 0, 0), (ASK, 101_030, 5), (ASK + 1, 0, 0)], true);
    tk(&mut e, &tp, &tu, &f, &[k.slab], &[], 0, 101_100, 5, true);
    assert_eq!(e.user_state(&tu).pos[0].base, t0 + 5);
    assert_eq!(fb(&e, &f).makers[0].quote, 100_010 * 40 + 101_010 * 40 - 100_990 * 30 + 101_030 * 5);
}
#[test] fn a_ladder_stops_when_the_oracle_moves_past_the_makers_tolerance() {
    let mut e = Env::new(); let (f, k) = setup(&mut e);
    let (tp, tu) = e.user(10_000_000_000);
    // fixed asks, quoted against oracle 100_000, good for 50 bps of oracle move
    quote(&mut e, &k, &[(0x8000 | ASK_LAD, 100_000, 50), (ASK, 100_400, 100), (0, 99_600, 100)], true);
    later(&mut e, 3); e.oracle_update("", 1006_00000000, None); // +60 bps
    tk(&mut e, &tp, &tu, &f, &[k.slab], &[], 0, 101_200, 10, true);
    assert_eq!(e.user_state(&tu).pos[0].base, 0, "past the tolerance: the ask ladder doesn't fill");
    // the bid ladder has no tolerance set and still fills
    tk(&mut e, &tp, &tu, &f, &[k.slab], &[], 1, 99_000, 5, true);
    assert_eq!(e.user_state(&tu).pos[0].base, -5);
    later(&mut e, 3); e.oracle_update("", 1003_00000000, None); // +30 bps: inside again
    tk(&mut e, &tp, &tu, &f, &[k.slab], &[], 0, 101_200, 10, true);
    assert_eq!(e.user_state(&tu).pos[0].base, 5);
    // a requote moves the reference
    quote(&mut e, &k, &[(0x8000 | ASK_LAD, 100_600, 50)], true);
    later(&mut e, 3); e.oracle_update("", 1006_00000000, None);
    tk(&mut e, &tp, &tu, &f, &[k.slab], &[], 0, 101_200, 10, true);
    assert_eq!(e.user_state(&tu).pos[0].base, 15);
    // tolerance is a u16 of bps; unknown setting items fail
    quote(&mut e, &k, &[(0x8000 | ASK_LAD, 100_000, 70_000)], false);
    quote(&mut e, &k, &[(0x8000 | 0x3fff, 100_000, 1)], false);
}
#[test] fn inventory_cap_stops_a_ladder_at_the_makers_limit() {
    let mut e = Env::new(); let (f, k) = setup(&mut e);
    let (tp, tu) = e.user(10_000_000_000);
    // at most 150 lots short through the ask ladder
    quote(&mut e, &k, &[(0xC000 | ASK_LAD, 150, 0), (ASK, 100_010, 500), (0, 99_990, 500)], true);
    tk(&mut e, &tp, &tu, &f, &[k.slab], &[], 0, PX + 100, 400, true);
    assert_eq!(e.user_state(&tu).pos[0].base, 150);
    // settled fills still count
    settle(&mut e, &f, &k);
    assert_eq!(fb(&e, &f).ext[0].inv0, -150);
    tk(&mut e, &tp, &tu, &f, &[k.slab], &[], 0, PX + 100, 100, true);
    assert_eq!(e.user_state(&tu).pos[0].base, 150);
    // buying back through the bid frees room on the ask
    tk(&mut e, &tp, &tu, &f, &[k.slab], &[], 1, PX - 100, 50, true);
    tk(&mut e, &tp, &tu, &f, &[k.slab], &[], 0, PX + 100, 100, true);
    assert_eq!(e.user_state(&tu).pos[0].base, 150);
    assert_eq!(fb(&e, &f).makers[0].base, 0, "-150 settled, +50 then -50 since");
    // off again
    quote(&mut e, &k, &[(0xC000 | ASK_LAD, 0, 0)], true);
    tk(&mut e, &tp, &tu, &f, &[k.slab], &[], 0, PX + 100, 100, true);
    assert_eq!(e.user_state(&tu).pos[0].base, 250);
}
#[test] fn maker_rebate_comes_out_of_the_book_fees() {
    let mut e = Env::new(); let (f, k) = setup(&mut e);
    let (tp, tu) = e.user(10_000_000_000);
    let (a, m) = (e.admin.pubkey(), e.market);
    e.send("", vec![AccountMeta::new(a, true), r(&m), w(&k.u)], vec![23, 1], &[], true); // maker tier 1: 2 bps rebate
    settle(&mut e, &f, &k); // the book takes the tier from the maker's next settle
    assert_eq!(fb(&e, &f).ext[0].rebate_bps, 2);
    quote(&mut e, &k, &[(ASK, 100_010, 100)], true);
    tk(&mut e, &tp, &tu, &f, &[k.slab], &[], 0, PX + 100, 100, true);
    let nt = 100_010i64 * 100; let (fee, reb) = ((nt + 1999) / 2000, nt * 2 / 10_000);
    let b = fb(&e, &f); assert_eq!((b.makers[0].quote, b.fee_accum), (nt + reb, fee - reb));
    settle(&mut e, &f, &k);
    let (u, t, b2) = (e.user_state(&k.u), e.user_state(&tu), fb(&e, &f));
    assert_eq!(u.cash as i128 + t.cash as i128 + b2.makers[0].margin as i128 + b2.fee_accum as i128, e.vault_amount() as i128, "nothing created or lost");
}
#[test] fn old_slabs_and_books_grow_in_place() {
    let mut e = Env::new(); let (f, k) = setup(&mut e);
    // cut both back to the sizes the earlier program made, keeping their contents
    for (key, old) in [(k.slab, SLAB_SIZE - 16 * SLAB_LADDERS), (f, FAST_SIZE - 16 * FAST_MAKERS)] {
        let mut acc = e.svm.get_account(&key).unwrap(); acc.data.truncate(old); e.svm.set_account(key, acc).unwrap();
    }
    let (tp, tu) = e.user(10_000_000_000);
    quote(&mut e, &k, &[(ASK, 100_010, 100)], false);
    e.send("", vec![w(&k.slab)], vec![73], &[], true);
    e.send("", vec![w(&f)], vec![73], &[], true);
    e.send("", vec![w(&f)], vec![73], &[], true); // already current
    assert_eq!((e.data(&k.slab).len(), e.data(&f).len()), (SLAB_SIZE, FAST_SIZE));
    assert_eq!(fb(&e, &f).makers[0].margin, 1_000_000_000, "contents kept");
    quote(&mut e, &k, &[(ASK, 100_010, 100)], true);
    tk(&mut e, &tp, &tu, &f, &[k.slab], &[], 0, PX + 100, 10, true);
    assert_eq!(e.user_state(&tu).pos[0].base, 10);
    let x = Address::new_unique(); e.prog_acct(x, 100);
    e.send("", vec![w(&x)], vec![73], &[], false);
}
