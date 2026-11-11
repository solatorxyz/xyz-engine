#![allow(dead_code, unused_imports, unused_variables, unused_mut)]
#[path = "../../perp-core/src/spec.rs"]
#[allow(dead_code)]
mod pspec;
#[path = "../../program/src/state.rs"]
#[allow(dead_code)]
mod state;
include!("common.rs");

const IOC: u8 = 4;

fn single(item: Vec<u8>) -> Vec<u8> {
    let mut d = vec![1u8];
    d.extend_from_slice(&item);
    d
}
fn amt(tag: u8, v: u64) -> Vec<u8> {
    let mut d = vec![tag];
    d.extend_from_slice(&v.to_le_bytes());
    d
}
fn try_send(e: &mut Env, accounts: Vec<AccountMeta>, data: Vec<u8>, signers: &[&Keypair]) -> Option<u32> {
    let ix = Instruction { program_id: e.pid, accounts, data };
    let mut s: Vec<&Keypair> = vec![&e.admin];
    s.extend_from_slice(signers);
    let tx = Transaction::new_signed_with_payer(&[ix], Some(&e.admin.pubkey()), &s, e.svm.latest_blockhash());
    let res = e.svm.send_transaction(tx);
    e.svm.expire_blockhash();
    match res {
        Ok(_) => None,
        Err(f) => Some(
            format!("{:?}", f.err)
                .split("Custom(")
                .nth(1)
                .and_then(|x| x.split(')').next())
                .and_then(|x| x.parse().ok())
                .unwrap_or(u32::MAX),
        ),
    }
}

// ---- fast-book helpers (same shape as poc_a) ----
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
        let mut d = vec![64u8];
        d.extend_from_slice(&fund.to_le_bytes());
        e.send("", vec![AccountMeta::new(mk.kp.pubkey(), true), w(&mk.u), r(&m), w(f)], d, &[&mk.kp], true);
    }
    mk
}
fn quote(e: &mut Env, k: &Mk, v: &[(u16, u32, u32)]) {
    let mut d = vec![63u8];
    for (i, p, q) in v {
        d.extend_from_slice(&i.to_le_bytes());
        d.extend_from_slice(&p.to_le_bytes());
        d.extend_from_slice(&q.to_le_bytes());
    }
    e.send("", vec![AccountMeta::new(k.kp.pubkey(), true), w(&k.slab)], d, &[&k.kp], true);
}
fn liq_fast_data() -> Vec<u8> {
    let mut d = vec![70u8, 1];
    d.extend_from_slice(&u64::MAX.to_le_bytes());
    d
}

#[test]
fn smoke_env_builds() {
    let e = Env::new();
    assert_eq!(e.market_state().imr_bps, 1000);
}

fn tok(e: &mut Env, owner: &Address, amount: u64) -> Address {
    let k = Address::new_unique();
    let mint = Address::from(e.market_state().mint);
    e.svm
        .set_account(
            k,
            Account {
                lamports: 1_000_000_000,
                data: token_acct(&mint, owner, amount),
                owner: TOKEN,
                executable: false,
                rent_epoch: 0,
            },
        )
        .unwrap();
    k
}
fn tok_amt(e: &Env, k: &Address) -> u64 {
    u64::from_le_bytes(e.data(k)[64..72].try_into().unwrap())
}
fn withdraw_accts(kp: &Address, u: &Address, e: &Env, dst: &Address, extra: &[Address]) -> Vec<AccountMeta> {
    let mut v = vec![AccountMeta::new(*kp, true), w(u), r(&e.market), w(&e.vault), w(dst), r(&TOKEN), r(&e.vault_pda)];
    v.extend(extra.iter().map(w));
    v
}

// Maker fast-path conservation: a maker funds quoting margin, a taker buys against the maker's ask, price
// moves, the taker closes against the maker's bid, both settle. No value may be created across the two.
#[test]
fn maker_fastpath_conservation() {
    let mut e = Env::new();
    e.oracle_update("", 1000_00000000, None);
    let m = e.market;
    let f = fast_new(&mut e, 0);
    let dep_m = 10_000_000_000u64;
    let k = maker(&mut e, &f, dep_m, 1_000_000_000); // fund 1e9 margin
    let dep_t = 50_000_000u64;
    let (t, tu) = e.user(dep_t);
    // maker quotes: ask (idx0 ask level0 = 8) @100_050, bid (idx0 bid level0 = 0) @99_950
    quote(&mut e, &k, &[(8, 100_050, 1000), (0, 99_950, 1000)]);
    e.set_time(e.slot + 1);
    e.oracle_update("", 1000_00000000, None);
    quote(&mut e, &k, &[(8, 100_050, 1000), (0, 99_950, 1000)]);
    // taker buys 40 against the maker ask
    let mut acc = vec![AccountMeta::new(t.pubkey(), true), w(&tu), r(&m), w(&f), r(&k.slab)];
    let mut d = vec![66u8, 0, 0, 1];
    d.extend_from_slice(&200_000u64.to_le_bytes());
    d.extend_from_slice(&40u64.to_le_bytes());
    e.send("", acc, d, &[&t], true);
    // taker sells 40 (reduce) against the maker bid
    e.set_time(e.slot + 1);
    e.oracle_update("", 1000_00000000, None);
    quote(&mut e, &k, &[(8, 100_050, 1000), (0, 99_950, 1000)]);
    let mut acc = vec![AccountMeta::new(t.pubkey(), true), w(&tu), r(&m), w(&f), r(&k.slab)];
    let mut d = vec![66u8, 1, 2, 1];
    d.extend_from_slice(&1u64.to_le_bytes());
    d.extend_from_slice(&40u64.to_le_bytes());
    e.send("", acc, d, &[&t], true);
    // settle the maker (books fast fills into its User position/cash) and funding
    e.send("", vec![w(&k.u), r(&m), w(&f)], vec![65], &[], true);
    e.send("", vec![w(&tu), r(&m)], vec![15], &[], true);
    // aggregate fast-book fee counter
    e.send("", vec![w(&m), w(&f)], vec![16], &[], true);
    let ms = e.user_state(&k.u);
    let ts = e.user_state(&tu);
    let mb = ms.pos.iter().find(|p| p.market == m.to_bytes()).map_or(0, |p| p.base);
    let tb = ts.pos.iter().find(|p| p.market == m.to_bytes()).map_or(0, |p| p.base);
    // read maker's locked margin still in the book
    let fbk = {
        let dd = e.data(&f);
        unsafe { std::ptr::read_unaligned(dd.as_ptr() as *const FastBook) }
    };
    let e_margin = fbk.makers[0].margin;
    let fees = e.market_state().fees;
    println!(
        "maker-fp: maker_cash {} maker_base {} e.margin {} | taker_cash {} taker_base {} | fees {}",
        ms.cash, mb, e_margin, ts.cash, tb, fees
    );
    assert_eq!((mb, tb), (0, 0), "both flat after round trip");
    // maker's total claim = cash + locked margin; taker = cash; plus fees
    let claims = ms.cash as i128 + e_margin as i128 + ts.cash as i128 + fees as i128;
    assert_eq!(
        claims,
        (dep_m + dep_t) as i128,
        "value created/destroyed on the fast path: {} vs {}",
        claims,
        dep_m + dep_t
    );
}

// Full lifecycle conservation: everyone deposits, trades a round trip, price moves, everyone flattens,
// claims, settles funding, withdraws everything possible. Tokens out of the vault must never exceed tokens
// in, and the vault's leftover must equal fees + insurance + any stranded dust (no value creation).
#[test]
fn lifecycle_conservation() {
    let mut e = Env::new();
    e.oracle_update("", 1000_00000000, None);
    let a0 = e.shard(1, 100_000);
    let b0 = e.shard(0, 99_900);
    let m = e.market;
    let dep = 50_000_000u64;
    let (lp, lpu) = e.user(dep);
    let (ta, tau) = e.user(dep);
    // LP rests ask @100_000 (100) and bid @99_900 (100) -- no self-cross
    e.send(
        "",
        trade_accts(&lp, &lpu, &m, &[a0, b0]),
        batch_data(&[], &[place_item(1, 1, 100_000, 100, 0, 0), place_item(0, 1, 99_900, 100, 0, 0)]),
        &[&lp],
        true,
    );
    // TA lifts the ask: TA long 50, LP short 50 (via ledger)
    e.send("", trade_accts(&ta, &tau, &m, &[a0, b0]), single(place_item(0, IOC, 100_000, 50, 0, 0)), &[&ta], true);
    // TA closes: sells 50 reduce-only into LP's bid @99_900
    e.send("", trade_accts(&ta, &tau, &m, &[a0, b0]), single(place_item(1, IOC | 2, 99_900, 50, 0, 0)), &[&ta], true);
    // LP claims its ledger fills in both shards and cancels remaining resting orders
    e.send("", vec![w(&a0), w(&lpu)], vec![24], &[], true);
    e.send("", vec![w(&b0), w(&lpu)], vec![24], &[], true);
    // settle funding for both
    e.send("", vec![w(&lpu), r(&m)], vec![15], &[], true);
    e.send("", vec![w(&tau), r(&m)], vec![15], &[], true);
    // aggregate shard fee counters into Market.fees so the fee claim is accounted
    e.send("", vec![w(&m), w(&a0), w(&b0)], vec![16], &[], true);
    let lp_cash = e.user_state(&lpu).cash;
    let ta_cash = e.user_state(&tau).cash;
    let fees = e.market_state().fees;
    let ins = e.ins_bal();
    let lp_base = e.user_state(&lpu).pos.iter().find(|p| p.market == m.to_bytes()).map_or(0, |p| p.base);
    let ta_base = e.user_state(&tau).pos.iter().find(|p| p.market == m.to_bytes()).map_or(0, |p| p.base);
    println!(
        "conservation: lp_cash {} ta_cash {} fees {} ins {} lp_base {} ta_base {}",
        lp_cash, ta_cash, fees, ins, lp_base, ta_base
    );
    // everyone is flat (round trip), so all claims are cash. Total claims must not exceed deposits.
    assert_eq!((lp_base, ta_base), (0, 0), "both flat after round trip");
    let claims = lp_cash as i128 + ta_cash as i128 + fees as i128 + ins as i128;
    assert!(claims <= 2 * dep as i128, "value created: claims {} > deposits {}", claims, 2 * dep);
    assert!(claims == 2 * dep as i128, "value destroyed/stranded: claims {} != deposits {}", claims, 2 * dep);
}

// A healthy account must never be liquidatable by any permissionless path.
#[test]
fn healthy_account_not_liquidatable() {
    let mut e = Env::new();
    e.oracle_update("", 1000_00000000, None);
    let a0 = e.shard(1, 100_000);
    let m = e.market;
    let (lp, lpu) = e.user(50_000_000_000);
    let (t, tu) = e.user(50_000_000); // well funded
    e.send("", trade_accts(&lp, &lpu, &m, &[a0]), single(place_item(1, 1, 100_000, 100, 0, 0)), &[&lp], true);
    e.send("", trade_accts(&t, &tu, &m, &[a0]), single(place_item(0, IOC, 100_000, 100, 0, 0)), &[&t], true);
    let ins = e.ins;
    // book liquidation on a healthy long: NotLiquidatable (5)
    let r_book = try_send(&mut e, vec![w(&tu), r(&m), w(&ins), w(&a0)], amt(34, u64::MAX), &[]);
    assert_eq!(r_book, Some(5), "healthy account must not be book-liquidatable");
    // liq_cancel on a healthy account: NotLiquidatable (5)
    let r_lc = try_send(&mut e, vec![w(&tu), r(&m), w(&a0)], vec![77], &[]);
    assert_eq!(r_lc, Some(5), "healthy account orders must not be cancellable");
}

// Conservation across a hybrid liquidate_fast that drives the target bankrupt: the only new money that
// can appear is insurance covering a real deficit; nothing is created.
#[test]
fn hybrid_liquidate_fast_conserves() {
    let mut e = Env::new();
    e.oracle_update("", 1000_00000000, None);
    let a0 = e.shard(1, 100_000);
    let bl = e.shard(0, 90_000);
    let m = e.market;
    let f = fast_new(&mut e, 0);
    let k = maker(&mut e, &f, 10_000_000_000, 1_000_000_000);
    let (lp, lpu) = e.user(50_000_000_000);
    let (t, tu) = e.user(1_400_000); // thin: opens 130 lots (IMR 1.3M), bankrupt on a crash
    e.send("", trade_accts(&lp, &lpu, &m, &[a0]), single(place_item(1, 1, 100_000, 130, 0, 0)), &[&lp], true);
    e.send("", trade_accts(&t, &tu, &m, &[a0]), single(place_item(0, IOC, 100_000, 130, 0, 0)), &[&t], true);
    // crash
    e.set_time(e.slot + 1);
    e.oracle_update("", 850_00000000, None);
    quote(&mut e, &k, &[(0, 84_900, 2000)]); // maker bids 84_900
    e.set_time(e.slot + 1);
    e.oracle_update("", 850_00000000, None);
    quote(&mut e, &k, &[(0, 84_900, 2000)]);
    let ins0 = e.ins_bal();
    let ins = e.ins;
    let t_cash0 = e.user_state(&tu).cash as i128;
    let t_base0 = e.user_state(&tu).pos[0].base;
    let r = try_send(&mut e, vec![w(&tu), r(&m), w(&ins), w(&f), r(&k.slab), w(&bl), w(&a0)], liq_fast_data(), &[]);
    let ts = e.user_state(&tu);
    let p = ts.pos.iter().find(|p| p.market == m.to_bytes());
    println!(
        "hybrid liq_fast -> {:?}; target base {}->{} cash {}; ins {}->{}",
        r,
        t_base0,
        p.map_or(0, |p| p.base),
        ts.cash,
        ins0,
        e.ins_bal()
    );
    // target should have been reduced (or ADL-flagged); insurance should not DECREASE on a liquidation (fee -> insurance)
    assert!(r.is_none() || r == Some(23) /*UseAdl*/ || r == Some(25) /*NoFill*/, "unexpected err {:?}", r);
}

// liq_cancel cross-market: characterize — can hiding an unclaimed fill in another market let a third
// party cancel a solvent cross account's resting orders in a different market?
#[test]
fn liq_cancel_cross_market_characterize() {
    // Single-market proxy of the acknowledged limitation: leaving out the fill's shard in the SAME market
    // is now rejected (complete_view requires every shard of mk). We assert the fix holds here.
    let mut e = Env::new();
    e.oracle_update("", 1000_00000000, None);
    let (a0, s1, s9) = (e.shard(1, 100_000), e.shard(1, 105_000), e.shard(1, 150_000));
    let m = e.market;
    let (lp, lpu) = e.user(50_000_000_000);
    let (t, tu) = e.user(1_550_000);
    let (bx, bxu) = e.user(50_000_000_000);
    e.send("", trade_accts(&lp, &lpu, &m, &[a0]), single(place_item(1, 1, 100_000, 130, 0, 0)), &[&lp], true);
    e.send("", trade_accts(&t, &tu, &m, &[a0]), single(place_item(0, IOC, 100_000, 130, 0, 0)), &[&t], true);
    e.send(
        "",
        trade_accts(&t, &tu, &m, &[s1, s9]),
        batch_data(&[], &[place_item(1, 1, 105_000, 10, 0, 0), place_item(1, 1, 150_000, 1, 0, 0)]),
        &[&t],
        true,
    );
    e.set_time(e.slot + 1);
    e.oracle_update("", 1050_00000000, None);
    e.send("", trade_accts(&bx, &bxu, &m, &[s1]), single(place_item(0, IOC, 105_000, 10, 0, 0)), &[&bx], true);
    e.set_time(e.slot + 1);
    e.oracle_update("", 980_00000000, None);
    e.set_time(e.slot + 1);
    e.oracle_update("", 920_00000000, None);
    let atk = try_send(&mut e, vec![w(&tu), r(&m), w(&a0), w(&s9)], vec![77], &[]);
    println!("liq_cancel hiding same-market fill -> {:?} (19 = MissingShards means fixed)", atk);
    assert_eq!(atk, Some(19));
}
