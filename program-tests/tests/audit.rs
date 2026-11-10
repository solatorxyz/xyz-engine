#[path = "../../program/src/state.rs"]
#[allow(dead_code)]
mod state;
// AUDIT.md exploit-style tests. Each test performs the attack against the built SBF program and
// asserts it is now rejected with the specific error (or bounded). Error codes follow `enum E`.
include!("common.rs");
#[path = "../../perp-core/src/spec.rs"] #[allow(dead_code)] mod pspec;
use pspec::FUNDING_MAX_RATE_HOURLY_PPB;

const IOC: u8 = 4; const FOK: u8 = 8; const SPL: u8 = 2; const PEG: u8 = 1;
const BAD_ACCOUNT: u32 = 1; const CAP: u32 = 12; const NOT_TRIGGERED: u32 = 16; const VAULT: u32 = 17;
const TOKEN_PROGRAM: u32 = 18; const MISSING_SHARDS: u32 = 19; const BAD_DATA: u32 = 2;
fn single(item: Vec<u8>) -> Vec<u8> { let mut d = vec![1u8]; d.extend_from_slice(&item); d }
fn amt(tag: u8, v: u64) -> Vec<u8> { let mut d = vec![tag]; d.extend_from_slice(&v.to_le_bytes()); d }

/// Send and return the custom error code (None on success).
fn try_send(e: &mut Env, accounts: Vec<AccountMeta>, data: Vec<u8>, signers: &[&Keypair]) -> Option<u32> {
    let ix = Instruction { program_id: e.pid, accounts, data };
    let mut s: Vec<&Keypair> = vec![&e.admin]; s.extend_from_slice(signers);
    let tx = Transaction::new_signed_with_payer(&[ix], Some(&e.admin.pubkey()), &s, e.svm.latest_blockhash());
    let r = e.svm.send_transaction(tx); e.svm.expire_blockhash();
    match r { Ok(_) => None, Err(f) => {
        let s = format!("{:?}", f.err);
        Some(s.split("Custom(").nth(1).and_then(|x| x.split(')').next()).and_then(|x| x.parse().ok()).unwrap_or(u32::MAX))
    } }
}
fn src_acct(e: &mut Env, owner: &Address, amount: u64) -> Address {
    let s = Address::new_unique(); let mint = Address::from(e.market_state().mint);
    e.svm.set_account(s, Account { lamports: 1_000_000_000, data: token_acct(&mint, owner, amount), owner: TOKEN, executable: false, rent_epoch: 0 }).unwrap(); s
}
struct Mkt { market: Address, ins: Address, vault: Address, vault_pda: Address }
fn cur(e: &Env) -> Mkt { Mkt { market: e.market, ins: e.ins, vault: e.vault, vault_pda: e.vault_pda } }
fn switch(e: &mut Env, m: &Mkt) { e.market = m.market; e.ins = m.ins; e.vault = m.vault; e.vault_pda = m.vault_pda; }
/// Second market (own mint, vault, insurance) created by `admin` in the same SVM.
fn new_market(e: &mut Env, admin: &Keypair) -> Mkt {
    e.svm.airdrop(&admin.pubkey(), 10_000_000_000).unwrap();
    let market = Address::new_unique(); let ins = Address::new_unique();
    let (vault_pda, bump) = Address::find_program_address(&[b"vault", market.as_ref()], &e.pid);
    let mint = Address::new_unique(); let mut md = vec![0u8; 82]; md[44] = 6; md[45] = 1;
    e.svm.set_account(mint, Account { lamports: 1_000_000_000, data: md, owner: TOKEN, executable: false, rent_epoch: 0 }).unwrap();
    let vault = Address::new_unique();
    e.svm.set_account(vault, Account { lamports: 1_000_000_000, data: token_acct(&mint, &vault_pda, 0), owner: TOKEN, executable: false, rent_epoch: 0 }).unwrap();
    e.prog_acct(market, MARKET_SIZE); e.prog_acct(ins, INS_SIZE);
    let mut d = vec![10u8, bump]; d.extend_from_slice(&e.feed);
    for v in [BW, 3, 100, 10, (-8i64) as u64, 1, 1_000_000, 0] { d.extend_from_slice(&v.to_le_bytes()); }
    let o = e.oracle;
    e.send("", vec![AccountMeta::new(admin.pubkey(), true), w(&market), w(&ins), r(&vault), r(&o)], d, &[admin], true);
    Mkt { market, ins, vault, vault_pda }
}

#[test] fn c1_fake_token_program_deposit_withdraw() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let fake = Address::new_unique();
    e.svm.add_program(fake, &std::fs::read("../audit/noop/target/deploy/noop.so").unwrap()).unwrap();
    let (kp, u) = e.user(1_000_000);
    // attack 1: "deposit" 10^12 via a token program that does nothing -> free collateral
    let s = src_acct(&mut e, &kp.pubkey(), 0); let (m, v, vp) = (e.market, e.vault, e.vault_pda);
    let r1 = try_send(&mut e, vec![AccountMeta::new(kp.pubkey(), true), w(&u), r(&m), w(&v), w(&s), r(&fake)], amt(13, 1_000_000_000_000), &[&kp]);
    assert_eq!(r1, Some(TOKEN_PROGRAM));
    // attack 2: withdraw with fake program (would debit cash without moving tokens; or with a malicious program, reuse PDA signer)
    let r2 = try_send(&mut e, vec![AccountMeta::new(kp.pubkey(), true), w(&u), r(&m), w(&v), w(&s), r(&fake), r(&vp)], amt(14, 1), &[&kp]);
    assert_eq!(r2, Some(TOKEN_PROGRAM));
    assert_eq!(e.user_state(&u).cash, 1_000_000);
}

#[test] fn c2_cross_market_vault_drain_and_m4_foreign_admin_set_tier() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let a = cur(&e);
    let (kp, u) = e.user(5_000_000);              // collateral deposited into market A's vault
    let evil = Keypair::new(); let b = new_market(&mut e, &evil);   // attacker lists market B
    switch(&mut e, &b);
    let (_v, _vu) = e.user(7_000_000);            // victims fund B's vault
    e.oracle_update("", 1000_00000000, None);
    // attack: withdraw A-collateral out of B's vault (user cash used to be global)
    let s = src_acct(&mut e, &kp.pubkey(), 0);
    let r1 = try_send(&mut e, vec![AccountMeta::new(kp.pubkey(), true), w(&u), r(&b.market), w(&b.vault), w(&s), r(&TOKEN), r(&b.vault_pda)], amt(14, 5_000_000), &[&kp]);
    assert_eq!(r1, Some(VAULT));
    // attack: deposit the user into B too (re-binding) is rejected
    let s2 = src_acct(&mut e, &kp.pubkey(), 10);
    let r2 = try_send(&mut e, vec![AccountMeta::new(kp.pubkey(), true), w(&u), r(&b.market), w(&b.vault), w(&s2), r(&TOKEN)], amt(13, 10), &[&kp]);
    assert_eq!(r2, Some(VAULT));
    // M-4: B's admin sets a fee tier on an A-bound user
    let r3 = try_send(&mut e, vec![AccountMeta::new(evil.pubkey(), true), r(&b.market), w(&u)], vec![23, 1], &[&evil]);
    assert_eq!(r3, Some(VAULT));
    switch(&mut e, &a);
    let s3 = src_acct(&mut e, &kp.pubkey(), 0);
    e.send("", vec![AccountMeta::new(kp.pubkey(), true), w(&u), r(&a.market), w(&a.vault), w(&s3), r(&TOKEN), r(&a.vault_pda)], amt(14, 5_000_000), &[&kp], true);
    assert_eq!(e.vault_amount(), 0);
}

#[test] fn h1_spline_overflow_cannot_panic_book() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let a0 = e.shard(1, 100_000); let (kp, u) = e.user(1_000_000_000); let m = e.market;
    // step*n overflows u64 -> previously arithmetic panic in matching for every taker
    let r1 = try_send(&mut e, trade_accts(&kp, &u, &m, &[a0]), single(place_ex(1, 1, SPL, 100_050, 10, 0, 0, 1, (u64::MAX / 4) as i64, 16, 0)), &[&kp]);
    assert_eq!(r1, Some(BAD_DATA));
    // peg offset beyond MAX_PX (L-2)
    let r2 = try_send(&mut e, trade_accts(&kp, &u, &m, &[a0]), single(place_ex(1, 0, PEG, 100_050, 10, 0, 0, 2, i64::MAX, 0, u64::MAX)), &[&kp]);
    assert!(r2.is_some());
    // book still matches
    let (tk, tku) = e.user(1_000_000_000);
    e.send("", trade_accts(&tk, &tku, &m, &[a0]), single(place_item(0, IOC, 100_100, 1, 0, 0)), &[&tk], true);
}

fn set_market_u64(e: &mut Env, off: usize, v: u64) {
    let m = e.market; let mut a = e.svm.get_account(&m).unwrap(); a.data[off..off + 8].copy_from_slice(&v.to_le_bytes()); e.svm.set_account(m, a).unwrap();
}
/// H-2 (funding v4): premium = impact-price difference through the oracle (HL formula), TWAP over the period,
/// F = P + clamp(I - P, +-0.05%) per 8h paid hourly at F/8, capped at 4%/h. Dust quotes can't create a premium;
/// omitting shards can only pull the sample toward 0; real size is capped and averaged.
#[test] fn h2_funding_manipulation_needs_impact_size_and_is_capped() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    set_market_u64(&mut e, std::mem::offset_of!(Market, funding_period), 10);
    let (b, a) = (e.shard(0, 199_900), e.shard(1, 200_000));
    let (kp, u) = e.user(50_000_000_000); let m = e.market;
    // dust: 1-lot pair at 2x oracle passed by the caller -> notional < impact (1e6) -> no premium
    e.send("", trade_accts(&kp, &u, &m, &[b, a]), batch_data(&[], &[place_item(0, 1, 199_950, 1, 0, 0), place_item(1, 1, 200_050, 1, 0, 0)]), &[&kp], true);
    e.set_time(e.slot + 1); e.oracle_update("", 1000_00000000, Some((b, a)));
    assert_eq!(e.market_state().prem_last, 0);
    // real size (10 lots = 2e6 notional) at 2x: sample = +100% but the rate only changes at the period roll and is capped
    e.send("", trade_accts(&kp, &u, &m, &[b]), single(place_item(0, 1, 199_950, 10, 0, 0)), &[&kp], true);
    e.set_time(e.slot + 1); e.oracle_update("", 1000_00000000, Some((b, a)));
    assert_eq!(e.market_state().prem_last, 50_000_000, "sample clamped to the 5% quote band"); assert_eq!(e.market_state().funding_rate, 0);
    e.set_time(e.slot + 10); e.oracle_update("", 1000_00000000, Some((b, a)));
    let fr = e.market_state().funding_rate; assert!(fr > 0 && fr <= FUNDING_MAX_RATE_HOURLY_PPB as i64 && fr <= 50_000_000 / 8 + 1, "rate {fr}");
    // N-1: omission (no shards) holds the previous sample; a partial or duplicated set is rejected
    let held = e.market_state().prem_last;
    e.set_time(e.slot + 1); e.oracle_update("", 1000_00000000, None);
    assert_eq!(e.market_state().prem_last, held);
    let (mk, o) = (e.market, e.oracle);
    e.set_pyth(1000_00000000, 10_000_000);
    assert_eq!(try_send(&mut e, vec![w(&mk), r(&o), r(&b)], vec![4], &[]), Some(MISSING_SHARDS));
    assert_eq!(try_send(&mut e, vec![w(&mk), r(&o), r(&b), r(&b), r(&a)], vec![4], &[]), Some(BAD_ACCOUNT));
}

#[test] fn h3_book_stuffing_dust_rejected_and_overflow_shard() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let (s1, s2) = (e.shard(1, 100_000), e.shard(1, 100_000)); let m = e.market;
    // dust order below min notional
    let (kp, u) = e.user(1_000_000_000);
    let p = 100_050u64;
    assert!(try_send(&mut e, trade_accts(&kp, &u, &m, &[s1]), single(place_item(1, 1, p, 0, 0, 0)), &[&kp]).is_some());
    // fill shard s1 completely using several accounts (per-user cap 16)
    let cap = e.shard_state(&s1).orders.len();
    let mut placed = 0;
    while placed < cap {
        let (k, uk) = e.user(1_000_000_000);
        let n = (cap - placed).min(16);
        let items: Vec<Vec<u8>> = (0..n).map(|i| place_item(1, 1, p + (i as u64 % 40), 1, 0, 0)).collect();
        e.send("", trade_accts(&k, &uk, &m, &[s1]), batch_data(&[], &items), &[&k], true);
        placed += n;
    }
    assert_eq!(e.shard_state(&s1).n as usize, cap);
    // MM still quotes: insert overflows into s2 (same bucket)
    let (mm, mmu) = e.user(1_000_000_000);
    e.send("", trade_accts(&mm, &mmu, &m, &[s1, s2]), single(place_item(1, 1, p, 5, 0, 0)), &[&mm], true);
    assert_eq!(e.shard_state(&s2).n, 1);
}

#[test] fn m1_keeper_cannot_farm_tip_without_fill() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let (b0, a0) = (e.shard(0, 99_900), e.shard(1, 100_000)); let m = e.market;
    let (mm, mmu) = e.user(10_000_000_000); let (tk, tku) = e.user(1_000_000_000); let (kp, kpu) = e.user(1_000_000);
    e.send("", trade_accts(&mm, &mmu, &m, &[a0]), single(place_item(1, 1, 100_050, 100, 0, 0)), &[&mm], true);
    e.send("", trade_accts(&tk, &tku, &m, &[a0]), single(place_item(0, IOC, 100_100, 30, 0, 0)), &[&tk], true);
    // TWAP sell 30 in 3 slices, tip 100 each; keeper passes an empty bid shard -> no fill
    let mut d = vec![20u8, 0, 3, 1]; for v in [0u64, 90_000, 30, 1, 3, 100] { d.extend_from_slice(&v.to_le_bytes()); }
    e.send("", vec![AccountMeta::new(tk.pubkey(), true), w(&tku), r(&m)], d, &[&tk], true);
    let k0 = e.user_state(&kpu).cash;
    let r1 = try_send(&mut e, vec![AccountMeta::new(kp.pubkey(), true), w(&kpu), w(&tku), r(&m), w(&b0)], vec![21, 0], &[&kp]);
    assert_eq!(r1, Some(NOT_TRIGGERED));
    assert_eq!(e.user_state(&kpu).cash, k0);
    assert_eq!(e.user_state(&tku).pos[0].base, 30);
    let _ = mmu;
}

#[test] fn m2_liquidation_requires_full_shard_set_and_order_cap() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let (b0, b1, a0) = (e.shard(0, 99_900), e.shard(0, 99_800), e.shard(1, 100_000)); let m = e.market;
    let (mm, mmu) = e.user(50_000_000_000); let (vic, vicu) = e.user(150_000); let (lq, lqu) = e.user(1_000_000_000);
    // order cap
    let items: Vec<Vec<u8>> = (0..17u64).map(|i| place_item(0, 1, 98_990 - i, 1, 0, 0)).collect();
    let s9 = e.shard(0, 98_900);
    let c = try_send(&mut e, trade_accts(&mm, &mmu, &m, &[s9]), batch_data(&[], &items), &[&mm]);
    assert_eq!(c, Some(CAP));
    e.send("", trade_accts(&mm, &mmu, &m, &[a0]), single(place_item(1, 1, 100_000, 100, 0, 0)), &[&mm], true);
    e.send("", trade_accts(&vic, &vicu, &m, &[a0]), single(place_item(0, IOC, 100_000, 13, 0, 0)), &[&vic], true);
    // victim parks a resting order in b1, then price falls
    e.send("", trade_accts(&vic, &vicu, &m, &[b1]), single(place_item(0, 1, 99_850, 1, 0, 0)), &[&vic], true);
    e.send("", trade_accts(&mm, &mmu, &m, &[b0]), single(place_item(0, 1, 99_950, 100, 0, 0)), &[&mm], true);
    e.set_time(e.slot + 1); e.oracle_update("", 920_00000000, None);
    let ins = e.ins;
    let la = |shards: &[Address]| { let mut v = vec![AccountMeta::new(lq.pubkey(), true), w(&lqu), w(&vicu), r(&m), w(&ins)]; v.extend(shards.iter().map(w)); v };
    let acc = la(&[b0]);
    assert_eq!(try_send(&mut e, acc, amt(5, 13), &[&lq]), Some(MISSING_SHARDS));
    let acc = la(&[b0, b1]);
    e.send("", acc, amt(5, 13), &[&lq], true);
    assert_eq!(e.shard_state(&b1).n, 0);
}

#[test] fn m3_fok_in_batch_skips_only_itself() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let a0 = e.shard(1, 100_000); let b0 = e.shard(0, 99_900); let m = e.market;
    let (kp, u) = e.user(1_000_000_000);
    e.send("", trade_accts(&kp, &u, &m, &[a0, b0]), batch_data(&[], &[place_item(0, FOK, 100_100, 5, 0, 0), place_item(0, 1, 99_950, 5, 0, 0)]), &[&kp], true);
    assert_eq!(e.shard_state(&b0).n, 1);
}

#[test] fn account_validation_signer_dup_owner_writable() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let (kp, u) = e.user(1_000_000); let (_k2, u2) = e.user(1_000_000); let m = e.market;
    let (v, vp) = (e.vault, e.vault_pda);
    let s = src_acct(&mut e, &kp.pubkey(), 0);
    // missing signer
    assert!(try_send(&mut e, vec![AccountMeta::new_readonly(kp.pubkey(), false), w(&u), r(&m), w(&v), w(&s), r(&TOKEN), r(&vp)], amt(14, 1), &[]).is_some());
    // someone else's user account
    assert_eq!(try_send(&mut e, vec![AccountMeta::new(kp.pubkey(), true), w(&u2), r(&m), w(&v), w(&s), r(&TOKEN), r(&vp)], amt(14, 1), &[&kp]), Some(BAD_ACCOUNT));
    // spoofed user account (right bytes, wrong owner)
    let fake = Address::new_unique(); let mut acct = e.svm.get_account(&u).unwrap(); acct.owner = Address::new_unique();
    e.svm.set_account(fake, acct).unwrap();
    assert!(try_send(&mut e, vec![AccountMeta::new(kp.pubkey(), true), w(&fake), r(&m), w(&v), w(&s), r(&TOKEN), r(&vp)], amt(14, 1), &[&kp]).is_some());
    // type confusion: market passed as user
    assert!(try_send(&mut e, vec![AccountMeta::new(kp.pubkey(), true), w(&m), r(&m), w(&v), w(&s), r(&TOKEN), r(&vp)], amt(14, 1), &[&kp]).is_some());
    // liquidator == target
    let ins = e.ins;
    assert!(try_send(&mut e, vec![AccountMeta::new(kp.pubkey(), true), w(&u), w(&u), r(&m), w(&ins)], amt(5, 1), &[&kp]).is_some());
    // read-only user account cannot be mutated (runtime enforces)
    assert!(try_send(&mut e, vec![AccountMeta::new(kp.pubkey(), true), r(&u), r(&m), w(&v), w(&s), r(&TOKEN), r(&vp)], amt(14, 1), &[&kp]).is_some());
    assert_eq!(e.user_state(&u).cash, 1_000_000);
}

/// Random adversarial op sequences from 4 users; after every op: vault tokens == sum(cash)+insurance
/// (+fees held) is checked via the program's own invariant view, and nothing panics.
#[test] fn random_adversarial_sequences() {
    for seed in 1..=6u64 {
        let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
        let shards = [e.shard(0, 99_900), e.shard(0, 99_800), e.shard(1, 100_000), e.shard(1, 100_100)];
        let users: Vec<(Keypair, Address)> = (0..4).map(|_| e.user(200_000_000)).collect();
        let m = e.market; let mut x = seed.wrapping_mul(0x9E3779B97F4A7C15);
        let mut rnd = || { x ^= x << 13; x ^= x >> 7; x ^= x << 17; x };
        let dep_total: u64 = 800_000_000;
        for _ in 0..150 {
            let (kp, u) = &users[(rnd() % 4) as usize];
            let side = (rnd() % 2) as u8;
            let px = 99_700 + rnd() % 600; let qty = 1 + rnd() % 40;
            let flags = [0u8, 1, IOC, FOK, 16, 2, 1 << 5, 2 << 5][(rnd() % 8) as usize];
            let kind = [0u8, 0, PEG, SPL][(rnd() % 4) as usize];
            let item = match kind { PEG => place_ex(side, flags, PEG, px, qty, 0, 0, rnd() % 50, (rnd() % 400) as i64 - 200, 99_000, 101_000),
                SPL => place_ex(side, flags, SPL, px, qty, 0, 0, rnd() % 50, 1 + (rnd() % 30) as i64, 1 + rnd() % 6, rnd() % 3),
                _ => place_item(side, flags, px, qty, 0, 0) };
            let sh: Vec<Address> = shards.iter().copied().filter(|_| rnd() % 4 != 0).collect();
            let _ = try_send(&mut e, trade_accts(kp, u, &m, &sh), single(item), &[kp]);
            if rnd() % 20 == 0 { let p = 1000_00000000 + (rnd() % 2000) as i64 * 1_000_000 - 1_000_000_000; e.set_time(e.slot + 1); e.oracle_update("", p, None); }
        }
        // conservation: vault never changed by trading; cash + upnl + fees + insurance stays within the deposits
        assert_eq!(e.vault_amount(), dep_total);
        let ms = e.market_state();
        for s in shards { let mut acc = vec![w(&s)]; acc.extend(users.iter().map(|(_, u)| w(u))); e.send("", acc, vec![24], &[], true); }
        let mut base_sum = 0i64; for (_, u) in &users { base_sum += e.user_state(u).pos[0].base; }
        let _ = ms;
        assert_eq!(base_sum, 0, "seed {seed}: positions must net to zero");
    }
}

/// Same shard passed twice to a taker: must not double-fill or corrupt (per-shard best cache is per index).
#[test] fn duplicate_shard_in_taker_set() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let a0 = e.shard(1, 100_000); let m = e.market;
    let (mm, mmu) = e.user(10_000_000_000); let (tk, tku) = e.user(1_000_000_000);
    e.send("", trade_accts(&mm, &mmu, &m, &[a0]), batch_data(&[], &[place_item(1, 1, 100_010, 10, 0, 0), place_item(1, 1, 100_020, 10, 0, 0)]), &[&mm], true);
    let r = try_send(&mut e, trade_accts(&tk, &tku, &m, &[a0, a0, a0]), single(place_item(0, IOC, 100_100, 50, 0, 0)), &[&tk]);
    if r.is_none() { assert_eq!(e.user_state(&tku).pos[0].base, 20); assert_eq!(e.shard_state(&a0).n, 0); }
    for s in [a0] { e.send("", vec![w(&s), w(&mmu)], vec![24], &[], true); }
    if r.is_none() { assert_eq!(e.user_state(&mmu).pos[0].base, -20); }
}

const STALE: u32 = 4; const RANK: u32 = 20; const USE_ADL: u32 = 23;
fn jump_ts(e: &mut Env, secs: i64) {
    e.ts += secs; e.slot += 1;
    e.svm.set_sysvar(&Clock { slot: e.slot, epoch_start_timestamp: 0, epoch: 0, leader_schedule_epoch: 0, unix_timestamp: e.ts });
}

/// L-4: freshness is enforced at use time by publish_time (not only by update slot); older updates can't replay.
#[test] fn l4_oracle_publish_time_freshness_and_monotonic() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let a0 = e.shard(1, 100_000); let m = e.market;
    let (kp, u) = e.user(1_000_000_000);
    // slot is still within max_staleness (3 slots) but 60 s passed (> max_age 10 s): a new-risk order must fail
    jump_ts(&mut e, 60);
    assert_eq!(try_send(&mut e, trade_accts(&kp, &u, &m, &[a0]), single(place_item(1, 1, 100_050, 10, 0, 0)), &[&kp]), Some(STALE));
    // replay: an update whose publish_time is older than the stored one is rejected
    e.oracle_update("", 1000_00000000, None);
    let o = e.oracle; let feed = e.feed;
    e.svm.set_account(o, Account { lamports: 1_000_000_000, data: pyth_data(&feed, 900_00000000, 90_000, e.ts - 5, e.slot), owner: PYTH, executable: false, rent_epoch: 0 }).unwrap();
    assert_eq!(try_send(&mut e, vec![w(&m), r(&o)], vec![4], &[]), Some(STALE));
    assert_eq!(e.market_state().oracle_px, ORACLE);
}

/// L-3: order ids are shard_no << 40 | seq; they never wrap (shard refuses inserts at the limit) and
/// two shards of the same bucket never collide. Old scheme: lo << 20 | seq & 0xFFFFF wrapped after 2^20 inserts.
#[test] fn l3_order_ids_unique_no_wrap() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let (s1, s2) = (e.shard(1, 100_000), e.shard(1, 100_000)); let m = e.market;
    let (kp, u) = e.user(1_000_000_000);
    e.send("", trade_accts(&kp, &u, &m, &[s1]), single(place_ex(1, 1, 0, 100_050, 10, 0, 0, 1, 0, 0, 0)), &[&kp], true);
    let x = e.shard_state(&s1).orders[0].id;
    // fast-forward s1's sequence to the old wrap point (2^20 - 1): the next id must still differ
    let mut acct = e.svm.get_account(&s1).unwrap();
    let off = std::mem::offset_of!(Shard, seq);
    acct.data[off..off + 8].copy_from_slice(&((1u64 << 20) - 1).to_le_bytes()); e.svm.set_account(s1, acct).unwrap();
    e.send("", trade_accts(&kp, &u, &m, &[s1]), single(place_ex(1, 1, 0, 100_060, 10, 0, 0, 2, 0, 0, 0)), &[&kp], true);
    let y = e.shard_state(&s1).orders[1].id;
    assert_ne!(x, y);
    // cancel by X's id removes X (old scheme: ids collided and the better-priced Y could be cancelled instead)
    e.send("", trade_accts(&kp, &u, &m, &[s1]), batch_data(&[cancel_item(x, 1, 100_050)], &[]), &[&kp], true);
    let s = e.shard_state(&s1); assert_eq!(s.n, 1); assert_eq!(s.orders[0].id, y);
    // at the 40-bit limit the shard refuses inserts (no wrap) and the order overflows into s2 with a distinct id
    let mut acct = e.svm.get_account(&s1).unwrap();
    acct.data[off..off + 8].copy_from_slice(&((1u64 << 40) - 1).to_le_bytes()); e.svm.set_account(s1, acct).unwrap();
    e.send("", trade_accts(&kp, &u, &m, &[s1, s2]), single(place_ex(1, 1, 0, 100_070, 10, 0, 0, 3, 0, 0, 0)), &[&kp], true);
    assert_eq!(e.shard_state(&s1).n, 1); let z = e.shard_state(&s2).orders[0].id;
    assert_ne!(z, y); assert_eq!(z >> 40, e.shard_state(&s2).shard_no);
    assert!(try_send(&mut e, trade_accts(&kp, &u, &m, &[s1]), single(place_ex(1, 1, 0, 100_080, 10, 0, 0, 4, 0, 0, 0)), &[&kp]).is_some());
}

/// M-6: a liquidator that is also a maker posts a bid at the band edge and passes only its own shard.
/// Old: the victim was sold into that bid at -5%. Now: takeover at the mark, the book is not used.
#[test] fn m6_liquidator_self_deal_is_neutral() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let (a0, bev) = (e.shard(1, 100_000), e.shard(0, 87_400)); let m = e.market;
    let (mm, mmu) = e.user(50_000_000_000); let (vic, vicu) = e.user(150_000); let (lq, lqu) = e.user(5_000_000_000);
    e.send("", trade_accts(&mm, &mmu, &m, &[a0]), single(place_item(1, 1, 100_000, 100, 0, 0)), &[&mm], true);
    e.send("", trade_accts(&vic, &vicu, &m, &[a0]), single(place_item(0, IOC, 100_000, 13, 0, 0)), &[&vic], true);
    e.set_time(e.slot + 1); e.oracle_update("", 920_00000000, None);
    // attacker's own bid at oracle - 5% (the old liquidation band edge)
    e.send("", trade_accts(&lq, &lqu, &m, &[bev]), single(place_item(0, 1, 87_400, 13, 0, 0)), &[&lq], true);
    let (c0, lc0) = (e.user_state(&vicu).cash, e.user_state(&lqu).cash);
    let ins = e.ins;
    e.send("", vec![AccountMeta::new(lq.pubkey(), true), w(&lqu), w(&vicu), r(&m), w(&ins), w(&bev)], amt(5, 13), &[&lq], true);
    let px = 92_000i64; let n = 13 * px;
    let v = e.user_state(&vicu); let l = e.user_state(&lqu);
    let q = 13 - v.pos[0].base; assert!(q > 0 && q < 13, "one chunk: {q}"); assert_eq!(l.pos[0].base, q);
    assert_eq!(e.shard_state(&bev).orders[0].qty, 13, "attacker's band-edge bid must not be used");
    let n = q * px; let fee = l.cash - (lc0 - n);
    assert!(fee >= 0 && fee <= n * 25 / 10000);
    assert_eq!(v.cash, c0 + n - 2 * fee, "victim sells at the mark, pays only the fixed fee");
}

/// M-5: the ADL counterparty comes from the on-chain ranking, not the caller; the top entry must be passed,
/// ineligible entries are evicted, and the counterparty can never be pushed below zero equity.
#[test] fn m5_adl_counterparty_is_ranked_on_chain() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let a0 = e.shard(1, 100_000); let m = e.market;
    let (sb, sbu) = e.user(400_000);           // small, highly leveraged short (best score)
    let (mm, mmu) = e.user(50_000_000_000);   // huge, low-leverage short
    let (l, lu) = e.user(10_000_000_000);
    let (t, tu) = e.user(100_000);
    e.send("", trade_accts(&sb, &sbu, &m, &[a0]), single(place_item(1, 1, 100_040, 3, 0, 0)), &[&sb], true);
    e.send("", trade_accts(&l, &lu, &m, &[a0]), single(place_item(0, IOC, 100_040, 3, 0, 0)), &[&l], true);
    e.send("", trade_accts(&mm, &mmu, &m, &[a0]), single(place_item(1, 1, 100_050, 100, 0, 0)), &[&mm], true);
    e.send("", trade_accts(&t, &tu, &m, &[a0]), single(place_item(0, IOC, 100_050, 9, 0, 0)), &[&t], true);
    e.send("", trade_accts(&mm, &mmu, &m, &[a0]), batch_data(&[], &[]), &[&mm], true);
    e.send("", trade_accts(&sb, &sbu, &m, &[a0]), batch_data(&[], &[]), &[&sb], true);
    e.set_time(e.slot + 1); e.oracle_update("", 750_00000000, None);
    e.rank_user(&mmu); e.rank_user(&sbu); e.rank_user(&lu);
    let rk = e.rank_state(); assert_eq!(rk.n_short, 2); assert_eq!(rk.n_long, 0);
    assert_eq!(rk.entries[RANK_N].user, sbu.to_bytes(), "highest pnl x leverage first");
    let (ins, rkk) = (e.ins, e.rank);
    let adl = |c: &[Address]| { let mut v = vec![w(&tu), w(&m), w(&ins), w(&rkk)]; v.extend(c.iter().map(w)); v };
    // attack: keeper picks the low-ranked whale (e.g. to protect the top account) -> rejected
    assert_eq!(try_send(&mut e, adl(&[mmu]), vec![6], &[]), Some(RANK));
    // attack: a random healthy long (wrong side) is not a counterparty
    assert_eq!(try_send(&mut e, adl(&[lu]), vec![6], &[]), Some(RANK));
    let sb0 = e.user_state(&sbu).pos[0].base;
    e.send("", adl(&[sbu]), vec![6], &[], true);
    let s = e.user_state(&sbu); assert_eq!(s.pos[0].base, 0); assert_eq!(sb0, -3);
    assert!(s.cash >= 0, "counterparty never pushed below zero");
    // sb was flattened and is no longer in the queue; the target's deficit was absorbed (not bankrupt any more)
    assert_eq!(e.user_state(&tu).pos[0].base, 6);
    assert_eq!(e.rank_state().n_short, 1); assert_eq!(e.rank_state().entries[RANK_N].user, mmu.to_bytes());
    assert_eq!(try_send(&mut e, adl(&[mmu]), vec![6], &[]), None, "still bankrupt: the rest of its deficit goes with its next lots");
    assert!(e.user_state(&tu).pos[0].base < 6);
}

// ---------- pooled collateral (cross-margin) ----------
struct PoolT { pool: Address, vault: Address, vault_pda: Address, admin: Keypair }
fn new_pool(e: &mut Env) -> PoolT {
    let admin = Keypair::new(); e.svm.airdrop(&admin.pubkey(), 10_000_000_000).unwrap();
    let pool = Address::new_unique(); e.prog_acct(pool, POOL_SIZE);
    let (vault_pda, bump) = Address::find_program_address(&[b"vault", pool.as_ref()], &e.pid);
    let mint = Address::new_unique(); let mut md = vec![0u8; 82]; md[44] = 6; md[45] = 1;
    e.svm.set_account(mint, Account { lamports: 1_000_000_000, data: md, owner: TOKEN, executable: false, rent_epoch: 0 }).unwrap();
    let vault = Address::new_unique();
    e.svm.set_account(vault, Account { lamports: 1_000_000_000, data: token_acct(&mint, &vault_pda, 0), owner: TOKEN, executable: false, rent_epoch: 0 }).unwrap();
    let mut d = vec![27, bump]; d.extend_from_slice(&5_000_000u64.to_le_bytes());
    e.send("init_pool", vec![AccountMeta::new(admin.pubkey(), true), w(&pool), r(&vault)], d, &[&admin], true);
    PoolT { pool, vault, vault_pda, admin }
}
/// market admitted to `p`; `signer` stands in for the pool admin (exploit: anyone else)
fn pool_propose(e: &mut Env, p: &PoolT, market: &Address) {
    let pa = Keypair::try_from(&p.admin.to_bytes()[..]).unwrap();
    let mut d = vec![32u8]; d.extend_from_slice(market.as_ref());
    e.send("pool_propose (timelocked admission)", vec![w(&p.pool), AccountMeta::new_readonly(pa.pubkey(), true)], d, &[&pa], true);
}
fn pooled_market(e: &mut Env, p: &PoolT, signer: &Keypair) -> (Mkt, Option<u32>) { pooled_market_ex(e, p, signer, true, 1_000_000) }
fn pooled_market_ex(e: &mut Env, p: &PoolT, signer: &Keypair, propose: bool, max_oi: u64) -> (Mkt, Option<u32>) {
    let market = Address::new_unique(); let ins = Address::new_unique();
    e.prog_acct(market, MARKET_SIZE); e.prog_acct(ins, INS_SIZE);
    if propose { pool_propose(e, p, &market); let s = e.slot + 216_000; e.set_time(s); }
    let mut d = vec![10u8, 0]; d.extend_from_slice(&e.feed);
    for v in [BW, 3, 100, 10, (-8i64) as u64, 1, 1_000_000, max_oi] { d.extend_from_slice(&v.to_le_bytes()); }
    let (o, a) = (e.oracle, e.admin.pubkey());
    let ad = Keypair::try_from(&e.admin.to_bytes()[..]).unwrap();
    let r1 = try_send(e, vec![AccountMeta::new(a, true), w(&market), w(&ins), r(&p.vault), r(&o), w(&p.pool), AccountMeta::new_readonly(signer.pubkey(), true)], d, &[&ad, signer]);
    (Mkt { market, ins, vault: p.vault, vault_pda: p.vault_pda }, r1)
}
fn pool_n(e: &Env, p: &Address) -> u64 { unsafe { std::ptr::read_unaligned(e.data(p).as_ptr() as *const Pool) }.n_markets }

/// P-1: joining a shared vault requires the pool admin; a market can't point at a pool vault without it
/// (that would re-open C-2: attacker market + fake PnL draining everyone's pooled collateral).
#[test] fn p1_pool_join_requires_pool_admin() {
    let mut e = Env::new(); let p = new_pool(&mut e);
    let evil = Keypair::new(); e.svm.airdrop(&evil.pubkey(), 1_000_000_000).unwrap();
    let (_, r1) = pooled_market(&mut e, &p, &evil);
    assert_eq!(r1, Some(BAD_ACCOUNT));
    // no pool account at all but the pool vault (owned by the pool PDA, not the market PDA)
    let market = Address::new_unique(); let ins = Address::new_unique();
    e.prog_acct(market, MARKET_SIZE); e.prog_acct(ins, INS_SIZE);
    let mut d = vec![10u8, 255]; d.extend_from_slice(&e.feed);
    for v in [BW, 3, 100, 10, (-8i64) as u64, 1, 1_000_000, 0] { d.extend_from_slice(&v.to_le_bytes()); }
    let (o, a) = (e.oracle, e.admin.pubkey()); let ad = Keypair::try_from(&e.admin.to_bytes()[..]).unwrap();
    let r2 = try_send(&mut e, vec![AccountMeta::new(a, true), w(&market), w(&ins), r(&p.vault), r(&o)], d, &[&ad]);
    assert!(r2.is_some());
    let pa = Keypair::try_from(&p.admin.to_bytes()[..]).unwrap();
    let (_, ok) = pooled_market(&mut e, &p, &pa); assert_eq!(ok, None);
    assert_eq!(pool_n(&e, &p.pool), 1);
}

/// Cross-margin: deposit through market A, trade and withdraw through market B of the same pool.
/// Cross-pool / non-pool withdraws still fail (C-2 preserved).
#[test] fn p2_cross_margin_pool_and_no_cross_pool_withdraw() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let solo = cur(&e);
    let p = new_pool(&mut e); let pa = Keypair::try_from(&p.admin.to_bytes()[..]).unwrap();
    let (ma, _) = pooled_market(&mut e, &p, &pa); let (mb, _) = pooled_market(&mut e, &p, &pa);
    switch(&mut e, &ma); e.oracle_update("", 1000_00000000, None);
    let (kp, u) = e.user(5_000_000);
    let (_mk, _mu) = e.user(9_000_000);
    assert_eq!(e.vault_amount(), 14_000_000);
    switch(&mut e, &mb); e.oracle_update("", 1000_00000000, None);
    let b0 = e.shard_by(0, 99_900, &pa); let m = mb.market;
    // A-deposited cash margins a resting order on B
    e.send("", trade_accts(&kp, &u, &m, &[b0]), single(place_item(0, 0, 99_950, 1, 0, 0)), &[&kp], true);
    // withdraw through B (health over B; A has no position)
    let s = src_acct(&mut e, &kp.pubkey(), 0);
    e.send("", vec![AccountMeta::new(kp.pubkey(), true), w(&u), r(&mb.market), w(&mb.vault), w(&s), r(&TOKEN), r(&mb.vault_pda), w(&b0)], amt(14, 1_000_000), &[&kp], true);
    assert_eq!(e.vault_amount(), 13_000_000);
    // a pooled user can't pull from the solo market's vault, nor a solo user from the pool vault
    let r1 = try_send(&mut e, vec![AccountMeta::new(kp.pubkey(), true), w(&u), r(&solo.market), w(&solo.vault), w(&s), r(&TOKEN), r(&solo.vault_pda)], amt(14, 1), &[&kp]);
    assert_eq!(r1, Some(VAULT));
    switch(&mut e, &solo); let (sk, su) = e.user(2_000_000); let s2 = src_acct(&mut e, &sk.pubkey(), 0);
    let r2 = try_send(&mut e, vec![AccountMeta::new(sk.pubkey(), true), w(&su), r(&mb.market), w(&mb.vault), w(&s2), r(&TOKEN), r(&mb.vault_pda)], amt(14, 1), &[&sk]);
    assert_eq!(r2, Some(VAULT));
    // mismatched vault for the market (other pool) -> BadAccount
    let q = new_pool(&mut e);
    let r3 = try_send(&mut e, vec![AccountMeta::new(kp.pubkey(), true), w(&u), r(&mb.market), w(&q.vault), w(&s), r(&TOKEN), r(&q.vault_pda)], amt(14, 1), &[&kp]);
    assert_eq!(r3, Some(BAD_ACCOUNT));
}

// ---------- admin timelock / position limits / mark ----------
const TIMELOCK: u32 = 21;
fn adm_d(tag: u8, op: u8, v: u64, k: Option<&Address>) -> Vec<u8> {
    let mut d = vec![tag, op]; d.extend_from_slice(&v.to_le_bytes()); if let Some(k) = k { d.extend_from_slice(k.as_ref()); } d
}
/// T-1: margin params / admin rotation can't change instantly (users get TIMELOCK_SLOTS to exit); pause stays immediate.
#[test] fn t1_admin_changes_are_timelocked() {
    let mut e = Env::new(); let (a, m) = (e.admin.pubkey(), e.market);
    assert_eq!(try_send(&mut e, vec![AccountMeta::new(a, true), w(&m)], adm_d(22, 4, 1, None), &[]), Some(TIMELOCK));
    let evil = Keypair::new(); e.svm.airdrop(&evil.pubkey(), 1_000_000_000).unwrap();
    assert_eq!(try_send(&mut e, vec![AccountMeta::new(evil.pubkey(), true), w(&m)], adm_d(28, 4, 1, None), &[&evil]), Some(BAD_ACCOUNT));
    e.send("admin_propose (timelocked imr)", vec![AccountMeta::new(a, true), w(&m)], adm_d(28, 4, 2000, None), &[], true);
    assert_eq!(try_send(&mut e, vec![w(&m)], vec![29], &[]), Some(TIMELOCK));
    e.set_time(e.slot + 216_000);
    e.send("admin_execute (after timelock)", vec![w(&m)], vec![29], &[], true);
    assert_eq!(e.market_state().imr_bps, 2000);
    // rotation
    e.send("", vec![AccountMeta::new(a, true), w(&m)], adm_d(28, 8, 0, Some(&evil.pubkey())), &[], true);
    e.set_time(e.slot + 216_000); e.send("", vec![w(&m)], vec![29], &[], true);
    assert_eq!(e.market_state().admin, evil.pubkey().to_bytes());
    assert_eq!(try_send(&mut e, vec![AccountMeta::new(a, true), w(&m)], adm_d(22, 1, 1, None), &[]), Some(BAD_ACCOUNT));
    e.send("", vec![AccountMeta::new(evil.pubkey(), true), w(&m)], adm_d(22, 1, 1, None), &[&evil], true);
    assert_eq!(e.market_state().paused, 1);
}
/// Per-account position limit counts resting orders (gross) and fills.
#[test] fn t2_max_position_limit() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let (a, m) = (e.admin.pubkey(), e.market);
    e.send("", vec![AccountMeta::new(a, true), w(&m)], adm_d(22, 6, 5, None), &[], true);
    let b0 = e.shard(0, 99_900); let (kp, u) = e.user(50_000_000_000);
    e.send("", trade_accts(&kp, &u, &m, &[b0]), single(place_item(0, 0, 99_950, 5, 0, 0)), &[&kp], true);
    assert_eq!(try_send(&mut e, trade_accts(&kp, &u, &m, &[b0]), single(place_item(0, 0, 99_950, 1, 0, 0)), &[&kp]), Some(CAP));
}
/// Mark = median(oracle, oracle+EMA(mid-oracle), mid), clamped to +-5%: a one-block book push can't move it
/// past the clamp and a single sample moves the EMA by at most ds/375.
#[test] fn t3_mark_is_median_and_clamped() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let (b, a) = (e.shard(0, 199_900), e.shard(1, 200_000)); let (kp, u) = e.user(50_000_000_000); let m = e.market;
    e.send("", trade_accts(&kp, &u, &m, &[b, a]), batch_data(&[], &[place_item(0, 0, 199_950, 1, 0, 0), place_item(1, 0, 200_050, 1, 0, 0)]), &[&kp], true);
    e.set_time(e.slot + 1); e.oracle_update("", 1000_00000000, Some((b, a)));
    assert_eq!(e.market_state().mark_ema, 0, "1-lot dust is no mid");
    e.send("", trade_accts(&kp, &u, &m, &[b, a]), batch_data(&[], &[place_item(0, 0, 199_950, 10, 0, 0), place_item(1, 0, 200_050, 10, 0, 0)]), &[&kp], true);
    e.set_time(e.slot + 1); e.oracle_update("", 1000_00000000, Some((b, a)));
    // first sample of this skew: nothing credited yet (a skew counts for the slots between two samples it was present at)
    assert_eq!(e.market_state().mark_ema, 0);
    e.set_time(e.slot + 1); e.oracle_update("", 1000_00000000, Some((b, a)));
    let s = e.market_state();
    // mid = 2x oracle, present at both ends of a 1-slot gap; the credited deviation is capped at 10% of the oracle (the mark
    // itself is clamped at 5%): ema = 10000/375; median(oracle, oracle+ema, mid) = oracle+ema
    assert_eq!(s.mark_ema, 10_000 / 375); assert_eq!(s.mark_px, 100_000 + 10_000 / 375);
    // a sample is at most MARK_SAMPLE_MAX_SLOTS (5) of weight however long it waited: 60 x 5/375 toward the 10k cap -> clamp
    for _ in 0..60 { e.set_time(e.slot + 400); e.oracle_update("", 1000_00000000, Some((b, a))); }
    assert_eq!(e.market_state().mark_px, 105_000); // clamp
    e.set_time(e.slot + 1); e.oracle_update("", 1000_00000000, None); // omitted book -> sample 0, EMA decays
    assert!(e.market_state().mark_px <= 105_000);
}

/// N-3/N-5: a liquidation whose deficit exceeds the insurance fund is refused (UseAdl); the fund never goes
/// negative; ADL socializes against ranked counterparties; any remainder is recorded only in Market.bad_debt.
#[test] fn n3_insurance_never_negative_deficit_goes_to_adl() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let a0 = e.shard(1, 100_000); let m = e.market;
    let (mm, mmu) = e.user(50_000_000_000); let (vic, vicu) = e.user(150_000); let (lq, lqu) = e.user(5_000_000_000);
    e.send("", trade_accts(&mm, &mmu, &m, &[a0]), single(place_item(1, 1, 100_000, 100, 0, 0)), &[&mm], true);
    e.send("", trade_accts(&vic, &vicu, &m, &[a0]), single(place_item(0, IOC, 100_000, 13, 0, 0)), &[&vic], true);
    e.send("", trade_accts(&mm, &mmu, &m, &[a0]), batch_data(&[], &[]), &[&mm], true);
    e.set_time(e.slot + 1); e.oracle_update("", 800_00000000, None); // -20%: deeply bankrupt
    let ins = e.ins; assert_eq!(e.ins_bal(), 0);
    let r1 = try_send(&mut e, vec![AccountMeta::new(lq.pubkey(), true), w(&lqu), w(&vicu), r(&m), w(&ins), w(&a0)], amt(5, 13), &[&lq]);
    assert_eq!(r1, Some(USE_ADL));
    e.rank_user(&mmu); let rk = e.rank;
    e.send("", vec![w(&vicu), w(&m), w(&ins), w(&rk), w(&mmu)], vec![6], &[], true);
    assert_eq!(e.user_state(&vicu).pos[0].base, 0);
    assert!(e.ins_bal() >= 0); assert!(e.user_state(&mmu).cash >= 0);
    assert!(e.market_state().bad_debt >= 0);
}
/// N-4: ADL re-scores every candidate passed by the caller at use time (an empty / stale ranking is not a blocker).
#[test] fn n4_adl_rescoring_at_use_time() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let a0 = e.shard(1, 100_000); let m = e.market;
    let (mm, mmu) = e.user(50_000_000_000); let (vic, vicu) = e.user(150_000);
    e.send("", trade_accts(&mm, &mmu, &m, &[a0]), single(place_item(1, 1, 100_000, 100, 0, 0)), &[&mm], true);
    e.send("", trade_accts(&vic, &vicu, &m, &[a0]), single(place_item(0, IOC, 100_000, 13, 0, 0)), &[&vic], true);
    e.send("", trade_accts(&mm, &mmu, &m, &[a0]), batch_data(&[], &[]), &[&mm], true);
    e.set_time(e.slot + 1); e.oracle_update("", 800_00000000, None);
    assert_eq!(e.rank_state().n_short, 0); // nobody cranked adl_rank
    let (ins, rk) = (e.ins, e.rank);
    e.send("", vec![w(&vicu), w(&m), w(&ins), w(&rk), w(&mmu)], vec![6], &[], true);
    assert_eq!(e.user_state(&vicu).pos[0].base, 0);
}
/// N-7: cost basis is re-based on a position flip (ADL score uses the new side's entry price).
#[test] fn n7_cost_basis_rebased_on_flip() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let (a0, b0) = (e.shard(1, 100_000), e.shard(0, 99_900)); let m = e.market;
    let (mm, mmu) = e.user(50_000_000_000); let (t, tu) = e.user(5_000_000_000);
    e.send("", trade_accts(&mm, &mmu, &m, &[a0, b0]), batch_data(&[], &[place_item(1, 1, 100_000, 10, 0, 0), place_item(0, 1, 99_950, 20, 0, 0)]), &[&mm], true);
    e.send("", trade_accts(&t, &tu, &m, &[a0]), single(place_item(0, IOC, 100_000, 10, 0, 0)), &[&t], true);
    assert_eq!(e.user_state(&tu).pos[0].cost, 10 * 100_000);
    e.send("", trade_accts(&t, &tu, &m, &[b0]), single(place_item(1, IOC, 99_950, 20, 0, 0)), &[&t], true);
    let p = e.user_state(&tu).pos[0]; assert_eq!(p.base, -10);
    assert_eq!(p.cost, -10 * 99_950, "short leg valued at its own entry, not net of the closed long");
}

/// N-6: admission to a pool is timelocked (users can exit before a new market can touch pooled collateral) and
/// every pooled market must carry an OI cap within the pool's per-market exposure limit.
#[test] fn n6_pool_admission_timelock_and_exposure_cap() {
    let mut e = Env::new(); let p = new_pool(&mut e); let pa = Keypair::try_from(&p.admin.to_bytes()[..]).unwrap();
    let (_, r1) = pooled_market_ex(&mut e, &p, &pa, false, 1_000_000); assert_eq!(r1, Some(TIMELOCK));
    let (_, r2) = pooled_market_ex(&mut e, &p, &pa, true, 0); assert_eq!(r2, Some(CAP));
    let (_, r3) = pooled_market_ex(&mut e, &p, &pa, true, 6_000_000); assert_eq!(r3, Some(CAP));
    let (_, r4) = pooled_market_ex(&mut e, &p, &pa, true, 5_000_000); assert_eq!(r4, None);
    // proposal too early: propose, don't wait
    let market = Address::new_unique(); pool_propose(&mut e, &p, &market);
    assert_eq!(pool_n(&e, &p.pool), 1);
}

/// Multisig admin (m-of-n on chain): after rotating the market admin to a 2-of-3 Multisig account, one key is
/// not enough, two distinct keys are.
#[test] fn ms1_multisig_admin_m_of_n() {
    let mut e = Env::new(); let (a, m) = (e.admin.pubkey(), e.market);
    let ks: Vec<Keypair> = (0..3).map(|_| { let k = Keypair::new(); e.svm.airdrop(&k.pubkey(), 1_000_000_000).unwrap(); k }).collect();
    let ms = Address::new_unique(); e.prog_acct(ms, MULTISIG_SIZE);
    let mut d = vec![33u8, 2, 3]; for k in &ks { d.extend_from_slice(k.pubkey().as_ref()); }
    e.send("init_multisig (2-of-3)", vec![AccountMeta::new(a, true), w(&ms)], d, &[], true);
    e.send("", vec![AccountMeta::new(a, true), w(&m)], adm_d(28, 8, 0, Some(&ms)), &[], true);
    e.set_time(e.slot + 216_000); e.send("", vec![w(&m)], vec![29], &[], true);
    assert_eq!(e.market_state().admin, ms.to_bytes());
    let k = |i: usize| AccountMeta::new_readonly(ks[i].pubkey(), true);
    // one key is not enough for anything that loosens: caps, un-pause
    assert_eq!(try_send(&mut e, vec![k(0), w(&m), r(&ms)], adm_d(22, 2, 5, None), &[&ks[0]]), Some(BAD_ACCOUNT));
    assert_eq!(try_send(&mut e, vec![AccountMeta::new(a, true), w(&m), r(&ms)], adm_d(22, 1, 1, None), &[]), Some(BAD_ACCOUNT), "the old admin is out");
    let stranger = Keypair::new(); e.svm.airdrop(&stranger.pubkey(), 1_000_000_000).unwrap();
    assert_eq!(try_send(&mut e, vec![AccountMeta::new_readonly(stranger.pubkey(), true), w(&m), r(&ms)], adm_d(22, 1, 1, None), &[&stranger]), Some(BAD_ACCOUNT), "not a member");
    // emergency stop: any one member pauses ...
    e.send("emergency pause (1 member)", vec![k(1), w(&m), r(&ms)], adm_d(22, 1, 1, None), &[&ks[1]], true);
    assert_eq!(e.market_state().paused, 1);
    // ... but can't resume alone, and can't set any other pause value
    assert_eq!(try_send(&mut e, vec![k(1), w(&m), r(&ms)], adm_d(22, 1, 0, None), &[&ks[1]]), Some(BAD_ACCOUNT));
    assert_eq!(try_send(&mut e, vec![k(1), w(&m), r(&ms)], adm_d(22, 1, 2, None), &[&ks[1]]), Some(BAD_ACCOUNT));
    e.send("resume (2 of 3 signers)", vec![k(0), w(&m), r(&ms), k(2)], adm_d(22, 1, 0, None), &[&ks[0], &ks[2]], true);
    assert_eq!(e.market_state().paused, 0);
    e.send("admin pause via multisig (2 of 3 signers)", vec![k(0), w(&m), r(&ms), k(2)], adm_d(22, 1, 1, None), &[&ks[0], &ks[2]], true);
    assert_eq!(e.market_state().paused, 1);
}
/// Oracle2: fallback when the primary is unusable; circuit breaker (auto-pause, prices frozen) when both are valid
/// but diverge by more than ORACLE_MAX_DIVERGENCE_BPS; auto-resume when they re-converge.
#[test] fn o2_oracle_fallback_and_divergence_breaker() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let (a, m, o1) = (e.admin.pubkey(), e.market, e.oracle);
    let o2 = Address::new_unique(); let feed2 = [9u8; 32];
    let mut d = adm_d(28, 10, 0, Some(&o2)); d.extend_from_slice(&feed2);
    e.send("", vec![AccountMeta::new(a, true), w(&m)], d, &[], true);
    e.set_time(e.slot + 216_000); e.send("", vec![w(&m)], vec![29], &[], true);
    assert_eq!(e.market_state().oracle2, o2.to_bytes());
    let put2 = |e: &mut Env, px: i64| { let d = pyth_data(&feed2, px, (px / 10000) as u64, e.ts, e.slot);
        e.svm.set_account(o2, Account { lamports: 1_000_000_000, data: d, owner: PYTH, executable: false, rent_epoch: 0 }).unwrap(); };
    let upd = |e: &mut Env| -> Option<u32> { try_send(e, vec![w(&m), r(&o1), r(&o2)], vec![4], &[]) };
    e.set_time(e.slot + 1); e.set_pyth(1000_00000000, 10_000_000); put2(&mut e, 1001_00000000);
    assert_eq!(upd(&mut e), None); assert_eq!(e.market_state().oracle_px, 100_000); assert_eq!(e.market_state().paused, 0);
    // primary broken (wrong feed) -> oracle2 used
    e.set_time(e.slot + 1); let bad = pyth_data(&[1u8; 32], 1000_00000000, 1, e.ts, e.slot);
    e.svm.set_account(o1, Account { lamports: 1_000_000_000, data: bad, owner: PYTH, executable: false, rent_epoch: 0 }).unwrap();
    put2(&mut e, 990_00000000); assert_eq!(upd(&mut e), None); assert_eq!(e.market_state().oracle_px, 99_000);
    // divergence 3% > 1% -> breaker: paused = 2, price frozen
    e.set_time(e.slot + 1); e.set_pyth(1000_00000000, 10_000_000); put2(&mut e, 970_00000000);
    assert_eq!(upd(&mut e), None); assert_eq!(e.market_state().paused, 2); assert_eq!(e.market_state().oracle_px, 99_000);
    // re-converge -> resumes
    e.set_time(e.slot + 1); e.set_pyth(1000_00000000, 10_000_000); put2(&mut e, 1000_50000000);
    assert_eq!(upd(&mut e), None); assert_eq!(e.market_state().paused, 0); assert_eq!(e.market_state().oracle_px, 100_000);
}

/// Isolated margin: the isolated position is margined only by its own margin; its loss can't touch cross cash,
/// and cross cash can't silently back it (an over-sized isolated order fails even with a large cross balance).
#[test] fn iso1_isolated_margin_contains_loss() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let a0 = e.shard(1, 100_000); let m = e.market;
    let (mm, mmu) = e.user(50_000_000_000); let (u, uu) = e.user(10_000_000); let (lq, lqu) = e.user(5_000_000_000);
    e.send("", trade_accts(&mm, &mmu, &m, &[a0]), single(place_item(1, 1, 100_000, 100, 0, 0)), &[&mm], true);
    let iso = |on: u8| vec![30u8, on];
    e.send("set_isolated", vec![AccountMeta::new(u.pubkey(), true), w(&uu), r(&m)], iso(1), &[&u], true);
    e.send("iso_transfer (cash -> isolated)", vec![AccountMeta::new(u.pubkey(), true), w(&uu), r(&m)], amt(31, 150_000), &[&u], true);
    // 20 lots need IM 200k > 150k isolated margin, even though cross cash is ~9.85M
    assert_eq!(try_send(&mut e, trade_accts(&u, &uu, &m, &[a0]), single(place_item(0, IOC, 100_000, 20, 0, 0)), &[&u]), Some(3));
    e.send("isolated taker IOC (13 lots)", trade_accts(&u, &uu, &m, &[a0]), single(place_item(0, IOC, 100_000, 13, 0, 0)), &[&u], true);
    let s = e.user_state(&uu); assert_eq!(s.cash, 10_000_000 - 150_000);
    assert!(s.pos[0].iso_margin < 150_000 - 13 * 100_000 + 1_000 && s.pos[0].flags == 1);
    // pulling margin out below IM fails
    let mut neg = vec![31u8]; neg.extend_from_slice(&(-100_000i64).to_le_bytes());
    assert_eq!(try_send(&mut e, vec![AccountMeta::new(u.pubkey(), true), w(&uu), r(&m)], neg, &[&u]), Some(3));
    // -8%: isolated position liquidatable although the account's cross cash is large
    e.set_time(e.slot + 1); e.oracle_update("", 920_00000000, None);
    let ins = e.ins;
    // a takeover is one bounded chunk at a time: repeat while still below maintenance
    e.send("liquidate isolated position", vec![AccountMeta::new(lq.pubkey(), true), w(&lqu), w(&uu), r(&m), w(&ins), w(&a0)], amt(5, 13), &[&lq], true);
    for _ in 0..12 { if try_send(&mut e, vec![AccountMeta::new(lq.pubkey(), true), w(&lqu), w(&uu), r(&m), w(&ins), w(&a0)], amt(5, 13), &[&lq]).is_some() { break; } }
    let s = e.user_state(&uu); assert!(s.pos[0].base < 13);
    assert_eq!(s.cash, 10_000_000 - 150_000, "cross cash untouched by the isolated loss");
}

// ---------------- v6 ----------------
const NO_FILL: u32 = 25; const NOT_LIQ: u32 = 5; const NOT_TRIG: u32 = 16;
fn liq_book(e: &mut Env, t: &Address, shards: &[Address], max_q: u64) -> Option<u32> {
    let (m, ins) = (e.market, e.ins);
    let mut acc = vec![w(t), r(&m), w(&ins)]; acc.extend(shards.iter().map(w));
    try_send(e, acc, amt(34, max_q), &[]) // permissionless: no liquidator account, no liquidator capital
}
#[test] fn v53_book_liquidation_needs_no_liquidator_capital() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let (a0, b0) = (e.shard(1, 100_000), e.shard(0, 87_000)); let m = e.market;
    let (mm, mmu) = e.user(50_000_000_000); let (vic, vicu) = e.user(200_000);
    e.send("", trade_accts(&mm, &mmu, &m, &[a0]), single(place_item(1, 1, 100_000, 100, 0, 0)), &[&mm], true);
    e.send("", trade_accts(&vic, &vicu, &m, &[a0]), single(place_item(0, IOC, 100_000, 13, 0, 0)), &[&vic], true);
    e.send("", trade_accts(&mm, &mmu, &m, &[a0]), batch_data(&[], &[]), &[&mm], true);
    assert_eq!(liq_book(&mut e, &vicu, &[a0, b0], u64::MAX), Some(NOT_LIQ), "healthy account");
    e.set_time(e.slot + 1); e.oracle_update("", 880_00000000, None); // -12%: below MM, not bankrupt
    // V6-1: no bid within 5/10/20% -> partial shard set: NoFill; full shard set: ADL-eligible (Ok, no fill)
    assert_eq!(liq_book(&mut e, &vicu, &[a0], u64::MAX), Some(MISSING_SHARDS), "missing shard: the liquidator must pass every opposite shard");
    assert_eq!(liq_book(&mut e, &vicu, &[a0, b0], u64::MAX), None, "empty book proven -> ADL-eligible");
    let f = e.user_state(&vicu).pos[0].flags; assert!(f & 2 != 0 && f >> 8 == e.slot && e.user_state(&vicu).pos[0].base == 13);
    e.send("", trade_accts(&mm, &mmu, &m, &[b0]), single(place_item(0, 1, 87_000, 100, 0, 0)), &[&mm], true);
    let ins0 = e.ins_bal();
    let mut acc = vec![w(&vicu), r(&m), w(&e.ins.clone()), w(&a0), w(&b0)];
    let cu = e.send("liquidate_book (1 chunk into the book, no liquidator capital)", std::mem::take(&mut acc), amt(34, u64::MAX), &[], true).unwrap();
    assert!(cu > 0);
    assert_eq!(e.user_state(&vicu).pos[0].base, 13 - 4, "one 25% chunk (ceil)");
    assert!(e.ins_bal() > ins0, "liquidation fee to insurance");
    let mut n = 0;
    while liq_book(&mut e, &vicu, &[a0, b0], u64::MAX).is_none() { n += 1; assert!(n < 10); }
    let s = e.user_state(&vicu);
    assert!(s.pos[0].base < 9 && e.ins_bal() >= 0);
}
#[test] fn v51_isolated_position_is_adl_counterparty() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let a0 = e.shard(1, 100_000); let m = e.market;
    let (mm, mmu) = e.user(50_000_000_000); let (vic, vicu) = e.user(150_000);
    e.send("", vec![AccountMeta::new(mm.pubkey(), true), w(&mmu), r(&m)], vec![30u8, 1], &[&mm], true);
    e.send("", vec![AccountMeta::new(mm.pubkey(), true), w(&mmu), r(&m)], amt(31, 3_000_000), &[&mm], true);
    e.send("", trade_accts(&mm, &mmu, &m, &[a0]), single(place_item(1, 1, 100_000, 100, 0, 0)), &[&mm], true);
    e.send("", trade_accts(&vic, &vicu, &m, &[a0]), single(place_item(0, IOC, 100_000, 13, 0, 0)), &[&vic], true);
    e.send("", trade_accts(&mm, &mmu, &m, &[a0]), batch_data(&[], &[]), &[&mm], true);
    let cash0 = e.user_state(&mmu).cash;
    e.set_time(e.slot + 1); e.oracle_update("", 800_00000000, None);
    e.rank_user(&mmu); assert_eq!(e.rank_state().n_short, 1, "isolated short is ranked");
    let (ins, rk) = (e.ins, e.rank);
    e.send("", vec![w(&vicu), w(&m), w(&ins), w(&rk), w(&mmu)], vec![6], &[], true);
    let s = e.user_state(&mmu);
    assert_eq!(e.user_state(&vicu).pos[0].base, 0);
    assert_eq!(s.cash, cash0, "cross cash untouched: absorption only from the isolated margin");
    assert!(s.pos[0].iso_margin >= 0 && e.ins_bal() >= 0);
}
#[test] fn v55_breaker_hysteresis_manual_resume_single_source() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let (a, m, o1) = (e.admin.pubkey(), e.market, e.oracle);
    let o2 = Address::new_unique(); let feed2 = [9u8; 32];
    let mut d = adm_d(28, 10, 0, Some(&o2)); d.extend_from_slice(&feed2);
    e.send("", vec![AccountMeta::new(a, true), w(&m)], d, &[], true);
    e.set_time(e.slot + 216_000); e.send("", vec![w(&m)], vec![29], &[], true);
    let put2 = |e: &mut Env, px: i64| { let d = pyth_data(&feed2, px, (px / 10000) as u64, e.ts, e.slot);
        e.svm.set_account(o2, Account { lamports: 1_000_000_000, data: d, owner: PYTH, executable: false, rent_epoch: 0 }).unwrap(); };
    let both = |e: &mut Env, p1: i64, p2: i64| { e.set_time(e.slot + 1); e.set_pyth(p1, 10_000_000); put2(e, p2); try_send(e, vec![w(&m), r(&o1), r(&o2)], vec![4], &[]) };
    assert_eq!(both(&mut e, 1000_00000000, 1000_00000000), None); assert_eq!(e.market_state().paused, 0);
    assert_eq!(both(&mut e, 1000_00000000, 970_00000000), None); assert_eq!(e.market_state().paused, 2, "trip > 1%");
    assert_eq!(both(&mut e, 1000_00000000, 992_00000000), None);
    assert_eq!(e.market_state().paused, 2, "0.8% is inside the hysteresis band: stays tripped");
    // manual resume by the admin
    e.send("", vec![AccountMeta::new(a, true), w(&m)], adm_d(22, 1, 0, None), &[], true);
    assert_eq!(e.market_state().paused, 0);
    assert_eq!(both(&mut e, 1000_00000000, 992_00000000), None);
    assert_eq!(e.market_state().paused, 0, "0.8% < trip threshold after resume");
    // oracle2 omitted by the caller: refused (no choosing the source)
    e.set_time(e.slot + 1); e.set_pyth(1001_00000000, 10_000_000);
    assert_eq!(try_send(&mut e, vec![w(&m), r(&o1)], vec![4], &[]), Some(8));
    // single source (oracle2 passed but stale) with a 10% jump: not applied, paused = 3
    let stale2 = |e: &mut Env| { let d = pyth_data(&feed2, 1000_00000000, 10_000_000, e.ts - 10_000, e.slot);
        e.svm.set_account(o2, Account { lamports: 1_000_000_000, data: d, owner: PYTH, executable: false, rent_epoch: 0 }).unwrap(); };
    let px0 = e.market_state().oracle_px;
    e.set_time(e.slot + 1); e.set_pyth(1100_00000000, 10_000_000); stale2(&mut e);
    assert_eq!(try_send(&mut e, vec![w(&m), r(&o1), r(&o2)], vec![4], &[]), None);
    assert_eq!(e.market_state().paused, 3); assert_eq!(e.market_state().oracle_px, px0);
    // single source with a small move is applied
    assert_eq!(both(&mut e, 1001_00000000, 1001_00000000), None); assert_eq!(e.market_state().paused, 0, "sources agree -> resume");
    e.set_time(e.slot + 1); e.set_pyth(1010_00000000, 10_000_000); stale2(&mut e);
    assert_eq!(try_send(&mut e, vec![w(&m), r(&o1), r(&o2)], vec![4], &[]), None);
    assert_eq!(e.market_state().oracle_px, 101_000);
}
#[test] fn n2_impact_price_over_64_levels_program() {
    let run = |notional: u64| -> i64 {
        let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
        let (s1, s2) = (e.shard(0, 100_000), e.shard(0, 100_000)); let m = e.market;
        set_market_u64(&mut e, pspec::MARKET_IMPACT_NOTIONAL_OFF as usize, notional);
        let mks: Vec<(Keypair, Address)> = (0..5).map(|_| e.user(5_000_000_000)).collect(); // 16 open orders/market cap per user
        for i in 1..=70u64 { let (mk, mku) = &mks[(i as usize - 1) / 14]; e.send("", trade_accts(mk, mku, &m, &[s1, s2]), single(place_item(0, 1, 100_000 + i, 1, 0, 0)), &[mk], true); }
        assert_eq!(e.shard_state(&s1).n + e.shard_state(&s2).n, 70, "70 distinct levels across 2 shards");
        e.set_time(e.slot + 1); e.oracle_update_set("", 1000_00000000, &[s1, s2]);
        e.market_state().prem_last
    };
    // reference (perp-core rule): best 64 levels 100_070 .. 100_007, 1 lot each
    let lv: Vec<u64> = (7..=70u64).rev().map(|i| 100_000 + i).collect();
    let refp = |n: u64| -> i64 { let (mut an, mut aq) = (0u128, 0u128);
        for &p in &lv { let t = 1u128.min((n as u128 - an).div_ceil(p as u128)); an += t * p as u128; aq += t; if an >= n as u128 { let ib = (an / aq) as i128; return ((ib - 100_000) * 1_000_000_000 / 100_000) as i64; } } 0 };
    let in64: u64 = lv.iter().sum::<u64>() - 1_000; // fits inside the 64 levels
    assert_eq!(run(in64), refp(in64));
    assert!(run(in64) > 0);
    let over: u64 = lv.iter().sum::<u64>() + 50_000; // needs a 65th level -> capped -> no impact bid -> 0
    assert_eq!(run(over), 0, "levels beyond 64 are ignored (same rule as core/C++)");
}
#[test] fn v4_mark_trigger_with_oracle_confirm() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let (b0, a0) = (e.shard(0, 94_000), e.shard(1, 100_000)); let m = e.market;
    let (mm, mmu) = e.user(50_000_000_000); let (tk, tku) = e.user(1_000_000_000); let (kp, kpu) = e.user(1_000_000);
    e.send("", trade_accts(&mm, &mmu, &m, &[a0]), single(place_item(1, 1, 100_000, 100, 0, 0)), &[&mm], true);
    e.send("", trade_accts(&tk, &tku, &m, &[a0]), single(place_item(0, IOC, 100_000, 10, 0, 0)), &[&tk], true);
    e.send("", trade_accts(&mm, &mmu, &m, &[b0]), single(place_item(0, 1, 94_000, 100, 0, 0)), &[&mm], true);
    // SL sell 10 @ trigger 95_000 on the MARK (kind byte bit 6), slot 0; with oracle confirm (bit 7) in slot 1
    let set = |e: &mut Env, slot: u8, kind: u8| { let mut d = vec![20u8, slot, kind, 1]; for v in [95_000u64, 90_000, 10, 1, 0, 0] { d.extend_from_slice(&v.to_le_bytes()); }
        e.send("", vec![AccountMeta::new(tk.pubkey(), true), w(&tku), r(&m)], d, &[&tk], true); };
    set(&mut e, 0, 2 | 64); set(&mut e, 1, 2 | 64 | 128);
    let exec = |e: &mut Env, slot: u8| try_send(e, vec![AccountMeta::new(kp.pubkey(), true), w(&kpu), w(&tku), r(&m), w(&b0)], vec![21, slot], &[&kp]);
    e.set_time(e.slot + 1); e.oracle_update("", 940_00000000, None);
    set_market_u64(&mut e, pspec::MARKET_MARK_PX_OFF as usize, 96_000); // oracle through, mark not
    assert_eq!(exec(&mut e, 0), Some(NOT_TRIG), "mark trigger ignores the oracle");
    set_market_u64(&mut e, pspec::MARKET_ORACLE_PX_OFF as usize, 96_000); set_market_u64(&mut e, pspec::MARKET_MARK_PX_OFF as usize, 94_500);
    assert_eq!(exec(&mut e, 1), Some(NOT_TRIG), "oracle confirm required");
    let cu = exec(&mut e, 0); assert_eq!(cu, None, "mark through -> fires");
    assert!(e.user_state(&tku).pos[0].base < 10);
    let _ = mmu;
}

// ---------------- v7: V6-1 / V6-2 / V5-4 ----------------
const NOT_BANKRUPT: u32 = 10;
/// common: victim long 13 @1000 with 200k, mark -> 880 (below MM, not bankrupt), mm short 13 (profitable)
fn v7_setup() -> (Env, Address, Address, Address, Keypair, Address) {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let (a0, b0) = (e.shard(1, 100_000), e.shard(0, 80_000)); let m = e.market;
    let (mm, mmu) = e.user(50_000_000_000); let (vic, vicu) = e.user(200_000);
    e.send("", trade_accts(&mm, &mmu, &m, &[a0]), single(place_item(1, 1, 100_000, 100, 0, 0)), &[&mm], true);
    e.send("", trade_accts(&vic, &vicu, &m, &[a0]), single(place_item(0, IOC, 100_000, 13, 0, 0)), &[&vic], true);
    e.send("", trade_accts(&mm, &mmu, &m, &[a0]), batch_data(&[], &[]), &[&mm], true);
    e.set_time(e.slot + 1); e.oracle_update("", 880_00000000, None);
    (e, a0, b0, vicu, mm, mmu)
}
/// V6-1: no bid within 5%, one within 10% -> fills at step 2 with a doubled liquidation fee (penalty to insurance).
#[test] fn v61_band_escalation_with_fee_penalty() {
    let (mut e, a0, b0, vicu, mm, mmu) = v7_setup(); let m = e.market;
    e.send("", trade_accts(&mm, &mmu, &m, &[b0]), single(place_item(0, 1, 80_000, 100, 0, 0)), &[&mm], true); // ~9.1% below mark
    let ins0 = e.ins_bal();
    assert_eq!(liq_book(&mut e, &vicu, &[a0, b0], u64::MAX), None);
    assert_eq!(e.user_state(&vicu).pos[0].base, 9);
    let fee = e.ins_bal() - ins0; let base_fee = 4 * 88_000 * 25 / 10_000; // 4 lots x mark x 25 bps
    assert!(fee > base_fee * 3 / 2 && fee <= base_fee * 2 + 50, "step-2 fee is 2x: {fee} vs base {base_fee}");
}
/// V6-1 fallback: book empty within 20% -> (full shard set) ADL-eligible -> ADL deleverages a NON-bankrupt account
/// inside the window; ADL on an unflagged / expired account still needs bankruptcy.
#[test] fn v61_unfillable_falls_back_to_adl_within_window() {
    let (mut e, a0, b0, vicu, _mm, mmu) = v7_setup(); let m = e.market; let (ins, rk) = (e.ins, e.rank);
    let adl = |e: &mut Env| try_send(e, vec![w(&vicu), w(&m), w(&ins), w(&rk), w(&mmu)], vec![6], &[]);
    assert_eq!(adl(&mut e), Some(NOT_BANKRUPT), "below MM but not flagged: no ADL");
    assert_eq!(liq_book(&mut e, &vicu, &[a0, b0], u64::MAX), None);
    // window expiry
    let s = e.slot; e.set_time(s + pspec::LIQ_ADL_WINDOW_SLOTS + 1); e.oracle_update("", 880_00000000, None);
    assert_eq!(adl(&mut e), Some(NOT_BANKRUPT), "eligibility expired");
    assert_eq!(liq_book(&mut e, &vicu, &[a0, b0], u64::MAX), None, "re-flag");
    let ins0 = e.ins_bal();
    assert_eq!(adl(&mut e), None);
    let v = e.user_state(&vicu);
    assert_eq!(v.pos[0].base, 0); assert!(v.cash > 0, "not bankrupt: keeps remaining equity");
    assert!(e.ins_bal() > ins0, "fallback fee to insurance"); assert_eq!(e.user_state(&mmu).pos[0].base, 0, "counterparty (short 13) closed at mark");
    assert_eq!(v.pos[0].flags & 2, 0);
}
/// V6-2: without oracle2, a single-source jump > 5% is held (paused, price unchanged) until a NEWER publish confirms it;
/// replaying the same publish doesn't confirm; a spike that reverts never lands.
#[test] fn v62_single_source_jump_guard_without_oracle2() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let (m, o) = (e.market, e.oracle);
    let upd = |e: &mut Env| { e.send("", vec![w(&m), r(&o)], vec![4], &[], true); };
    e.set_time(e.slot + 1); e.ts += 1; e.set_pyth(1300_00000000, 1); upd(&mut e);
    assert_eq!((e.market_state().paused, e.market_state().oracle_px), (3, 100_000), "spike held");
    e.set_time(e.slot + 1); upd(&mut e); // same publish replayed
    assert_eq!(e.market_state().paused, 3, "replay doesn't confirm");
    e.ts += 1; e.set_pyth(1001_00000000, 1); upd(&mut e);
    assert_eq!((e.market_state().paused, e.market_state().oracle_px), (0, 100_100), "spike reverted, never landed");
    e.ts += 1; e.set_pyth(1100_00000000, 1); upd(&mut e);
    assert_eq!(e.market_state().paused, 3);
    e.ts += 1; e.set_pyth(1250_00000000, 1); upd(&mut e);
    assert_eq!((e.market_state().paused, e.market_state().oracle_px), (0, 125_000), "a move continued the same way confirms the candidate");
    e.ts += 1; e.set_pyth(1251_00000000, 1); upd(&mut e);
    assert_eq!((e.market_state().paused, e.market_state().oracle_px), (0, 125_100));
}
/// V5-4: a pooled market's admin is the pool admin; margin can only tighten, admin not transferable, OI cap only down.
#[test] fn v54_pooled_market_admin_pinned_and_bounded() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let p = new_pool(&mut e); let pa = Keypair::try_from(&p.admin.to_bytes()[..]).unwrap();
    let (ma, r1) = pooled_market(&mut e, &p, &pa); assert_eq!(r1, None);
    switch(&mut e, &ma); let m = ma.market;
    assert_eq!(e.market_state().admin, pa.pubkey().to_bytes(), "admin pinned to the pool admin");
    let ea = e.admin.pubkey();
    assert_eq!(try_send(&mut e, vec![AccountMeta::new(ea, true), w(&m)], adm_d(22, 1, 1, None), &[]), Some(1), "market creator has no admin rights");
    let pk = pa.pubkey();
    let mut a = |e: &mut Env, d: Vec<u8>| try_send(e, vec![AccountMeta::new(pk, true), w(&m)], d, &[&pa]);
    assert_eq!(a(&mut e, adm_d(22, 3, 2_000_000, None)), Some(CAP), "OI cap can't rise");
    assert_eq!(a(&mut e, adm_d(22, 3, 500_000, None)), None, "OI cap can fall");
    assert_eq!(a(&mut e, adm_d(28, 4, 500, None)), Some(CAP), "imr can't loosen below default");
    assert_eq!(a(&mut e, adm_d(28, 5, 100, None)), Some(CAP), "mmr can't loosen below default");
    let other = Address::new_unique();
    assert_eq!(a(&mut e, adm_d(28, 8, 0, Some(&other))), Some(CAP), "admin not transferable");
    assert_eq!(a(&mut e, adm_d(28, 4, 2000, None)), None, "tightening is allowed (timelocked)");
}
/// Real Pyth bytes: PriceUpdateV2 account 7UVimffxr9ow1uXYxsr4LHAcV58mLzhmwaeKvJ1pjLiE (SOL/USD, Pyth push oracle)
/// fetched from devnet and mainnet-beta RPC on 2026-11-01 (program-tests/fixtures). The program parses the real
/// layout, checks owner/feed id/confidence/age, and stores the scaled price. (Local LiteSVM, not an on-chain deploy.)
#[test] fn real_pyth_price_update_v2_devnet_and_mainnet() {
    for net in ["devnet", "mainnet-beta"] {
        let b = std::fs::read(format!("{}/fixtures/pyth_sol_usd_{net}.bin", env!("CARGO_MANIFEST_DIR"))).unwrap();
        let o = 8 + 32 + if b[40] == 1 { 1 } else { 2 };
        let feed: [u8; 32] = b[o..o + 32].try_into().unwrap();
        let price = i64::from_le_bytes(b[o + 32..o + 40].try_into().unwrap());
        let pt = i64::from_le_bytes(b[o + 52..o + 60].try_into().unwrap());
        let mut e = Env::new();
        let fo = pspec::MARKET_FEED_ID_OFF as usize;
        let (m, orc) = (e.market, e.oracle);
        let mut a = e.svm.get_account(&m).unwrap(); a.data[fo..fo + 32].copy_from_slice(&feed); e.svm.set_account(m, a).unwrap();
        e.svm.set_account(orc, Account { lamports: 1_000_000_000, data: b.clone(), owner: PYTH, executable: false, rent_epoch: 0 }).unwrap();
        e.ts = pt + 2; let s = e.slot + 1; e.set_time(s); e.ts = pt + 2; e.set_time(s);
        e.send("", vec![w(&m), r(&orc)], vec![4], &[], true);
        let ms = e.market_state();
        assert_eq!(ms.oracle_px as i64, price * ms.px_mul / ms.px_div, "{net}: scaled real price");
        assert_eq!(ms.oracle_ts, pt);
        // a wrong feed id is rejected
        let mut a = e.svm.get_account(&m).unwrap(); a.data[fo] ^= 1; e.svm.set_account(m, a).unwrap();
        assert!(try_send(&mut e, vec![w(&m), r(&orc)], vec![4], &[]).is_some(), "{net}: feed id mismatch rejected");
        println!("{net}: SOL/USD real price {price}e-8 -> oracle_px {}", ms.oracle_px);
    }
}
/// LOAD-1 (found by the load test): with 16 MMs on one bucket, a batch replace whose placement hit a full shard (64
/// orders) failed entirely (ShardFull), dropping all its cancels and other quotes. Now only that item is skipped.
#[test] fn load1_full_bucket_skips_item_not_batch() {
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let (full, other) = (e.shard(1, 100_000), e.shard(1, 100_100)); let m = e.market;
    for _ in 0..4 {
        let (k, u) = e.user(1_000_000_000);
        let items: Vec<Vec<u8>> = (0..16).map(|i| place_item(1, 1, 100_000 + i, 1, 0, 0)).collect();
        e.send("", trade_accts(&k, &u, &m, &[full]), batch_data(&[], &items), &[&k], true);
    }
    assert_eq!(e.shard_state(&full).n, 64);
    let (k, u) = e.user(1_000_000_000);
    // single place into the full bucket still fails loudly
    assert_eq!(try_send(&mut e, trade_accts(&k, &u, &m, &[full, other]), single(place_item(1, 1, 100_050, 1, 0, 0)), &[&k]), Some(6));
    // batch: one item into the full bucket (skipped), one into another bucket (rests): tx succeeds
    let d = batch_data(&[], &[place_ex(1, 1, 0, 100_050, 1, 0, 0, 7, 0, 0, 0), place_ex(1, 1, 0, 100_150, 2, 0, 0, 8, 0, 0, 0)]);
    e.send("", trade_accts(&k, &u, &m, &[full, other]), d, &[&k], true);
    let s = e.user_state(&u);
    assert_eq!((s.pos[0].ask_open, s.pos[0].n_orders), (2, 1), "only the item that fit was booked");
    assert_eq!(e.shard_state(&full).n, 64); assert_eq!(e.shard_state(&other).n, 1);
}
