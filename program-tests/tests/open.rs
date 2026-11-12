// A market on its own (no pool), as anyone may open one: its bond, its fees, and the floor under its margin.
#[path = "../../program/src/state.rs"]
#[allow(dead_code)]
mod state;
include!("common.rs");

fn place(e: &mut Env, kp: &Keypair, u: &Address, shards: &[Address], side: u8, fl: u8, px: u64, q: u64) {
    let m = e.market;
    let mut d = vec![1u8];
    d.extend_from_slice(&place_item(side, fl, px, q, 0, 0));
    e.send("", trade_accts(kp, u, &m, shards), d, &[kp], true);
}
fn token(e: &mut Env, owner: &Address, amount: u64) -> Address {
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
fn token_amount(e: &Env, k: &Address) -> u64 {
    u64::from_le_bytes(e.data(k)[64..72].try_into().unwrap())
}
fn fund(e: &mut Env, payer: &Keypair, src: &Address, amt: u64, ok: bool) {
    let mut d = vec![90u8];
    d.extend_from_slice(&amt.to_le_bytes());
    let (m, ins, v) = (e.market, e.ins, e.vault);
    e.send(
        "ins_fund",
        vec![AccountMeta::new(payer.pubkey(), true), r(&m), w(&ins), w(&v), w(src), r(&TOKEN)],
        d,
        &[payer],
        ok,
    );
}
fn claim(e: &mut Env, who: &Keypair, dst: &Address, ok: bool) {
    let (m, ins, v, pda) = (e.market, e.ins, e.vault, e.vault_pda);
    e.send(
        "fee_claim",
        vec![AccountMeta::new(who.pubkey(), true), w(&m), w(&ins), w(&v), w(dst), r(&TOKEN), r(&pda)],
        vec![91u8],
        &[who],
        ok,
    );
}
fn propose(e: &mut Env, op: u8, v: u64, ok: bool) {
    let (a, m) = (e.admin.pubkey(), e.market);
    let mut d = vec![28u8, op];
    d.extend_from_slice(&v.to_le_bytes());
    e.send("", vec![AccountMeta::new(a, true), w(&m)], d, &[], ok);
}

#[test]
fn a_bond_is_paid_into_the_insurance_and_stays_in_the_vault() {
    let mut e = Env::new();
    let (v0, i0) = (e.vault_amount(), e.ins_bal());
    assert_eq!(i0, 0, "a market's insurance starts empty");
    // anybody may pay: the payer here is not the market's admin
    let payer = Keypair::new();
    e.svm.airdrop(&payer.pubkey(), 1_000_000_000).unwrap();
    let src = token(&mut e, &payer.pubkey(), 5_000_000_000);
    fund(&mut e, &payer, &src, 2_000_000_000, true);
    assert_eq!(e.ins_bal(), 2_000_000_000);
    assert_eq!(e.vault_amount(), v0 + 2_000_000_000, "the tokens are in the market's vault");
    assert_eq!(token_amount(&e, &src), 3_000_000_000);
    // again adds up; nothing is refused but nothing
    fund(&mut e, &payer, &src, 1, true);
    assert_eq!(e.ins_bal(), 2_000_000_001);
    fund(&mut e, &payer, &src, 0, false);
    fund(&mut e, &payer, &src, 4_000_000_000, false); // more than the payer has: the token program refuses
    assert_eq!(e.ins_bal(), 2_000_000_001);
    // the insurance and the vault must be this market's own
    let (m, ins, v) = (e.market, e.ins, e.vault);
    let other = token(&mut e, &payer.pubkey(), 0);
    let mut d = vec![90u8];
    d.extend_from_slice(&1u64.to_le_bytes());
    e.send(
        "",
        vec![AccountMeta::new(payer.pubkey(), true), r(&m), w(&ins), w(&other), w(&src), r(&TOKEN)],
        d.clone(),
        &[&payer],
        false,
    );
    let fake = Address::new_unique();
    e.prog_acct(fake, INS_SIZE);
    e.send(
        "",
        vec![AccountMeta::new(payer.pubkey(), true), r(&m), w(&fake), w(&v), w(&src), r(&TOKEN)],
        d,
        &[&payer],
        false,
    );
    // a deposit cap is a limit on traders' money: a bond is not counted against it
    let a = e.admin.pubkey();
    let mut d = vec![22u8, 2];
    d.extend_from_slice(&1u64.to_le_bytes());
    e.send("", vec![AccountMeta::new(a, true), w(&m)], d, &[], true);
    fund(&mut e, &payer, &src, 1_000, true);
    assert_eq!(e.ins_bal(), 2_000_001_001);
}

#[test]
fn a_markets_fees_go_half_to_its_creator_and_half_to_its_insurance() {
    let mut e = Env::new();
    let b = e.shard(0, 100_000);
    let a = e.shard(1, 100_000);
    let (mk, mu) = e.user(10_000_000_000);
    let (tk, tu) = e.user(10_000_000_000);
    e.oracle_update("", 100_000_000_000, None);
    // a maker rests a bid for 1,000 lots at 100,050; a taker sells into it: notional 100,050,000, taker fee 5 bps = 50,025
    place(&mut e, &mk, &mu, &[b], 0, 1, 100_050, 1_000);
    place(&mut e, &tk, &tu, &[a, b], 1, 4, 100_050, 1_000);
    let m = e.market;
    e.send("", vec![w(&m), w(&a), w(&b)], vec![16], &[], true);
    let fees = e.market_state().fees;
    assert_eq!(fees, 50_025, "what the taker paid");
    let admin = e.admin.insecure_clone();
    let dst = token(&mut e, &admin.pubkey(), 0);
    let v0 = e.vault_amount();
    // nobody but the market's admin
    let stranger = Keypair::new();
    e.svm.airdrop(&stranger.pubkey(), 1_000_000_000).unwrap();
    let sdst = token(&mut e, &stranger.pubkey(), 0);
    claim(&mut e, &stranger, &sdst, false);
    assert_eq!(e.market_state().fees, 50_025);
    claim(&mut e, &admin, &dst, true);
    // 50,025 / 2 = 25,012.5: the odd unit stays with the insurance
    assert_eq!(token_amount(&e, &dst), 25_012);
    assert_eq!(e.ins_bal(), 25_013);
    assert_eq!(e.vault_amount(), v0 - 25_012, "only the creator's half leaves the vault");
    assert_eq!(e.market_state().fees, 0);
    // nothing left to claim
    claim(&mut e, &admin, &dst, false);
    // The vault still holds exactly what it owes. The taker's cash has the sale and the fee in it; the maker's fill
    // is booked when its account is next touched, so its cash is written here as it will be: the deposit less the cost.
    let taker = e.user_state(&tu).cash as i128;
    assert_eq!(taker, 10_000_000_000 + 100_050_000 - 50_025);
    let maker = 10_000_000_000i128 - 100_050_000;
    assert_eq!(e.vault_amount() as i128, taker + maker + e.ins_bal() as i128, "vault = traders' cash + insurance");
    // a pooled market's fees sit in a vault it shares: not paid out here
    place(&mut e, &mk, &mu, &[b], 0, 1, 100_050, 1_000);
    place(&mut e, &tk, &tu, &[a, b], 1, 4, 100_050, 1_000);
    e.send("", vec![w(&m), w(&a), w(&b)], vec![16], &[], true);
    assert!(e.market_state().fees > 0);
    let mut ms = e.market_state();
    ms.pool = [7u8; 32];
    let mut acc = e.svm.get_account(&m).unwrap();
    acc.data.copy_from_slice(unsafe {
        std::slice::from_raw_parts(&ms as *const Market as *const u8, std::mem::size_of::<Market>())
    });
    e.svm.set_account(m, acc).unwrap();
    claim(&mut e, &admin, &dst, false);
}

#[test]
fn margin_has_the_same_floor_in_a_market_on_its_own() {
    let mut e = Env::new();
    assert_eq!(e.market_state().pool, [0u8; 32], "no pool");
    assert_eq!((e.market_state().imr_bps, e.market_state().mmr_bps), (1000, 500));
    // ADMIN_IMR 4, ADMIN_MMR 5: below the defaults is refused when proposed, in a market on its own as in a pooled one
    propose(&mut e, 4, 999, false);
    propose(&mut e, 5, 499, false);
    propose(&mut e, 4, 600, false);
    assert_eq!(e.market_state().pending_op, 0);
    // stricter is allowed, after the wait
    propose(&mut e, 4, 2000, true);
    let (m, s) = (e.market, e.slot + 216_000);
    e.set_time(s);
    e.send("", vec![w(&m)], vec![29], &[], true);
    assert_eq!(e.market_state().imr_bps, 2000);
    // and a value already waiting when this build arrived cannot slip through at execute
    let mut ms = e.market_state();
    ms.pending_op = 5;
    ms.pending_val = 100;
    ms.pending_eta = e.slot;
    let mut acc = e.svm.get_account(&m).unwrap();
    acc.data.copy_from_slice(unsafe {
        std::slice::from_raw_parts(&ms as *const Market as *const u8, std::mem::size_of::<Market>())
    });
    e.svm.set_account(m, acc).unwrap();
    e.send("", vec![w(&m)], vec![29], &[], false);
    assert_eq!(e.market_state().mmr_bps, 500);
    // a market on its own can still be handed to another admin (ADMIN_SET_ADMIN 8), which a pooled one cannot
    let mut ms = e.market_state();
    ms.pending_op = 0;
    let mut acc = e.svm.get_account(&m).unwrap();
    acc.data.copy_from_slice(unsafe {
        std::slice::from_raw_parts(&ms as *const Market as *const u8, std::mem::size_of::<Market>())
    });
    e.svm.set_account(m, acc).unwrap();
    let next = Address::new_unique();
    let a = e.admin.pubkey();
    let mut d = vec![28u8, 8];
    d.extend_from_slice(&0u64.to_le_bytes());
    d.extend_from_slice(next.as_ref());
    e.send("", vec![AccountMeta::new(a, true), w(&m)], d, &[], true);
    assert_eq!(e.market_state().pending_key, next.to_bytes());
}
