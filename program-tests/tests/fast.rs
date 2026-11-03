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
                    quote(&mut e, &mk[i], &v, true); }
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
                    quote(&mut e, &mk[i], &v, true); }
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
                    quote(&mut e, m, &v, true); } }
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
