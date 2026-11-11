// Audit B proofs of concept (collateral / accounting / governance). Each test performs an attack against the
// built SBF program and ASSERTS THAT IT SUCCEEDS: a passing test means the bug is present. Invert the final
// assertions into regression tests once fixed.
#[path = "../../program/src/state.rs"]
#[allow(dead_code)]
mod state;
include!("common.rs");

const IOC: u8 = 4;
fn amt(tag: u8, v: u64) -> Vec<u8> {
    let mut d = vec![tag];
    d.extend_from_slice(&v.to_le_bytes());
    d
}
fn single(item: Vec<u8>) -> Vec<u8> {
    let mut d = vec![1u8];
    d.extend_from_slice(&item);
    d
}
fn try_send(e: &mut Env, accounts: Vec<AccountMeta>, data: Vec<u8>, signers: &[&Keypair]) -> Option<u32> {
    let ix = Instruction { program_id: e.pid, accounts, data };
    let mut s: Vec<&Keypair> = vec![&e.admin];
    s.extend_from_slice(signers);
    let tx = Transaction::new_signed_with_payer(&[ix], Some(&e.admin.pubkey()), &s, e.svm.latest_blockhash());
    let r = e.svm.send_transaction(tx);
    e.svm.expire_blockhash();
    match r {
        Ok(_) => None,
        Err(f) => {
            let s = format!("{:?}", f.err);
            Some(
                s.split("Custom(")
                    .nth(1)
                    .and_then(|x| x.split(')').next())
                    .and_then(|x| x.parse().ok())
                    .unwrap_or(u32::MAX),
            )
        }
    }
}
fn tok(e: &mut Env, owner: &Address, amount: u64) -> Address {
    let s = Address::new_unique();
    let mint = Address::from(e.market_state().mint);
    e.svm
        .set_account(
            s,
            Account {
                lamports: 1_000_000_000,
                data: token_acct(&mint, owner, amount),
                owner: TOKEN,
                executable: false,
                rent_epoch: 0,
            },
        )
        .unwrap();
    s
}
fn tok_amt(e: &Env, k: &Address) -> u64 {
    u64::from_le_bytes(e.data(k)[64..72].try_into().unwrap())
}
fn place(e: &mut Env, kp: &Keypair, u: &Address, shards: &[Address], side: u8, fl: u8, px: u64, q: u64) {
    let m = e.market;
    e.send("", trade_accts(kp, u, &m, shards), single(place_item(side, fl, px, q, 0, 0)), &[kp], true);
}
fn equity(e: &Env, u: &Address) -> i128 {
    let (s, m) = (e.user_state(u), e.market_state());
    let px = if m.mark_px != 0 { m.mark_px } else { m.oracle_px } as i128;
    s.cash as i128
        + s.pos.iter().filter(|p| p.market != [0; 32]).map(|p| p.iso_margin as i128 + p.base as i128 * px).sum::<i128>()
}
fn withdraw_accts(kp: &Address, u: &Address, e: &Env, dst: &Address, extra: &[Address]) -> Vec<AccountMeta> {
    let mut v = vec![AccountMeta::new(*kp, true), w(u), r(&e.market), w(&e.vault), w(dst), r(&TOKEN), r(&e.vault_pda)];
    v.extend(extra.iter().map(w));
    v
}

// ---------------------------------------------------------------------------------------------------------
// B-1 (Critical): the Vault account's address is the authority of its sub-accounts. A Vault created with a
// keypair (the normal System create_account path) lets that keypair sign as the authority and call the plain
// user `withdraw` on subs[0], taking every depositor's money.
#[test]
fn poc_b1_vault_keypair_drains_depositors_via_sub_withdraw() {
    let mut e = Env::new();
    e.oracle_update("", 1000_00000000, None);
    let vk = Keypair::new(); // the vault account's own keypair (held by whoever created the account)
    let (v, sub) = (vk.pubkey(), Address::new_unique());
    e.prog_acct(v, VAULT_SIZE);
    e.prog_acct(sub, USER_SIZE);
    let (a, m) = (e.admin.pubkey(), e.market);
    let mut d = vec![50u8];
    for x in [100u64, 0, 0, 0] {
        d.extend_from_slice(&x.to_le_bytes());
    }
    e.send("", vec![AccountMeta::new(a, true), w(&v), r(&m)], d, &[], true);
    e.send("", vec![AccountMeta::new(a, true), w(&v), w(&sub)], vec![51], &[], true);
    // victim deposits 10_000 USDC into the vault
    let victim = Keypair::new();
    e.svm.airdrop(&victim.pubkey(), 1_000_000_000).unwrap();
    let vd = Address::new_unique();
    e.prog_acct(vd, VDEP_SIZE);
    let src = tok(&mut e, &victim.pubkey(), 10_000_000_000);
    let tv = e.vault;
    e.send(
        "",
        vec![AccountMeta::new(victim.pubkey(), true), w(&v), w(&vd), w(&sub), r(&m), w(&tv), w(&src), r(&TOKEN)],
        amt(53, 10_000_000_000),
        &[&victim],
        true,
    );
    assert_eq!(e.user_state(&sub).cash, 10_000_000_000);
    // attacker: sign with the vault keypair as `authority` of subs[0] and withdraw everything (no lockup, no shares)
    let thief = tok(&mut e, &Address::new_unique(), 0);
    let acc = withdraw_accts(&v, &sub, &e, &thief, &[]);
    e.send("", acc, amt(14, 10_000_000_000), &[&vk], false); // fixed: a program-owned account never signs (vault keypair is no authority)
    assert_eq!(tok_amt(&e, &thief), 0);
    assert_eq!(e.user_state(&sub).cash, 10_000_000_000);
}

// B-2 (High): auth() accepts a direct signature by the admin key. When the admin is a Multisig account, that
// account's own keypair (it must exist: the account was created with System create_account, and pool/vault
// admins are taken from a signer) is a 1-of-1 bypass of the m-of-n rule.
#[test]
fn poc_b2_multisig_account_keypair_bypasses_m_of_n() {
    let mut e = Env::new();
    let (a, m) = (e.admin.pubkey(), e.market);
    let ks: Vec<Keypair> = (0..3).map(|_| Keypair::new()).collect();
    let msk = Keypair::new();
    let ms = msk.pubkey();
    e.prog_acct(ms, MULTISIG_SIZE);
    let mut d = vec![33u8, 2, 3];
    for k in &ks {
        d.extend_from_slice(k.pubkey().as_ref());
    }
    e.send("", vec![AccountMeta::new(a, true), w(&ms)], d, &[], true);
    let mut d = vec![28u8, 8];
    d.extend_from_slice(&0u64.to_le_bytes());
    d.extend_from_slice(ms.as_ref());
    e.send("", vec![AccountMeta::new(a, true), w(&m)], d, &[], true);
    e.set_time(e.slot + 216_000);
    e.send("", vec![w(&m)], vec![29], &[], true);
    assert_eq!(e.market_state().admin, ms.to_bytes());
    // no multisig member signs: the multisig account's keypair alone proposes an oracle switch (and pauses)
    let mut d = vec![28u8, 11];
    d.extend_from_slice(&0u64.to_le_bytes());
    d.extend_from_slice(Address::new_unique().as_ref());
    assert_eq!(try_send(&mut e, vec![AccountMeta::new_readonly(ms, true), w(&m)], d, &[&msk]), Some(1)); // fixed: the multisig account itself never signs
    let mut d = vec![22u8, 1];
    d.extend_from_slice(&1u64.to_le_bytes());
    assert_eq!(try_send(&mut e, vec![AccountMeta::new_readonly(ms, true), w(&m)], d, &[&msk]), Some(1));
    assert_eq!(e.market_state().paused, 0);
    assert_eq!(e.market_state().pending_op, 0);
}

// B-3 (High): withdraw values the account without the maker fills sitting in shard ledgers it did not pass.
// The filled qty still counts in bid_open (initial margin at the current mark), but the loss on it does not.
#[test]
fn poc_b3_withdraw_ignores_unclaimed_maker_fill_loss() {
    let mut e = Env::new();
    e.oracle_update("", 1000_00000000, None);
    let b0 = e.shard(0, 100_000);
    let (mk, mu) = e.user(1_100_000_000); // maker: 1.1B collateral
    let (tk, tu) = e.user(100_000_000_000);
    place(&mut e, &mk, &mu, &[b0], 0, 1, 100_000, 100_000); // bid 100k lots @ 100_000 = 1e10 notional (11% margin)
    place(&mut e, &tk, &tu, &[b0], 1, IOC, 100_000, 100_000); // filled; maker's fill sits in the shard ledger
    for p in [955, 910, 870, 830, 800] {
        e.oracle_update("", p * 100_000_000, None);
    } // -20%: maker is bankrupt
    let dst = tok(&mut e, &mk.pubkey(), 0);
    // honest path (shard passed -> fill claimed) is refused
    let acc = withdraw_accts(&mk.pubkey(), &mu, &e, &dst, &[b0]);
    assert_eq!(try_send(&mut e, acc, amt(14, 300_000_000), &[&mk]), Some(3), "Margin");
    // omit the shard: 300M leaves the vault
    let acc = withdraw_accts(&mk.pubkey(), &mu, &e, &dst, &[]);
    assert_eq!(try_send(&mut e, acc, amt(14, 300_000_000), &[&mk]), Some(19)); // fixed: resting orders -> every shard of the market must be passed
    assert_eq!(tok_amt(&e, &dst), 0);
    e.send("", vec![w(&b0), w(&mu)], vec![24], &[], true); // crank_claim books the fill
    let eq = equity(&e, &mu);
    assert!(eq > -1_000_000_000, "nothing left the vault: {eq}");
}

// B-4 (High): set_isolated only looks at shard state (base / open / n_orders); unsettled FastBook fills are
// invisible. A maker flips the market to isolated with fills pending, settle books the whole position into
// iso_margin (= -notional, zero equity), the maker releases its quoting margin and withdraws all cross cash.
#[test]
fn poc_b4_set_isolated_with_unsettled_fast_fills_strands_unmargined_position() {
    let mut e = Env::new();
    e.oracle_update("", 1000_00000000, None);
    let f = Address::new_unique();
    e.prog_acct(f, FAST_SIZE);
    let (a, m) = (e.admin.pubkey(), e.market);
    e.send("", vec![AccountMeta::new(a, true), w(&m), w(&f)], amt(61, 0), &[], true);
    let (kp, u) = e.user(1_100_000_000);
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
    e.send("", vec![AccountMeta::new(kp.pubkey(), true), w(&u), r(&m), w(&f)], amt(64, 1_000_000_000), &[&kp], true);
    let mut q = vec![63u8];
    q.extend_from_slice(&0u16.to_le_bytes());
    q.extend_from_slice(&99_990u32.to_le_bytes());
    q.extend_from_slice(&100_000u32.to_le_bytes());
    e.send("", vec![AccountMeta::new(kp.pubkey(), true), w(&slab)], q, &[&kp], true);
    // attacker's other account sells into the bid
    let (tp, tu) = e.user(10_000_000_000);
    let mut d = vec![66u8, 1, 0, 1];
    d.extend_from_slice(&99_000u64.to_le_bytes());
    d.extend_from_slice(&100_000u64.to_le_bytes());
    e.send("", vec![AccountMeta::new(tp.pubkey(), true), w(&tu), r(&m), w(&f), r(&slab)], d, &[&tp], true);
    assert_eq!(e.user_state(&tu).pos[0].base, -100_000);
    // maker: isolate the market while the long is still unsettled, then settle
    e.send("", vec![AccountMeta::new(kp.pubkey(), true), w(&u), r(&m)], vec![30, 1], &[&kp], false);
    // fixed: a maker market stays cross
}

// B-5 (Medium): exec_trigger fills a TP/SL/TWAP against whatever shards the permissionless keeper passes; with
// strict_take = 0 (the default) the keeper routes the user's stop to its own worse bid instead of the best bid.
#[test]
fn poc_b5_keeper_routes_stop_loss_to_its_own_worse_bid() {
    let mut e = Env::new();
    e.oracle_update("", 1000_00000000, None);
    let (a0, b_hi, b_lo) = (e.shard(1, 100_000), e.shard(0, 99_000), e.shard(0, 90_000));
    let (mm, mmu) = e.user(10_000_000_000);
    let (v, vu) = e.user(1_000_000_000);
    let (kp, ku) = e.user(1_000_000_000);
    place(&mut e, &mm, &mmu, &[a0], 1, 1, 100_000, 10);
    place(&mut e, &v, &vu, &[a0], 0, IOC, 100_000, 10); // victim long 10
    place(&mut e, &mm, &mmu, &[b_hi], 0, 1, 99_000, 10); // honest best bid 99_000
    place(&mut e, &kp, &ku, &[b_lo], 0, 1, 90_000, 10); // keeper's own bid 90_000
                                                        // victim SL: sell 10 when oracle <= 99_500, limit 90_000 (10% slippage)
    let m = e.market;
    let mut d = vec![20u8, 0, 2, 1];
    for x in [99_500u64, 90_000, 10, 0, 0, 0] {
        d.extend_from_slice(&x.to_le_bytes());
    }
    e.send("", vec![AccountMeta::new(v.pubkey(), true), w(&vu), r(&m)], d, &[&v], true);
    e.oracle_update("", 995_00000000, None);
    let c0 = e.user_state(&vu).cash;
    e.send("", vec![AccountMeta::new(kp.pubkey(), true), w(&ku), w(&vu), r(&m), w(&b_lo)], vec![21, 0], &[&kp], false); // fixed: MissingShards
    let got = e.user_state(&vu).cash - c0;
    assert!(got < 10 * 90_001, "victim sold at the keeper's 90_000 while 99_000 was bid: proceeds {got}");
    assert_eq!(e.shard_state(&b_hi).n, 1, "best bid untouched");
}

// B-6 (Low): the keeper tip is debited from cross cash after the only check (TP/SL: equity >= 0), so a flat
// account can be pushed to negative cash with no position to liquidate (unrecoverable deficit, paid to keeper).
#[test]
fn poc_b6_trigger_tip_leaves_negative_cash() {
    let mut e = Env::new();
    e.oracle_update("", 1000_00000000, None);
    let (a0, b0) = (e.shard(1, 100_000), e.shard(0, 100_000));
    let (mm, mmu) = e.user(10_000_000_000);
    let (v, vu) = e.user(20_000);
    let (kp, ku) = e.user(1);
    place(&mut e, &mm, &mmu, &[a0], 1, 1, 100_000, 1);
    place(&mut e, &v, &vu, &[a0], 0, IOC, 100_000, 1);
    place(&mut e, &mm, &mmu, &[b0], 0, 1, 100_000, 1);
    let m = e.market;
    let mut d = vec![20u8, 0, 1, 1];
    for x in [1u64, 1, 1, 1_000_000, 0, 0] {
        d.extend_from_slice(&x.to_le_bytes());
    }
    e.send("", vec![AccountMeta::new(v.pubkey(), true), w(&vu), r(&m)], d, &[&v], true);
    e.send("", vec![AccountMeta::new(kp.pubkey(), true), w(&ku), w(&vu), r(&m), w(&b0)], vec![21, 0], &[&kp], false); // fixed: a tip larger than what the account has left is refused (Margin)
    let s = e.user_state(&vu);
    assert!(s.cash >= 0 || s.pos.iter().any(|p| p.base != 0), "flat account left with cash {}", s.cash);
}

// B-7 (Low): a session-key delegate's per-order max_qty is checked against the wire qty, but a spline order
// rests a2*qty + a3*n(n-1)/2 lots: the cap is bypassed by any multiple.
#[test]
fn poc_b7_delegate_qty_cap_bypassed_with_spline() {
    let mut e = Env::new();
    e.oracle_update("", 1000_00000000, None);
    let b0 = e.shard(0, 90_000);
    let (kp, u) = e.user(100_000_000_000);
    let m = e.market;
    let sk = Keypair::new();
    e.svm.airdrop(&sk.pubkey(), 1_000_000_000).unwrap();
    let rec = Address::new_unique();
    e.prog_acct(rec, DELEGATE_SIZE);
    let mut d = vec![42u8];
    d.extend_from_slice(sk.pubkey().as_ref());
    d.extend_from_slice(&(e.slot + 100).to_le_bytes());
    d.extend_from_slice(&10u64.to_le_bytes());
    e.send("", vec![AccountMeta::new(kp.pubkey(), true), r(&u), w(&rec)], d, &[&kp], true);
    // spline bid: 10 lots per level, 16 levels, +1000 per level => 16*10 + 1000*120 lots resting
    let it = place_ex(0, 0, 2, 90_000, 10, 0, 0, 0, 100, 16, 1000);
    let mut acc = trade_accts(&sk, &u, &m, &[b0]);
    acc.push(r(&rec));
    e.send("", acc, single(it), &[&sk], false); // fixed: cap on the resting total
    let o = e.shard_state(&b0).orders[0];
    assert_eq!(o.qty, 0, "nothing rests");
}
