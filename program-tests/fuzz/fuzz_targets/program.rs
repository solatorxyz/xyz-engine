#![no_main]
//! Fuzzes the DEPLOYED SBF program (perp_program.so) through LiteSVM: byte-driven sequences of real
//! instructions from 4 users (place/batch/cancel, oracle moves, liquidate, ADL rank/ADL, withdraw, triggers,
//! isolated margin). Program errors are fine; invariants after every input (after claiming everything):
//! vault == Σcash + Σiso_margin + insurance + fees, Σbase == 0, oi_long == Σ long base, insurance >= 0.
use libfuzzer_sys::fuzz_target;
#[path = "../../../program/src/state.rs"] #[allow(dead_code)] mod state;
include!("../../tests/common.rs");
fn single(item: Vec<u8>) -> Vec<u8> { let mut d = vec![1u8]; d.extend_from_slice(&item); d }
fn amt(tag: u8, v: u64) -> Vec<u8> { let mut d = vec![tag]; d.extend_from_slice(&v.to_le_bytes()); d }
fn trig(slot: u8, kind: u8, side: u8, vals: [u64; 6]) -> Vec<u8> { let mut d = vec![20u8, slot, kind, side]; for v in vals { d.extend_from_slice(&v.to_le_bytes()); } d }
fn tx(e: &mut Env, accounts: Vec<AccountMeta>, data: Vec<u8>, signers: &[&Keypair]) -> bool {
    let ix = Instruction { program_id: e.pid, accounts, data };
    let mut s: Vec<&Keypair> = vec![&e.admin]; s.extend_from_slice(signers);
    let t = Transaction::new_signed_with_payer(&[ix], Some(&e.admin.pubkey()), &s, e.svm.latest_blockhash());
    let ok = e.svm.send_transaction(t).is_ok(); e.svm.expire_blockhash(); ok
}
fuzz_target!(|data: &[u8]| {
    if data.len() < 8 { return; }
    let mut e = Env::new(); e.oracle_update("", 1000_00000000, None);
    let sh = [e.shard(0, 99_900), e.shard(0, 99_000), e.shard(1, 100_000), e.shard(1, 101_000)];
    let us: Vec<(Keypair, Address)> = (0..4).map(|i| e.user(200_000 + 3_000_000 * i)).collect();
    let (lq, lqu) = e.user(1_000_000_000);
    let m = e.market; let (ins, rk) = (e.ins, e.rank);
    let mut px: i64 = 1000_00000000;
    // structure-aware seed (V6-4): data[0] bit 0 -> user0 long 18 @100k on 200k, bids resting, oracle stepped to 935 (user0 just below MM)
    if data[0] & 1 == 1 {
        let ((k0, u0), (k1, u1), (k2, u2)) = ((&us[0].0, us[0].1), (&us[1].0, us[1].1), (&us[2].0, us[2].1));
        tx(&mut e, trade_accts(k1, &u1, &m, &sh), single(place_item(1, 1, 100_000, 40, 0, 0)), &[k1]);
        tx(&mut e, trade_accts(k0, &u0, &m, &sh), single(place_item(0, 4, 100_000, 18, 0, 0)), &[k0]);
        for (p, q) in [(99_900u64, 6u64), (99_050, 6), (99_000, 12)] { tx(&mut e, trade_accts(k2, &u2, &m, &sh), single(place_item(0, 1, p, q, 0, 0)), &[k2]); }
        for p2 in [970_00000000i64, 950_00000000, 935_00000000] { let s = e.slot + 1; e.set_time(s); e.ts += 1; e.set_pyth(p2, (p2 / 10000) as u64);
            let mut a = vec![w(&m), r(&e.oracle)]; a.extend(sh.iter().map(r)); tx(&mut e, a, vec![4], &[]); px = p2; }
    }
    let stats = std::env::var("LSVM_STATS").is_ok(); let mut okc = [(0u32, 0u32); 14];
    for c in data.chunks_exact(8) {
        let op = (c[0] % 14) as usize; let ok0 = (e.cu.len(), e.svm.latest_blockhash());
        let _ = ok0;
        let (kp, u) = (&us[(c[1] % 4) as usize].0, us[(c[1] % 4) as usize].1);
        let price = 98_000 + (u16::from_le_bytes([c[2], c[3]]) % 4_000) as u64 / 10 * 10;
        let qty = 1 + c[4] as u64 % 40; let fl = c[5] & 0b0001_1111; let side = c[6] & 1;
        let r = match op {
            0..=3 => tx(&mut e, trade_accts(kp, &u, &m, &sh), single(place_item(side, fl, price, qty, 0, if c[7] & 1 == 1 { 300 } else { 0 })), &[kp]),
            4 => { // replace: cancel all of the user's orders by cid range + place 2
                let d = batch_data(&[cancel_item((c[7] % 4) as u64, side | 128, 0)], &[place_ex(side, 1, 0, price, qty, 0, 0, (c[7] % 4) as u64, 0, 0, 0), place_ex(1 - side, 1, 0, price + 200, qty, 0, 0, 0, 0, 0, 0)]);
                tx(&mut e, trade_accts(kp, &u, &m, &sh), d, &[kp]) }
            5 => { px = (px + (c[7] as i64 - 128) * 1_0000000).clamp(500_00000000, 1500_00000000); // <= 1.28%/step (jump guard 5%)
                   let s = e.slot + 1 + (c[2] % 5) as u64; e.set_time(s); e.ts += 1; e.set_pyth(px, (px / 10000) as u64);
                   let mut a = vec![w(&m), r(&e.oracle)]; if c[3] & 1 == 1 { a.extend(sh.iter().map(r)); } tx(&mut e, a, vec![4], &[]) }
            6 => { let mut a = vec![AccountMeta::new(lq.pubkey(), true), w(&lqu), w(&u), r(&m), w(&ins)]; a.extend(sh.iter().map(w)); tx(&mut e, a, amt(5, qty), &[&lq]) }
            7 => tx(&mut e, vec![r(&m), w(&rk), w(&u)], vec![25], &[]),
            8 => { let mut a = vec![w(&u), w(&m), w(&ins), w(&rk)]; a.extend(us.iter().map(|x| w(&x.1))); tx(&mut e, a, vec![6], &[]) }
            9 => { let s = Address::new_unique(); let mint = Address::from(e.market_state().mint);
                   e.svm.set_account(s, Account { lamports: 1_000_000_000, data: token_acct(&mint, &kp.pubkey(), 0), owner: TOKEN, executable: false, rent_epoch: 0 }).unwrap();
                   let mut a = vec![AccountMeta::new(kp.pubkey(), true), w(&u), r(&m), w(&e.vault), w(&s), r(&TOKEN), r(&e.vault_pda)]; a.extend(sh.iter().map(w));
                   tx(&mut e, a, amt(14, qty * 10_000), &[kp]) }
            10 => { tx(&mut e, vec![AccountMeta::new(kp.pubkey(), true), w(&u), r(&m)], vec![30, c[7] & 1], &[kp]);
                    tx(&mut e, vec![AccountMeta::new(kp.pubkey(), true), w(&u), r(&m)], amt(31, qty * 5_000), &[kp]) }
            11 | 13 => { // 13: mark-based trigger (bit 6), optionally oracle-confirmed (bit 7)
                   let kind = (1 + c[2] % 3) | if op == 13 { 0x40 | (c[3] & 0x80) } else { 0 };
                   tx(&mut e, vec![AccountMeta::new(kp.pubkey(), true), w(&u), r(&m)], trig(c[7] % 4, kind, side, [price, price, qty, 3, 1, 2]), &[kp]);
                   let mut a = vec![AccountMeta::new(lq.pubkey(), true), w(&lqu), w(&u), r(&m)]; a.extend(sh.iter().map(w)); tx(&mut e, a, vec![21, c[7] % 4], &[&lq]) }
            _ => { // 12: permissionless book liquidation (all shards, or a subset -> NoFill proof missing)
                   let mut a = vec![w(&u), r(&m), w(&ins)]; let k = if c[7] & 1 == 0 { 4 } else { 1 + (c[7] as usize >> 1) % 4 };
                   a.extend(sh[..k].iter().map(w)); tx(&mut e, a, amt(34, qty), &[]) }
        };
        okc[op].1 += 1; if r { okc[op].0 += 1; }
    }
    if stats { eprintln!("ok/total per op: {:?}", okc); }
    // settle everything, then check conservation
    let all: Vec<Address> = us.iter().map(|x| x.1).chain([lqu]).collect();
    for (kp, u) in us.iter().chain([(lq.insecure_clone(), lqu)].iter()) {
        tx(&mut e, trade_accts(kp, u, &m, &sh), batch_data(&[], &[]), &[kp]);
        tx(&mut e, vec![w(u), r(&m)], vec![15], &[]);
    }
    let mut agg = vec![w(&m), w(&ins)]; agg.extend(sh.iter().map(w)); tx(&mut e, agg, vec![16], &[]);
    let st: Vec<User> = all.iter().map(|u| e.user_state(u)).collect();
    let cash: i128 = st.iter().map(|s| s.cash as i128 + s.pos.iter().map(|p| p.iso_margin as i128).sum::<i128>()).sum();
    let base: i64 = st.iter().map(|s| s.pos.iter().map(|p| p.base).sum::<i64>()).sum();
    let longs: i64 = st.iter().map(|s| s.pos.iter().map(|p| p.base.max(0)).sum::<i64>()).sum();
    let ms = e.market_state();
    assert_eq!(base, 0, "net base");
    assert!(e.ins_bal() >= 0, "insurance negative");
    assert_eq!(ms.oi_long as i64, longs, "oi");
    // bad debt is value that left the system on paper: vault + bad_debt covers all claims
    assert_eq!(cash + e.ins_bal() as i128 + ms.fees as i128, e.vault_amount() as i128 + ms.bad_debt as i128, "conservation");
});
