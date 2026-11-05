#[path = "../../program/src/state.rs"]
#[allow(dead_code)]
mod state;
// Fix-review round 5 (HEAD 3360554) proofs of concept. Each test runs the scenario against the built SBF program and asserts what
// the CURRENT code does: a PASSING test means the issue is PRESENT.
include!("common.rs");

const IOC: u8 = 4;
fn single(item: Vec<u8>) -> Vec<u8> { let mut d = vec![1u8]; d.extend_from_slice(&item); d }
fn code(r: Result<(), String>) -> Option<u32> {
    match r { Ok(()) => None, Err(s) => Some(s.split("Custom(").nth(1).and_then(|x| x.split(')').next()).and_then(|x| x.parse().ok()).unwrap_or(u32::MAX)) }
}
fn try_send(e: &mut Env, accounts: Vec<AccountMeta>, data: Vec<u8>, signers: &[&Keypair]) -> Option<u32> {
    let ix = Instruction { program_id: e.pid, accounts, data };
    let mut s: Vec<&Keypair> = vec![&e.admin]; s.extend_from_slice(signers);
    let tx = Transaction::new_signed_with_payer(&[ix], Some(&e.admin.pubkey()), &s, e.svm.latest_blockhash());
    let r = e.svm.send_transaction(tx); e.svm.expire_blockhash();
    code(match r { Ok(_) => Ok(()), Err(f) => Err(format!("{:?}\n{}", f.err, f.meta.logs.join("\n"))) })
}
/// (compute units consumed, error text if the transaction failed)
fn send_cu(e: &mut Env, ixs: Vec<Instruction>) -> (u64, Option<String>) {
    let s: Vec<&Keypair> = vec![&e.admin];
    let tx = Transaction::new_signed_with_payer(&ixs, Some(&e.admin.pubkey()), &s, e.svm.latest_blockhash());
    let r = e.svm.send_transaction(tx); e.svm.expire_blockhash();
    match r { Ok(m) => (m.compute_units_consumed, None), Err(f) => (f.meta.compute_units_consumed, Some(format!("{:?}", f.err))) }
}
fn cu_limit(n: u32) -> Instruction {
    let mut v = vec![2u8]; v.extend_from_slice(&n.to_le_bytes());
    Instruction { program_id: Address::from_str_const("ComputeBudget111111111111111111111111111111"), accounts: vec![], data: v }
}

// ---------- fast book helpers (as review4.rs) ----------
struct Mk { kp: Keypair, slab: Address }
fn fast_new(e: &mut Env, idx: u64) -> Address {
    let k = Address::new_unique(); e.prog_acct(k, FAST_SIZE);
    let mut d = vec![61u8]; d.extend_from_slice(&idx.to_le_bytes());
    let (a, m) = (e.admin.pubkey(), e.market);
    e.send("", vec![AccountMeta::new(a, true), w(&m), w(&k)], d, &[], true); k
}
fn maker(e: &mut Env, f: &Address, dep: u64, fund: i64) -> Mk {
    let (kp, u) = e.user(dep);
    let slab = Address::new_unique(); e.prog_acct(slab, SLAB_SIZE);
    e.send("", vec![AccountMeta::new(kp.pubkey(), true), r(&u), w(&slab)], vec![60], &[&kp], true);
    let (a, m) = (e.admin.pubkey(), e.market);
    e.send("", vec![AccountMeta::new(a, true), r(&m), w(f), w(&slab), AccountMeta::new_readonly(kp.pubkey(), true)], vec![62], &[&kp], true);
    if fund != 0 { let mut d = vec![64u8]; d.extend_from_slice(&fund.to_le_bytes());
        e.send("", vec![AccountMeta::new(kp.pubkey(), true), w(&u), r(&m), w(f)], d, &[&kp], true); }
    Mk { kp, slab }
}
fn quote(e: &mut Env, k: &Mk, v: &[(u16, u32, u32)]) {
    let mut d = vec![63u8]; for (i, p, q) in v { d.extend_from_slice(&i.to_le_bytes()); d.extend_from_slice(&p.to_le_bytes()); d.extend_from_slice(&q.to_le_bytes()); }
    e.send("", vec![AccountMeta::new(k.kp.pubkey(), true), w(&k.slab)], d, &[&k.kp], true);
}

// =====================================================================================================================
// R5-1 (MEDIUM, new in HEAD): the mark EMA weighs each book sample by the slots since the PREVIOUS sample (event_seq). The weight
// belongs to the time the previous book stood, but it is given to the new sample's mid, whatever the book looked like during the
// gap. update_oracle is permissionless, so whoever sends a sample chooses which book state gets the gap's weight. An attacker
// skews the impact mid for one slot (a sized bid inside the spread, rested 1 slot, cancelled right after its own sample) and
// samples 1 slot before the pusher's 10 s sample: the skewed mid takes 24 of every 25 slots of weight, the pusher's honest
// sample 1. Between samples every price-only update sets mark = oracle + EMA, so the mark follows the skew although the book
// showed it 4% of the time (a time-weighted EMA would carry ~1/25 of it).
// =====================================================================================================================
const P: i64 = 1000_00000000;
fn ema_run(attack: bool, cycles: u64) -> (i64, u64, u64) {
    let mut e = Env::new(); e.oracle_update("", P, None);
    let (b0, b1, a0) = (e.shard(0, 98_000), e.shard(0, 101_900), e.shard(1, 102_000)); let m = e.market;
    let (h, hu) = e.user(50_000_000_000); let (x, xu) = e.user(50_000_000_000);
    // honest book: 98_000 / 102_000, mid 100_000 = oracle
    e.send("", trade_accts(&h, &hu, &m, &[b0, a0]), batch_data(&[], &[place_item(0, 1, 98_000, 50, 0, 0), place_item(1, 1, 102_000, 50, 0, 0)]), &[&h], true);
    let all = [b0, b1, a0];
    e.set_time(e.slot + 2); e.oracle_update_set("", P, &all); // pusher's sample
    for c in 0..cycles {
        for s in 1..=25u64 {
            let t = e.slot + 1; e.set_time(t);
            if s == 25 { e.oracle_update_set("", P, &all); continue; } // the pusher's sample, every 25 slots (SAMPLE_S = 10 s)
            if attack && s == 24 {
                e.oracle_update_set("", P, &all); // attacker's sample: its 1-slot book gets the 23 slots since the last sample
                e.send("", trade_accts(&x, &xu, &m, &[b1]), batch_data(&[cancel_item(c + 1, 128, 101_900)], &[]), &[&x], true);
                continue;
            }
            if s % 2 == 0 || s == 23 { e.oracle_update("", P, None); } // the pusher's price-only updates
            if attack && s == 23 { // 50 lots bid at 101_900 (impact size), inside the 98_000 / 102_000 spread: mid 101_950
                e.send("", trade_accts(&x, &xu, &m, &[b1]), single(place_ex(0, 1, 0, 101_900, 50, 0, 0, c + 1, 0, 0, 0)), &[&x], true);
            }
        }
    }
    let t = e.slot + 1; e.set_time(t); e.oracle_update("", P, None); // price-only update: mark = oracle + EMA
    let ms = e.market_state(); (ms.mark_ema, ms.mark_px, ms.oracle_px)
}
#[test] fn r5_1_timed_one_slot_sample_dominates_mark_ema() {
    let (ctl, atk) = (ema_run(false, 20), ema_run(true, 20));
    println!("R5-1 control: ema {} mark {} oracle {}", ctl.0, ctl.1, ctl.2);
    println!("R5-1 attack (1 slot of every 25 at mid 101_950, sampled just before the pusher): ema {} mark {} oracle {}", atk.0, atk.1, atk.2);
    let skew = 1_950i64; let time_weighted = skew / 25;
    println!("R5-1 time-weighted share of the skew would be ~{}; got {} ({}x)", time_weighted, atk.0, atk.0 / time_weighted.max(1));
    assert!(ctl.0.abs() < 50, "control: honest book, EMA ~0");
    assert!(atk.0 <= time_weighted, "fixed: a skew present at one sample end only is not credited (ema {})", atk.0);
}

// =====================================================================================================================
// R5-2 (MEDIUM, new in HEAD): liquidate_fast now calls take() over every opposite shard before EACH maker level (up to
// LIQ_FAST_MAX_LEVELS = 24, plus the final pass and two band escalations). Every take() call re-reads all passed shards; a shard
// holding any peg/spline order (SPECIAL_BIT) is re-scanned in full (shard_best evaluates every order's effective price). With
// 16 bid shards x 64 peg bids far below the market (never fillable, cheap to rest) and a liquidation that walks maker levels,
// the per-level shard scan multiplies. The crank sends liquidations with a 400k CU limit and max_qty = u64::MAX.
// =====================================================================================================================
fn liq_cu(shards: bool, pegs: bool, max_qty: u64, cu: u32) -> (u64, Option<String>, i64) {
    let mut e = Env::new(); e.oracle_update("", P, None);
    let f = fast_new(&mut e, 0);
    let ks: Vec<Mk> = (0..8).map(|_| maker(&mut e, &f, 10_000_000_000, 1_000_000_000)).collect();
    let (tp, tu) = e.user(1_000_000_000);
    quote(&mut e, &ks[0], &[(8, 100_010, 100_000)]);
    let m = e.market;
    let mut d = vec![66u8, 0, 0, 1]; d.extend_from_slice(&(ORACLE + 100).to_le_bytes()); d.extend_from_slice(&90_000u64.to_le_bytes());
    e.send("", vec![AccountMeta::new(tp.pubkey(), true), w(&tu), r(&m), w(&f), r(&ks[0].slab)], d, &[&tp], true);
    let s = e.slot + 1; e.set_time(s); e.oracle_update("", 910_00000000, None);
    let mut sh = vec![];
    if shards { for i in 0..16u64 { sh.push(e.shard(0, 46_000 + 100 * i)); } } // peg bids anchor at oracle + offset (46_000 + 100 i)
    if pegs {
        for (i, x) in sh.clone().iter().enumerate() {
            for _ in 0..4 {
                let (kp, u) = e.user(10_000_000_000);
                for _ in 0..2 {
                    let items: Vec<Vec<u8>> = (0..8).map(|_| place_ex(0, 1, 1, 0, 1, 0, 0, 0, -45_000 + 100 * i as i64, 0, 0)).collect();
                    e.send("", trade_accts(&kp, &u, &m, &[*x]), batch_data(&[], &items), &[&kp], true);
                }
            }
            assert_eq!(e.shard_state(x).n, 64);
        }
    }
    for (i, k) in ks.iter().enumerate() {
        let v: Vec<(u16, u32, u32)> = (0..8).map(|l| (l as u16, 90_990 - (i as u32) * 10 - l as u32 * 5, 300)).collect();
        quote(&mut e, k, &v);
    }
    let ins = e.ins;
    let mut a = vec![w(&tu), r(&m), w(&ins), w(&f)]; a.extend(ks.iter().map(|k| r(&k.slab))); a.extend(sh.iter().map(w));
    let mut d = vec![70u8, 8]; d.extend_from_slice(&max_qty.to_le_bytes());
    let b0 = e.user_state(&tu).pos[0].base;
    let pid = e.pid;
    let (c, err) = send_cu(&mut e, vec![cu_limit(cu), Instruction { program_id: pid, accounts: a, data: d }]);
    (c, err, b0 - e.user_state(&tu).pos[0].base)
}
#[test] fn r5_2_liquidate_fast_shard_rescan_per_level_cu() {
    let base = liq_cu(false, false, u64::MAX, 1_400_000);
    let empty = liq_cu(true, false, u64::MAX, 1_400_000);
    let full = liq_cu(true, true, u64::MAX, 1_400_000);
    let crank = liq_cu(true, true, u64::MAX, 400_000);
    let one = liq_cu(true, true, 300, 1_400_000);
    println!("R5-2 no shards:                 {} CU, err {:?}, sold {}", base.0, base.1, base.2);
    println!("R5-2 16 empty bid shards:       {} CU, err {:?}, sold {}", empty.0, empty.1, empty.2);
    println!("R5-2 16 x 64 peg bids, 1.4M:    {} CU, err {:?}, sold {}", full.0, full.1, full.2);
    println!("R5-2 16 x 64 peg bids, 400k (crank limit): {} CU, err {:?}, sold {}", crank.0, crank.1, crank.2);
    println!("R5-2 16 x 64 peg bids, max_qty 300 (1 level): {} CU, err {:?}, sold {}", one.0, one.1, one.2);
    assert!(base.1.is_none() && empty.1.is_none());
    assert!(crank.1.is_none() && full.0 < 200_000, "fixed: the shards are scanned once, not before every maker level ({} CU)", full.0);
}

// =====================================================================================================================
// R5-3 (MEDIUM, keeper/crank.py vs program): the crank passes the rank's top TWO entries plus the largest same-side positions by
// |base| (4 in all) and never sends adl_rank (IX 25). With the top two entries stale (positions closed) and the third a valid
// small, highly leveraged counterparty, the program evicts the two stale ones, finds the third on top, not passed -> Rank. The
// evictions revert with the transaction, so the crank sends the same set forever.
// =====================================================================================================================
#[test] fn r5_3_two_stale_top_entries_block_crank_adl() {
    let mut e = Env::new(); e.oracle_update("", P, None);
    let (a0, ah) = (e.shard(1, 100_000), e.shard(1, 85_000)); let m = e.market;
    let (t, tu) = e.user(1_500_000); let (lp, lpu) = e.user(50_000_000_000);
    let deps = [3_000_000u64, 3_500_000, 1_200_000, 5_000_000_000, 5_000_000_000];
    let lots = [40u64, 40, 10, 20, 20];
    let cs: Vec<(Keypair, Address)> = deps.iter().map(|d| e.user(*d)).collect();
    for (i, (k, u)) in cs.iter().enumerate() { e.send("", trade_accts(k, u, &m, &[a0]), single(place_item(1, 1, 100_000, lots[i], 0, 0)), &[k], true); }
    e.send("", trade_accts(&t, &tu, &m, &[a0]), single(place_item(0, IOC, 100_000, 130, 0, 0)), &[&t], true);
    let mut acc = vec![w(&a0)]; acc.extend(cs.iter().map(|c| w(&c.1)));
    e.send("", acc, vec![24], &[], true);
    for p in [950, 900, 850] { e.set_time(e.slot + 1); e.oracle_update("", p * 100_000_000, None); }
    for c in cs.iter() { e.rank_user(&c.1); }
    let rk = e.rank_state();
    let order: Vec<usize> = (0..rk.n_short as usize).map(|i| cs.iter().position(|c| c.1.to_bytes() == rk.entries[RANK_N + i].user).unwrap()).collect();
    println!("R5-3 short-side rank (C index, score): {:?}", (0..rk.n_short as usize).map(|i| (order[i] + 1, rk.entries[RANK_N + i].score)).collect::<Vec<_>>());
    assert_eq!(&order[..3], &[0, 1, 2], "C1, C2, C3 lead the queue");
    // C1 and C2 buy back their shorts at 85_000: their entries are now stale
    e.send("", trade_accts(&lp, &lpu, &m, &[ah]), single(place_item(1, 1, 85_000, 80, 0, 0)), &[&lp], true);
    for i in 0..2 { let (k, u) = (&cs[i].0, cs[i].1); e.send("", trade_accts(k, &u, &m, &[ah]), single(place_item(0, IOC, 85_000, 40, 0, 0)), &[k], true); }
    let (ins, rkk) = (e.ins, e.rank);
    let adl = |e: &mut Env, who: &[usize]| { let mut a = vec![w(&tu), w(&m), w(&ins), w(&rkk)]; a.extend(who.iter().map(|&i| w(&cs[i].1))); a.extend([w(&a0), w(&ah)]); try_send(e, a, vec![6], &[]) };
    // crank: rank_top(k=2) = [C1, C2], then by |base| desc: C4 (20), C5 (20), C3 (10) -> first 4 = [C1, C2, C4, C5]
    let crank = adl(&mut e, &[0, 1, 3, 4]);
    let full = adl(&mut e, &[0, 1, 2, 3, 4]);
    println!("R5-3 ADL with the crank's set [C1,C2,C4,C5] -> {:?}; with C3 too -> {:?}", crank, full);
    assert_eq!((crank, full), (Some(20), None), "issue present: the crank's candidate set can never pass");
}
