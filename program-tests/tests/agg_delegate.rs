#[path = "../../program/src/state.rs"]
#[allow(dead_code)]
mod state;
// Aggregated keeper oracle (IX 40/41) and session-key delegate (IX 42) tests against the built SBF program.
include!("common.rs");
const BAD_ACCOUNT: u32 = 1;
const E_ORACLE: u32 = 8;
const CAP: u32 = 12;
const BREAKER: u32 = 24;
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
fn post(px: i64, conf: i64, ts: i64) -> Vec<u8> {
    let mut d = vec![41u8];
    for v in [px, conf, ts, 3] {
        d.extend_from_slice(&v.to_le_bytes());
    }
    d
}
/// Replace the market's oracle account with a program-owned aggregated account bound to `keeper`.
fn agg_env(keeper: &Keypair) -> Env {
    let mut e = Env::new();
    let o = e.oracle;
    e.prog_acct(o, state::AGG_SIZE);
    let mut d = vec![40u8];
    d.extend_from_slice(&e.feed);
    d.extend_from_slice(&(-8i32).to_le_bytes());
    d.extend_from_slice(&2000u64.to_le_bytes());
    assert_eq!(try_send(&mut e, vec![AccountMeta::new(keeper.pubkey(), true), w(&o)], d.clone(), &[keeper]), None);
    assert_eq!(
        try_send(&mut e, vec![AccountMeta::new(keeper.pubkey(), true), w(&o)], d, &[keeper]),
        Some(BAD_ACCOUNT),
        "re-init"
    );
    e
}
#[test]
fn agg_oracle_drives_market_and_rejects_bad_posts() {
    let k = Keypair::new();
    let mut e = agg_env(&k);
    let (o, m) = (e.oracle, e.market);
    let ts = e.ts;
    assert_eq!(
        try_send(&mut e, vec![AccountMeta::new(k.pubkey(), true), w(&o)], post(1000_00000000, 10_000_000, ts), &[&k]),
        None
    );
    assert_eq!(try_send(&mut e, vec![w(&m), r(&o)], vec![4], &[]), None, "update_oracle reads aggregated account");
    assert_eq!(e.market_state().oracle_px, 100_000);
    let evil = Keypair::new();
    assert_eq!(
        try_send(&mut e, vec![AccountMeta::new(evil.pubkey(), true), w(&o)], post(1_00000000, 1, ts + 1), &[&evil]),
        Some(BAD_ACCOUNT),
        "unauthorised updater"
    );
    // replay / overtaken post (ts not newer): lands as a no-op, nothing applied (an out-of-order post must not sink its tx)
    let agg_before = e.svm.get_account(&o).unwrap().data;
    assert_eq!(
        try_send(&mut e, vec![AccountMeta::new(k.pubkey(), true), w(&o)], post(1234_00000000, 1, ts), &[&k]),
        None,
        "non-increasing ts is a no-op"
    );
    assert_eq!(e.svm.get_account(&o).unwrap().data, agg_before, "AGG bytes untouched (price, counter, ext)");
    let mut stale_ext = post(1234_00000000, 1, ts);
    stale_ext.extend_from_slice(&1234_00000000i64.to_le_bytes());
    stale_ext.extend_from_slice(&ts.to_le_bytes());
    assert_eq!(
        try_send(&mut e, vec![AccountMeta::new(k.pubkey(), true), w(&o)], stale_ext, &[&k]),
        None,
        "stale post with ext bytes: no-op"
    );
    assert_eq!(e.svm.get_account(&o).unwrap().data, agg_before, "ext not stored from a stale post");
    assert_eq!(try_send(&mut e, vec![w(&m), r(&o)], vec![4], &[]), None);
    assert_eq!(e.market_state().oracle_px, 100_000, "old post not applied");
    assert_eq!(
        try_send(&mut e, vec![AccountMeta::new(k.pubkey(), true), w(&o)], post(1234_00000000, 1, ts - 5), &[&k]),
        None,
        "older ts is a no-op"
    );
    assert_eq!(try_send(&mut e, vec![w(&m), r(&o)], vec![4], &[]), None);
    assert_eq!(e.market_state().oracle_px, 100_000);
    assert_eq!(
        try_send(&mut e, vec![AccountMeta::new(k.pubkey(), true), w(&o)], post(1000_00000000, 1, ts + 60), &[&k]),
        Some(E_ORACLE),
        "future ts"
    );
    assert_eq!(
        try_send(&mut e, vec![AccountMeta::new(k.pubkey(), true), w(&o)], post(1300_00000000, 1, ts + 1), &[&k]),
        Some(BREAKER),
        "30% jump > 20% cap"
    );
    assert_eq!(
        try_send(&mut e, vec![AccountMeta::new(k.pubkey(), true), w(&o)], post(-5, 1, ts + 1), &[&k]),
        Some(E_ORACLE),
        "non-positive"
    );
    // stale: market max_age 10 s
    e.set_time(e.slot + 100);
    assert_eq!(
        try_send(&mut e, vec![w(&m), r(&o)], vec![4], &[]).is_some(),
        true,
        "stale aggregated price rejected by update_oracle"
    );
}
#[test]
fn agg_account_spoof_by_other_program_rejected() {
    let mut e = Env::new();
    let (o, m) = (e.oracle, e.market);
    let mut d = vec![0u8; state::AGG_SIZE];
    d[0..8].copy_from_slice(&state::AGG_DISC);
    d[40] = 1;
    d[41..73].copy_from_slice(&e.feed);
    d[73..81].copy_from_slice(&1000_00000000i64.to_le_bytes());
    d[89..93].copy_from_slice(&(-8i32).to_le_bytes());
    d[93..101].copy_from_slice(&e.ts.to_le_bytes());
    e.svm
        .set_account(
            o,
            Account {
                lamports: 1_000_000_000,
                data: d,
                owner: Address::new_unique(),
                executable: false,
                rent_epoch: 0,
            },
        )
        .unwrap();
    assert!(try_send(&mut e, vec![w(&m), r(&o)], vec![4], &[]).is_some());
}
fn set_del(
    user: &Address,
    rec: &Address,
    owner: &Keypair,
    del: &Address,
    exp: u64,
    maxq: u64,
) -> (Vec<AccountMeta>, Vec<u8>) {
    let mut d = vec![42u8];
    d.extend_from_slice(del.as_ref());
    d.extend_from_slice(&exp.to_le_bytes());
    d.extend_from_slice(&maxq.to_le_bytes());
    (vec![AccountMeta::new(owner.pubkey(), true), r(user), w(rec)], d)
}
#[test]
fn delegate_can_place_cancel_only_within_limits() {
    let mut e = Env::new();
    e.oracle_update("", 1000_00000000, None);
    let b0 = e.shard(0, 99_000);
    let (kp, u) = e.user(1_000_000_000);
    let m = e.market;
    let sk = Keypair::new();
    e.svm.airdrop(&sk.pubkey(), 1_000_000_000).unwrap();
    let rec = Address::new_unique();
    e.prog_acct(rec, state::DELEGATE_SIZE);
    let acc = |_: ()| {
        let mut v = trade_accts(&sk, &u, &m, &[b0]);
        v.push(r(&rec));
        v
    };
    // before approval: rejected
    assert_eq!(
        try_send(
            &mut e,
            acc(()),
            {
                let mut d = vec![1u8];
                d.extend(place_item(0, 0, 99_050, 5, 0, 0));
                d
            },
            &[&sk]
        ),
        Some(BAD_ACCOUNT)
    );
    // a stranger cannot approve
    let x = Keypair::new();
    let (a1, d1) = set_del(&u, &rec, &x, &sk.pubkey(), e.slot + 100, 10);
    assert_eq!(try_send(&mut e, a1, d1, &[&x]), Some(BAD_ACCOUNT));
    let (a2, d2) = set_del(&u, &rec, &kp, &sk.pubkey(), e.slot + 100, 10);
    assert_eq!(try_send(&mut e, a2, d2, &[&kp]), None);
    // place within cap OK, over cap rejected
    assert_eq!(
        try_send(
            &mut e,
            acc(()),
            {
                let mut d = vec![1u8];
                d.extend(place_item(0, 0, 99_050, 5, 0, 0));
                d
            },
            &[&sk]
        ),
        None
    );
    assert_eq!(e.shard_state(&b0).n, 1);
    assert_eq!(
        try_send(
            &mut e,
            acc(()),
            {
                let mut d = vec![1u8];
                d.extend(place_item(0, 0, 99_050, 11, 0, 0));
                d
            },
            &[&sk]
        ),
        Some(CAP)
    );
    // cancel via batch
    let id = e.shard_state(&b0).orders[0].id;
    assert_eq!(try_send(&mut e, acc(()), batch_data(&[cancel_item(id, 0, 99_050)], &[]), &[&sk]), None);
    assert_eq!(e.shard_state(&b0).n, 0);
    // withdraw / deposit / isolated / triggers stay owner-only
    let s = Address::new_unique();
    let (v, vp) = (e.vault, e.vault_pda);
    assert_eq!(
        try_send(
            &mut e,
            vec![AccountMeta::new(sk.pubkey(), true), w(&u), r(&m), w(&v), w(&s), r(&TOKEN), r(&vp), r(&rec)],
            {
                let mut d = vec![14u8];
                d.extend_from_slice(&1u64.to_le_bytes());
                d
            },
            &[&sk]
        ),
        Some(BAD_ACCOUNT)
    );
    assert_eq!(
        try_send(&mut e, vec![AccountMeta::new(sk.pubkey(), true), w(&u), r(&m), r(&rec)], vec![30, 1], &[&sk]),
        Some(BAD_ACCOUNT)
    );
    // expiry and revoke
    e.set_time(e.slot + 101);
    assert_eq!(
        try_send(
            &mut e,
            acc(()),
            {
                let mut d = vec![1u8];
                d.extend(place_item(0, 0, 99_050, 5, 0, 0));
                d
            },
            &[&sk]
        ),
        Some(BAD_ACCOUNT),
        "expired"
    );
    e.oracle_update("", 1000_00000000, None);
    let (a3, d3) = set_del(&u, &rec, &kp, &sk.pubkey(), 0, 10);
    assert_eq!(try_send(&mut e, a3, d3, &[&kp]), None);
    assert_eq!(
        try_send(
            &mut e,
            acc(()),
            {
                let mut d = vec![1u8];
                d.extend(place_item(0, 0, 99_050, 5, 0, 0));
                d
            },
            &[&sk]
        ),
        Some(BAD_ACCOUNT),
        "revoked"
    );
    // a record bound to user A cannot authorise on user B
    let (kp2, u2) = e.user(1_000_000_000);
    let _ = kp2;
    let (a4, d4) = set_del(&u, &rec, &kp, &sk.pubkey(), e.slot + 100, 10);
    assert_eq!(try_send(&mut e, a4, d4, &[&kp]), None);
    let mut v = trade_accts(&sk, &u2, &m, &[b0]);
    v.push(r(&rec));
    assert_eq!(
        try_send(
            &mut e,
            v,
            {
                let mut d = vec![1u8];
                d.extend(place_item(0, 0, 99_050, 5, 0, 0));
                d
            },
            &[&sk]
        ),
        Some(BAD_ACCOUNT)
    );
}
