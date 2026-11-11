#[path = "../../program/src/state.rs"]
#[allow(dead_code)]
mod state;
// Withdrawal lane (from percolator v16 `Eq_withdraw` / `Eq_no_positive_credit`): collateral can only leave against equity
// without positive unrealized PnL. A mark that moved in the account's favour (manipulated or just briefly wrong) can't be
// turned into a withdrawal of principal; realized PnL and excess margin can be withdrawn, for longs and shorts alike.
include!("common.rs");

const E_MARGIN: u32 = 3;

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
fn withdraw(e: &mut Env, kp: &Keypair, u: &Address, v: u64) -> Option<u32> {
    let s = Address::new_unique();
    let mint = Address::from(e.market_state().mint);
    e.svm
        .set_account(
            s,
            Account {
                lamports: 1_000_000_000,
                data: token_acct(&mint, &kp.pubkey(), 0),
                owner: TOKEN,
                executable: false,
                rent_epoch: 0,
            },
        )
        .unwrap();
    let (m, vt, vp) = (e.market, e.vault, e.vault_pda);
    try_send(
        e,
        vec![AccountMeta::new(kp.pubkey(), true), w(u), r(&m), w(&vt), w(&s), r(&TOKEN), r(&vp)],
        amt(14, v),
        &[kp],
    )
}
/// taker opens 10 lots at 1000 (notional 1,000,000; IM 100,000) with `dep`, `long` or short. Returns (taker, user, maker, maker user, shard used).
fn open(e: &mut Env, dep: u64, long: bool) -> (Keypair, Address, Keypair, Address, Address) {
    e.oracle_update("", 1000_00000000, None);
    let (maker_side, taker_side) = if long { (1u8, 0u8) } else { (0u8, 1u8) };
    let sh = e.shard(maker_side, 100_000);
    let (mk, mu) = e.user(100_000_000_000);
    let (t, tu) = e.user(dep);
    let m = e.market;
    e.send("", trade_accts(&mk, &mu, &m, &[sh]), single(place_item(maker_side, 1, 100_000, 10, 0, 0)), &[&mk], true);
    assert_eq!(
        try_send(e, trade_accts(&t, &tu, &m, &[sh]), single(place_item(taker_side, 4, 100_000, 10, 0, 0)), &[&t]),
        None,
        "open"
    );
    (t, tu, mk, mu, sh)
}
fn eq_at_mark(e: &Env, u: &Address) -> i128 {
    let s = e.user_state(u);
    let mp = e.market_state().mark_px as i128;
    s.cash as i128 + s.pos[0].base as i128 * mp
}

#[test]
fn short_cannot_withdraw_principal_against_an_unrealized_gain() {
    let mut e = Env::new();
    let (t, tu, ..) = open(&mut e, 200_000, false);
    for px in [975i64, 950, 925, 900] {
        e.oracle_update("down", px * 100_000_000, None);
    } // short gains ~100,000
    let eq = eq_at_mark(&e, &tu);
    assert!(eq > 290_000, "unrealized gain counted at the mark: {eq}");
    // old lane: eq - IM(90,000) ~ 200,000 withdrawable. New lane: principal ~200,000 - fee - IM 90,000 ~ 109,000
    assert_eq!(withdraw(&mut e, &t, &tu, 150_000), Some(E_MARGIN), "gain is not withdrawable collateral while open");
    assert_eq!(withdraw(&mut e, &t, &tu, 100_000), None, "excess principal is");
}
#[test]
fn long_can_withdraw_excess_margin() {
    // a long's cash is negative (it paid the notional); the old `amt <= cash` rule locked all of its excess margin
    let mut e = Env::new();
    let (t, tu, ..) = open(&mut e, 300_000, true);
    assert!(e.user_state(&tu).cash < 0);
    assert_eq!(withdraw(&mut e, &t, &tu, 150_000), None, "300,000 - fee - IM 100,000 leaves ~199,000");
    assert_eq!(withdraw(&mut e, &t, &tu, 60_000), Some(E_MARGIN), "but not below the initial requirement");
}
#[test]
fn long_gain_withdrawable_after_it_is_realized() {
    let mut e = Env::new();
    let (t, tu, mk, mu, _) = open(&mut e, 200_000, true);
    for px in [1025i64, 1050, 1075, 1100] {
        e.oracle_update("up", px * 100_000_000, None);
    }
    assert_eq!(withdraw(&mut e, &t, &tu, 150_000), Some(E_MARGIN), "unrealized +100,000 does not count");
    // close into a bid at 1100: the gain becomes cash
    let b = e.shard(0, 110_000);
    let m = e.market;
    e.send("", trade_accts(&mk, &mu, &m, &[b]), single(place_item(0, 1, 110_000, 10, 0, 0)), &[&mk], true);
    assert_eq!(
        try_send(&mut e, trade_accts(&t, &tu, &m, &[b]), single(place_item(1, 4 | 2, 110_000, 10, 0, 0)), &[&t]),
        None,
        "close"
    );
    let c = e.user_state(&tu).cash;
    assert!(c > 280_000, "realized: {c}");
    assert_eq!(withdraw(&mut e, &t, &tu, 280_000), None, "realized gain withdraws");
}
#[test]
fn no_positions_is_exactly_cash() {
    let mut e = Env::new();
    e.oracle_update("", 1000_00000000, None);
    let (t, tu) = e.user(50_000);
    assert_eq!(withdraw(&mut e, &t, &tu, 50_001), Some(E_MARGIN));
    assert_eq!(withdraw(&mut e, &t, &tu, 50_000), None);
    assert_eq!(e.user_state(&tu).cash, 0);
}
