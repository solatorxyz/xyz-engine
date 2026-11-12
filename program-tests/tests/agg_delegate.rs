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

// ---- a publisher's price, brought by anyone (IX 89) ----
const BAD_DATA: u32 = 2;
const ED_ID: Address = Address::new_from_array([
    3, 125, 70, 214, 124, 147, 251, 190, 18, 249, 66, 143, 131, 141, 64, 255, 5, 112, 116, 73, 39, 244, 138, 100, 252,
    202, 112, 68, 128, 0, 0, 0,
]);
const IXS_ID: Address = Address::new_from_array([
    6, 167, 213, 23, 24, 123, 209, 102, 53, 218, 212, 4, 85, 253, 194, 192, 193, 36, 198, 143, 33, 86, 117, 165, 219,
    186, 203, 95, 8, 0, 0, 0,
]);
/// Ed25519 program instruction over `sigs`; `ix` = instruction index written in the offsets (u16::MAX = this instruction).
fn ed_ix_at(sigs: &[(&Keypair, Vec<u8>)], ix: u16) -> Instruction {
    let n = sigs.len();
    let base = 2 + 14 * n;
    let (mut d, mut body) = (vec![n as u8, 0], vec![]);
    for (kp, msg) in sigs {
        let pk = base + body.len();
        body.extend_from_slice(kp.pubkey().as_ref());
        let so = base + body.len();
        body.extend_from_slice(kp.sign_message(msg).as_ref());
        let mo = base + body.len();
        body.extend_from_slice(msg);
        for x in [so as u16, ix, pk as u16, ix, mo as u16, msg.len() as u16, ix] {
            d.extend_from_slice(&x.to_le_bytes());
        }
    }
    d.extend(body);
    Instruction { program_id: ED_ID, accounts: vec![], data: d }
}
/// What the publisher signs: the magic, the feed, then agg_post's values.
fn signed_msg(magic: &[u8; 8], feed: &Address, v: &[i64]) -> Vec<u8> {
    let mut m = magic.to_vec();
    m.extend_from_slice(feed.as_ref());
    for x in v {
        m.extend_from_slice(&x.to_le_bytes());
    }
    m
}
/// The transaction a stranger sends: the given instructions, paid and signed by the test's payer only.
fn bring(e: &mut Env, ixs: Vec<Instruction>) -> Option<u32> {
    let tx = Transaction::new_signed_with_payer(&ixs, Some(&e.admin.pubkey()), &[&e.admin], e.svm.latest_blockhash());
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

#[test]
fn a_signed_price_is_brought_by_anyone() {
    const MAGIC: &[u8; 8] = b"etaggpx1";
    let k = Keypair::new();
    let mut e = agg_env(&k);
    let (o, m, pid, ts) = (e.oracle, e.market, e.pid, e.ts);
    let post = |feeds: &[Address]| Instruction {
        program_id: pid,
        accounts: std::iter::once(r(&IXS_ID)).chain(feeds.iter().map(w)).collect(),
        data: vec![89u8],
    };
    let push = Instruction { program_id: pid, accounts: vec![w(&m), r(&o)], data: vec![4u8] };
    let px_of = |e: &Env| i64::from_le_bytes(e.svm.get_account(&o).unwrap().data[73..81].try_into().unwrap());

    // the publisher signed (price 1000, at ts); a stranger brings it and pushes it to the market in the same transaction
    let good = signed_msg(MAGIC, &o, &[1000_00000000, 10_000_000, ts, 3]);
    assert_eq!(bring(&mut e, vec![ed_ix_at(&[(&k, good.clone())], u16::MAX), post(&[o]), push.clone()]), None);
    assert_eq!((px_of(&e), e.market_state().oracle_px), (1000_00000000, 100_000));
    // the same signed value again changes nothing (and does not fail the transaction it rides in)
    let before = e.svm.get_account(&o).unwrap().data;
    assert_eq!(bring(&mut e, vec![ed_ix_at(&[(&k, good.clone())], u16::MAX), post(&[o])]), None);
    assert_eq!(e.svm.get_account(&o).unwrap().data, before);
    // a newer one moves the feed; the older one is dead from then on
    let newer = signed_msg(MAGIC, &o, &[1010_00000000, 10_000_000, ts + 1, 3]);
    assert_eq!(bring(&mut e, vec![ed_ix_at(&[(&k, newer)], u16::MAX), post(&[o])]), None);
    assert_eq!(px_of(&e), 1010_00000000);
    assert_eq!(bring(&mut e, vec![ed_ix_at(&[(&k, good.clone())], u16::MAX), post(&[o])]), None);
    assert_eq!(px_of(&e), 1010_00000000);

    // who signed: only the feed's own publisher counts
    let evil = Keypair::new();
    let forged = signed_msg(MAGIC, &o, &[1_00000000, 1, ts + 2, 3]);
    assert_eq!(bring(&mut e, vec![ed_ix_at(&[(&evil, forged.clone())], u16::MAX), post(&[o])]), Some(BAD_DATA));
    // what was signed: this feed, this magic, and nothing shorter or longer than the values
    let other = Address::new_unique();
    for bad in [
        signed_msg(MAGIC, &other, &[1011_00000000, 10_000_000, ts + 2, 3]),
        signed_msg(b"etnextpx", &o, &[1011_00000000, 10_000_000, ts + 2, 3]),
        signed_msg(MAGIC, &o, &[1011_00000000, 10_000_000, ts + 2]),
        signed_msg(MAGIC, &o, &[1011_00000000, 10_000_000, ts + 2, 3, 0]),
    ] {
        assert_eq!(bring(&mut e, vec![ed_ix_at(&[(&k, bad)], u16::MAX), post(&[o])]), Some(BAD_DATA));
    }
    // where the signature is: in the instruction right before, with its key and message in that instruction's own data
    let fresh = signed_msg(MAGIC, &o, &[1011_00000000, 10_000_000, ts + 2, 3]);
    assert_eq!(bring(&mut e, vec![post(&[o])]), Some(BAD_DATA), "no signature at all");
    assert_eq!(
        bring(&mut e, vec![ed_ix_at(&[(&k, fresh.clone())], u16::MAX), push.clone(), post(&[o])]),
        Some(BAD_DATA),
        "not right before"
    );
    assert_ne!(
        bring(&mut e, vec![ed_ix_at(&[(&k, fresh.clone())], 0), post(&[o])]),
        None,
        "offsets into another instruction"
    );
    assert_eq!(px_of(&e), 1010_00000000, "none of the above wrote the feed");

    // when: at most two seconds old, and not ahead of the clock; the bringer cannot sit on a price
    for (pt, why) in [(ts - 3, "three seconds old"), (ts + 6, "ahead of the clock")] {
        let msg = signed_msg(MAGIC, &o, &[1011_00000000, 10_000_000, pt, 3]);
        assert_eq!(bring(&mut e, vec![ed_ix_at(&[(&k, msg)], u16::MAX), post(&[o])]), Some(E_ORACLE), "{why}");
    }
    // the publisher's own post (it signs the transaction) has no such bound: staleness there is update_oracle's to judge
    e.set_time(e.slot + 20);
    let now = e.ts;
    assert!(now >= ts + 6, "the clock moved");
    assert_eq!(
        try_send(&mut e, vec![AccountMeta::new(k.pubkey(), true), w(&o)], post_old(1011_00000000, ts + 3), &[&k]),
        None
    );
    assert_eq!(px_of(&e), 1011_00000000);
    // how far: the feed's jump limit applies to a brought price as to a posted one (20% here)
    let far = signed_msg(MAGIC, &o, &[1300_00000000, 10_000_000, now, 3]);
    assert_eq!(bring(&mut e, vec![ed_ix_at(&[(&k, far)], u16::MAX), post(&[o])]), Some(BREAKER));
    // the longer form carries the external perp price too
    let ext = signed_msg(MAGIC, &o, &[1012_00000000, 10_000_000, now, 3, 1012_50000000, now]);
    assert_eq!(bring(&mut e, vec![ed_ix_at(&[(&k, ext)], u16::MAX), post(&[o])]), None);
    let d = e.svm.get_account(&o).unwrap().data;
    assert_eq!(i64::from_le_bytes(d[133..141].try_into().unwrap()), 1012_50000000);
    // a feed that is not one, or two feeds with one signature: the whole transaction fails
    let one = signed_msg(MAGIC, &o, &[1013_00000000, 10_000_000, now + 1, 3]);
    assert_eq!(bring(&mut e, vec![ed_ix_at(&[(&k, one.clone())], u16::MAX), post(&[m])]), Some(BAD_ACCOUNT));
    let o2 = Address::new_unique();
    e.prog_acct(o2, state::AGG_SIZE);
    let mut init = vec![40u8];
    init.extend_from_slice(&e.feed);
    init.extend_from_slice(&(-8i32).to_le_bytes());
    init.extend_from_slice(&0u64.to_le_bytes());
    let k2 = Keypair::new();
    assert_eq!(try_send(&mut e, vec![AccountMeta::new(k2.pubkey(), true), w(&o2)], init, &[&k2]), None);
    assert_eq!(bring(&mut e, vec![ed_ix_at(&[(&k, one.clone())], u16::MAX), post(&[o, o2])]), Some(BAD_DATA));
    assert_eq!(px_of(&e), 1012_00000000, "all or nothing");
    // two publishers' prices in one Ed25519 instruction, two feeds in one post: what a two-of-three market needs
    let two = signed_msg(MAGIC, &o2, &[1013_10000000, 10_000_000, now + 1, 3]);
    assert_eq!(bring(&mut e, vec![ed_ix_at(&[(&k2, two), (&k, one)], u16::MAX), post(&[o, o2])]), None);
    let p2 = i64::from_le_bytes(e.svm.get_account(&o2).unwrap().data[73..81].try_into().unwrap());
    assert_eq!((px_of(&e), p2), (1013_00000000, 1013_10000000));
}
fn post_old(px: i64, ts: i64) -> Vec<u8> {
    post(px, 10_000_000, ts)
}
