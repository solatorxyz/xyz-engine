#[path = "../../program/src/state.rs"]
#[allow(dead_code)]
mod state;
// Two markets on one collateral pool, random orders, price walks, withdrawals and liquidations by a third party.
// The single-market fuzz target never has a second position in play, so nothing there exercises: health summed over markets,
// funding settled across markets, a withdrawal or a liquidation that must see the other market, cash shared by both.
// Checked: lots net to zero in each market at every step; a withdrawal never leaves the account under its initial requirement
// at the mark; a liquidation only hits an account under maintenance and never grows its position; at the end, with everything
// claimed and settled, cash + insurance + fees is exactly the tokens in the vault.
include!("common.rs");

struct Mkt { market: Address, ins: Address, vault: Address, vault_pda: Address, sh: Vec<Address> }
fn switch(e: &mut Env, m: &Mkt) { e.market = m.market; e.ins = m.ins; e.vault = m.vault; e.vault_pda = m.vault_pda; }
const WIDTH: u64 = 2_000; // 2% price buckets, so six a side cover 94_000..106_000
fn pool2(e: &mut Env) -> (Mkt, Mkt) {
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
        for v in [WIDTH, 3, 100, 10, (-8i64) as u64, 1, 1_000_000, 1_000_000_000] { d.extend_from_slice(&v.to_le_bytes()); }
        let (o, a) = (e.oracle, e.admin.pubkey());
        e.send("", vec![AccountMeta::new(a, true), w(&market), w(&ins), r(&vault), r(&o), w(&pool), AccountMeta::new_readonly(admin.pubkey(), true)], d, &[&admin], true);
        let mut m = Mkt { market, ins, vault, vault_pda, sh: vec![] };
        switch(e, &m);
        for side in 0..2u8 { for i in 0..6u64 { m.sh.push(e.shard_by(side, 94_000 + i * WIDTH, &admin)); } }
        m
    };
    let ma = mk(e); let mb = mk(e);
    (ma, mb)
}
fn try_send(e: &mut Env, accounts: Vec<AccountMeta>, data: Vec<u8>, signers: &[&Keypair]) -> Option<u32> {
    let ix = Instruction { program_id: e.pid, accounts, data };
    let mut s: Vec<&Keypair> = vec![&e.admin]; s.extend_from_slice(signers);
    let tx = Transaction::new_signed_with_payer(&[ix], Some(&e.admin.pubkey()), &s, e.svm.latest_blockhash());
    let r = e.svm.send_transaction(tx); e.svm.expire_blockhash();
    match r { Ok(_) => None, Err(f) => { let s = format!("{:?}", f.err);
        Some(s.split("Custom(").nth(1).and_then(|x| x.split(')').next()).and_then(|x| x.parse().ok()).unwrap_or(u32::MAX)) } }
}
struct Rng(u64);
impl Rng { fn n(&mut self, m: u64) -> u64 { self.0 ^= self.0 << 13; self.0 ^= self.0 >> 7; self.0 ^= self.0 << 17; self.0 % m } }

fn mkt(e: &Env, m: &Mkt) -> Market { unsafe { std::ptr::read_unaligned(e.data(&m.market).as_ptr() as *const Market) } }
fn mark_of(s: &Market) -> i128 { (if s.mark_px != 0 { s.mark_px } else { s.oracle_px }) as i128 }
fn base_in(e: &Env, u: &Address, m: &Mkt) -> i64 { e.user_state(u).pos.iter().find(|p| p.market == m.market.to_bytes()).map_or(0, |p| p.base) }
/// accounts of a trade in `m`: every shard of `m`, then the other market (cross margin)
fn accts(kp: &Keypair, u: &Address, m: &Mkt, other: &Mkt) -> Vec<AccountMeta> { let mut v = trade_accts(kp, u, &m.market, &m.sh); v.push(r(&other.market)); v }
/// claim every fill and settle funding in both markets, so the booked account is the whole truth
fn sync(e: &mut Env, kp: &Keypair, u: &Address, ms: [&Mkt; 2]) {
    for i in 0..2 { let _ = try_send(e, accts(kp, u, ms[i], ms[1 - i]), batch_data(&[], &[]), &[kp]); }
}
/// (equity at the mark, initial requirement of the positions, maintenance requirement) over both markets; band-free IM is a
/// lower bar than the program's, the MM includes the band as the program's does
fn health(e: &Env, u: &Address, ms: [&Mkt; 2]) -> (i128, i128, i128) {
    let us = e.user_state(u); let (mut eq, mut im, mut mm) = (us.cash as i128, 0i128, 0i128);
    for m in ms { let s = mkt(e, m); let px = mark_of(&s); let b = base_in(e, u, m) as i128;
        let band = if s.oracle_px == 0 { 0 } else { (s.oracle_conf as u128 * 10000 / s.oracle_px as u128).min(50) as i128 };
        eq += b * px; im += b.abs() * px * s.imr_bps as i128 / 10000; mm += (b.abs() * px * (s.mmr_bps as i128 + band) + 9999) / 10000; }
    (eq, im, mm)
}
fn net_base(e: &Env, users: &[(Keypair, Address)], m: &Mkt) -> i128 {
    let mut n: i128 = users.iter().map(|x| base_in(e, &x.1, m) as i128).sum();
    for s in &m.sh { let s = e.shard_state(s); n += s.ledger[..s.nled as usize].iter().map(|l| l.base as i128).sum::<i128>(); }
    n
}

/// Third-party book liquidation of `u` in `m` (its orders in the other market cleared first). True if it went through; then the
/// account must have been under maintenance and its position must only have shrunk.
fn liquidate(e: &mut Env, kp: &Keypair, u: &Address, m: &Mkt, o: &Mkt, errs: &mut std::collections::BTreeMap<u32, u32>) -> bool {
    sync(e, kp, u, [m, o]);
    let (eq0, _, mm0) = health(e, u, [m, o]); let b0 = base_in(e, u, m);
    let mut lc = vec![w(u), r(&o.market)]; lc.extend(o.sh.iter().map(w)); lc.push(r(&m.market));
    let _ = try_send(e, lc, vec![77], &[]);
    let mut a = vec![w(u), r(&m.market), w(&m.ins)]; a.extend(m.sh.iter().map(w)); a.push(r(&o.market));
    let mut d = vec![34u8]; d.extend_from_slice(&u64::MAX.to_le_bytes());
    match try_send(e, a, d, &[]) {
        Some(c) => { if eq0 < mm0 && b0 != 0 { *errs.entry(c).or_default() += 1; } false }
        None => { let b1 = base_in(e, u, m);
            assert!(eq0 < mm0, "an account was liquidated with equity {eq0} >= maintenance {mm0}");
            assert!(b1.unsigned_abs() <= b0.unsigned_abs() && (b1 == 0 || (b1 > 0) == (b0 > 0)), "liquidation moved the position {b0} -> {b1}"); true }
    }
}
#[test] fn two_markets_one_pool_random() {
    let (mut wd_ok, mut liq_ok, mut fills, mut under) = (0u32, 0u32, 0u32, 0u32); let mut liq_err = std::collections::BTreeMap::<u32, u32>::new();
    for seed in 1..=40u64 {
        let mut g = Rng(seed.wrapping_mul(0x9E3779B97F4A7C15) | 1);
        let mut e = Env::new(); let (ma, mb) = pool2(&mut e);
        switch(&mut e, &ma);
        let mut px = [1000_00000000i64, 1000_00000000i64];
        e.set_time(e.slot + 1);
        for (i, m) in [&ma, &mb].into_iter().enumerate() { switch(&mut e, m); e.oracle_update("", px[i], None); }
        switch(&mut e, &ma);
        // two deep accounts that keep the books quoted, four small ones that run at high leverage
        let users: Vec<(Keypair, Address)> = (0..6).map(|i| e.user(if i < 2 { 2_000_000_000 } else { 800_000 + g.n(3_000_000) })).collect();
        let deposits: u64 = e.vault_amount(); let mut withdrawn: u64 = 0;
        let ms = [&ma, &mb];
        for step in 0..500 {
            let mi = g.n(2) as usize; let (m, o) = (ms[mi], ms[1 - mi]);
            let ui = g.n(6) as usize; let (kp, u) = (users[ui].0.insecure_clone(), users[ui].1);
            let mid = (px[mi] / 1_000_000) as u64;
            match g.n(12) {
                // resting quotes from the deep accounts, both sides around the price
                0..=3 => { let mk = g.n(2) as usize; let (kp, u) = (users[mk].0.insecure_clone(), users[mk].1);
                    let its: Vec<Vec<u8>> = (0..4).map(|j| { let side = (j % 2) as u8; let off = 20 + g.n(600);
                        let p = if side == 0 { mid - off } else { mid + off }; place_item(side, 1, p.clamp(94_000, 105_999), 20 + g.n(200), 0, 0) }).collect();
                    let _ = try_send(&mut e, accts(&kp, &u, m, o), batch_data(&[cancel_item(0, 64, 0)], &its), &[&kp]); }
                // takers: IOC through the book, sometimes reduce-only
                4..=7 => { // small accounts lean one way (two long, two short), so they run up to their margin limit
                    let side = if ui >= 2 && g.n(8) != 0 { (ui % 2) as u8 } else { g.n(2) as u8 }; let lim = if side == 0 { mid + 1_500 } else { mid - 1_500 }.clamp(94_000, 105_999);
                    let fl = if g.n(5) == 0 { 4 | 2 } else { 4 };
                    let b0 = base_in(&e, &u, m);
                    let mut d = vec![1u8]; d.extend_from_slice(&place_item(side, fl, lim, 1 + g.n(if ui < 2 { 250 } else { 40 }), 0, 0));
                    if try_send(&mut e, accts(&kp, &u, m, o), d, &[&kp]).is_none() && base_in(&e, &u, m) != b0 { fills += 1; } }
                                8 | 9 => { // mostly small steps; one in four is a 2.5-4.5% shock, which is what takes a 10x account under maintenance
                    let d = if g.n(4) == 0 { (250 + g.n(201) as i64) * if g.n(2) == 0 { 1 } else { -1 } } else { g.n(361) as i64 - 180 }; px[mi] = (px[mi] as i128 * (10_000 + d as i128) / 10_000) as i64;
                    px[mi] = px[mi].clamp(945_00000000, 1055_00000000); }
                // withdrawal: a share of cash, every shard and both markets passed
                10 => { sync(&mut e, &kp, &u, ms);
                    let cash = e.user_state(&u).cash; if cash <= 0 { continue; }
                    let amt = 1 + g.n(cash as u64);
                    let dst = Address::new_unique(); let mint = Address::from(mkt(&e, m).mint);
                    e.svm.set_account(dst, Account { lamports: 1_000_000_000, data: token_acct(&mint, &kp.pubkey(), 0), owner: TOKEN, executable: false, rent_epoch: 0 }).unwrap();
                    let mut a = vec![AccountMeta::new(kp.pubkey(), true), w(&u), r(&m.market), w(&m.vault), w(&dst), r(&TOKEN), r(&m.vault_pda)];
                    a.extend(m.sh.iter().map(w)); a.extend(o.sh.iter().map(w)); a.push(r(&o.market));
                    let mut d = vec![14u8]; d.extend_from_slice(&amt.to_le_bytes());
                    if try_send(&mut e, a, d, &[&kp]).is_none() { wd_ok += 1; withdrawn += amt;
                        let (eq, im, _) = health(&e, &u, ms);
                        assert!(eq >= im, "seed {seed} step {step}: user {ui} withdrew {amt} and is left with equity {eq} under its initial requirement {im}"); } }
                // a third party tries to liquidate a random account, healthy or not
                _ => { let r = liquidate(&mut e, &kp, &u, m, o, &mut liq_err); if r { liq_ok += 1; } }
            }
            e.set_time(e.slot + 1);
            for (i, m) in ms.into_iter().enumerate() { switch(&mut e, m); e.oracle_update("", px[i], None); }
            switch(&mut e, &ma);
            for m in ms { assert_eq!(net_base(&e, &users, m), 0, "seed {seed} step {step}: lots don't net to zero in a market"); }
            // after each price move: every account under maintenance is liquidated in the market where it holds more
            for x in users[2..].iter() { let (eq, _, mm) = health(&e, &x.1, ms); if eq >= mm { continue; }
                under += 1;
                let big = if base_in(&e, &x.1, &ma).unsigned_abs() >= base_in(&e, &x.1, &mb).unsigned_abs() { 0 } else { 1 };
                if liquidate(&mut e, &x.0.insecure_clone(), &x.1, ms[big], ms[1 - big], &mut liq_err) { liq_ok += 1; } }
        }
        // everything claimed and settled at one index, fees folded: the books must equal the vault to the unit
        for (kp, u) in users.iter() { sync(&mut e, kp, u, ms); }
        for (kp, u) in users.iter() { sync(&mut e, kp, u, ms); let _ = kp; e.send("", vec![w(u), r(&ma.market), r(&mb.market)], vec![15], &[], true); }
        let mut money: i128 = users.iter().map(|x| e.user_state(&x.1).cash as i128).sum();
        for m in ms {
            let mut agg = vec![w(&m.market), w(&m.ins)]; agg.extend(m.sh.iter().map(w)); e.send("", agg, vec![16], &[], true);
            let s = mkt(&e, m); switch(&mut e, m);
            money += s.fees as i128 + e.ins_bal() as i128 - s.bad_debt as i128;
            for k in &m.sh { let sh = e.shard_state(k); assert_eq!(sh.nled, 0, "seed {seed}: unclaimed fills at the end"); money += sh.fee_accum as i128; }
            assert_eq!(net_base(&e, &users, m), 0);
        }
        switch(&mut e, &ma);
        assert_eq!(e.vault_amount(), deposits - withdrawn, "seed {seed}: vault != deposits - withdrawals");
        assert_eq!(money, e.vault_amount() as i128, "seed {seed}: cash + insurance + fees - bad debt != vault");
    }
    println!("two markets: {fills} taker fills, {wd_ok} withdrawals, {liq_ok} liquidations ({under} account-steps under maintenance); refused while under maintenance, by error: {liq_err:?}");
    assert!(fills > 1500 && wd_ok > 200 && liq_ok > 20, "the run did not exercise the paths: {fills} fills, {wd_ok} withdrawals, {liq_ok} liquidations");
    assert!(liq_err.is_empty(), "accounts under maintenance that could not be liquidated: {liq_err:?}");
}
