#[path = "../../program/src/state.rs"]
#[allow(dead_code)]
mod state;
// OracleSet (IX 43): median of several independent aggregated publishers with a quorum.
include!("common.rs");
const BAD_DATA: u32 = 2; const E_ORACLE: u32 = 8;
fn try_send(e: &mut Env, accounts: Vec<AccountMeta>, data: Vec<u8>, signers: &[&Keypair]) -> Option<u32> {
    let ix = Instruction { program_id: e.pid, accounts, data };
    let mut s: Vec<&Keypair> = vec![&e.admin]; s.extend_from_slice(signers);
    let tx = Transaction::new_signed_with_payer(&[ix], Some(&e.admin.pubkey()), &s, e.svm.latest_blockhash());
    let r = e.svm.send_transaction(tx); e.svm.expire_blockhash();
    match r { Ok(_) => None, Err(f) => { let s = format!("{:?}", f.err);
        Some(s.split("Custom(").nth(1).and_then(|x| x.split(')').next()).and_then(|x| x.parse().ok()).unwrap_or(u32::MAX)) } }
}
fn post(px: i64, ts: i64) -> Vec<u8> { let mut d = vec![41u8]; for v in [px, 1_000_000, ts, 3] { d.extend_from_slice(&v.to_le_bytes()); } d }
fn member(e: &mut Env, k: &Keypair) -> Address {
    let a = Address::new_unique(); e.prog_acct(a, state::AGG_SIZE);
    let mut d = vec![40u8]; d.extend_from_slice(&e.feed); d.extend_from_slice(&(-8i32).to_le_bytes()); d.extend_from_slice(&2000u64.to_le_bytes());
    assert_eq!(try_send(e, vec![AccountMeta::new(k.pubkey(), true), w(&a)], d, &[k]), None);
    a
}
fn oset_ix(q: u8, ms: &[Address]) -> Vec<u8> { let mut d = vec![43u8, q, ms.len() as u8]; for m in ms { d.extend_from_slice(m.as_ref()); } d }
/// Market oracle becomes an OracleSet of 3 publishers, quorum 2.
fn set_env() -> (Env, Vec<Keypair>, Vec<Address>) {
    let mut e = Env::new(); let o = e.oracle; e.prog_acct(o, state::OSET_SIZE);
    let ks: Vec<Keypair> = (0..3).map(|_| Keypair::new()).collect();
    let ms: Vec<Address> = ks.iter().map(|k| member(&mut e, k)).collect();
    let a = e.admin.pubkey();
    assert_eq!(try_send(&mut e, vec![AccountMeta::new(a, true), w(&o)], oset_ix(2, &ms), &[]), None);
    (e, ks, ms)
}
fn upd(e: &mut Env, ms: &[Address]) -> Option<u32> {
    let (m, o) = (e.market, e.oracle);
    let mut acc = vec![w(&m), r(&o)]; for x in ms { acc.push(r(x)); }
    try_send(e, acc, vec![4], &[])
}
#[test] fn median_of_members_sets_price() {
    let (mut e, ks, ms) = set_env(); let ts = e.ts;
    for (i, p) in [1000_00000000i64, 1002_00000000, 1001_00000000].iter().enumerate() {
        let (k, a) = (&ks[i], ms[i]);
        assert_eq!(try_send(&mut e, vec![AccountMeta::new(k.pubkey(), true), w(&a)], post(*p, ts), &[k]), None);
    }
    assert_eq!(upd(&mut e, &ms), None);
    assert_eq!(e.market_state().oracle_px, 100_100);
}
#[test] fn one_outlier_does_not_move_the_median() {
    let (mut e, ks, ms) = set_env(); let ts = e.ts;
    for (i, p) in [1000_00000000i64, 1000_00000000, 1150_00000000].iter().enumerate() {
        let (k, a) = (&ks[i], ms[i]);
        assert_eq!(try_send(&mut e, vec![AccountMeta::new(k.pubkey(), true), w(&a)], post(*p, ts), &[k]), None);
    }
    assert_eq!(upd(&mut e, &ms), None);
    assert_eq!(e.market_state().oracle_px, 100_000);
}
#[test] fn below_quorum_is_rejected() {
    let (mut e, ks, ms) = set_env(); let ts = e.ts;
    let (k, a) = (&ks[0], ms[0]);
    assert_eq!(try_send(&mut e, vec![AccountMeta::new(k.pubkey(), true), w(&a)], post(1000_00000000, ts), &[k]), None);
    assert_eq!(upd(&mut e, &ms), Some(E_ORACLE), "1 of 3 posted, quorum 2");
    let (k, a) = (&ks[1], ms[1]);
    assert_eq!(try_send(&mut e, vec![AccountMeta::new(k.pubkey(), true), w(&a)], post(1000_00000000, ts), &[k]), None);
    assert_eq!(upd(&mut e, &ms[..1]), Some(E_ORACLE), "members not passed do not count");
    assert_eq!(upd(&mut e, &ms), None);
}
#[test] fn init_validation() {
    let mut e = Env::new(); let o = e.oracle; e.prog_acct(o, state::OSET_SIZE);
    let ks: Vec<Keypair> = (0..4).map(|_| Keypair::new()).collect();
    let ms: Vec<Address> = ks.iter().map(|k| member(&mut e, k)).collect();
    let a = e.admin.pubkey();
    assert_eq!(try_send(&mut e, vec![AccountMeta::new(a, true), w(&o)], oset_ix(2, &ms), &[]), Some(BAD_DATA), "quorum must exceed n/2");
    assert_eq!(try_send(&mut e, vec![AccountMeta::new(a, true), w(&o)], oset_ix(2, &[ms[0], ms[0], ms[1]]), &[]), Some(BAD_DATA), "duplicate member");
    assert_eq!(try_send(&mut e, vec![AccountMeta::new(a, true), w(&o)], oset_ix(3, &ms), &[]), None);
}
#[test] fn every_member_must_be_passed() {
    let (mut e, ks, ms) = set_env(); let ts = e.ts;
    for i in 0..3 { let (k, a) = (&ks[i], ms[i]); assert_eq!(try_send(&mut e, vec![AccountMeta::new(k.pubkey(), true), w(&a)], post(1000_00000000, ts), &[k]), None); }
    assert_eq!(upd(&mut e, &ms[..2]), Some(E_ORACLE), "two of three passed: refused even though the quorum is 2");
    assert_eq!(upd(&mut e, &ms), None);
}
#[test] fn two_disagreeing_members_are_unusable() {
    let (mut e, ks, ms) = set_env(); let ts = e.ts;
    // one member stale (never posted): the remaining two disagree by 3% -> conf too wide
    for (i, p) in [(0usize, 1000_00000000i64), (1, 1030_00000000)] { let (k, a) = (&ks[i], ms[i]); assert_eq!(try_send(&mut e, vec![AccountMeta::new(k.pubkey(), true), w(&a)], post(p, ts), &[k]), None); }
    assert_eq!(upd(&mut e, &ms), Some(E_ORACLE));
    // agreeing pair: price is their mean
    let (k, a) = (&ks[1], ms[1]); assert_eq!(try_send(&mut e, vec![AccountMeta::new(k.pubkey(), true), w(&a)], post(1000_20000000, ts + 1), &[k]), None);
    assert_eq!(upd(&mut e, &ms), None);
    assert_eq!(e.market_state().oracle_px, 100_010);
}
