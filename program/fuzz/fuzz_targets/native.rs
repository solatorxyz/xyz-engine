#![no_main]
//! Coverage-guided fuzzing of the program's OWN processor code (program/src/lib.rs, the same source that is
//! built to SBF), compiled natively with libFuzzer/sancov instrumentation. The harness lays out accounts in
//! the runtime's RuntimeAccount format, calls `perp_program::process_instruction` directly, and rolls back
//! account data on error (transaction atomicity). Host differences vs SBF: Clock comes from HOST_CLOCK; SPL
//! token CPIs are no-ops on host, so vault balances are tracked by the harness from successful
//! deposit/withdraw. Overflow checks are on, so any arithmetic overflow panics = crash = finding.
//! Invariants after each input: Σbase = 0, oi_long = Σ long base, insurance >= 0,
//! tracked vault + bad_debt == Σcash + Σiso_margin + insurance + fees (after claiming and aggregating).
use libfuzzer_sys::fuzz_target;
use perp_program::state::*;
use pinocchio::account::{AccountView, RuntimeAccount};
use solana_address::Address;
use std::collections::HashMap;

const TOKEN: Address = Address::from_str_const("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
const PYTH: Address = Address::from_str_const("rec5EKMGg6MxZYaMdyBfgwp4d5rB9T1VQH5pJv5LtFJ");
type K = [u8; 32];
fn ad(k: &K) -> Address { Address::new_from_array(*k) }
fn kk(a: &Address) -> K { let mut b = [0u8; 32]; b.copy_from_slice(a.as_ref()); b }
const HDR: usize = core::mem::size_of::<RuntimeAccount>();

struct Acct { buf: Vec<u64> } // 8-aligned: RuntimeAccount header + data
impl Acct {
    fn new(key: &Address, owner: &Address, data: &[u8]) -> Self {
        let mut buf = vec![0u64; (HDR + data.len() + 7) / 8 + 1];
        let p = buf.as_mut_ptr() as *mut u8;
        unsafe {
            let ra = p as *mut RuntimeAccount;
            (*ra).borrow_state = u8::MAX; (*ra).address = key.clone(); (*ra).owner = owner.clone(); (*ra).lamports = 1_000_000_000; (*ra).data_len = data.len() as u64;
            std::ptr::copy_nonoverlapping(data.as_ptr(), p.add(HDR), data.len());
        }
        Acct { buf }
    }
    fn ra(&mut self) -> *mut RuntimeAccount { self.buf.as_mut_ptr() as *mut RuntimeAccount }
    fn data(&self) -> &[u8] { unsafe { let p = self.buf.as_ptr() as *const u8; std::slice::from_raw_parts(p.add(HDR), (*(p as *const RuntimeAccount)).data_len as usize) } }
    fn data_mut(&mut self) -> &mut [u8] { unsafe { let p = self.buf.as_mut_ptr() as *mut u8; let n = (*(p as *const RuntimeAccount)).data_len as usize; std::slice::from_raw_parts_mut(p.add(HDR), n) } }
}
struct World { pid: Address, accts: HashMap<K, Acct>, slot: u64, ts: i64, vault_amt: i128, n: u64, okn: u64 }
impl World {
    fn key(&mut self) -> K { self.n += 1; let mut b = [0u8; 32]; b[..8].copy_from_slice(&self.n.to_le_bytes()); b[31] = 0xaa; b }
    fn add(&mut self, owner: &Address, data: &[u8]) -> K { let k = self.key(); self.accts.insert(k, Acct::new(&ad(&k), owner, data)); k }
    fn prog(&mut self, size: usize) -> K { let pid = kk(&self.pid); self.add(&ad(&pid), &vec![0u8; size]) }
    /// metas: (key, signer, writable). Ok => state kept; Err => rolled back.
    fn call(&mut self, metas: &[(K, bool, bool)], data: &[u8]) -> bool {
        *perp_program::HOST_CLOCK.lock().unwrap() = (self.slot, self.ts);
        let mut snap: Vec<(K, Vec<u8>)> = vec![];
        for (k, _, _) in metas { if !snap.iter().any(|(x, _)| x == k) { if let Some(a) = self.accts.get(k) { snap.push((*k, a.data().to_vec())); } else { return false; } } }
        let mut views: Vec<AccountView> = Vec::with_capacity(metas.len());
        for (k, s, w) in metas {
            let a = self.accts.get_mut(k).unwrap(); let ra = a.ra();
            unsafe { (*ra).is_signer |= *s as u8; (*ra).is_writable |= *w as u8; views.push(AccountView::new_unchecked(ra)); }
        }
        let r = perp_program::process_instruction(&self.pid, &mut views, data);
        if std::env::var("NATIVE_DEBUG").is_ok() { eprintln!("ix {} -> {:?}", data[0], r); }
        for (k, _, _) in metas { let ra = self.accts.get_mut(k).unwrap().ra(); unsafe { (*ra).is_signer = 0; (*ra).is_writable = 0; } }
        if r.is_err() { for (k, d) in snap { self.accts.get_mut(&k).unwrap().data_mut().copy_from_slice(&d); } }
        if r.is_ok() { self.okn += 1; }
        r.is_ok()
    }
    fn st<T>(&self, k: &K) -> T { unsafe { std::ptr::read_unaligned(self.accts[k].data().as_ptr() as *const T) } }
}
fn token_acct(mint: &K, owner: &K) -> Vec<u8> { let mut d = vec![0u8; 165]; d[0..32].copy_from_slice(mint); d[32..64].copy_from_slice(owner); d[108] = 1; d }
fn pyth(feed: &[u8; 32], price: i64, conf: u64, ts: i64) -> Vec<u8> {
    let mut d = vec![0u8; 133];
    d[0..8].copy_from_slice(&[34, 241, 35, 99, 157, 126, 244, 205]); d[40] = 1;
    d[41..73].copy_from_slice(feed); d[73..81].copy_from_slice(&price.to_le_bytes()); d[81..89].copy_from_slice(&conf.to_le_bytes());
    d[89..93].copy_from_slice(&(-8i32).to_le_bytes()); d[93..101].copy_from_slice(&ts.to_le_bytes()); d[101..109].copy_from_slice(&ts.to_le_bytes()); d
}
fn place(side: u8, flags: u8, kind: u8, price: u64, qty: u64, band: u16, cid: u64, a1: i64, a2: u64, a3: u64) -> Vec<u8> {
    let mut b = vec![side, flags, kind]; for v in [price, qty, 0u64] { b.extend_from_slice(&v.to_le_bytes()); }
    b.extend_from_slice(&band.to_le_bytes()); b.extend_from_slice(&cid.to_le_bytes());
    b.extend_from_slice(&a1.to_le_bytes()); b.extend_from_slice(&a2.to_le_bytes()); b.extend_from_slice(&a3.to_le_bytes()); b
}
fn le(tag: u8, v: u64) -> Vec<u8> { let mut d = vec![tag]; d.extend_from_slice(&v.to_le_bytes()); d }

fuzz_target!(|data: &[u8]| {
    if data.len() < 8 { return; }
    let pid = Address::new_from_array([7u8; 32]);
    let mut w = World { pid, accts: HashMap::new(), slot: 1000, ts: 1_800_000_000, vault_amt: 0, n: 0, okn: 0 };
    let admin = w.add(&Address::default(), &[]);
    let market = w.prog(MARKET_SIZE); let ins = w.prog(INS_SIZE); let rank = w.prog(RANK_SIZE);
    let (pda, bump) = Address::find_program_address(&[b"vault", &market[..]], &w.pid); let pda = kk(&pda);
    let mint = w.key();
    let vault = w.add(&TOKEN, &token_acct(&mint, &pda));
    let feed = [7u8; 32];
    let oracle = w.add(&PYTH, &pyth(&feed, 1000_00000000, 10_000_000, w.ts));
    let tok = kk(&TOKEN); w.accts.insert(tok, Acct::new(&TOKEN, &Address::default(), &[]));
    let mut d = vec![10u8, bump]; d.extend_from_slice(&feed);
    for v in [100u64, 3, 100, 10, (-8i64) as u64, 1, 1_000_000, 0] { d.extend_from_slice(&v.to_le_bytes()); }
    assert!(w.call(&[(admin, true, true), (market, false, true), (ins, false, true), (vault, false, false), (oracle, false, false)], &d));
    assert!(w.call(&[(admin, true, true), (market, false, true), (rank, false, true)], &[26]));
    let mut sh = vec![];
    for (side, lo) in [(0u8, 99_900u64), (0, 99_000), (1, 100_000), (1, 101_000)] {
        let s = w.prog(SHARD_SIZE); let mut d = vec![11u8, side]; d.extend_from_slice(&lo.to_le_bytes());
        assert!(w.call(&[(admin, true, true), (market, false, true), (s, false, true)], &d)); sh.push(s);
    }
    let upd = |w: &mut World, px: i64, with_shards: bool, sh: &[K]| {
        let t = w.ts; w.accts.insert(oracle, Acct::new(&ad(&oracle), &PYTH, &pyth(&feed, px, (px / 10000) as u64, t)));
        let mut m = vec![(market, false, true), (oracle, false, false)]; if with_shards { m.extend(sh.iter().map(|s| (*s, false, false))); }
        w.call(&m, &[4])
    };
    upd(&mut w, 1000_00000000, false, &sh);
    let mut us = vec![];
    for i in 0..5u64 {
        let auth = w.add(&Address::default(), &[]); let u = w.prog(USER_SIZE);
        assert!(w.call(&[(auth, true, true), (u, false, true)], &[12]));
        let src = w.add(&TOKEN, &token_acct(&mint, &auth));
        let amt = if i == 4 { 1_000_000_000 } else { 200_000 + 3_000_000 * i };
        assert!(w.call(&[(auth, true, true), (u, false, true), (market, false, false), (vault, false, true), (src, false, true), (tok, false, false)], &le(13, amt)));
        w.vault_amt += amt as i128; us.push((auth, u, src));
    }
    let (lqa, lqu, _) = us[4];
    let trade = |a: K, u: K, sh: &[K]| -> Vec<(K, bool, bool)> { let mut m = vec![(a, true, true), (u, false, true), (market, false, false)]; m.extend(sh.iter().map(|s| (*s, false, true))); m };
    let mut px: i64 = 1000_00000000;
    // Structure-aware seeding (V6-4): data[0] bit 0 starts from a near-liquidation state: user0 long 18 lots
    // with 200k, user2 bids resting near the top, oracle stepped to 935 (three <5% steps): user0 just below MM.
    if data[0] & 1 == 1 {
        let (a1, u1, _) = us[1]; let (a0, u0, _) = us[0]; let (a2, u2, _) = us[2];
        let mut d = vec![1u8]; d.extend(place(1, 1, 0, 100_000, 40, 0, 0, 0, 0, 0)); w.call(&trade(a1, u1, &sh), &d);
        let mut d = vec![1u8]; d.extend(place(0, perp_program::spec::FLAG_IOC as u8, 0, 100_000, 18, 0, 0, 0, 0, 0)); w.call(&trade(a0, u0, &sh), &d);
        for (p, q) in [(99_900u64, 6u64), (99_050, 6), (99_000, 12)] { let mut d = vec![1u8]; d.extend(place(0, 1, 0, p, q, 0, 0, 0, 0, 0)); w.call(&trade(a2, u2, &sh), &d); }
        for p2 in [970_00000000i64, 950_00000000, 935_00000000] { w.slot += 1; w.ts += 1; upd(&mut w, p2, true, &sh); px = p2; }
    }
    let mut okc = [0u32; 16]; let mut tot = [0u32; 16];
        for c in data.chunks_exact(8) {
        let (a, u, src) = us[(c[1] % 4) as usize];
        let price = 98_000 + (u16::from_le_bytes([c[2], c[3]]) % 4_000) as u64 / 10 * 10;
        let qty = 1 + c[4] as u64 % 40; let fl = c[5]; let side = c[6] & 1;
        let ok0 = w.okn; tot[(c[0] % 16) as usize] += 1;
        match c[0] % 16 {
            0..=3 => { let k = if c[7] & 0x30 == 0x10 { 1 } else if c[7] & 0x30 == 0x20 { 2 } else { 0 };
                       let (a1, a2, a3) = match k { 1 => ((c[2] as i64) - 128, 0, 0), 2 => (1 + (c[2] % 7) as i64, 1 + (c[3] % 8) as u64, (c[7] % 3) as u64), _ => (0, 0, 0) };
                       let mut d = vec![1u8]; d.extend(place(side, fl & 0x7f, k, if k == 1 { 0 } else { price }, qty, if c[7] & 1 == 1 { 300 } else { 0 }, (c[7] % 4) as u64, a1, a2, a3));
                       w.call(&trade(a, u, &sh), &d); }
            4 => { let mut d = vec![3u8]; // batch: header(tag, market u16, n_cancel, n_place) + cancel by cid + 2 places
                   d.extend_from_slice(&0u16.to_le_bytes()); d.push(1); d.push(2);
                   d.extend_from_slice(&((c[7] % 4) as u64).to_le_bytes()); d.push(side | 128); d.extend_from_slice(&0u64.to_le_bytes());
                   d.extend(place(side, 1, 0, price, qty, 0, (c[7] % 4) as u64, 0, 0, 0)); d.extend(place(1 - side, 1, 0, price + 200, qty, 0, 0, 0, 0, 0));
                   w.call(&trade(a, u, &sh), &d); }
            5 => { px = (px + (c[7] as i64 - 128) * 1_0000000).clamp(500_00000000, 1500_00000000); // <= 1.3% per step
                   w.slot += 1 + (c[2] % 5) as u64; w.ts += 1; upd(&mut w, px, c[3] & 1 == 1, &sh); }
            6 => { let mut m = vec![(lqa, true, true), (lqu, false, true), (u, false, true), (market, false, false), (ins, false, true)]; m.extend(sh.iter().map(|s| (*s, false, true))); w.call(&m, &le(5, qty)); }
            7 => { w.call(&[(market, false, false), (rank, false, true), (u, false, true)], &[25]); }
            8 => { let mut m = vec![(u, false, true), (market, false, true), (ins, false, true), (rank, false, true)]; m.extend(us.iter().map(|x| (x.1, false, true))); w.call(&m, &[6]); }
            9 => { let mut m = vec![(a, true, true), (u, false, true), (market, false, false), (vault, false, true), (src, false, true), (tok, false, false), (pda, false, false)];
                   if !w.accts.contains_key(&pda) { w.accts.insert(pda, Acct::new(&ad(&pda), &Address::default(), &[])); }
                   m.extend(sh.iter().map(|s| (*s, false, true)));
                   if w.call(&m, &le(14, qty * 10_000)) { w.vault_amt -= (qty * 10_000) as i128; } }
            10 => { w.call(&[(a, true, true), (u, false, true), (market, false, false)], &[30, c[7] & 1]);
                    w.call(&[(a, true, true), (u, false, true), (market, false, false)], &le(31, if c[7] & 2 == 0 { qty * 5_000 } else { (-((qty * 5_000) as i64)) as u64 })); }
            11 => { let mut d = vec![20u8, c[7] % 4, (1 + c[2] % 3) | (c[3] & 0xc0), side]; for v in [price, price, qty, 3, 1, 2] { d.extend_from_slice(&v.to_le_bytes()); }
                    w.call(&[(a, true, true), (u, false, true), (market, false, false)], &d);
                    let mut m = vec![(lqa, true, true), (lqu, false, true), (u, false, true), (market, false, false)]; m.extend(sh.iter().map(|s| (*s, false, true))); w.call(&m, &[21, c[7] % 4]); }
            12 => { let mut m = vec![(u, false, true), (market, false, false), (ins, false, true)]; m.extend(sh.iter().map(|s| (*s, false, true))); w.call(&m, &le(34, qty)); }
            13 => { let mut d = vec![2u8]; d.extend_from_slice(&0u16.to_le_bytes()); d.push(1); d.push(0);
                    let id = ((c[2] % 4) as u64) << 40 | (c[7] as u64 % 16); d.extend_from_slice(&id.to_le_bytes()); d.push(side); d.extend_from_slice(&price.to_le_bytes());
                    w.call(&trade(a, u, &sh), &d); }
            14 => { w.call(&[(u, false, true), (market, false, false)], &[15]); }
            _ => { let mut m = vec![(admin, true, true), (market, false, true)]; let _ = &mut m; w.call(&m, &[22, 1, (c[7] & 1), 0, 0, 0, 0, 0, 0, 0]); }
        }
        if w.okn > ok0 { okc[(c[0] % 16) as usize] += 1; }
    }
    if std::env::var("NATIVE_STATS").is_ok() { eprintln!("ok/total per op: {:?}", okc.iter().zip(tot.iter()).collect::<Vec<_>>()); }
    // settle + claim everything, aggregate, then check invariants
    for (a, u, _) in us.clone() { let mut d = vec![3u8]; d.extend_from_slice(&0u16.to_le_bytes()); d.push(0); d.push(0); w.call(&trade(a, u, &sh), &d); w.call(&[(u, false, true), (market, false, false)], &[15]); }
    let mut m = vec![(market, false, true), (ins, false, true)]; m.extend(sh.iter().map(|s| (*s, false, true))); w.call(&m, &[16]);
    let st: Vec<User> = us.iter().map(|x| w.st::<User>(&x.1)).collect();
    let cash: i128 = st.iter().map(|s| s.cash as i128 + s.pos.iter().filter(|p| p.flags & 1 != 0).map(|p| p.iso_margin as i128).sum::<i128>()).sum();
    let base: i64 = st.iter().map(|s| s.pos.iter().map(|p| p.base).sum::<i64>()).sum();
    let longs: i64 = st.iter().map(|s| s.pos.iter().map(|p| p.base.max(0)).sum::<i64>()).sum();
    let ms: Market = w.st(&market); let ib: Insurance = w.st(&ins);
    assert_eq!(base, 0, "net base");
    assert!(ib.balance >= 0, "insurance negative");
    assert_eq!(ms.oi_long as i64, longs, "oi");
    assert_eq!(cash + ib.balance as i128 + ms.fees as i128, w.vault_amt + ms.bad_debt as i128, "conservation");
});
