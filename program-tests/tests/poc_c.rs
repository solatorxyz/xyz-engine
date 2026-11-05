#[path = "../../program/src/state.rs"]
#[allow(dead_code)]
mod state;
// Audit C proofs of concept (order book / oracle & funding / liquidation & ADL).
// Each test performs the attack against the built SBF program and asserts the exploit SUCCEEDS on the current code.
include!("common.rs");

const IOC: u8 = 4; const SPL: u8 = 2;
fn single(item: Vec<u8>) -> Vec<u8> { let mut d = vec![1u8]; d.extend_from_slice(&item); d }
fn amt(tag: u8, v: u64) -> Vec<u8> { let mut d = vec![tag]; d.extend_from_slice(&v.to_le_bytes()); d }
fn code(r: Result<(), String>) -> Option<u32> {
    match r { Ok(()) => None, Err(s) => Some(s.split("Custom(").nth(1).and_then(|x| x.split(')').next()).and_then(|x| x.parse().ok()).unwrap_or(u32::MAX)) }
}
/// Send several instructions in ONE transaction (atomic). Err carries the debug error + logs.
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
#[allow(dead_code)]
fn set_market_u64(e: &mut Env, off: usize, v: u64) {
    let m = e.market; let mut acct = e.svm.get_account(&m).unwrap();
    acct.data[off..off + 8].copy_from_slice(&v.to_le_bytes()); e.svm.set_account(m, acct).unwrap();
}

// ------------------------------------------------------------------------------------------------------------------
// C-1 (Critical): the same shard passed twice to `take` leaves a stale (index, price) cache entry.
//   (a) the next order in the shard fills at the previous (better) order's price;
//   (b) after the shard empties, the stale entry fills a PHANTOM copy of the removed order (remove_order does not clear
//       the slot): the maker's ledger gets open_reduce > ask_open, so `claim` underflows forever -> the maker's fills are
//       never booked and its position in this market can never be liquidated (every liquidation path needs that shard).
// ------------------------------------------------------------------------------------------------------------------
#[test] fn c1a_duplicate_shard_fills_spline_ladder_at_first_level_price() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let a0 = e.shard(1, 100_000); let m = e.market;
    let (mk, mku) = e.user(10_000_000_000); let (tk, tku) = e.user(10_000_000_000);
    // victim maker: spline ask, 8 levels of 10 lots, 1% apart: 100_000, 101_000, ..., 107_000
    e.send("", trade_accts(&mk, &mku, &m, &[a0]), single(place_ex(1, 0, SPL, 100_000, 10, 0, 0, 0, 1_000, 8, 0)), &[&mk], true);
    assert_eq!(e.shard_state(&a0).orders[0].qty, 80);
    // attacker: buy 80 with limit 100_000 (only level 0 is at that price) and the shard passed 8 times
    let accts = trade_accts(&tk, &tku, &m, &[a0, a0, a0, a0, a0, a0, a0, a0]);
    assert_eq!(try_send(&mut e, accts, single(place_item(0, IOC, 100_000, 80, 0, 0)), &[&tk]), Some(1)); // fixed: a shard passed twice is refused
}

#[test] fn c1b_duplicate_shard_phantom_fill_bricks_maker_and_makes_it_unliquidatable() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let (a0, a1, b0) = (e.shard(1, 100_000), e.shard(1, 100_100), e.shard(0, 99_900)); let m = e.market;
    let (mk, mku) = e.user(200_000); let (tk, tku) = e.user(10_000_000_000); let (cp, cpu) = e.user(10_000_000_000);
    // M rests an ask of 10 in shard a0
    e.send("", trade_accts(&mk, &mku, &m, &[a0]), single(place_item(1, 0, 100_050, 10, 0, 0)), &[&mk], true);
    // T buys 15 with a0 passed twice: 10 real + 5 phantom lots from M's removed order
    assert_eq!(try_send(&mut e, trade_accts(&tk, &tku, &m, &[a0, a0]), single(place_item(0, IOC, 100_050, 15, 0, 0)), &[&tk]), Some(1)); // fixed: a shard passed twice is refused
}

#[test] fn c1c_duplicate_shard_next_limit_order_filled_at_previous_price() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let a0 = e.shard(1, 100_000); let m = e.market;
    let (mm, mmu) = e.user(10_000_000_000); let (tk, tku) = e.user(1_000_000_000);
    e.send("", trade_accts(&mm, &mmu, &m, &[a0]), batch_data(&[], &[place_item(1, 1, 100_010, 10, 0, 0), place_item(1, 1, 100_090, 10, 0, 0)]), &[&mm], true);
    // exactly 20 (the existing duplicate_shard_in_taker_set test asks 50 with 3 copies, which aborts and hides this)
    assert_eq!(try_send(&mut e, trade_accts(&tk, &tku, &m, &[a0, a0]), single(place_item(0, IOC, 100_010, 20, 0, 0)), &[&tk]), Some(1)); // fixed: a shard passed twice is refused
}

// ------------------------------------------------------------------------------------------------------------------
// C-2 (High): liquidate_book takes from whatever shards the (permissionless) caller passes. The caller omits the shard
// with the real bids and passes only its own bid 20% below the mark: the band escalation (5% -> 10% -> 20%) walks down
// to it, and the target's chunk is sold 20% under the mark while a bid 0.1% under the mark rests in another shard.
// ------------------------------------------------------------------------------------------------------------------
#[test] fn c2_liquidate_book_caller_steers_fill_to_own_far_bid() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let (a0, bgood, bfar) = (e.shard(1, 100_000), e.shard(0, 87_900), e.shard(0, 79_200)); let m = e.market;
    let (mm, mmu) = e.user(50_000_000_000); let (vic, vicu) = e.user(200_000); let (lq, lqu) = e.user(5_000_000_000);
    e.send("", trade_accts(&mm, &mmu, &m, &[a0]), single(place_item(1, 1, 100_000, 100, 0, 0)), &[&mm], true);
    e.send("", trade_accts(&vic, &vicu, &m, &[a0]), single(place_item(0, IOC, 100_000, 13, 0, 0)), &[&vic], true);
    e.send("", trade_accts(&mm, &mmu, &m, &[a0]), batch_data(&[], &[]), &[&mm], true);
    e.set_time(e.slot + 1); e.oracle_update("", 880_00000000, None); // below MM, not bankrupt
    // honest deep bid just under the mark
    e.send("", trade_accts(&mm, &mmu, &m, &[bgood]), single(place_item(0, 1, 87_950, 100, 0, 0)), &[&mm], true);
    // attacker's bid at mark - 20%
    e.send("", trade_accts(&lq, &lqu, &m, &[bfar]), single(place_item(0, 1, 79_200, 100, 0, 0)), &[&lq], true);
    let c0 = e.user_state(&vicu).cash;
    let ins = e.ins;
    // permissionless book liquidation, attacker passes only its own shard
    assert_eq!(try_send(&mut e, vec![w(&vicu), r(&m), w(&ins), w(&bfar)], amt(34, u64::MAX), &[]), Some(19)); // fixed: every opposite shard must be passed
}

// ------------------------------------------------------------------------------------------------------------------
// C-3 (High): ADL settles the target's WHOLE-account deficit on the first counterparty step, however small the
// counterparty's closed size: a 1-lot short with a large balance pays the deficit of a 13-lot long out of its own cash,
// and the bankrupt target is reset to zero equity while keeping 12 lots open (a free option funded by the counterparty).
// ------------------------------------------------------------------------------------------------------------------
#[test] fn c3_adl_one_lot_counterparty_pays_entire_deficit_target_keeps_position() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let (a0, a1) = (e.shard(1, 100_000), e.shard(1, 100_100)); let m = e.market;
    let (mm, mmu) = e.user(50_000_000_000); let (vic, vicu) = e.user(1_500_000);
    let (c, cu) = e.user(5_000_000); // small short (1 lot) with 5M cash
    e.send("", trade_accts(&c, &cu, &m, &[a1]), single(place_item(1, 1, 100_100, 1, 0, 0)), &[&c], true);
    e.send("", trade_accts(&mm, &mmu, &m, &[a0]), single(place_item(1, 1, 100_000, 129, 0, 0)), &[&mm], true);
    e.send("", trade_accts(&vic, &vicu, &m, &[a0, a1]), single(place_item(0, IOC, 100_100, 130, 0, 0)), &[&vic], true);
    e.send("", trade_accts(&c, &cu, &m, &[a1]), batch_data(&[], &[]), &[&c], true); // claim: C is short 1
    e.send("", trade_accts(&mm, &mmu, &m, &[a0]), batch_data(&[], &[]), &[&mm], true); // claim: MM is short 129
    assert_eq!(e.user_state(&cu).pos[0].base, -1);
    e.set_time(e.slot + 1); e.oracle_update("", 800_00000000, None); // -20%: target deeply bankrupt
    // only C gets ranked (adl_rank is permissionless; nobody is obliged to rank the whale)
    e.rank_user(&cu);
    let c_cash0 = e.user_state(&cu).cash;
    let (ins, rk) = (e.ins, e.rank);
    e.send("", vec![w(&vicu), w(&m), w(&ins), w(&rk), w(&cu)], vec![6], &[], true);
    let v = e.user_state(&vicu); let cc = e.user_state(&cu);
    let paid = c_cash0 - 80_000 - cc.cash; // C pays 80_000 to buy back its 1 lot at the mark; the rest is the deficit
    println!("C-3: target base {} equity {}; 1-lot counterparty paid {paid} of the deficit; ins {}, bad_debt {}", v.pos[0].base,
        v.cash as i128 + v.pos[0].base as i128 * 80_000, e.ins_bal(), e.market_state().bad_debt);
    assert_eq!(v.pos[0].base, 129, "target keeps 129 of 130 lots");
    assert!(v.cash as i128 + 129 * 80_000 < 0, "fixed: only the closed lot's share of the deficit is covered; the rest stays with the target");
}

// ------------------------------------------------------------------------------------------------------------------
// C-4 (High): one OracleSet publisher (of 3, quorum 2) halts the market's oracle for good: agg_post stores an unchecked
// ext_ts, and read_ext computes `now - ts` -> i64 overflow panic for ts = i64::MIN. read_ext_primary reads every member,
// so update_oracle aborts for everyone even though the other two members are fine.
// ------------------------------------------------------------------------------------------------------------------
fn post(px: i64, ts: i64) -> Vec<u8> { let mut d = vec![41u8]; for v in [px, 1_000_000, ts, 3] { d.extend_from_slice(&v.to_le_bytes()); } d }
fn post_ext(px: i64, ts: i64, ext: i64, ext_ts: i64) -> Vec<u8> { let mut d = post(px, ts); d.extend_from_slice(&ext.to_le_bytes()); d.extend_from_slice(&ext_ts.to_le_bytes()); d }
fn member(e: &mut Env, k: &Keypair) -> Address {
    let a = Address::new_unique(); e.prog_acct(a, state::AGG_SIZE);
    let mut d = vec![40u8]; d.extend_from_slice(&e.feed); d.extend_from_slice(&(-8i32).to_le_bytes()); d.extend_from_slice(&2000u64.to_le_bytes());
    assert_eq!(try_send(e, vec![AccountMeta::new(k.pubkey(), true), w(&a)], d, &[k]), None);
    a
}
fn set_env() -> (Env, Vec<Keypair>, Vec<Address>) {
    let mut e = Env::new(); let o = e.oracle; e.prog_acct(o, state::OSET_SIZE);
    let ks: Vec<Keypair> = (0..3).map(|_| Keypair::new()).collect();
    let ms: Vec<Address> = ks.iter().map(|k| member(&mut e, k)).collect();
    let a = e.admin.pubkey();
    let mut d = vec![43u8, 2, 3]; for x in &ms { d.extend_from_slice(x.as_ref()); }
    assert_eq!(try_send(&mut e, vec![AccountMeta::new(a, true), w(&o)], d, &[]), None);
    (e, ks, ms)
}
fn upd_set(e: &mut Env, ms: &[Address], extra: &[Address]) -> Option<u32> {
    let (m, o) = (e.market, e.oracle);
    let mut acc = vec![w(&m), r(&o)]; for x in ms { acc.push(r(x)); } for x in extra { acc.push(r(x)); }
    try_send(e, acc, vec![4], &[])
}
#[test] fn c4_one_publisher_bricks_oracleset_with_ext_ts_min() {
    let (mut e, ks, ms) = set_env(); let ts = e.ts;
    for i in 0..3 { let (k, a) = (&ks[i], ms[i]); assert_eq!(try_send(&mut e, vec![AccountMeta::new(k.pubkey(), true), w(&a)], post(1000_00000000, ts), &[k]), None); }
    assert_eq!(upd_set(&mut e, &ms, &[]), None);
    // publisher C posts a normal price with ext_ts = i64::MIN
    let (k, a) = (&ks[2], ms[2]);
    assert_eq!(try_send(&mut e, vec![AccountMeta::new(k.pubkey(), true), w(&a)], post_ext(1000_00000000, ts + 1, 1000_00000000, i64::MIN), &[k]), None);
    for i in 0..2 { let (k, a) = (&ks[i], ms[i]); assert_eq!(try_send(&mut e, vec![AccountMeta::new(k.pubkey(), true), w(&a)], post(1000_00000000, ts + 1), &[k]), None); }
    e.set_time(e.slot + 1);
    let (m, o) = (e.market, e.oracle);
    let mut acc = vec![w(&m), r(&o)]; for x in &ms { acc.push(r(x)); }
    let err = send_ixs(&mut e, vec![(acc, vec![4])], &[]); assert!(err.is_ok(), "fixed: a bad ext time is stored as none");

    // C can keep it there: every later post can carry the same ext_ts
}

// ------------------------------------------------------------------------------------------------------------------
// C-5 (Medium): a single publisher alone sets the external perp price (no quorum on ext), and the book mid can be
// flashed in the same transaction (place -> update_oracle -> cancel). Together they move the mark to the +5% clamp in
// ONE update, without any EMA build-up. Same for a thin shard book (fast-path liquidity is ignored when no FastBook is
// passed).
// ------------------------------------------------------------------------------------------------------------------
#[test] fn c5_single_publisher_ext_plus_flash_mid_moves_mark_5pct_in_one_update() {
    let (mut e, ks, ms) = set_env(); let ts = e.ts;
    for i in 0..3 { let (k, a) = (&ks[i], ms[i]); assert_eq!(try_send(&mut e, vec![AccountMeta::new(k.pubkey(), true), w(&a)], post(1000_00000000, ts), &[k]), None); }
    assert_eq!(upd_set(&mut e, &ms, &[]), None);
    let (b0, a0) = (e.shard(0, 108_000), e.shard(1, 108_000)); let m = e.market;
    let (x, xu) = e.user(100_000_000);
    // honest members post price only; member C (malicious or compromised) adds ext = +8%
    for i in 0..2 { let (k, a) = (&ks[i], ms[i]); assert_eq!(try_send(&mut e, vec![AccountMeta::new(k.pubkey(), true), w(&a)], post(1000_00000000, ts + 1), &[k]), None); }
    let (k, a) = (&ks[2], ms[2]);
    e.set_time(e.slot + 1);
    let now = e.ts;
    assert_eq!(try_send(&mut e, vec![AccountMeta::new(k.pubkey(), true), w(&a)], post_ext(1000_00000000, ts + 1, 1080_00000000, now), &[k]), None);
    // attacker tx: place bid 108_000 / ask 108_050 (1 lot each), update_oracle with the full shard set, cancel both
    let o = e.oracle;
    let mut upd_acc = vec![w(&m), r(&o)]; for z in &ms { upd_acc.push(r(z)); } upd_acc.push(r(&b0)); upd_acc.push(r(&a0));
    let place = batch_data(&[], &[place_item(0, 1, 108_000, 1, 0, 0), place_item(1, 1, 108_050, 1, 0, 0)]);
    let ids = { let n = e.shard_state(&b0).seq & ((1 << 40) - 1); (e.shard_state(&b0).shard_no << 40 | (n + 1), e.shard_state(&a0).shard_no << 40 | (e.shard_state(&a0).seq & ((1 << 40) - 1)) + 1) };
    let cancel = batch_data(&[cancel_item(ids.0, 0, 108_000), cancel_item(ids.1, 1, 108_050)], &[]);
    let r = send_ixs(&mut e, vec![(trade_accts(&x, &xu, &m, &[b0, a0]), place), (upd_acc, vec![4]), (trade_accts(&x, &xu, &m, &[b0, a0]), cancel)], &[&x]);
    assert!(r.is_ok(), "{r:?}");
    let s = e.market_state();
    println!("C-5: oracle {} ext {} mark {}", s.oracle_px, s.ext_px, s.mark_px);
    assert!(s.mark_px < 101_000, "fixed: mark {} not moved by one publisher + a same-slot order", s.mark_px);
    assert_eq!(e.shard_state(&b0).n + e.shard_state(&a0).n, 0, "no order left on the book: zero fill risk");
}

// ------------------------------------------------------------------------------------------------------------------
// C-6 (High): premium / funding manipulation with zero fill risk. An order placed and cancelled in the same transaction
// is sampled by update_oracle in between. With a thin shard book (e.g. a market whose liquidity is on the fast path:
// fast quotes count only if the caller passes the FastBook) one 1-lot bid sets the premium sample; the attacker's
// update is the last one per slot, so it carries the TWAP weight. One period later the funding rate is ~1%/h.
// ------------------------------------------------------------------------------------------------------------------
#[test] fn c6_flash_bid_sets_premium_and_funding_rate() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let b0 = e.shard(0, 108_000); let m = e.market;
    let (x, xu) = e.user(100_000_000);
    let o = e.oracle;
    let period = e.market_state().funding_period;
    let start = e.market_state().period_start;
    let mut rate_seen = 0i64;
    while e.slot < start + period + 2 {
        // the attacker's tx: [update_oracle (book omitted: sample held), place 10-lot bid, update_oracle (full shard set), cancel]
        e.set_time(e.slot + if e.slot + 400 < start + period { 300 } else { 1 }); e.set_pyth(1000_00000000, 1000_0000);
        let seq = e.shard_state(&b0).seq & ((1 << 40) - 1);
        let id = e.shard_state(&b0).shard_no << 40 | (seq + 1);
        let place = single(place_item(0, 1, 108_000, 10, 0, 0));
        let cancel = batch_data(&[cancel_item(id, 0, 108_000)], &[]);
        let r = send_ixs(&mut e, vec![(vec![w(&m), r(&o)], vec![4]), (trade_accts(&x, &xu, &m, &[b0]), place), (vec![w(&m), r(&o), r(&b0)], vec![4]),
            (trade_accts(&x, &xu, &m, &[b0]), cancel)], &[&x]);
        assert!(r.is_ok(), "{r:?}");
        rate_seen = e.market_state().funding_rate;
    }
    let s = e.market_state();
    println!("C-6: prem_last {} ppb, funding_rate {} ppb/period (cap {}), book orders left {}", s.prem_last, rate_seen, pspec::FUNDING_MAX_RATE_HOURLY_PPB, e.shard_state(&b0).n);
    assert!(s.prem_last.abs() < 1_000_000, "fixed: same-slot order ignored, premium {}", s.prem_last);
    let _ = rate_seen;
    assert_eq!(e.shard_state(&b0).n, 0, "nothing ever rested between transactions");
    assert_eq!(e.user_state(&xu).pos[0].base, 0, "attacker never traded");
}
#[path = "../../perp-core/src/spec.rs"] #[allow(dead_code)] mod pspec;

// ------------------------------------------------------------------------------------------------------------------
// C-7 (Medium): oracle source cherry-picking / breaker bypass. With oracle2 configured, any caller picks which source
// lands: omit oracle2 -> primary alone; pass a non-primary account in slot 1 -> oracle2 alone. Each is only checked
// against a 5% move from the stored price, never against the other source. Even while the divergence breaker is tripped
// (paused = 2, "prices not updated"), prices keep moving -- and liquidations run while paused.
// ------------------------------------------------------------------------------------------------------------------
#[test] fn c7_breaker_bypass_and_source_choice() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let (a, m, o1) = (e.admin.pubkey(), e.market, e.oracle);
    let o2 = Address::new_unique(); let feed2 = [9u8; 32];
    let mut d = vec![28u8, 10]; d.extend_from_slice(&0u64.to_le_bytes()); d.extend_from_slice(o2.as_ref()); d.extend_from_slice(&feed2);
    e.send("", vec![AccountMeta::new(a, true), w(&m)], d, &[], true);
    e.set_time(e.slot + 216_000); e.send("", vec![w(&m)], vec![29], &[], true);
    let put2 = |e: &mut Env, px: i64| { let d = pyth_data(&feed2, px, (px / 10000) as u64, e.ts, e.slot);
        e.svm.set_account(o2, Account { lamports: 1_000_000_000, data: d, owner: PYTH, executable: false, rent_epoch: 0 }).unwrap(); };
    // sources diverge 4%: honest crank (both passed) trips the breaker, price frozen at 100_000
    e.set_time(e.slot + 1); e.set_pyth(1000_00000000, 10_000_000); put2(&mut e, 960_00000000);
    assert_eq!(try_send(&mut e, vec![w(&m), r(&o1), r(&o2)], vec![4], &[]), None);
    assert_eq!((e.market_state().paused, e.market_state().oracle_px), (2, 100_000));
    // attacker: slot-1 account = anything but the primary, oracle2 among the rest -> oracle2 alone is applied
    let junk = Address::new_unique();
    assert_eq!(try_send(&mut e, vec![w(&m), r(&junk), r(&o2)], vec![4], &[]), Some(8)); // fixed: with oracle2 configured the primary and oracle2 are both required
}

// ------------------------------------------------------------------------------------------------------------------
// C-8 (Medium): cancel by order id only searches the FIRST passed shard whose bucket covers `price` and then returns Ok.
// Overflow shards (AUDIT H-3) share a bucket, so an order that sits in the second shard is silently NOT cancelled.
// ------------------------------------------------------------------------------------------------------------------
#[test] fn c8_cancel_silently_misses_order_in_second_shard_of_bucket() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let (s1, s2) = (e.shard(1, 100_000), e.shard(1, 100_000)); let m = e.market;
    let (kp, u) = e.user(1_000_000_000);
    e.send("", trade_accts(&kp, &u, &m, &[s2]), single(place_item(1, 1, 100_050, 10, 0, 0)), &[&kp], true);
    let id = e.shard_state(&s2).orders[0].id;
    // MM cancels, passing both shards of the bucket (the SDK passes the bucket's shards)
    assert_eq!(try_send(&mut e, trade_accts(&kp, &u, &m, &[s1, s2]), batch_data(&[cancel_item(id, 1, 100_050)], &[]), &[&kp]), None, "cancel reports success");
    assert_eq!(e.shard_state(&s2).n, 0, "fixed: cancel finds the order in its own shard");
}
