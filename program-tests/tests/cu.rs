#[path = "../../program/src/state.rs"]
#[allow(dead_code)]
mod state;
// CU measurement + invariant scenarios on the built SBF program.
include!("common.rs");
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

#[test]
fn compute_units_and_invariants() {
    let mut e = Env::new();
    let (b0, b1) = (e.shard(0, 99_900), e.shard(0, 99_800));
    let (a0, a1, a2) = (e.shard(1, 100_000), e.shard(1, 100_100), e.shard(1, 100_200));
    let all = [b0, b1, a0, a1, a2];
    e.oracle_update("update_oracle (Pyth parse+conf+staleness, funding accrue)", 1000_00000000, None);
    let dep_mm = 50_000_000_000u64;
    let dep_t = 2_000_000_000u64;
    let (mm, mmu) = e.user(dep_mm);
    let (tk, tku) = e.user(dep_t);
    let (lq, lqu) = e.user(1_000_000_000);
    let (vic, vicu) = e.user(150_000); // will be leveraged ~9x
    let m = e.market;

    // MM: 5 levels x 2 sides post-only with oracle band 15bps + expiry
    let exp = e.slot + 50;
    let quotes = |off: u64| -> Vec<Vec<u8>> {
        let mut v = vec![];
        for l in 0..5u64 {
            v.push(place_item(0, 1, ORACLE - 20 - l * 40 - off, 100, exp, 150));
            v.push(place_item(1, 1, ORACLE + 10 + l * 50 + off, 100, exp, 150));
        }
        v
    };
    let single = place_item(0, 1, ORACLE - 150, 10, 0, 0);
    let mut d1 = vec![1u8];
    d1.extend_from_slice(&single);
    e.send("place 1 post-only (resting, no match)", trade_accts(&mm, &mmu, &m, &[b1, a0]), d1, &[&mm], true);
    e.send(
        "batch place 10 (5 bid + 5 ask levels)",
        trade_accts(&mm, &mmu, &m, &all),
        batch_data(&[], &quotes(0)),
        &[&mm],
        true,
    );
    // collect live ids from shard accounts (same decode the C++ bot does)
    let live = |e: &Env| -> Vec<Vec<u8>> {
        let mut c = vec![];
        for s in all {
            let st = e.shard_state(&s);
            for o in &st.orders[..st.n as usize] {
                if o.owner == mmu.to_bytes() && o.qty == 100 {
                    c.push(cancel_item(o.id, st.side as u8, o.price));
                }
            }
        }
        c
    };
    let c = live(&e);
    assert_eq!(c.len(), 10);
    e.send(
        "batch replace (cancel 10 + place 10)",
        trade_accts(&mm, &mmu, &m, &all),
        batch_data(&c, &quotes(1)),
        &[&mm],
        true,
    );
    let c = live(&e);
    assert_eq!(c.len(), 10);
    e.send("batch cancel 10", trade_accts(&mm, &mmu, &m, &all), batch_data(&c, &[]), &[&mm], true);
    e.send("", trade_accts(&mm, &mmu, &m, &all), batch_data(&[], &quotes(0)), &[&mm], true);

    // taker IOC buy sweeping 5 ask levels across 3 shards (asks at 100010,100060,100110,100160,100210)
    let mut d = vec![1u8];
    d.extend_from_slice(&place_item(0, 4, ORACLE + 300, 500, 0, 0));
    e.send("taker IOC: match 5 levels across 3 shards", trade_accts(&tk, &tku, &m, &[a0, a1, a2]), d, &[&tk], true);
    let t = e.user_state(&tku);
    assert_eq!(t.pos[0].base, 500);
    let mut d = vec![1u8];
    d.extend_from_slice(&place_item(1, 4, ORACLE - 1000, 100, 0, 0));
    e.send("taker IOC: match 1 level", trade_accts(&tk, &tku, &m, &[b0]), d, &[&tk], true);

    // margin: taker can't withdraw beyond initial margin
    let tsrc = Address::new_unique();
    let mint = Address::from(e.market_state().mint);
    e.svm
        .set_account(
            tsrc,
            Account {
                lamports: 1_000_000_000,
                data: token_acct(&mint, &tk.pubkey(), 0),
                owner: TOKEN,
                executable: false,
                rent_epoch: 0,
            },
        )
        .unwrap();
    let wd = |amt: u64| {
        let mut d = vec![14u8];
        d.extend_from_slice(&amt.to_le_bytes());
        d
    };
    let (vp, ve) = (e.vault_pda, e.vault);
    let wacc = |kp: &Keypair, u: &Address| {
        vec![AccountMeta::new(kp.pubkey(), true), w(u), r(&m), w(&ve), w(&tsrc), r(&TOKEN), r(&vp)]
    };
    e.send("withdraw too much (rejected by margin)", wacc(&tk, &tku), wd(dep_t - 1000), &[&tk], false);
    e.send("withdraw (margin check + PDA-signed SPL CPI)", wacc(&tk, &tku), wd(100_000_000), &[&tk], true);

    // MM claims fills lazily on next batch
    let mm_before = e.user_state(&mmu);
    let c = live(&e);
    e.send(
        "batch replace + claim maker fills (3 shards w/ ledgers)",
        trade_accts(&mm, &mmu, &m, &all),
        batch_data(&c, &quotes(0)),
        &[&mm],
        true,
    );
    let mm_after = e.user_state(&mmu);
    assert_eq!(mm_before.pos[0].base, 0);
    assert_eq!(mm_after.pos[0].base, -400);

    // victim goes ~9x long (IMR 10%) buying from MM asks
    let mut d = vec![1u8];
    d.extend_from_slice(&place_item(0, 4, ORACLE + 300, 13, 0, 0));
    e.send("", trade_accts(&vic, &vicu, &m, &[a0, a1, a2]), d, &[&vic], true);
    let mut d = vec![1u8];
    d.extend_from_slice(&place_item(0, 4, ORACLE + 300, 50, 0, 0));
    e.send("over-leverage rejected (IMR)", trade_accts(&vic, &vicu, &m, &[a0, a1, a2]), d, &[&vic], false);

    // oracle drops 8%: victim below maintenance. MM requotes around the new price with wide band.
    e.set_time(e.slot + 1);
    e.oracle_update_set("", 960_00000000, &all); // two <5% steps (single-source jump guard, V6-2)
    e.set_time(e.slot + 1);
    e.oracle_update_set("update_oracle + premium/mark from full shard set (5 shards)", 920_00000000, &all);
    assert!(try_partial(&mut e, &[b0, a0]), "partial shard set must be rejected (N-1)");
    let mm_low: Vec<Vec<u8>> = (0..5u64).map(|l| place_item(0, 1, 91_990 - l * 40, 100, 0, 0)).collect();
    let mut bshards = vec![e.shard(0, 91_900), e.shard(0, 91_800)];
    let c = live(&e);
    let mut acc = all.to_vec();
    acc.extend_from_slice(&bshards);
    e.send("", trade_accts(&mm, &mmu, &m, &acc), batch_data(&c, &mm_low), &[&mm], true);
    let mut la = vec![AccountMeta::new(lq.pubkey(), true), w(&lqu), w(&vicu), r(&m), w(&e.ins)];
    la.extend(bshards.iter().map(w));
    e.send(
        "liquidate (takeover at mark: health x2, transfer, fees)",
        la.clone(),
        {
            let mut d = vec![5u8];
            d.extend_from_slice(&200u64.to_le_bytes());
            d
        },
        &[&lq],
        true,
    );
    // one bounded chunk per call: repeat until the account is healthy or flat, then it is refused
    for _ in 0..16 {
        if try_send(
            &mut e,
            la.clone(),
            {
                let mut d = vec![5u8];
                d.extend_from_slice(&200u64.to_le_bytes());
                d
            },
            &[&lq],
        )
        .is_some()
        {
            break;
        }
    }
    e.send(
        "liquidate healthy/flat account (rejected)",
        la,
        {
            let mut d = vec![5u8];
            d.extend_from_slice(&200u64.to_le_bytes());
            d
        },
        &[&lq],
        false,
    );
    assert!(e.ins_bal() > 0);

    // ADL: a second victim long; oracle crashes 20% with no bids in band -> bankrupt, ADL vs short taker
    let (v2, v2u) = e.user(100_000);
    let asks2: Vec<Vec<u8>> = vec![place_item(1, 1, 92_010, 200, 0, 0)];
    let a3 = e.shard(1, 92_000);
    e.send("", trade_accts(&mm, &mmu, &m, &[a3]), batch_data(&[], &asks2), &[&mm], true);
    let mut d = vec![1u8];
    d.extend_from_slice(&place_item(0, 4, 92_100, 9, 0, 0));
    e.send("", trade_accts(&v2, &v2u, &m, &[a3]), d, &[&v2], true);
    e.set_time(e.slot + 1);
    e.oracle_update("", 750_00000000, None);
    // keeper picks the counterparty: taker is short 100? taker base = 400; MM is short. Use MM (claim first).
    bshards.push(a3);
    let mut acc = all.to_vec();
    acc.extend_from_slice(&bshards);
    e.send("", trade_accts(&mm, &mmu, &m, &acc), batch_data(&[], &[]), &[&mm], true); // claim
    e.rank_user(&mmu);
    e.rank_user(&lqu);
    e.rank_user(&tku);
    assert_eq!(e.rank_state().entries[RANK_N].user, mmu.to_bytes()); // MM: most profitable short
    let (ins, rk) = (e.ins, e.rank);
    e.send("adl_rank (permissionless re-score)", vec![r(&m), w(&rk), w(&mmu)], vec![25], &[], true);
    e.send(
        "ADL (bankrupt -> on-chain ranked counterparty -> insurance)",
        vec![w(&v2u), w(&m), w(&ins), w(&rk), w(&mmu)],
        vec![6],
        &[],
        true,
    );
    assert_eq!(e.user_state(&v2u).pos[0].base, 0);
    assert!(e.user_state(&v2u).cash >= 0);

    // funding settle + lazy OI/fee aggregation crank
    e.send("settle_funding (permissionless, 1 position)", vec![w(&tku), r(&m)], vec![15], &[], true);
    let mut agg = vec![w(&m)];
    agg.extend(acc.iter().map(w));
    e.send("aggregate OI/fees from 9 shards (crank)", agg, vec![16], &[], true);

    // invariants: claim everything for everyone, then Σcash + insurance + fees == vault, Σbase == 0
    for (kp, u) in [(&mm, &mmu), (&tk, &tku), (&lq, &lqu), (&vic, &vicu), (&v2, &v2u)] {
        e.send("", trade_accts(kp, u, &m, &acc), batch_data(&[], &[]), &[kp], true);
        e.send("", vec![w(u), r(&m)], vec![15], &[], true);
    }
    let mut agg = vec![w(&m), w(&e.ins)];
    agg.extend(acc.iter().map(w));
    e.send("", agg, vec![16], &[], true);
    let users = [mmu, tku, lqu, vicu, v2u];
    let sum_cash: i128 = users.iter().map(|u| e.user_state(u).cash as i128).sum();
    let sum_base: i64 = users.iter().map(|u| e.user_state(u).pos.iter().map(|p| p.base).sum::<i64>()).sum();
    let ms = e.market_state();
    println!(
        "Σcash={} insurance={} fees={} vault={} Σbase={} oi_long={}",
        sum_cash,
        e.ins_bal(),
        ms.fees,
        e.vault_amount(),
        sum_base,
        ms.oi_long
    );
    assert_eq!(sum_base, 0);
    // funding: Σ base*Δindex == 0 only once every account settled at the same index (done above)
    assert_eq!(sum_cash + e.ins_bal() as i128 + ms.fees as i128, e.vault_amount() as i128);
    let long_oi: i64 = users.iter().map(|u| e.user_state(u).pos.iter().map(|p| p.base.max(0)).sum::<i64>()).sum();
    assert_eq!(ms.oi_long as i64, long_oi);

    // TS SDK fixture: real program logs + account bytes (sdk/test/sdk.test.ts decodes them)
    let hex = |d: &[u8]| d.iter().map(|b| format!("{:02x}", b)).collect::<String>();
    let evs: Vec<Vec<u8>> = e
        .all_logs
        .iter()
        .filter_map(|l| l.strip_prefix("Program data: "))
        .map(|s| s.split(' ').flat_map(b64).collect())
        .collect();
    let cnt = |k: u8| evs.iter().filter(|v| v[0] == k).count();
    let nfills: usize = evs.iter().filter(|v| v[0] == 1).map(|v| v[34] as usize).sum();
    let logs_json = e.all_logs.iter().map(|l| format!("{:?}", l)).collect::<Vec<_>>().join(",\n");
    let fx = format!("{{\"market_key\": \"{}\", \"tk\": \"{}\", \"mm\": \"{}\",\n\"market\": \"{}\",\n\"user_tk\": \"{}\",\n\"user_mm\": \"{}\",\n\"insurance\": \"{}\",\n\"rank\": \"{}\",\n\"expect\": {{\"tk_base\": {}, \"mm_base\": {}, \"oi_long\": {}, \"fill_events\": {}, \"fills\": {}, \"liquidations\": {}, \"adl\": {}, \"triggers\": {}, \"oracle_px\": {}, \"mark_px\": {}}},\n\"logs\": [{}]}}\n",
        m, tku, mmu, hex(&e.data(&m)), hex(&e.data(&tku)), hex(&e.data(&mmu)), hex(&e.data(&e.ins)), hex(&e.data(&e.rank)),
        e.user_state(&tku).pos[0].base, e.user_state(&mmu).pos[0].base, ms.oi_long, cnt(1), nfills, cnt(2), cnt(4), cnt(3), ms.oracle_px, ms.mark_px, logs_json);
    std::fs::create_dir_all("../sdk/test").unwrap();
    std::fs::write("../sdk/test/fixture.json", fx).unwrap();

    e.cu.extend(v6_cu());
    let mut f = std::fs::File::create("../cu_results.md").unwrap();
    writeln!(f, "| instruction (LiteSVM, Agave SBF runtime, perp_program.so) | CU |\n|---|---|").unwrap();
    for (l, c) in &e.cu {
        println!("{:>8} CU  {}", c, l);
        writeln!(f, "| {} | {} |", l, c).unwrap();
    }
    println!("sizes: market={} shard={} user={} ins={}", MARKET_SIZE, SHARD_SIZE, USER_SIZE, INS_SIZE);
}

// ---------------- order types (v3) ----------------
const ALO: u8 = 1;
const RO: u8 = 2;
const IOC: u8 = 4;
const FOK: u8 = 8;
const SLIDE: u8 = 16;
const STP_NEWEST: u8 = 1 << 5;
const STP_DECR: u8 = 2 << 5;
const PEG: u8 = 1;
const SPL: u8 = 2;
fn try_partial(e: &mut Env, sh: &[Address]) -> bool {
    let mut acc = vec![w(&e.market), r(&e.oracle)];
    acc.extend(sh.iter().map(r));
    e.send("", acc, vec![4], &[], false).is_none()
}
fn single(item: Vec<u8>) -> Vec<u8> {
    let mut d = vec![1u8];
    d.extend_from_slice(&item);
    d
}
fn cid_cancel(cid: u64, side: u8) -> Vec<u8> {
    cancel_item(cid, side | 128, 0)
}
fn trig(slot: u8, kind: u8, side: u8, vals: [u64; 6]) -> Vec<u8> {
    let mut d = vec![20u8, slot, kind, side];
    for v in vals {
        d.extend_from_slice(&v.to_le_bytes());
    }
    d
}

#[test]
fn order_types_cu_and_invariants() {
    let mut e = Env::new();
    let (b0, b1) = (e.shard(0, 99_900), e.shard(0, 99_800));
    let (a0, a1, a2) = (e.shard(1, 100_000), e.shard(1, 100_100), e.shard(1, 100_200));
    let (bl, bl2) = (e.shard(0, 91_900), e.shard(0, 91_800));
    let all = [b0, b1, a0, a1, a2];
    e.oracle_update("", 1000_00000000, None);
    let (mm, mmu) = e.user(50_000_000_000);
    let (tk, tku) = e.user(2_000_000_000);
    let (kp, kpu) = e.user(1_000_000);
    let m = e.market;
    let a = e.admin.pubkey();
    e.send(
        "set_tier (admin: MM tier 1 = rebate)",
        vec![AccountMeta::new(a, true), r(&m), w(&mmu)],
        vec![23, 1],
        &[],
        true,
    );

    // ---- discrete vs spline MM quoting (same 5x2 levels, same prices/sizes) ----
    let exp = e.slot + 500;
    let discrete = |off: u64| -> Vec<Vec<u8>> {
        let mut v = vec![];
        for l in 0..5u64 {
            v.push(place_ex(0, ALO, 0, ORACLE - 20 - l * 40 - off, 100, exp, 150, 1 + l, 0, 0, 0));
            v.push(place_ex(1, ALO, 0, ORACLE + 10 + l * 50 + off, 100, exp, 150, 11 + l, 0, 0, 0));
        }
        v
    };
    let ladders = |off: u64| -> Vec<Vec<u8>> {
        vec![
            place_ex(0, ALO | SLIDE, SPL, ORACLE - 20 - off, 100, exp, 150, 100, 40, 5, 0),
            place_ex(1, ALO | SLIDE, SPL, ORACLE + 10 + off, 100, exp, 150, 200, 50, 5, 0),
        ]
    };
    let cids_d: Vec<Vec<u8>> = (0..5u64).flat_map(|l| [cid_cancel(1 + l, 0), cid_cancel(11 + l, 1)]).collect();
    let cids_s = vec![cid_cancel(100, 0), cid_cancel(200, 1)];
    e.send(
        "discrete: place 10 orders (5 bid + 5 ask levels)",
        trade_accts(&mm, &mmu, &m, &all),
        batch_data(&[], &discrete(0)),
        &[&mm],
        true,
    );
    e.send(
        "discrete: replace by cid (cancel 10 + place 10)",
        trade_accts(&mm, &mmu, &m, &all),
        batch_data(&cids_d, &discrete(1)),
        &[&mm],
        true,
    );
    e.send("discrete: cancel 10 by cid", trade_accts(&mm, &mmu, &m, &all), batch_data(&cids_d, &[]), &[&mm], true);
    e.send(
        "spline: place 2 ladders (=10 levels)",
        trade_accts(&mm, &mmu, &m, &[b0, a0]),
        batch_data(&[], &ladders(0)),
        &[&mm],
        true,
    );
    let s = e.shard_state(&a0);
    assert_eq!(s.n, 1);
    assert_eq!(s.orders[0].qty, 500);
    e.send(
        "spline: replace by cid (cancel 2 + place 2 ladders)",
        trade_accts(&mm, &mmu, &m, &[b0, a0]),
        batch_data(&cids_s, &ladders(1)),
        &[&mm],
        true,
    );
    e.send(
        "spline: cancel 2 ladders by cid",
        trade_accts(&mm, &mmu, &m, &[b0, a0]),
        batch_data(&cids_s, &[]),
        &[&mm],
        true,
    );
    assert_eq!(e.user_state(&mmu).pos[0].ask_open, 0);
    // sweeps: taker buys 500 through 5 levels
    e.send("", trade_accts(&mm, &mmu, &m, &all), batch_data(&[], &discrete(0)), &[&mm], true);
    e.send(
        "taker IOC sweep 5 discrete levels (3 shards)",
        trade_accts(&tk, &tku, &m, &[a0, a1, a2]),
        single(place_item(0, IOC, ORACLE + 300, 500, 0, 0)),
        &[&tk],
        true,
    );
    let evs: Vec<Vec<u8>> = e.logs.iter().filter_map(|l| l.strip_prefix("Program data: ")).map(b64).collect();
    assert_eq!(evs.len(), 1, "one batched fill event per take");
    assert_eq!((evs[0][0], evs[0][34]), (1, 5), "5 fills in the event");
    e.send("", trade_accts(&mm, &mmu, &m, &all), batch_data(&cids_d, &[]), &[&mm], true);
    e.send("", trade_accts(&mm, &mmu, &m, &[b0, a0]), batch_data(&[], &ladders(0)), &[&mm], true);
    e.send(
        "taker IOC sweep 5 spline levels (1 shard)",
        trade_accts(&tk, &tku, &m, &[a0]),
        single(place_item(0, IOC, ORACLE + 300, 500, 0, 0)),
        &[&tk],
        true,
    );
    assert_eq!(e.user_state(&tku).pos[0].base, 1000);
    assert_eq!(e.shard_state(&a0).n, 0);
    e.send("", trade_accts(&mm, &mmu, &m, &all), batch_data(&cids_s, &[]), &[&mm], true); // claim + clear bid ladder
    assert_eq!(e.user_state(&mmu).pos[0].base, -1000);

    // ---- peg: ask at oracle+15, repriced lazily when oracle moves ----
    e.send(
        "place oracle-peg ask (offset +15, min/max band)",
        trade_accts(&mm, &mmu, &m, &[a0]),
        single(place_ex(1, 0, PEG, 0, 50, 0, 0, 300, 15, 90_000, 110_000)),
        &[&mm],
        true,
    );
    e.set_time(e.slot + 1);
    e.oracle_update("", 1000_40000000, None); // oracle -> 100_040, peg -> 100_055
    let c0 = e.user_state(&tku).cash;
    e.send(
        "taker IOC vs peg (lazy reprice)",
        trade_accts(&tk, &tku, &m, &[a0]),
        single(place_item(0, IOC, 100_100, 10, 0, 0)),
        &[&tk],
        true,
    );
    let paid = c0 - e.user_state(&tku).cash;
    assert_eq!(paid, 100_055 * 10 + (100_055 * 10 * 5 + 9999) / 10000);

    // ---- ALO reject / slide ----
    let before = e.shard_state(&a0).n;
    e.send(
        "ALO crossing (skipped, tx ok)",
        trade_accts(&tk, &tku, &m, &[b0, a0]),
        single(place_item(0, ALO, 100_060, 1, 0, 0)),
        &[&tk],
        true,
    );
    assert_eq!(e.shard_state(&a0).n, before);
    e.send(
        "ALO slide (re-priced to best ask - 1 tick)",
        trade_accts(&tk, &tku, &m, &[a0]),
        single(place_ex(1, ALO | SLIDE, 0, 100_000, 1, 0, 0, 7, 0, 0, 0)),
        &[&tk],
        true,
    );
    // best ask is our new 100_000? best bid empty -> no cross -> rests at 100_000; now slide a bid into it:
    let bshard = e.shard(0, 100_000);
    e.send("", trade_accts(&tk, &tku, &m, &[bshard, a0]), batch_data(&[cid_cancel(7, 1)], &[]), &[&tk], true);
    e.send(
        "",
        trade_accts(&mm, &mmu, &m, &[bshard, a0]),
        single(place_ex(0, ALO | SLIDE, 0, 100_060, 1, 0, 0, 8, 0, 0, 0)),
        &[&mm],
        true,
    );
    let sb = e.shard_state(&bshard);
    assert_eq!(sb.n, 1);
    assert_eq!(sb.orders[0].price, 100_054); // best ask (peg 100_055) - 1
    e.send("", trade_accts(&mm, &mmu, &m, &[bshard]), batch_data(&[cid_cancel(8, 0)], &[]), &[&mm], true);

    // ---- FOK ----
    let b0_ = e.user_state(&tku).pos[0].base;
    // FOK that cannot fill is skipped; the other item in the same batch still executes (AUDIT M-3)
    e.send(
        "FOK unfilled in batch (skipped, rest of batch runs)",
        trade_accts(&tk, &tku, &m, &[a0]),
        batch_data(&[], &[place_item(0, FOK, 100_100, 1000, 0, 0), place_item(0, IOC, 100_100, 1, 0, 0)]),
        &[&tk],
        true,
    );
    assert_eq!(e.user_state(&tku).pos[0].base, b0_ + 1);
    e.send("FOK filled", trade_accts(&tk, &tku, &m, &[a0]), single(place_item(0, FOK, 100_100, 5, 0, 0)), &[&tk], true);

    // ---- STP: MM crosses its own peg ask ----
    e.send(
        "STP decrement (no fill, both reduced)",
        trade_accts(&mm, &mmu, &m, &[a0]),
        single(place_item(0, IOC | STP_DECR, 100_100, 5, 0, 0)),
        &[&mm],
        true,
    );
    assert_eq!(e.shard_state(&a0).orders[0].qty, 29);
    e.send(
        "STP cancel-newest (taker dropped)",
        trade_accts(&mm, &mmu, &m, &[b0, a0]),
        single(place_item(0, STP_NEWEST, 100_100, 5, 0, 0)),
        &[&mm],
        true,
    );
    assert_eq!(e.shard_state(&a0).orders[0].qty, 29);
    assert_eq!(e.shard_state(&b0).n, 0);
    e.send("", trade_accts(&mm, &mmu, &m, &[a0]), batch_data(&[cid_cancel(300, 1)], &[]), &[&mm], true);

    // ---- triggers: SL on taker long, TWAP ----
    let mmq: Vec<Vec<u8>> = (0..5u64).map(|l| place_item(0, ALO, 99_990 - l * 20, 400, 0, 0)).collect();
    e.send("", trade_accts(&mm, &mmu, &m, &all), batch_data(&[], &mmq), &[&mm], true);
    let tb = e.user_state(&tku).pos[0].base as u64;
    e.send(
        "set_trigger SL (reduce-only)",
        vec![AccountMeta::new(tk.pubkey(), true), w(&tku), r(&m)],
        trig(0, 2, 1, [100_010, 99_000, tb, 500, 0, 0]),
        &[&tk],
        true,
    );
    let ex = |e: &mut Env, label: &str, s: u8, ok: bool| {
        let acc = {
            let mut v = vec![AccountMeta::new(kp.pubkey(), true), w(&kpu), w(&tku), r(&m)];
            v.extend(all.iter().map(w));
            v.push(w(&bshard));
            v.push(w(&bl));
            v.push(w(&bl2));
            v
        };
        e.send(label, acc, vec![21, s], &[&kp], ok);
    }; // every opposite shard
    ex(&mut e, "exec SL before trigger (rejected)", 0, false);
    e.set_time(e.slot + 1);
    e.oracle_update("", 1000_00000000, None);
    let k0 = e.user_state(&kpu).cash;
    ex(&mut e, "exec SL trigger (keeper crank, IOC reduce-only)", 0, true);
    assert_eq!(e.user_state(&tku).pos[0].base, 0);
    assert_eq!(e.user_state(&kpu).cash, k0 + 500);
    e.send(
        "",
        vec![AccountMeta::new(tk.pubkey(), true), w(&tku), r(&m)],
        trig(1, 3, 1, [0, 99_000, 30, 1, 2, 3]),
        &[&tk],
        true,
    );
    ex(&mut e, "exec TWAP slice (1/3, sell 10)", 1, true);
    ex(&mut e, "", 1, false);
    e.set_time(e.slot + 2);
    e.oracle_update("", 1000_00000000, None);
    ex(&mut e, "", 1, true);
    e.set_time(e.slot + 2);
    e.oracle_update("", 1000_00000000, None);
    ex(&mut e, "", 1, true);
    assert_eq!(e.user_state(&tku).pos[0].base, -30);
    assert_eq!(e.user_state(&tku).triggers[1].kind, 0);

    // ---- admin: pause + TVL cap ----
    let adm = |op: u8, v: u64| {
        let mut d = vec![22u8, op];
        d.extend_from_slice(&v.to_le_bytes());
        d
    };
    e.send("admin pause", vec![AccountMeta::new(a, true), w(&m)], adm(1, 1), &[], true);
    e.send(
        "paused: new risk rejected",
        trade_accts(&tk, &tku, &m, &[a0]),
        single(place_item(1, ALO, 100_500, 1, 0, 0)),
        &[&tk],
        false,
    );
    e.send(
        "paused: reduce-only allowed",
        trade_accts(&tk, &tku, &m, &[a0, a1, a2]),
        single(place_item(0, RO, 100_300, 1, 0, 0)),
        &[&tk],
        true,
    );
    e.send("", vec![AccountMeta::new(a, true), w(&m)], adm(1, 0), &[], true);
    let cap = e.vault_amount() + 10;
    e.send("", vec![AccountMeta::new(a, true), w(&m)], adm(2, cap), &[], true);
    let src = Address::new_unique();
    let mint = Address::from(e.market_state().mint);
    e.svm
        .set_account(
            src,
            Account {
                lamports: 1_000_000_000,
                data: token_acct(&mint, &tk.pubkey(), 1000),
                owner: TOKEN,
                executable: false,
                rent_epoch: 0,
            },
        )
        .unwrap();
    let dep = |amt: u64| {
        let mut d = vec![13u8];
        d.extend_from_slice(&amt.to_le_bytes());
        d
    };
    let dacc = vec![AccountMeta::new(tk.pubkey(), true), w(&tku), r(&m), w(&e.vault), w(&src), r(&TOKEN)];
    e.send("deposit over TVL cap (rejected)", dacc.clone(), dep(11), &[&tk], false);
    e.send("deposit under TVL cap (cap check + CPI)", dacc, dep(10), &[&tk], true);
    e.send("", vec![AccountMeta::new(a, true), w(&m)], adm(2, 0), &[], true);

    // ---- liquidation cancels the target's resting orders ----
    let (vic, vicu) = e.user(150_000);
    e.send(
        "",
        trade_accts(&mm, &mmu, &m, &[a0, a1]),
        batch_data(&[], &[place_item(1, ALO, 100_010, 100, 0, 0)]),
        &[&mm],
        true,
    );
    e.send("", trade_accts(&vic, &vicu, &m, &[bl]), single(place_item(0, ALO, 91_950, 1, 0, 0)), &[&vic], true);
    e.send("", trade_accts(&vic, &vicu, &m, &[a0]), single(place_item(0, IOC, 100_300, 12, 0, 0)), &[&vic], true);
    e.set_time(e.slot + 1);
    e.oracle_update("", 920_00000000, None);
    e.send("", trade_accts(&mm, &mmu, &m, &all), batch_data(&[], &[]), &[&mm], true); // MM claims
    e.send(
        "",
        trade_accts(&mm, &mmu, &m, &[bl2]),
        batch_data(&[], &[place_item(0, ALO, 91_890, 100, 0, 0)]),
        &[&mm],
        true,
    );
    let mut la = vec![AccountMeta::new(kp.pubkey(), true), w(&kpu), w(&vicu), r(&m), w(&e.ins)];
    la.extend([bl, bl2, bshard].iter().chain(all.iter()).map(w)); // resting orders: every shard of the market
    e.send(
        "liquidate (cancel target resting orders + close)",
        la,
        {
            let mut d = vec![5u8];
            d.extend_from_slice(&100u64.to_le_bytes());
            d
        },
        &[&kp],
        true,
    );
    let v = e.user_state(&vicu);
    assert!(v.pos[0].base < 12 && v.pos[0].bid_open == 0, "orders cancelled, one chunk taken over");
    assert_eq!(e.shard_state(&bl).n, 0);

    // ---- crank_claim: permissionless ledger drain ----
    e.send("crank_claim (1 shard -> 1 maker)", vec![w(&bl2), w(&mmu)], vec![24], &[], true);
    assert_eq!(e.shard_state(&bl2).nled, 0);

    // invariants
    let acc: Vec<Address> = vec![b0, b1, a0, a1, a2, bl, bl2, bshard];
    for (kpx, u) in [(&mm, &mmu), (&tk, &tku), (&kp, &kpu), (&vic, &vicu)] {
        e.send("", trade_accts(kpx, u, &m, &acc), batch_data(&[], &[]), &[kpx], true);
        e.send("", vec![w(u), r(&m)], vec![15], &[], true);
    }
    let mut agg = vec![w(&m), w(&e.ins)];
    agg.extend(acc.iter().map(w));
    e.send("", agg, vec![16], &[], true);
    let users = [mmu, tku, kpu, vicu];
    let sum_cash: i128 = users.iter().map(|u| e.user_state(u).cash as i128).sum();
    let sum_base: i64 = users.iter().map(|u| e.user_state(u).pos.iter().map(|p| p.base).sum::<i64>()).sum();
    let ms = e.market_state();
    assert_eq!(sum_base, 0);
    assert_eq!(sum_cash + e.ins_bal() as i128 + ms.fees as i128, e.vault_amount() as i128);
    let long_oi: i64 = users.iter().map(|u| e.user_state(u).pos.iter().map(|p| p.base.max(0)).sum::<i64>()).sum();
    assert_eq!(ms.oi_long as i64, long_oi);
    let mut f = std::fs::File::create("../cu_order_types.md").unwrap();
    writeln!(f, "| order-type instruction (LiteSVM, Agave 4.3 SBF runtime, perp_program.so) | CU |\n|---|---|")
        .unwrap();
    for (l, c) in &e.cu {
        if l.starts_with("deposit (SPL") {
            continue;
        }
        println!("{:>8} CU  {}", c, l);
        writeln!(f, "| {} | {} |", l, c).unwrap();
    }
}

/// v6 instructions measured on their own Env: book liquidation (1 chunk) and a mark-based SL trigger.
fn v6_cu() -> Vec<(String, u64)> {
    let mut e = Env::new();
    e.oracle_update("", 1000_00000000, None);
    let (a0, b0) = (e.shard(1, 100_000), e.shard(0, 87_000));
    let m = e.market;
    let (mm, mmu) = e.user(50_000_000_000);
    let (vic, vicu) = e.user(200_000);
    let (kp, kpu) = e.user(1_000_000);
    e.send("", trade_accts(&mm, &mmu, &m, &[a0]), single(place_item(1, 1, 100_000, 100, 0, 0)), &[&mm], true);
    e.send("", trade_accts(&vic, &vicu, &m, &[a0]), single(place_item(0, IOC, 100_000, 13, 0, 0)), &[&vic], true);
    e.send("", trade_accts(&mm, &mmu, &m, &[a0]), batch_data(&[], &[]), &[&mm], true);
    e.send("", trade_accts(&mm, &mmu, &m, &[b0]), single(place_item(0, 1, 87_000, 100, 0, 0)), &[&mm], true);
    e.send(
        "",
        vec![AccountMeta::new(vic.pubkey(), true), w(&vicu), r(&m)],
        trig(0, 2 | 64, 1, [99_000, 80_000, 3, 1, 0, 0]),
        &[&vic],
        true,
    );
    e.cu.clear();
    e.set_time(e.slot + 1);
    e.oracle_update("", 880_00000000, None);
    let ins = e.ins;
    let mut d = vec![34u8];
    d.extend_from_slice(&u64::MAX.to_le_bytes());
    e.send(
        "liquidate_book (permissionless, 1 chunk into the book, no liquidator capital)",
        vec![w(&vicu), r(&m), w(&ins), w(&a0), w(&b0)],
        d,
        &[],
        true,
    );
    e.send(
        "exec SL trigger on mark (keeper crank, IOC reduce-only)",
        vec![AccountMeta::new(kp.pubkey(), true), w(&kpu), w(&vicu), r(&m), w(&b0)],
        vec![21, 0],
        &[&kp],
        true,
    );
    e.cu
}
