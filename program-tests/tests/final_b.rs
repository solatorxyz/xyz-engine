#[path = "../../program/src/state.rs"]
#[allow(dead_code)]
mod state;
// Final audit, part B (dispatch / authority / oracle / vault). Each test performs the scenario against the built SBF program and
// asserts what the CURRENT code does: a PASSING test means the issue is PRESENT. Invert the final assertions once fixed.
include!("common.rs");

const E_STALE: u32 = 4; const E_CAP: u32 = 12; const E_BAD_DATA: u32 = 2;

fn try_send(e: &mut Env, accounts: Vec<AccountMeta>, data: Vec<u8>, signers: &[&Keypair]) -> Option<u32> {
    let ix = Instruction { program_id: e.pid, accounts, data };
    let mut s: Vec<&Keypair> = vec![&e.admin]; s.extend_from_slice(signers);
    let tx = Transaction::new_signed_with_payer(&[ix], Some(&e.admin.pubkey()), &s, e.svm.latest_blockhash());
    let r = e.svm.send_transaction(tx); e.svm.expire_blockhash();
    match r { Ok(_) => None, Err(f) => { let s = format!("{:?}", f.err);
        Some(s.split("Custom(").nth(1).and_then(|x| x.split(')').next()).and_then(|x| x.parse().ok()).unwrap_or(u32::MAX)) } }
}
fn adm_d(tag: u8, op: u8, v: u64, k: Option<&Address>) -> Vec<u8> {
    let mut d = vec![tag, op]; d.extend_from_slice(&v.to_le_bytes()); if let Some(k) = k { d.extend_from_slice(k.as_ref()); } d
}
/// Our own aggregated oracle account (IX 40) at `at`, posted to by `publisher`.
fn agg_new(e: &mut Env, at: Address, publisher: &Keypair, feed: &[u8; 32]) {
    e.prog_acct(at, state::AGG_SIZE);
    let mut d = vec![40u8]; d.extend_from_slice(feed); d.extend_from_slice(&(-8i32).to_le_bytes()); d.extend_from_slice(&0u64.to_le_bytes());
    assert_eq!(try_send(e, vec![AccountMeta::new(publisher.pubkey(), true), w(&at)], d, &[publisher]), None);
}
/// IX 41: price (1e-8 units), conf, publish time, n_sources.
fn agg_post(e: &mut Env, at: &Address, publisher: &Keypair, px: i64, pt: i64) -> Option<u32> {
    let mut d = vec![41u8]; for v in [px, px / 10000, pt, 3] { d.extend_from_slice(&v.to_le_bytes()); }
    try_send(e, vec![AccountMeta::new(publisher.pubkey(), true), w(at)], d, &[publisher])
}
fn upd(e: &mut Env, extra: &[Address]) -> Option<u32> {
    let (m, o) = (e.market, e.oracle);
    let mut acc = vec![w(&m), r(&o)]; acc.extend(extra.iter().map(r));
    try_send(e, acc, vec![4], &[])
}
fn single(item: Vec<u8>) -> Vec<u8> { let mut d = vec![1u8]; d.extend_from_slice(&item); d }
fn place(e: &mut Env, kp: &Keypair, u: &Address, shards: &[Address], side: u8, fl: u8, px: u64, q: u64) {
    let m = e.market; e.send("", trade_accts(kp, u, &m, shards), single(place_item(side, fl, px, q, 0, 0)), &[kp], true);
}
/// A 10x long of `lots` at 1000 for `taker` against a resting ask.
fn long10x(e: &mut Env, dep: u64, lots: u64) -> (Keypair, Address, Address) {
    let a0 = e.shard(1, 100_000);
    let (mk, mu) = e.user(100_000_000_000); let (t, tu) = e.user(dep);
    place(e, &mk, &mu, &[a0], 1, 1, 100_000, lots);
    place(e, &t, &tu, &[a0], 0, 4, 100_000, lots);
    (t, tu, a0)
}
fn equity(e: &Env, u: &Address) -> i128 {
    let s = e.user_state(u); let mp = e.market_state().mark_px as i128;
    s.cash as i128 + s.pos.iter().filter(|p| p.market != [0; 32]).map(|p| p.base as i128 * mp).sum::<i128>()
}

// =====================================================================================================================
// B-1 (MEDIUM): the oracle2 cross-check (divergence breaker) is skipped whenever oracle2's publish time is older than the
// LAST APPLIED publish time: update_oracle reads oracle2 with `floor = m.oracle_ts`, which is the primary's publish time, so
// read_pyth returns Stale for an oracle2 update that is perfectly current by wall clock but published a second after the
// primary's previous post. (Ok(p1), Some(Err)) then takes the `single` path: only the 5%-per-update move check applies and the
// 1% divergence breaker never fires. A primary publisher that posts `publish_time = now + 5` (agg_post allows up to +5 s)
// keeps oracle2 permanently "stale" and drifts the price by up to 5% per update with the cross-check silently disabled; a
// lagging oracle2 (Pyth aggregate times trail the wall clock) produces the same silence by accident.
// Control: the same 3% divergence with the primary stamped at `now` trips the breaker (paused = 2, price frozen).
// =====================================================================================================================
fn o2_env(ahead: i64) -> (Env, Option<u32>, Option<u32>) {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let (p, q) = (Keypair::new(), Keypair::new());
    for k in [&p, &q] { e.svm.airdrop(&k.pubkey(), 10_000_000_000).unwrap(); }
    // the primary becomes an AGG posted to by publisher P; oracle2 an AGG posted to by publisher Q (any PriceUpdateV2-shaped
    // account works the same: the fields read are identical)
    let (o1, feed) = (e.oracle, e.feed); agg_new(&mut e, o1, &p, &feed);
    let o2 = Address::new_unique(); let feed2 = [9u8; 32]; agg_new(&mut e, o2, &q, &feed2);
    let (a, m) = (e.admin.pubkey(), e.market);
    let mut d = adm_d(28, 10, 0, Some(&o2)); d.extend_from_slice(&feed2);
    e.send("", vec![AccountMeta::new(a, true), w(&m)], d, &[], true);
    let s = e.slot + 216_000; e.set_time(s); e.send("", vec![w(&m)], vec![29], &[], true);
    assert_eq!(e.market_state().oracle2, o2.to_bytes());
    // t0: both agree at 1000; P stamps its post `ahead` seconds into the future (allowed: <= now + 5), Q at now
    let t0 = e.ts;
    assert_eq!(agg_post(&mut e, &o1, &p, 1000_00000000, t0 + ahead), None);
    assert_eq!(agg_post(&mut e, &o2, &q, 1000_00000000, t0), None);
    assert_eq!(upd(&mut e, &[o2]), None);
    assert_eq!((e.market_state().oracle_px, e.market_state().paused), (100_000, 0));
    // one second later: P says 1030 (3% off), Q still says 1000 and is current (published this second)
    let s = e.slot + 3; e.set_time(s); let t1 = e.ts; assert_eq!(t1, t0 + 1);
    assert_eq!(agg_post(&mut e, &o1, &p, 1030_00000000, t1 + ahead), None);
    assert_eq!(agg_post(&mut e, &o2, &q, 1000_00000000, t1), None);
    let r = upd(&mut e, &[o2]);
    (e, r, None)
}
#[test] fn b1_oracle2_divergence_breaker_skipped_when_primary_stamps_ahead() {
    // control: honest timestamps -> 3% divergence trips the breaker, price frozen
    let (e, r, _) = o2_env(0);
    assert_eq!(r, None);
    assert_eq!((e.market_state().paused, e.market_state().oracle_px), (2, 100_000), "control: breaker trips on a 3% divergence");
    // the primary stamps +5 s: oracle2 (1 s old by wall clock) is 'Stale' against the floor, no cross-check, price moves 3%
    let (e, r, _) = o2_env(5);
    assert_eq!(r, None);
    assert_eq!((e.market_state().paused, e.market_state().oracle_px), (2, 100_000), "fixed: oracle2 keeps its own replay floor, the breaker trips");
    // a 2 s lead is enough (oracle2's stamp t1 < floor t0 + 2): an oracle2 whose publish times trail the primary's by more than
    // one second (Pyth aggregate times routinely trail the wall clock) is permanently excluded from the cross-check by accident
    let (e, r, _) = o2_env(2);
    assert_eq!(r, None);
    assert_eq!((e.market_state().paused, e.market_state().oracle_px), (2, 100_000), "fixed: 2 s lead, breaker still trips");
}

// =====================================================================================================================
// B-2 (MEDIUM, design): on a market priced by a single AGG (no oracle2), the jump guard (AUDIT V6-2) holds a > 5% move until
// "a LATER publish confirms it" -- but the confirming publish may come from the SAME publisher key. One compromised publisher
// key moves the price by any amount in two posts one second apart (pause of new risk in between, liquidations open after).
// Production markets use an OracleSet (median of 3, quorum 2) which is not exposed to this; any market still on one AGG is.
// =====================================================================================================================
#[test] fn b2_single_agg_publisher_confirms_its_own_jump() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let (t, tu, _a0) = long10x(&mut e, 1_100_000_000, 100_000); let _ = t; // 10B notional on 1.1B equity
    let p = Keypair::new(); e.svm.airdrop(&p.pubkey(), 10_000_000_000).unwrap();
    let (o1, feed) = (e.oracle, e.feed); agg_new(&mut e, o1, &p, &feed);
    let t0 = e.ts;
    assert_eq!(agg_post(&mut e, &o1, &p, 1000_00000000, t0), None); assert_eq!(upd(&mut e, &[]), None);
    assert!(equity(&e, &tu) > 0);
    // post 1: -50% -> held as a candidate, market paused (3)
    let s = e.slot + 3; e.set_time(s); let t1 = e.ts;
    assert_eq!(agg_post(&mut e, &o1, &p, 500_00000000, t1), None); assert_eq!(upd(&mut e, &[]), None);
    assert_eq!((e.market_state().oracle_px, e.market_state().paused), (100_000, 3));
    // post 2, same key, one second later: "confirmed", price halves, the long is bankrupt
    let s = e.slot + 3; e.set_time(s); let t2 = e.ts;
    assert_eq!(agg_post(&mut e, &o1, &p, 500_00000000, t2), None); assert_eq!(upd(&mut e, &[]), None);
    assert_eq!((e.market_state().oracle_px, e.market_state().paused), (50_000, 0), "one key, two posts: price -50%");
    assert!(equity(&e, &tu) < 0, "every 10x long is now bankrupt at the attacker's price");
}

// =====================================================================================================================
// B-3 (MEDIUM, liveness under stress): without oracle2 every primary (single AGG or OracleSet) goes through the jump guard.
// A move > 5% is accepted only when the NEXT publish is within ORACLE_RESUME_BPS (0.5%) of the held candidate; otherwise the
// candidate is replaced and nothing is applied. In a fast move (> 0.5% between consecutive publishes, i.e. per second) the
// oracle therefore never updates: the market goes stale within 3 slots, deposits are refused (paused = 3), liquidations fail
// with Stale, and when the price finally settles the oracle jumps straight to the bottom -- positions that could have been
// liquidated on the way down are then bankrupt (insurance / ADL instead of orderly liquidation).
// =====================================================================================================================
#[test] fn b3_fast_crash_stalls_the_oracle_and_blocks_liquidation() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let (_t, tu, a0) = long10x(&mut e, 1_100_000_000, 100_000); // 10B notional on 1.1B: liquidatable below ~947, bankrupt below 890
    let (m, ins) = (e.market, e.ins);
    let (kp, u) = e.user(0); // a bystander who will try to add margin during the hold
    let liq = |e: &mut Env| { let mut d = vec![34u8]; d.extend_from_slice(&u64::MAX.to_le_bytes()); try_send(e, vec![w(&tu), r(&m), w(&ins), w(&a0)], d, &[]) };
    // the market falls 1% a second: 940, 930, ..., 890 (every publish > 0.5% from the previous one)
    for (i, px) in [940i64, 930, 920, 910, 900, 890].iter().enumerate() {
        let s = e.slot + 3; e.set_time(s); e.set_pyth(px * 100_000_000, 1_000_000);
        assert_eq!(upd(&mut e, &[]), None);
        // fixed: the first > 5% step is held; each later publish continuing the same way confirms and is applied
        if i == 0 { assert_eq!((e.market_state().oracle_px, e.market_state().paused), (100_000, 3)); }
        else { assert_eq!((e.market_state().oracle_px as i64, e.market_state().paused), (px * 100, 0), "step {px} applied"); }
    }
    // the oracle followed the crash step by step: the long is liquidatable on the way down, not bankrupt at the bottom
    assert_eq!(e.market_state().oracle_px, 89_000);
    assert_ne!(liq(&mut e), Some(E_STALE));
}

// =====================================================================================================================
// B-4 (MEDIUM, governance): a collateral pool's admin is a single hot key fixed forever. init_pool takes the admin from a
// signer (`sg`: program-owned accounts never sign), so a Multisig account cannot be the pool admin; there is no instruction
// that changes Pool.admin; every pooled market's admin is pinned to it (init_market) and ADMIN_SET_ADMIN is refused for
// pooled markets at propose and execute. A compromised pool-admin key therefore cannot be rotated out: it can pause / unpause
// every pooled market at will, propose ADMIN_ORACLE to its own AGG for each market, and the only defence is to keep
// cancelling the proposal (admin_propose op 0, which the attacker can re-propose) with the same compromised key, forever.
// Mitigation available today: make the pool admin a PDA of an external multisig program (it signs through CPI and is not
// program-owned, so `sg` accepts it); the program's own Multisig account cannot be used, and nothing can be rotated later.
// =====================================================================================================================
#[test] fn b4_pool_admin_is_a_fixed_hot_key() {
    let mut e = Env::new();
    // a 2-of-3 multisig account (created from a keypair, as every program-owned account is)
    let ms = Keypair::new(); e.prog_acct(ms.pubkey(), MULTISIG_SIZE);
    let ks: Vec<Keypair> = (0..3).map(|_| Keypair::new()).collect();
    let mut d = vec![33u8, 2, 3]; for k in &ks { d.extend_from_slice(k.pubkey().as_ref()); }
    let a = e.admin.pubkey(); e.send("", vec![AccountMeta::new(a, true), w(&ms.pubkey())], d, &[], true);
    // a pool whose admin should be the multisig: refused (a program-owned account's signature does not count), and there is
    // no other way to name an admin
    let pool = Address::new_unique(); e.prog_acct(pool, POOL_SIZE);
    let (vault_pda, bump) = Address::find_program_address(&[b"vault", pool.as_ref()], &e.pid);
    let mint = Address::from(e.market_state().mint); let vault = Address::new_unique();
    e.svm.set_account(vault, Account { lamports: 1_000_000_000, data: token_acct(&mint, &vault_pda, 0), owner: TOKEN, executable: false, rent_epoch: 0 }).unwrap();
    let mut d = vec![27u8, bump]; d.extend_from_slice(&0u64.to_le_bytes());
    assert_eq!(try_send(&mut e, vec![AccountMeta::new(ms.pubkey(), true), w(&pool), r(&vault)], d.clone(), &[&ms]), Some(E_BAD_DATA), "multisig can't be the pool admin");
    // so the admin is a plain key
    let adm = Keypair::new(); e.svm.airdrop(&adm.pubkey(), 10_000_000_000).unwrap();
    assert_eq!(try_send(&mut e, vec![AccountMeta::new(adm.pubkey(), true), w(&pool), r(&vault)], d, &[&adm]), None);
    // a pooled market: admin pinned to that key
    let market = Address::new_unique(); let ins = Address::new_unique(); e.prog_acct(market, MARKET_SIZE); e.prog_acct(ins, INS_SIZE);
    let mut d = vec![32u8]; d.extend_from_slice(market.as_ref());
    e.send("", vec![w(&pool), AccountMeta::new_readonly(adm.pubkey(), true)], d, &[&adm], true);
    let s = e.slot + 216_000; e.set_time(s);
    let mut d = vec![10u8, 0]; d.extend_from_slice(&e.feed);
    for v in [BW, 3, 100, 10, (-8i64) as u64, 1, 1_000_000, 1_000_000_000] { d.extend_from_slice(&v.to_le_bytes()); }
    let o = e.oracle;
    e.send("", vec![AccountMeta::new(a, true), w(&market), w(&ins), r(&vault), r(&o), w(&pool), AccountMeta::new_readonly(adm.pubkey(), true)], d, &[&adm], true);
    let md = e.data(&market); let m: Market = unsafe { std::ptr::read_unaligned(md.as_ptr() as *const Market) };
    assert_eq!(m.admin, adm.pubkey().to_bytes());
    // ... and can never be moved to a multisig: SET_ADMIN is refused on a pooled market, Pool.admin has no setter
    assert_eq!(try_send(&mut e, vec![AccountMeta::new(adm.pubkey(), true), w(&market)], adm_d(28, 8, 0, Some(&ms.pubkey())), &[&adm]), Some(E_CAP));
    // meanwhile the key alone pauses / unpauses the market instantly and arms a 24 h oracle switch to a key it controls
    assert_eq!(try_send(&mut e, vec![AccountMeta::new(adm.pubkey(), true), w(&market)], adm_d(22, 1, 1, None), &[&adm]), None);
    let own = Address::new_unique();
    assert_eq!(try_send(&mut e, vec![AccountMeta::new(adm.pubkey(), true), w(&market)], adm_d(28, 11, 0, Some(&own)), &[&adm]), None);
    let md = e.data(&market); let m: Market = unsafe { std::ptr::read_unaligned(md.as_ptr() as *const Market) };
    assert_eq!((m.paused, m.pending_op, m.pending_key), (1, 11, own.to_bytes()));
}

// =====================================================================================================================
// B-5 (LOW): vault_takeover checks the inventory cap against the STORED Vault.nav (last vault_crank / deposit / withdraw),
// not the live value. After the vault's taken-over position loses, a second takeover passes a cap the live NAV would refuse.
// The crank is expected to call vault_crank first; nothing in the program requires it.
// =====================================================================================================================
struct V { v: Address, sub: Address, link: Address }
fn vault(e: &mut Env, cap_bps: u64) -> V {
    let (v, sub, link) = (Address::new_unique(), Address::new_unique(), Address::new_unique());
    e.prog_acct(v, VAULT_SIZE); e.prog_acct(sub, USER_SIZE); e.prog_acct(link, VLINK_SIZE);
    let (a, m) = (e.admin.pubkey(), e.market);
    let mut d = vec![50u8]; for x in [100u64, 10000, 0, 10000] { d.extend_from_slice(&x.to_le_bytes()); }
    e.send("", vec![AccountMeta::new(a, true), w(&v), r(&m)], d, &[], true);
    e.send("", vec![AccountMeta::new(a, true), w(&v), w(&sub)], vec![51], &[], true);
    let mut d = vec![52u8]; d.extend_from_slice(&cap_bps.to_le_bytes());
    e.send("", vec![AccountMeta::new(a, true), r(&v), r(&m), w(&link)], d, &[], true);
    V { v, sub, link }
}
fn vst(e: &Env, k: &Address) -> Vault { unsafe { std::ptr::read_unaligned(e.data(k).as_ptr() as *const Vault) } }
fn vdep(e: &mut Env, x: &V, amt: u64) {
    let kp = Keypair::new(); e.svm.airdrop(&kp.pubkey(), 10_000_000_000).unwrap();
    let vd = Address::new_unique(); e.prog_acct(vd, VDEP_SIZE);
    let src = Address::new_unique(); let mint = Address::from(e.market_state().mint);
    e.svm.set_account(src, Account { lamports: 1_000_000_000, data: token_acct(&mint, &kp.pubkey(), amt), owner: TOKEN, executable: false, rent_epoch: 0 }).unwrap();
    let (m, tv) = (e.market, e.vault); let mut d = vec![53u8]; d.extend_from_slice(&amt.to_le_bytes());
    e.send("", vec![AccountMeta::new(kp.pubkey(), true), w(&x.v), w(&vd), w(&x.sub), r(&m), w(&tv), w(&src), r(&TOKEN)], d, &[&kp], true);
}
fn takeover(e: &mut Env, x: &V, tu: &Address, shard: &Address) -> Option<u32> {
    let (m, ins) = (e.market, e.ins); let mut d = vec![55u8]; d.extend_from_slice(&u64::MAX.to_le_bytes());
    try_send(e, vec![w(&x.v), w(&x.sub), w(tu), r(&m), w(&ins), r(&x.link), w(shard)], d, &[])
}
#[test] fn b5_vault_takeover_cap_uses_stale_stored_nav() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let x = vault(&mut e, 5500); vdep(&mut e, &x, 1_000_000_000); // 1B capital, inventory cap 55% of NAV
    let a0 = e.shard(1, 100_000); let (mk, mu) = e.user(100_000_000_000);
    let tgt = |e: &mut Env, lots: u64| { let (t, tu) = e.user(55_000_000); place(e, &mk, &mu, &[a0], 1, 1, 100_000, lots); place(e, &t, &tu, &[a0], 0, 4, 100_000, lots); tu };
    let (t1, t2) = (tgt(&mut e, 5_000), tgt(&mut e, 5_000)); // two 10x longs, 0.5B notional each
    for p in [955, 915] { e.oracle_update("", p * 100_000_000, None); }
    assert_eq!(takeover(&mut e, &x, &t1, &a0), None); // 0.4575B <= 0.55B: in cap. t2 is also below 2/3 of maintenance now
    assert_eq!(takeover(&mut e, &x, &t2, &a0), Some(E_CAP), "second position: 0.915B > 0.55B, refused (control)");
    // the market halves; nobody cranks the vault. Stored NAV still 1B (set at the deposit), live NAV ~0.79B
    for p in [870, 830, 790, 750, 715, 680, 650, 620, 590, 560, 530, 500] { e.oracle_update("", p * 100_000_000, None); }
    let v = vst(&e, &x.v); assert!(v.nav >= 1_000_000_000, "stored nav stale: {}", v.nav);
    let live = equity(&e, &x.sub); assert!(live < 800_000_000, "live nav: {live}");
    // t2 (bankrupt, 0.25B at 500) is taken over: the sub then holds 0.5B against 0.79B live NAV (63% > the 55% cap), and the
    // takeover itself pays t2's 0.19B shortfall from vault capital -- yet the cap check passes against the stored 1B
    assert_eq!(takeover(&mut e, &x, &t2, &a0), Some(E_CAP), "fixed: the cap is judged on the live NAV");
}
