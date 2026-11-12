// Re-implemented matching fixes, discovery bounds, external mark and the vault, against the real SBF program.
#[path = "../../program/src/state.rs"]
#[allow(dead_code)]
mod state;
include!("common.rs");
const ASK: u16 = 8;
/// Market config on a live market: only through the timelock (ADMIN_MARKET_CFG 13); the immediate IX is refused once priced.
fn cfg_key(e: &mut Env, bps: u64, th: u64, mx: u64, strict: u64, key: [u8; 32]) {
    let (a, m) = (e.admin.pubkey(), e.market);
    let mut d = vec![69u8];
    for v in [bps, th, mx, strict] {
        d.extend_from_slice(&v.to_le_bytes());
    }
    d.extend_from_slice(&key);
    e.send("", vec![AccountMeta::new(a, true), w(&m)], d, &[], false);
    let v = bps | th << 16 | mx << 32 | strict << 48;
    let mut d = vec![28u8, 13];
    d.extend_from_slice(&v.to_le_bytes());
    d.extend_from_slice(&key);
    e.send("", vec![AccountMeta::new(a, true), w(&m)], d, &[], true);
    let ms = e.market_state();
    let px = ms.oracle_px as i64 * ms.px_div / ms.px_mul.max(1);
    let s = e.slot + 216_000;
    e.set_time(s);
    e.send("", vec![w(&m)], vec![29], &[], true);
    e.oracle_update("", px, None);
}
fn cfg(e: &mut Env, bps: u64, th: u64, mx: u64, strict: u64) {
    cfg_key(e, bps, th, mx, strict, [0u8; 32]);
}
fn session(e: &mut Env, on: u8) {
    let (a, m) = (e.admin.pubkey(), e.market);
    e.send("", vec![AccountMeta::new(a, true), w(&m)], vec![68, on], &[], true);
}
fn place(e: &mut Env, kp: &Keypair, u: &Address, shards: &[Address], side: u8, fl: u8, px: u64, q: u64, ok: bool) {
    let m = e.market;
    let mut d = vec![1u8];
    d.extend_from_slice(&place_item(side, fl, px, q, 0, 0));
    e.send("", trade_accts(kp, u, &m, shards), d, &[kp], ok);
}
#[test]
fn p2_older_order_wins_a_price_tie_across_shards() {
    let mut e = Env::new();
    e.oracle_update("", 1000_00000000, None);
    let (a, b) = (e.shard(1, 100_000), e.shard(1, 100_000));
    let (m1, u1) = e.user(10_000_000_000);
    let (m2, u2) = e.user(10_000_000_000);
    let (t, tu) = e.user(10_000_000_000);
    place(&mut e, &m1, &u1, &[b], 1, 1, 100_010, 10, true);
    let s = e.slot;
    e.set_time(s + 2);
    e.oracle_update("", 1000_00000000, None);
    place(&mut e, &m2, &u2, &[a], 1, 1, 100_010, 10, true);
    place(&mut e, &t, &tu, &[a, b], 0, 4, 100_010, 10, true);
    assert_eq!(e.shard_state(&b).n, 0, "older order in the second shard fills first");
    assert_eq!(e.shard_state(&a).n, 1);
}
#[test]
fn p1_overflowed_shard_stays_closed_until_empty() {
    let mut e = Env::new();
    e.oracle_update("", 1000_00000000, None);
    let (a, b) = (e.shard(1, 100_000), e.shard(1, 100_000));
    let (t, tu) = e.user(10_000_000_000);
    let mut us = vec![];
    for _ in 0..5 {
        us.push(e.user(10_000_000_000));
    }
    let mut n = 0;
    'outer: for (kp, u) in &us {
        for _ in 0..16 {
            if n == 64 {
                break 'outer;
            }
            place(&mut e, kp, u, &[a], 1, 1, 100_010, 10, true);
            n += 1;
        }
    }
    assert_eq!(e.shard_state(&a).n, 64);
    let (kp, u) = &us[4];
    place(&mut e, kp, u, &[a, b], 1, 1, 100_010, 10, true);
    assert_eq!(e.shard_state(&b).n, 1, "65th order overflows");
    assert!(e.shard_state(&a).seq & (1 << 62) != 0);
    place(&mut e, &t, &tu, &[a, b], 0, 4, 100_010, 10, true);
    assert_eq!(e.shard_state(&a).n, 63);
    let (kp2, u2) = e.user(10_000_000_000);
    place(&mut e, &kp2, &u2, &[a, b], 1, 1, 100_010, 10, true);
    assert_eq!((e.shard_state(&a).n, e.shard_state(&b).n), (63, 2), "the freed slot can't be used to jump the queue");
}
#[test]
fn p3_taker_must_see_every_shard_but_writes_only_what_it_hits() {
    let mut e = Env::new();
    e.oracle_update("", 1000_00000000, None);
    let (a0, a1, b0) = (e.shard(1, 100_000), e.shard(1, 100_100), e.shard(0, 99_900));
    let (mk, mu) = e.user(10_000_000_000);
    let (t, tu) = e.user(10_000_000_000);
    place(&mut e, &mk, &mu, &[a0], 1, 1, 100_010, 10, true);
    place(&mut e, &mk, &mu, &[a1], 1, 1, 100_110, 10, true);
    place(&mut e, &mk, &mu, &[b0], 0, 1, 99_950, 10, true);
    cfg(&mut e, 0, 0, 0, 1);
    place(&mut e, &t, &tu, &[a1], 0, 4, 100_110, 5, false); // would skip the better 100_010
    place(&mut e, &t, &tu, &[a0], 0, 4, 100_010, 5, false); // partial set
    let m = e.market;
    let mut d = vec![1u8];
    d.extend_from_slice(&place_item(0, 4, 100_010, 5, 0, 0));
    e.send("", vec![AccountMeta::new(t.pubkey(), true), w(&tu), r(&m), w(&a0), r(&a1)], d.clone(), &[&t], true);
    assert_eq!(e.user_state(&tu).pos[0].base, 5);
    let mut d = vec![1u8];
    d.extend_from_slice(&place_item(0, 4, 100_010, 1, 0, 0));
    e.send("", vec![AccountMeta::new(t.pubkey(), true), w(&tu), r(&m), r(&a0), w(&a1)], d, &[&t], false);
    // hit a read-only shard
}
#[test]
fn verify1_adl_does_not_count_a_shortfall_twice() {
    let mut e = Env::new();
    e.oracle_update("", 1000_00000000, None);
    let a0 = e.shard(1, 100_000);
    let (c1, cu1) = e.user(600_000_000);
    let (c2, cu2) = e.user(600_000_000);
    let (t, tu) = e.user(1_100_000_000);
    place(&mut e, &c1, &cu1, &[a0], 1, 1, 100_000, 50_000, true);
    place(&mut e, &c2, &cu2, &[a0], 1, 1, 100_000, 50_000, true);
    place(&mut e, &t, &tu, &[a0], 0, 4, 100_000, 100_000, true);
    e.oracle_update("", 600_00000000, None);
    let (m, ins, rk) = (e.market, e.ins, e.rank);
    e.send("", vec![w(&a0), w(&cu1), w(&cu2)], vec![24], &[], true);
    e.rank_user(&cu1);
    e.rank_user(&cu2);
    // ADL steps carry only the closed lots' share of the deficit: repeat until the target is flat
    for _ in 0..4 {
        if e.user_state(&tu).pos[0].base == 0 {
            break;
        }
        e.rank_user(&cu1);
        e.rank_user(&cu2);
        e.send("", vec![w(&tu), w(&m), w(&ins), w(&rk), w(&cu1), w(&cu2)], vec![6], &[], true);
    }
    let (u, ms) = (e.user_state(&tu), e.market_state());
    let eq = u.cash as i128 + u.pos[0].base as i128 * ms.mark_px as i128;
    assert_eq!(eq, 0, "target is made whole to zero; the unrecovered part is only in bad_debt");
    // each counterparty paid only its closed lots' share, out of its profit: nothing left to socialize here
    assert!(ms.bad_debt >= 0);
    assert!(e.user_state(&cu1).cash >= 0 && e.user_state(&cu2).cash >= 0);
}
#[test]
fn bounds_hold_mark_while_closed_with_reanchors_and_a_hard_cap() {
    let mut e = Env::new();
    e.oracle_update("", 1000_00000000, None);
    cfg(&mut e, 500, 9000, 2, 0);
    e.oracle_update("", 1000_00000000, None);
    assert_eq!(e.market_state().bound_ref, 100_000);
    session(&mut e, 1);
    let step = |e: &mut Env, px: i64| {
        let s = e.slot;
        e.set_time(s + 1);
        e.oracle_update("", px, None);
    };
    step(&mut e, 1030_00000000);
    assert_eq!((e.market_state().bound_ref, e.market_state().bound_up), (100_000, 0));
    step(&mut e, 1046_00000000);
    assert_eq!((e.market_state().bound_ref, e.market_state().bound_up), (105_000, 1));
    step(&mut e, 1080_00000000);
    step(&mut e, 1098_00000000);
    assert_eq!((e.market_state().bound_ref, e.market_state().bound_up), (110_250, 2));
    for p in [1130, 1160, 1190] {
        step(&mut e, p * 100_000_000);
    }
    let m = e.market_state();
    assert_eq!((m.bound_ref, m.bound_up), (110_250, 2), "no resets left: hard cap until the market reopens");
    assert_eq!(m.mark_px, 115_762, "mark stops at the cap (+15.76%, same as the WTI example)");
    // nothing trades outside the range
    let a0 = e.shard(1, 118_900);
    let (mk, mu) = e.user(10_000_000_000);
    let (t, tu) = e.user(100_000_000_000);
    place(&mut e, &mk, &mu, &[a0], 1, 1, 118_950, 10, true);
    place(&mut e, &t, &tu, &[a0], 0, 4, 119_000, 10, true);
    assert_eq!(e.user_state(&tu).pos.iter().map(|p| p.base).sum::<i64>(), 0);
    session(&mut e, 0);
    step(&mut e, 1190_00000000);
    let m = e.market_state();
    assert_eq!((m.bound_ref, m.bound_up), (119_000, 0));
    assert_eq!(m.mark_px, 119_000);
    place(&mut e, &t, &tu, &[a0], 0, 4, 119_000, 10, true);
    assert_eq!(e.user_state(&tu).pos[0].base, 10);
}
#[test]
fn session_key_flips_only_session() {
    let mut e = Env::new();
    e.oracle_update("", 1000_00000000, None);
    let (k, _) = e.user(1000);
    let (x, _) = e.user(1000);
    cfg_key(&mut e, 500, 9000, 2, 0, k.pubkey().to_bytes());
    let m = e.market;
    let mut d = vec![28u8, 13];
    d.extend_from_slice(&0u64.to_le_bytes());
    d.extend_from_slice(&k.pubkey().to_bytes());
    e.send("", vec![AccountMeta::new(k.pubkey(), true), w(&m)], d, &[&k], false); // the session key can't propose config
    e.send("", vec![AccountMeta::new(x.pubkey(), true), w(&m)], vec![68, 1], &[&x], false);
    e.send("", vec![AccountMeta::new(k.pubkey(), true), w(&m)], vec![68, 1], &[&k], true);
    assert_eq!(e.market_state().session, 1);
}
fn agg_oracle(e: &mut Env, px: i64, ext: i64, ext_ts: i64) {
    let mut d = pyth_data(&e.feed, px, (px / 10000) as u64, e.ts, e.slot);
    d.resize(160, 0);
    d[0..8].copy_from_slice(b"PERPAGG\x01");
    d[133..141].copy_from_slice(&ext.to_le_bytes());
    d[141..149].copy_from_slice(&ext_ts.to_le_bytes());
    let (o, pid) = (e.oracle, e.pid);
    e.svm
        .set_account(o, Account { lamports: 1_000_000_000, data: d, owner: pid, executable: false, rent_epoch: 0 })
        .unwrap();
}
#[test]
fn mark_blends_book_and_external_perp() {
    let mut e = Env::new();
    e.oracle_update("", 1000_00000000, None);
    let f = Address::new_unique();
    e.prog_acct(f, FAST_SIZE);
    let mut d = vec![61u8];
    d.extend_from_slice(&0u64.to_le_bytes());
    let (a, m) = (e.admin.pubkey(), e.market);
    e.send("", vec![AccountMeta::new(a, true), w(&m), w(&f)], d, &[], true);
    let (kp, u) = e.user(10_000_000_000);
    let slab = Address::new_unique();
    e.prog_acct(slab, SLAB_SIZE);
    e.send("", vec![AccountMeta::new(kp.pubkey(), true), r(&u), w(&slab)], vec![60], &[&kp], true);
    e.send(
        "",
        vec![AccountMeta::new(a, true), r(&m), w(&f), w(&slab), AccountMeta::new_readonly(kp.pubkey(), true)],
        vec![62],
        &[&kp],
        true,
    );
    let mut d = vec![64u8];
    d.extend_from_slice(&1_000_000_000i64.to_le_bytes());
    e.send("", vec![AccountMeta::new(kp.pubkey(), true), w(&u), r(&m), w(&f)], d, &[&kp], true);
    let mut q = vec![63u8];
    for (i, p) in [(0u16, 100_090u32), (ASK, 100_110)] {
        q.extend_from_slice(&i.to_le_bytes());
        q.extend_from_slice(&p.to_le_bytes());
        q.extend_from_slice(&100u32.to_le_bytes());
    }
    e.send("", vec![AccountMeta::new(kp.pubkey(), true), w(&slab)], q, &[&kp], true);
    let o = e.oracle;
    let (s, ts) = (e.slot, e.ts);
    let _ = s;
    e.set_time(s + 1);
    let ts2 = e.ts.max(ts);
    agg_oracle(&mut e, 1000_00000000, 1002_00000000, ts2);
    e.send("", vec![w(&m), r(&o), r(&f), r(&slab)], vec![4], &[], true);
    assert_eq!(e.market_state().mark_px, 100_100, "book sits between oracle and external");
    assert_eq!(e.market_state().ext_px, 100_200);
    e.set_time(s + 2);
    let t3 = e.ts;
    agg_oracle(&mut e, 1000_00000000, 999_00000000, t3);
    e.send("", vec![w(&m), r(&o), r(&f), r(&slab)], vec![4], &[], true);
    let mp = e.market_state().mark_px;
    assert!(mp >= 100_000 && mp < 100_010, "external pulls mark back to the adjusted oracle: {mp}");
    // an old external price is ignored
    e.set_time(s + 3);
    let t4 = e.ts;
    agg_oracle(&mut e, 1000_00000000, 1100_00000000, t4 - 60);
    e.send("", vec![w(&m), r(&o), r(&f), r(&slab)], vec![4], &[], true);
    assert!(e.market_state().mark_px <= 100_100);
    // omitting the slab is refused
    e.set_time(s + 4);
    agg_oracle(&mut e, 1000_00000000, 0, 0);
    e.send("", vec![w(&m), r(&o), r(&f)], vec![4], &[], false);
}
struct V {
    v: Address,
    sub: Address,
    link: Address,
}
fn vault(e: &mut Env, cap_bps: u64) -> V {
    let (v, sub, sub1, link) =
        (Address::new_unique(), Address::new_unique(), Address::new_unique(), Address::new_unique());
    e.prog_acct(v, VAULT_SIZE);
    e.prog_acct(sub, USER_SIZE);
    e.prog_acct(sub1, USER_SIZE);
    e.prog_acct(link, VLINK_SIZE);
    let (a, m) = (e.admin.pubkey(), e.market);
    let mut d = vec![50u8];
    for x in [100u64, 2000, 0, 2500] {
        d.extend_from_slice(&x.to_le_bytes());
    }
    e.send("", vec![AccountMeta::new(a, true), w(&v), r(&m)], d, &[], true);
    e.send("", vec![AccountMeta::new(a, true), w(&v), w(&sub)], vec![51], &[], true);
    e.send("", vec![AccountMeta::new(a, true), w(&v), w(&sub1)], vec![51], &[], true);
    let mut d = vec![52u8];
    d.extend_from_slice(&cap_bps.to_le_bytes());
    e.send("", vec![AccountMeta::new(a, true), r(&v), r(&m), w(&link)], d, &[], true);
    V { v, sub, link }
}
fn vst(e: &Env, k: &Address) -> Vault {
    unsafe { std::ptr::read_unaligned(e.data(k).as_ptr() as *const Vault) }
}
fn vds(e: &Env, k: &Address) -> VaultDeposit {
    unsafe { std::ptr::read_unaligned(e.data(k).as_ptr() as *const VaultDeposit) }
}
fn holder(e: &mut Env, amt: u64) -> (Keypair, Address, Address) {
    let kp = Keypair::new();
    e.svm.airdrop(&kp.pubkey(), 10_000_000_000).unwrap();
    let vd = Address::new_unique();
    e.prog_acct(vd, VDEP_SIZE);
    let src = Address::new_unique();
    let mint = Address::from(e.market_state().mint);
    e.svm
        .set_account(
            src,
            Account {
                lamports: 1_000_000_000,
                data: token_acct(&mint, &kp.pubkey(), amt),
                owner: TOKEN,
                executable: false,
                rent_epoch: 0,
            },
        )
        .unwrap();
    (kp, vd, src)
}
fn vdep(e: &mut Env, x: &V, kp: &Keypair, vd: &Address, src: &Address, amt: u64, ok: bool) {
    let (m, tv) = (e.market, e.vault);
    let mut d = vec![53u8];
    d.extend_from_slice(&amt.to_le_bytes());
    e.send(
        "",
        vec![AccountMeta::new(kp.pubkey(), true), w(&x.v), w(vd), w(&x.sub), r(&m), w(&tv), w(src), r(&TOKEN)],
        d,
        &[kp],
        ok,
    );
}
fn vwd(e: &mut Env, x: &V, kp: &Keypair, vd: &Address, dst: &Address, sh: u64, ok: bool) {
    let (m, tv, pda) = (e.market, e.vault, e.vault_pda);
    let mut d = vec![54u8];
    d.extend_from_slice(&sh.to_le_bytes());
    e.send(
        "",
        vec![AccountMeta::new(kp.pubkey(), true), w(&x.v), w(vd), w(&x.sub), r(&m), w(&tv), w(dst), r(&TOKEN), r(&pda)],
        d,
        &[kp],
        ok,
    );
}
fn underwater(e: &mut Env, dep: u64, lots: u64) -> (Address, Address) {
    let a0 = e.shard(1, 100_000);
    let (mk, mu) = e.user(100_000_000_000);
    let (t, tu) = e.user(dep);
    place(e, &mk, &mu, &[a0], 1, 1, 100_000, lots, true);
    place(e, &t, &tu, &[a0], 0, 4, 100_000, lots, true);
    (a0, tu)
}
fn takeover(e: &mut Env, x: &V, tu: &Address, ok: bool) {
    let (m, ins) = (e.market, e.ins);
    let mut d = vec![55u8];
    d.extend_from_slice(&u64::MAX.to_le_bytes());
    e.send("", vec![w(&x.v), w(&x.sub), w(tu), r(&m), w(&ins), r(&x.link)], d, &[], ok);
}
#[test]
fn vault_shares_lockup_cap_and_withdraw() {
    let mut e = Env::new();
    e.oracle_update("", 1000_00000000, None);
    let x = vault(&mut e, 5000);
    let (a, av, asrc) = holder(&mut e, 10_000_000_000);
    vdep(&mut e, &x, &a, &av, &asrc, 10_000_000_000, true);
    assert_eq!(vst(&e, &x.v).shares, 10_000_000_000);
    assert_eq!(vst(&e, &x.v).nav, 10_000_000_000);
    assert_eq!(e.user_state(&x.sub).cash, 10_000_000_000);
    vwd(&mut e, &x, &a, &av, &asrc, 1_000_000_000, false); // locked
    let s = e.slot;
    e.set_time(s + 101);
    e.oracle_update("", 1000_00000000, None);
    vwd(&mut e, &x, &a, &av, &asrc, 3_000_000_000, false); // over the daily cap
    let t0 = e.vault_amount();
    vwd(&mut e, &x, &a, &av, &asrc, 1_000_000_000, true);
    assert_eq!(e.vault_amount(), t0 - 1_000_000_000);
    assert_eq!(vds(&e, &av).shares, 9_000_000_000);
    // someone else's receipt is worthless
    let (b, _bv, bsrc) = holder(&mut e, 100);
    vwd(&mut e, &x, &b, &av, &bsrc, 1, false);
    // a later depositor pays NAV: same share price
    let (c, cv, csrc) = holder(&mut e, 900_000_000);
    vdep(&mut e, &x, &c, &cv, &csrc, 900_000_000, true);
    assert_eq!(vds(&e, &cv).shares, 900_000_000);
}
#[test]
fn vault_takes_over_only_below_two_thirds_of_maintenance_and_within_cap() {
    let mut e = Env::new();
    e.oracle_update("", 1000_00000000, None);
    let x = vault(&mut e, 5000);
    let (a, av, asrc) = holder(&mut e, 100_000_000_000);
    vdep(&mut e, &x, &a, &av, &asrc, 100_000_000_000, true);
    let (_a0, tu) = underwater(&mut e, 1_100_000_000, 100_000); // 10B notional on 1.1B equity
    e.oracle_update("", 955_00000000, None); // healthy
    takeover(&mut e, &x, &tu, false);
    e.oracle_update("", 930_00000000, None); // eq 0.395B: under maintenance but above 2/3 of it, the book goes first
    takeover(&mut e, &x, &tu, false);
    e.oracle_update("", 915_00000000, None); // eq 0.245B < 0.305B
    takeover(&mut e, &x, &tu, true);
    assert_eq!(e.user_state(&tu).pos[0].base, 0);
    assert_eq!(e.user_state(&x.sub).pos[0].base, 100_000);
    let (m, _) = (e.market, 0);
    let (sub, v) = (x.sub, x.v);
    e.send("", vec![w(&v), w(&sub), r(&m)], vec![56], &[], true);
    let n = vst(&e, &v).nav;
    assert!(n >= 100_000_000_000, "the vault earned the liquidation fee at the mark: {n}");
}
#[test]
fn vault_capital_absorbs_a_bankrupt_position_and_drawdown_stops_it() {
    let mut e = Env::new();
    e.oracle_update("", 1000_00000000, None);
    let x = vault(&mut e, 10000);
    let (a, av, asrc) = holder(&mut e, 1_000_000_000);
    vdep(&mut e, &x, &a, &av, &asrc, 1_000_000_000, true);
    let (_a0, tu) = underwater(&mut e, 1_100_000_000, 100_000);
    for p in [955, 910, 870, 830] {
        e.oracle_update("", p * 100_000_000, None);
    } // eq -0.7B: bankrupt
    assert!(e.user_state(&tu).cash + (e.user_state(&tu).pos[0].base * e.market_state().mark_px as i64) < 0);
    let v0 = e.user_state(&x.sub).cash;
    takeover(&mut e, &x, &tu, false); // 1B of capital is too small for 8.3B of notional
    let (b, bv, bsrc) = holder(&mut e, 9_000_000_000);
    vdep(&mut e, &x, &b, &bv, &bsrc, 9_000_000_000, true);
    takeover(&mut e, &x, &tu, true);
    let t = e.user_state(&tu);
    assert_eq!((t.pos[0].market == [0; 32] || t.pos[0].base == 0, t.cash), (true, 0));
    assert!(e.user_state(&x.sub).cash < v0 + 9_000_000_000 + 8_300_000_000, "shortfall paid from vault capital");
    let (m, v, sub) = (e.market, x.v, x.sub);
    e.send("", vec![w(&v), w(&sub), r(&m)], vec![56], &[], true);
    for p in [790, 750, 715, 680, 650, 620] {
        e.oracle_update("", p * 100_000_000, None);
    }
    e.send("", vec![w(&v), w(&sub), r(&m)], vec![56], &[], true);
    assert_eq!(vst(&e, &v).halted, 1, "25% drawdown stops takeovers");
    let (c, cv, csrc) = holder(&mut e, 1000);
    vdep(&mut e, &x, &c, &cv, &csrc, 1000, false);
}
/// spec::VAULT_VIRTUAL
fn tok_amt(e: &Env, k: &Address) -> u64 {
    u64::from_le_bytes(e.data(k)[64..72].try_into().unwrap())
}
fn vcrank(e: &mut Env, x: &V) {
    let (m, v, sub) = (e.market, x.v, x.sub);
    e.send("", vec![w(&v), w(&sub), r(&m)], vec![56], &[], true);
}
fn redeemable(e: &Env, x: &V, vd: &Address) -> i128 {
    let v = vst(e, &x.v);
    vds(e, vd).shares as i128 * (v.nav as i128 + 1_000_000i128) / (v.shares as i128 + 1_000_000i128)
}
/// After the vault loses on a taken-over position, a new depositor buys in at the lower NAV: their deposit is worth what they
/// paid, and they take out what they put in. The earlier holder carries the loss alone.
#[test]
fn vault_loss_is_not_passed_to_later_depositors() {
    let mut e = Env::new();
    e.oracle_update("", 1000_00000000, None);
    let x = vault(&mut e, 10000);
    let (a, av, asrc) = holder(&mut e, 100_000_000_000);
    vdep(&mut e, &x, &a, &av, &asrc, 100_000_000_000, true);
    let (_a0, tu) = underwater(&mut e, 1_100_000_000, 100_000);
    e.oracle_update("", 915_00000000, None);
    takeover(&mut e, &x, &tu, true);
    e.oracle_update("", 880_00000000, None);
    vcrank(&mut e, &x);
    let nav = vst(&e, &x.v).nav;
    assert!(nav < 100_000_000_000, "the vault is down: {nav}");
    let a_before = redeemable(&e, &x, &av);
    let (b, bv, bsrc) = holder(&mut e, 10_000_000_000);
    vdep(&mut e, &x, &b, &bv, &bsrc, 10_000_000_000, true);
    let bval = redeemable(&e, &x, &bv);
    assert!((bval - 10_000_000_000).abs() <= 2, "new deposit is worth what was paid: {bval}");
    assert!((redeemable(&e, &x, &av) - a_before).abs() <= 2, "the earlier holder is not diluted");
    let s = e.slot;
    e.set_time(s + 101);
    e.oracle_update("", 880_00000000, None);
    let sh = vds(&e, &bv).shares;
    let t0 = tok_amt(&e, &bsrc);
    vwd(&mut e, &x, &b, &bv, &bsrc, sh, true);
    let got = tok_amt(&e, &bsrc) - t0;
    assert!((got as i128 - 10_000_000_000).abs() <= 2, "B takes out what B put in: {got}");
}
/// Cash backing a taken-over position can't leave: a withdrawal that would put the vault below initial margin fails.
#[test]
fn vault_withdraw_keeps_positions_margined() {
    let mut e = Env::new();
    e.oracle_update("", 1000_00000000, None);
    let x = vault(&mut e, 10000);
    let (_a0, tu) = underwater(&mut e, 1_100_000_000, 100_000);
    let (a, av, asrc) = holder(&mut e, 12_000_000_000);
    vdep(&mut e, &x, &a, &av, &asrc, 12_000_000_000, true);
    let (m, v, pda) = (e.market, x.v, x.sub);
    let _ = (m, v, pda);
    // raise the day cap so only the margin rule can stop the withdrawal
    let ad = e.admin.pubkey();
    let mut d = vec![57u8];
    for q in [100u64, 10000, 2500, 0] {
        d.extend_from_slice(&q.to_le_bytes());
    }
    e.send("", vec![AccountMeta::new(ad, true), w(&x.v)], d, &[], true);
    for p in [955, 915] {
        e.oracle_update("", p * 100_000_000, None);
    }
    takeover(&mut e, &x, &tu, true);
    let s = e.slot;
    e.set_time(s + 101);
    e.oracle_update("", 915_00000000, None);
    let sh = vds(&e, &av).shares;
    vwd(&mut e, &x, &a, &av, &asrc, sh, false); // all of it: the 9.15B position would be unmargined
    vwd(&mut e, &x, &a, &av, &asrc, sh / 20, true); // a small slice is fine
}

fn shard_move(e: &mut Env, adm: &Keypair, shard: &Address, lo: u64, ok: bool) {
    let m = e.market;
    let mut d = vec![88u8]; // IX_SHARD_MOVE
    d.extend_from_slice(&lo.to_le_bytes());
    e.send("", vec![AccountMeta::new(adm.pubkey(), true), r(&m), w(shard)], d, &[adm], ok);
}

#[test]
fn an_empty_shard_moves_to_where_the_price_went() {
    let mut e = Env::new();
    e.oracle_update("", 1000_00000000, None);
    let adm = e.admin.insecure_clone();
    let (a, b) = (e.shard(1, 100_000), e.shard(0, 100_000 - BW));
    let (mk, mu) = e.user(10_000_000_000);
    let (tk, tu) = e.user(10_000_000_000);
    let before = e.shard_state(&a);
    let (n_shards, n_side) = (e.market_state().n_shards, e.market_state().n_side);

    // an ask rests in the shard: it cannot be moved from under it
    place(&mut e, &mk, &mu, &[a], 1, 1, 100_010, 10, true);
    shard_move(&mut e, &adm, &a, 100_000 + BW, false);
    // a taker lifts it: no order is left, but the maker's fill waits in the shard to be claimed. Still not movable.
    place(&mut e, &tk, &tu, &[a], 0, 4, 100_010, 10, true);
    let s = e.shard_state(&a);
    assert_eq!((s.n, s.nled), (0, 1));
    shard_move(&mut e, &adm, &a, 100_000 + BW, false);
    assert_eq!(e.shard_state(&a).lo, 100_000);
    // the maker's next transaction over the shard claims the fill (it places an order and takes it back)
    place(&mut e, &mk, &mu, &[a], 1, 1, 100_020, 5, true);
    let id = e.shard_state(&a).orders[0].id;
    let m = e.market;
    e.send("", trade_accts(&mk, &mu, &m, &[a]), batch_data(&[cancel_item(id, 1, 100_020)], &[]), &[&mk], true);
    let s = e.shard_state(&a);
    assert_eq!((s.n, s.nled), (0, 0));
    assert_eq!(e.user_state(&mu).pos[0].base, -10, "the fill was booked before the shard went anywhere");

    // the price range above is outside this shard; after the move it is inside, and the old range is outside
    let far = 100_000 + BW + 10;
    place(&mut e, &mk, &mu, &[a], 1, 1, far, 5, false);
    shard_move(&mut e, &tk, &a, 100_000 + BW, false); // not the admin
    shard_move(&mut e, &adm, &a, 100_000 + BW + 1, false); // not on the price grid
    shard_move(&mut e, &adm, &m, 100_000 + BW, false); // not a shard
    shard_move(&mut e, &adm, &a, 100_000 + BW, true);
    let after = e.shard_state(&a);
    assert_eq!((after.lo, after.width), (100_000 + BW, BW));
    assert_eq!(
        (after.side, after.shard_no, after.market),
        (before.side, before.shard_no, before.market),
        "its side, its number and its market are what they were"
    );
    assert!(after.seq >= 2, "the order sequence went on, so no order id is issued twice: {}", after.seq);
    let ms = e.market_state();
    assert_eq!((ms.n_shards, ms.n_side), (n_shards, n_side), "the market counts the same shards");
    place(&mut e, &mk, &mu, &[a], 1, 1, far, 5, true);
    assert_eq!(e.shard_state(&a).n, 1);
    place(&mut e, &mk, &mu, &[a], 1, 1, 100_010, 5, false);
    // the other side's shard was not touched
    assert_eq!(e.shard_state(&b).lo, 100_000 - BW);
}
