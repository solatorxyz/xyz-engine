#[path = "../../program/src/state.rs"]
#[allow(dead_code)]
mod state;
// Fix-review round 5 (HEAD 1334d69) proofs of concept. Each test performs the scenario against the built SBF program and asserts
// what the CURRENT code does: a PASSING test means the issue is PRESENT.
include!("common.rs");

const IOC: u8 = 4;
fn single(item: Vec<u8>) -> Vec<u8> { let mut d = vec![1u8]; d.extend_from_slice(&item); d }
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

// ---------- two markets sharing one collateral pool (same as review.rs / review2.rs) ----------
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
fn ranked(e: &Env, side: usize, u: &Address) -> bool {
    let r = e.rank_state(); let n = if side == 0 { r.n_long } else { r.n_short } as usize;
    r.entries[side * RANK_N..side * RANK_N + n].iter().any(|x| x.user == u.to_bytes())
}

// =====================================================================================================================
// G-1 (MEDIUM/HIGH, regression of 1334d69 "ADL skips candidates whose markets are missing"): adl() evicts every passed account
// from the rank (rank_remove) BEFORE `settle_funding_all(..).is_err() -> continue`. Leaving out one market of a candidate
// therefore evicts it and the ADL falls through to the next entry. The permissionless caller picks the counterparty: the top
// ranked short (which also trades market B) is skipped and a lower-ranked one is deleveraged and charged the deficit. The
// shipped crank passes no candidate markets at all, so it does this to every multi-market candidate by itself.
// Control (B passed): the top-ranked candidate is deleveraged.
// =====================================================================================================================
fn g1_setup(pass_b: bool) -> (Option<u32>, i64, i64, bool) {
    let mut e = Env::new();
    let (ma, mb, pa) = pool2(&mut e);
    upd(&mut e, &[&ma, &mb], 1000_00000000);
    switch(&mut e, &ma);
    let (a0, b0) = (e.shard_by(1, 100_000, &pa), e.shard_by(0, 84_900, &pa));
    let (t, tu) = e.user(1_500_000);
    let (c1, c1u) = e.user(50_000_000); let (c2, c2u) = e.user(50_000_000);
    let rk = Address::new_unique(); e.prog_acct(rk, RANK_SIZE);
    e.send("", vec![AccountMeta::new(pa.pubkey(), true), w(&ma.market), w(&rk)], vec![26], &[&pa], true); e.rank = rk;
    e.send("", trade_accts(&c1, &c1u, &ma.market, &[a0]), single(place_item(1, 1, 100_000, 100, 0, 0)), &[&c1], true);
    e.send("", trade_accts(&c2, &c2u, &ma.market, &[a0]), single(place_item(1, 1, 100_000, 30, 0, 0)), &[&c2], true);
    e.send("", trade_accts(&t, &tu, &ma.market, &[a0]), single(place_item(0, IOC, 100_000, 130, 0, 0)), &[&t], true);
    e.send("", trade_accts(&c2, &c2u, &ma.market, &[a0]), vec![3, 0, 0, 0, 0], &[&c2], true); // claim
    // C1 also trades market B (one tiny resting bid is enough to have a position slot there)
    switch(&mut e, &mb);
    let bb = e.shard_by(0, 50_000, &pa);
    let mut acc = trade_accts(&c1, &c1u, &mb.market, &[bb, a0]); acc.push(r(&ma.market));
    e.send("", acc, single(place_item(0, 1, 50_000, 1, 0, 0)), &[&c1], true); // also claims its A fill
    for p in [950, 900, 850] { e.set_time(e.slot + 1); upd(&mut e, &[&ma, &mb], p * 100_000_000); }
    switch(&mut e, &ma);
    e.send("", vec![r(&ma.market), w(&rk), w(&c1u), r(&mb.market)], vec![25], &[], true);
    e.send("", vec![r(&ma.market), w(&rk), w(&c2u)], vec![25], &[], true);
    assert_eq!(e.rank_state().entries[RANK_N].user, c1u.to_bytes(), "C1 is the top-ranked short");
    let mut acc = vec![w(&tu), w(&ma.market), w(&ma.ins), w(&rk), w(&c1u), w(&c2u)];
    if pass_b { acc.push(r(&mb.market)); }
    acc.extend([w(&a0), w(&b0)]);
    let res = try_send(&mut e, acc, vec![6], &[]);
    (res, base_in(&e, &c1u, &ma.market), base_in(&e, &c2u, &ma.market), ranked(&e, 1, &c1u))
}
#[test] fn g1_adl_caller_steers_counterparty_by_omitting_a_market() {
    let ctl = g1_setup(true);
    println!("G-1 control (C1's market passed): adl {:?}, C1 base {}, C2 base {}, C1 still ranked {}", ctl.0, ctl.1, ctl.2, ctl.3);
    assert_eq!((ctl.0, ctl.1, ctl.2), (None, 0, -30), "control: the top-ranked C1 is deleveraged");
    let atk = g1_setup(false);
    println!("G-1 attack (C1's market omitted): adl {:?}, C1 base {}, C2 base {}, C1 still ranked {}", atk.0, atk.1, atk.2, atk.3);
    assert_eq!((atk.0, atk.1, atk.2, atk.3), (Some(20), -100, -30, true), "fixed: the top candidate can't be skipped by leaving out its market");
}

// =====================================================================================================================
// G-2 (MEDIUM): liq_cancel judges the account on the shards the CALLER passes: claim_all only books fills in passed shards and
// nothing requires the shards holding the target's unclaimed fills. Leaving out the shard with a profitable unclaimed fill
// makes a solvent account look below maintenance, and anyone can cancel its other resting orders (here: in the same market;
// the same works through any other market's IX 77). Liquidations are not affected (no_resting forces every such shard in).
// Control: with that shard passed the call is refused (NotLiquidatable).
// =====================================================================================================================
#[test] fn g2_liq_cancel_on_solvent_account_by_hiding_a_fill() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let (a0, s1, s9) = (e.shard(1, 100_000), e.shard(1, 105_000), e.shard(1, 150_000)); let m = e.market;
    let (lp, lpu) = e.user(50_000_000_000); let (t, tu) = e.user(1_550_000); let (bx, bxu) = e.user(50_000_000_000);
    e.send("", trade_accts(&lp, &lpu, &m, &[a0]), single(place_item(1, 1, 100_000, 130, 0, 0)), &[&lp], true);
    e.send("", trade_accts(&t, &tu, &m, &[a0]), single(place_item(0, IOC, 100_000, 130, 0, 0)), &[&t], true);
    // T rests a take-profit ask at 105_000 (10 lots) and a far ask at 150_000
    e.send("", trade_accts(&t, &tu, &m, &[s1, s9]), batch_data(&[], &[place_item(1, 1, 105_000, 10, 0, 0), place_item(1, 1, 150_000, 1, 0, 0)]), &[&t], true);
    // spike: a buyer lifts T's ask at 105_000; then the price falls to 92_000
    e.set_time(e.slot + 1); e.oracle_update("", 1050_00000000, None);
    e.send("", trade_accts(&bx, &bxu, &m, &[s1]), single(place_item(0, IOC, 105_000, 10, 0, 0)), &[&bx], true);
    e.set_time(e.slot + 1); e.oracle_update("", 980_00000000, None);
    e.set_time(e.slot + 1); e.oracle_update("", 920_00000000, None);
    let ctl = try_send(&mut e, vec![w(&tu), r(&m), w(&a0), w(&s1), w(&s9)], vec![77], &[]);
    let atk = try_send(&mut e, vec![w(&tu), r(&m), w(&a0), w(&s9)], vec![77], &[]);
    let p = e.user_state(&tu).pos[0];
    println!("G-2: liq_cancel with the fill's shard -> {:?}; without it -> {:?}; T n_orders {} ask_open {}", ctl, atk, p.n_orders, p.ask_open);
    assert_eq!(ctl, Some(5), "control: solvent once the fill is claimed");
    assert_eq!(atk, Some(19), "fixed: liq_cancel needs every shard of the market (the hidden fill is claimed)");
    // the account is solvent: claim the hidden fill and the liquidation is refused
    e.send("", vec![w(&s1), w(&tu)], vec![24], &[], true);
    let ins = e.ins;
    let liq = try_send(&mut e, vec![w(&tu), r(&m), w(&ins), w(&a0), w(&s1), w(&s9)], { let mut d = vec![34u8]; d.extend_from_slice(&u64::MAX.to_le_bytes()); d }, &[]);
    println!("G-2: liquidate_book after claiming -> {:?}", liq);
    assert_eq!(liq, Some(5), "the account was never liquidatable");
}

// =====================================================================================================================
// G-3 (HIGH, keeper/oracle_push.py): the pusher now sends the WHOLE book (all shards + books + slabs) with the oracle update
// every SAMPLE_S, inside the same instruction and with the old budget of 16_000 CU per market (+2_000). The book scan is paid
// per resting order; anyone can rest cheap orders (MIN_ORDER_NOTIONAL) to push update_oracle over that budget. The pusher then
// fails the whole update (price included), backs the market off, and retries with the book forever (SAMPLED is only set on a
// successful send): the market's oracle is no longer pushed and goes stale.
// This test measures update_oracle's CU with the book passed against the pusher's budget for one market.
// =====================================================================================================================
#[test] fn g3_book_sample_exceeds_pusher_cu_budget() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let m = e.market;
    let bids: Vec<Address> = (0..4u64).map(|i| e.shard(0, 99_000 - i * 1000)).collect();
    let asks: Vec<Address> = (0..4u64).map(|i| e.shard(1, 101_000 + i * 1000)).collect();
    let mut all = bids.clone(); all.extend(asks.iter().cloned());
    let o = e.oracle;
    let mut cu_at = vec![];
    for round in 0..8u64 {
        // 8 users x 16 one-lot orders spread over the 8 shards (2 per shard each)
        for _ in 0..2 {
            let (x, xu) = e.user(10_000_000);
            for half in 0..2u64 {
                let mut its = vec![];
                for (j, s) in all.iter().enumerate() {
                    let _ = s; let bid = j < 4; let lo = if bid { 99_000 - j as u64 * 1000 } else { 101_000 + (j as u64 - 4) * 1000 };
                    its.push(place_item(if bid { 0 } else { 1 }, 1, lo + round * 10 + half, 1, 0, 0));
                }
                e.send("", trade_accts(&x, &xu, &m, &all), batch_data(&[], &its), &[&x], true);
            }
        }
        let s = e.slot + 1; e.set_time(s); e.set_pyth(1000_00000000, 1000_0000);
        let mut acc = vec![w(&m), r(&o)]; acc.extend(all.iter().map(r));
        let cu = e.send("upd", acc, vec![4], &[], true).unwrap();
        let n: u64 = all.iter().map(|s| e.shard_state(s).n).sum();
        cu_at.push((n, cu));
    }
    println!("G-3: (resting orders, update_oracle CU with the whole book): {:?}; pusher budget for one market = 18_000", cu_at);
    let worst = cu_at.last().unwrap().1;
    assert!(worst > 18_000, "BUG present when a populated book costs more than the pusher's budget ({} CU)", worst);
}

// ---------- fast book helpers (same as review2.rs) ----------
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
// G-4 (MEDIUM, mirror of the fix in liquidate_fast, not addressed): in a hybrid market liquidate_book fills ONLY shard orders
// (up to 5% / 10% / 20% from the mark) and never looks at the registered maker book. The permissionless liquidator can choose
// liquidate_book over liquidate_fast and route the chunk to its own resting bid 4.9% under the mark while makers bid 0.1% under
// it; the shipped crank also tries liquidate_book first whenever the market has shards. The target (and, past bankruptcy,
// insurance / ADL counterparties) eats the difference.
// =====================================================================================================================
#[test] fn g4_liquidate_book_routes_around_maker_quotes() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let (a0, bl) = (e.shard(1, 100_000), e.shard(0, 87_500)); let m = e.market;
    let f = fast_new(&mut e, 0);
    let k = maker(&mut e, &f, 10_000_000_000, 1_000_000_000);
    let (lp, lpu) = e.user(50_000_000_000); let (t, tu) = e.user(1_500_000); let (x, xu) = e.user(50_000_000_000);
    e.send("", trade_accts(&lp, &lpu, &m, &[a0]), single(place_item(1, 1, 100_000, 200, 0, 0)), &[&lp], true);
    e.send("", trade_accts(&t, &tu, &m, &[a0]), single(place_item(0, IOC, 100_000, 130, 0, 0)), &[&t], true);
    e.set_time(e.slot + 1); e.oracle_update("", 920_00000000, None);
    // liquidator's own bid 4.9% under the mark; makers bid 91_900 (0.1% under)
    e.send("", trade_accts(&x, &xu, &m, &[bl]), single(place_item(0, 1, 87_500, 100, 0, 0)), &[&x], true);
    quote(&mut e, &k, &[(0, 91_900, 1000)]);
    e.set_time(e.slot + 1); e.oracle_update("", 920_00000000, None); quote(&mut e, &k, &[(0, 91_900, 1000)]);
    let ins = e.ins;
    let cash0 = e.user_state(&tu).cash;
    let mut acc = vec![w(&tu), r(&m), w(&ins), w(&bl), w(&a0)];
    let res = try_send(&mut e, std::mem::take(&mut acc), { let mut d = vec![34u8]; d.extend_from_slice(&u64::MAX.to_le_bytes()); d }, &[]);
    let (sold, cash1) = (130 - e.user_state(&tu).pos[0].base, e.user_state(&tu).cash);
    println!("G-4: liquidate_book -> {:?}; sold {} lots for {} (avg {}), maker bid 91_900 untouched", res, sold, cash1 - cash0, if sold > 0 { (cash1 - cash0) / sold as i64 } else { 0 });
    assert_eq!(res, Some(1), "fixed: a side with maker quotes is liquidated through liquidate_fast"); let _ = (sold, cash1, cash0);
}

#[test] fn g0_sizes() { println!("SHARD_SIZE {} FAST_SIZE {}", SHARD_SIZE, FAST_SIZE); assert_eq!(SHARD_SIZE, 10096); }
