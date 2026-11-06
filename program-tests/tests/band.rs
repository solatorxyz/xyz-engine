#[path = "../../program/src/state.rs"]
#[allow(dead_code)]
mod state;
// Oracle band (spec::BAND_MAX_BPS): the oracle's confidence (an OracleSet's publisher dispersion) is added to the initial and
// maintenance requirements, so margin tightens by itself while sources disagree. Equity stays at the mark.
include!("common.rs");

const E_MARGIN: u32 = 3; const E_NOT_LIQ: u32 = 5;

fn try_send(e: &mut Env, accounts: Vec<AccountMeta>, data: Vec<u8>, signers: &[&Keypair]) -> Option<u32> {
    let ix = Instruction { program_id: e.pid, accounts, data };
    let mut s: Vec<&Keypair> = vec![&e.admin]; s.extend_from_slice(signers);
    let tx = Transaction::new_signed_with_payer(&[ix], Some(&e.admin.pubkey()), &s, e.svm.latest_blockhash());
    let r = e.svm.send_transaction(tx); e.svm.expire_blockhash();
    match r { Ok(_) => None, Err(f) => { let s = format!("{:?}", f.err);
        Some(s.split("Custom(").nth(1).and_then(|x| x.split(')').next()).and_then(|x| x.parse().ok()).unwrap_or(u32::MAX)) } }
}
fn single(item: Vec<u8>) -> Vec<u8> { let mut d = vec![1u8]; d.extend_from_slice(&item); d }
/// oracle at `px` (1e-8 units) with confidence `conf_bps` of the price, applied through IX 4
fn oracle(e: &mut Env, px: i64, conf_bps: i64) {
    let s = e.slot + 2; e.set_time(s);
    e.set_pyth(px, (px * conf_bps / 10000) as u64);
    let (m, o) = (e.market, e.oracle);
    assert_eq!(try_send(e, vec![w(&m), r(&o)], vec![4], &[]), None, "oracle update");
}
/// taker `dep` tries a 10x long of 10 lots at 1000 against a resting ask; returns the placement result
fn open_long(e: &mut Env, dep: u64, conf_bps: i64) -> Option<u32> {
    oracle(e, 1000_00000000, conf_bps);
    let a0 = e.shard(1, 100_000);
    let (mk, mu) = e.user(100_000_000_000); let (t, tu) = e.user(dep);
    let m = e.market;
    e.send("", trade_accts(&mk, &mu, &m, &[a0]), single(place_item(1, 1, 100_000, 10, 0, 0)), &[&mk], true);
    try_send(e, trade_accts(&t, &tu, &m, &[a0]), single(place_item(0, 4, 100_000, 10, 0, 0)), &[&t])
}
#[test] fn initial_margin_widens_with_the_oracle_band() {
    // notional 1,000,000; IM 100,000 (10x). Deposit = IM + 0.3%: fine when the oracle is tight (1 bp), refused at 40 bps.
    let mut e = Env::new(); assert_eq!(open_long(&mut e, 103_000, 1), None, "tight oracle: 0.3% slack is enough");
    let mut e = Env::new(); assert_eq!(open_long(&mut e, 103_000, 40), Some(E_MARGIN), "40 bps band: requirement 104,000");
    let mut e = Env::new(); assert_eq!(open_long(&mut e, 104_600, 40), None, "with the band covered it opens");
}
#[test] fn band_is_capped() {
    // 90 bps confidence (under the market's 100 bps limit) counts as 50 bps: IM + 0.6% is enough, IM + 0.4% is not
    let mut e = Env::new(); assert_eq!(open_long(&mut e, 106_000, 90), None);
    let mut e = Env::new(); assert_eq!(open_long(&mut e, 104_000, 90), Some(E_MARGIN));
}
#[test] fn maintenance_trigger_widens_but_equity_does_not_move() {
    // 10 lots long from 1000 with 106,000: at 946 equity = 106,000 - fee - 54,000 ~ 51,500; mm = 47,300 (+4,730 at 50 bps)
    let mut e = Env::new(); assert_eq!(open_long(&mut e, 106_000, 1), None);
    let mut e = Env::new(); oracle(&mut e, 1000_00000000, 1);
    let a0 = e.shard(1, 100_000); let b0 = e.shard(0, 94_000);
    let (mk, mu) = e.user(100_000_000_000); let (t, tu) = e.user(106_000); let m = e.market;
    e.send("", trade_accts(&mk, &mu, &m, &[a0]), single(place_item(1, 1, 100_000, 10, 0, 0)), &[&mk], true);
    assert_eq!(try_send(&mut e, trade_accts(&t, &tu, &m, &[a0]), single(place_item(0, 4, 100_000, 10, 0, 0)), &[&t]), None);
    e.send("", trade_accts(&mk, &mu, &m, &[b0]), single(place_item(0, 1, 94_000, 10, 0, 0)), &[&mk], true); // bid to liquidate into
    // price down in confirmed steps (jump guard): 975, 946
    for px in [975i64, 946] { e.oracle_update("step", px * 100_000_000, None); }
    let eq_at_mark = { let s = e.user_state(&tu); let mp = e.market_state().mark_px as i128; s.cash as i128 + s.pos[0].base as i128 * mp };
    assert!(eq_at_mark > 47_300 && eq_at_mark < 52_030, "scenario sits between mm and mm + band: {eq_at_mark}");
    let (ins, mk_) = (Address::from(e.market_state().insurance), e.market);
    let liq = |e: &mut Env| { let mut d = vec![34u8]; d.extend_from_slice(&u64::MAX.to_le_bytes()); try_send(e, vec![w(&tu), r(&mk_), w(&ins), w(&b0), w(&a0)], d, &[]) };
    assert_eq!(liq(&mut e), Some(E_NOT_LIQ), "tight oracle: healthy at the mark");
    oracle(&mut e, 946_00000000, 50); // sources disagree by 0.5%: maintenance requirement 47,300 -> 52,030
    let eq2 = { let s = e.user_state(&tu); let mp = e.market_state().mark_px as i128; s.cash as i128 + s.pos[0].base as i128 * mp };
    assert_eq!(eq2, eq_at_mark, "equity at the mark is unchanged by the band");
    assert_eq!(liq(&mut e), None, "wide band: liquidatable, closed into the book");
    let s = e.user_state(&tu); assert!(s.pos[0].base < 10 || s.pos[0].market == [0; 32], "position reduced");
}
