// Shared LiteSVM harness (include!-d by cu.rs and audit.rs).
// Runs the real SBF program (target/deploy/perp_program.so) in LiteSVM (Agave runtime) and
// records compute units per instruction. Also checks conservation and margin rules.
use litesvm::LiteSVM;
use solana_account::Account;
use solana_address::Address;
use solana_clock::Clock;
use solana_instruction::{AccountMeta, Instruction};
use solana_keypair::Keypair;
use solana_signer::Signer;
use solana_transaction::Transaction;
use state::*;
use std::io::Write;

const TOKEN: Address = Address::from_str_const("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
const PYTH: Address = Address::from_str_const("rec5EKMGg6MxZYaMdyBfgwp4d5rB9T1VQH5pJv5LtFJ");
const BW: u64 = 100;
const ORACLE: u64 = 100_000;

struct Env { svm: LiteSVM, pid: Address, admin: Keypair, market: Address, ins: Address, rank: Address, vault: Address, vault_pda: Address, oracle: Address,
             feed: [u8; 32], slot: u64, ts: i64, cu: Vec<(String, u64)>, all_logs: Vec<String>, logs: Vec<String> }

fn token_acct(mint: &Address, owner: &Address, amount: u64) -> Vec<u8> {
    let mut d = vec![0u8; 165];
    d[0..32].copy_from_slice(mint.as_ref()); d[32..64].copy_from_slice(owner.as_ref());
    d[64..72].copy_from_slice(&amount.to_le_bytes()); d[108] = 1; d
}
fn pyth_data(feed: &[u8; 32], price: i64, conf: u64, ts: i64, slot: u64) -> Vec<u8> {
    let mut d = vec![0u8; 133];
    d[0..8].copy_from_slice(&[34, 241, 35, 99, 157, 126, 244, 205]); d[40] = 1;
    d[41..73].copy_from_slice(feed); d[73..81].copy_from_slice(&price.to_le_bytes()); d[81..89].copy_from_slice(&conf.to_le_bytes());
    d[89..93].copy_from_slice(&(-8i32).to_le_bytes()); d[93..101].copy_from_slice(&ts.to_le_bytes()); d[101..109].copy_from_slice(&ts.to_le_bytes());
    d[125..133].copy_from_slice(&slot.to_le_bytes()); d
}
fn w(k: &Address) -> AccountMeta { AccountMeta::new(*k, false) }
fn r(k: &Address) -> AccountMeta { AccountMeta::new_readonly(*k, false) }

impl Env {
    fn new() -> Env {
        let mut svm = LiteSVM::new();
        let pid = Address::new_unique();
        let so = ["../program/target/deploy/perp_program.so", "../../program/target/deploy/perp_program.so", concat!(env!("CARGO_MANIFEST_DIR"), "/../program/target/deploy/perp_program.so")].iter().find_map(|p| std::fs::read(p).ok()).expect("perp_program.so");
        svm.add_program(pid, &so).unwrap();
        let admin = Keypair::new(); svm.airdrop(&admin.pubkey(), 100_000_000_000).unwrap();
        let market = Address::new_unique(); let ins = Address::new_unique(); let oracle = Address::new_unique();
        let (vault_pda, bump) = Address::find_program_address(&[b"vault", market.as_ref()], &pid);
        let mint = Address::new_unique();
        let mut md = vec![0u8; 82]; md[44] = 6; md[45] = 1;
        svm.set_account(mint, Account { lamports: 1_000_000_000, data: md, owner: TOKEN, executable: false, rent_epoch: 0 }).unwrap();
        let vault = Address::new_unique();
        svm.set_account(vault, Account { lamports: 1_000_000_000, data: token_acct(&mint, &vault_pda, 0), owner: TOKEN, executable: false, rent_epoch: 0 }).unwrap();
        let feed = [7u8; 32];
        let rank = Address::new_unique();
        let mut e = Env { svm, pid, admin, market, ins, rank, vault, vault_pda, oracle, feed, slot: 1000, ts: 1_800_000_000, cu: vec![], all_logs: vec![], logs: vec![] };
        e.set_time(1000);
        e.prog_acct(market, MARKET_SIZE); e.prog_acct(ins, INS_SIZE);
        e.set_pyth(1000_00000000, 50_000_000);
        let mut d = vec![10u8, bump]; d.extend_from_slice(&feed);
        // bucket_width, max_staleness, max_conf_bps, max_age, expo, px_mul, px_div, max_oi
        for v in [BW, 3, 100, 10, (-8i64) as u64, 1, 1_000_000, 0] { d.extend_from_slice(&v.to_le_bytes()); }
        let a = e.admin.pubkey();
        e.send("init_market", vec![AccountMeta::new(a, true), w(&market), w(&ins), r(&vault), r(&oracle)], d, &[], true);
        e.prog_acct(rank, RANK_SIZE);
        e.send("init_rank (admin)", vec![AccountMeta::new(a, true), w(&market), w(&rank)], vec![26], &[], true);
        e
    }
    fn set_time(&mut self, slot: u64) {
        self.ts += ((slot - self.slot.min(slot)) as i64 * 4) / 10; self.slot = slot;
        self.svm.set_sysvar(&Clock { slot, epoch_start_timestamp: 0, epoch: 0, leader_schedule_epoch: 0, unix_timestamp: self.ts });
    }
    fn set_pyth(&mut self, price: i64, conf: u64) {
        let d = pyth_data(&self.feed, price, conf, self.ts, self.slot);
        self.svm.set_account(self.oracle, Account { lamports: 1_000_000_000, data: d, owner: PYTH, executable: false, rent_epoch: 0 }).unwrap();
    }
    fn prog_acct(&mut self, k: Address, size: usize) {
        self.svm.set_account(k, Account { lamports: 1_000_000_000, data: vec![0; size], owner: self.pid, executable: false, rent_epoch: 0 }).unwrap();
    }
    fn send(&mut self, label: &str, accounts: Vec<AccountMeta>, data: Vec<u8>, signers: &[&Keypair], ok: bool) -> Option<u64> {
        let ix = Instruction { program_id: self.pid, accounts, data };
        let mut s: Vec<&Keypair> = vec![&self.admin]; s.extend_from_slice(signers);
        let tx = Transaction::new_signed_with_payer(&[ix], Some(&self.admin.pubkey()), &s, self.svm.latest_blockhash());
        let res = self.svm.send_transaction(tx); self.svm.expire_blockhash();
        match (res, ok) {
            (Ok(m), true) => { self.logs = m.logs.clone(); self.all_logs.extend(m.logs.iter().cloned()); if !label.is_empty() { self.cu.push((label.to_string(), m.compute_units_consumed)); } Some(m.compute_units_consumed) }
            (Err(f), false) => { let _ = f; None }
            (Ok(_), false) => panic!("{label}: expected failure but succeeded"),
            (Err(f), true) => panic!("{label}: {:?}\n{}", f.err, f.meta.logs.join("\n")),
        }
    }
    fn oracle_update(&mut self, label: &str, price: i64, shards: Option<(Address, Address)>) {
        self.set_pyth(price, (price / 10000) as u64);
        let mut acc = vec![w(&self.market), r(&self.oracle)];
        if let Some((b, a)) = shards { acc.push(r(&b)); acc.push(r(&a)); }
        self.send(label, acc.clone(), vec![4], &[], true);
        self.confirm_jump(label, price, acc);
    }
    /// AUDIT V6-2: a single-source jump > 5% is held until a later publish confirms it; test helpers that move the
    /// price in one step publish the confirmation (newer publish_time) so the scenario proceeds.
    /// (the CU row, if labelled, is the confirming update: the one that actually applies the price)
    fn confirm_jump(&mut self, label: &str, price: i64, acc: Vec<AccountMeta>) {
        let m = self.market_state();
        if m.paused == 3 && m.oracle2 == [0; 32] && m.oracle2_px != 0 {
            if !label.is_empty() { self.cu.pop(); }
            self.ts += 1; self.set_pyth(price, (price / 10000) as u64);
            self.send(label, acc, vec![4], &[], true);
        }
    }
    fn oracle_update_set(&mut self, label: &str, price: i64, shards: &[Address]) {
        self.set_pyth(price, (price / 10000) as u64);
        let mut acc = vec![w(&self.market), r(&self.oracle)]; acc.extend(shards.iter().map(r));
        self.send(label, acc.clone(), vec![4], &[], true);
        self.confirm_jump(label, price, acc);
    }
    /// shard created by a given admin (pooled markets: the pool admin, AUDIT V5-4)
    #[allow(dead_code)]
    fn shard_by(&mut self, side: u8, lo: u64, adm: &Keypair) -> Address {
        let k = Address::new_unique(); self.prog_acct(k, SHARD_SIZE);
        let mut d = vec![11u8, side]; d.extend_from_slice(&lo.to_le_bytes());
        self.send("", vec![AccountMeta::new(adm.pubkey(), true), w(&self.market), w(&k)], d, &[adm], true); k
    }
    fn shard(&mut self, side: u8, lo: u64) -> Address {
        let k = Address::new_unique(); self.prog_acct(k, SHARD_SIZE);
        let mut d = vec![11u8, side]; d.extend_from_slice(&lo.to_le_bytes());
        let a = self.admin.pubkey();
        self.send("", vec![AccountMeta::new(a, true), w(&self.market), w(&k)], d, &[], true); k
    }
    fn user(&mut self, deposit: u64) -> (Keypair, Address) {
        let kp = Keypair::new(); self.svm.airdrop(&kp.pubkey(), 10_000_000_000).unwrap();
        let u = Address::new_unique(); self.prog_acct(u, USER_SIZE);
        self.send("", vec![AccountMeta::new(kp.pubkey(), true), w(&u)], vec![12], &[&kp], true);
        let src = Address::new_unique();
        let mint = Address::from(self.market_state().mint);
        self.svm.set_account(src, Account { lamports: 1_000_000_000, data: token_acct(&mint, &kp.pubkey(), deposit), owner: TOKEN, executable: false, rent_epoch: 0 }).unwrap();
        let mut d = vec![13u8]; d.extend_from_slice(&deposit.to_le_bytes());
        self.send("deposit (SPL transfer CPI)", vec![AccountMeta::new(kp.pubkey(), true), w(&u), r(&self.market), w(&self.vault), w(&src), r(&TOKEN)], d, &[&kp], true);
        (kp, u)
    }
    /// permissionless ADL re-score of `u` in the current market
    fn rank_user(&mut self, u: &Address) { let (m, rk) = (self.market, self.rank); self.send("", vec![r(&m), w(&rk), w(u)], vec![25], &[], true); }
    fn rank_state(&self) -> Rank { unsafe { std::ptr::read_unaligned(self.data(&self.rank).as_ptr() as *const Rank) } }
    fn data(&self, k: &Address) -> Vec<u8> { self.svm.get_account(k).unwrap().data }
    fn market_state(&self) -> Market { unsafe { std::ptr::read_unaligned(self.data(&self.market).as_ptr() as *const Market) } }
    fn user_state(&self, k: &Address) -> User { unsafe { std::ptr::read_unaligned(self.data(k).as_ptr() as *const User) } }
    fn shard_state(&self, k: &Address) -> Shard { unsafe { std::ptr::read_unaligned(self.data(k).as_ptr() as *const Shard) } }
    fn ins_bal(&self) -> i64 { unsafe { std::ptr::read_unaligned(self.data(&self.ins).as_ptr() as *const Insurance) }.balance }
    fn vault_amount(&self) -> u64 { u64::from_le_bytes(self.data(&self.vault)[64..72].try_into().unwrap()) }
}
fn b64(s: &str) -> Vec<u8> {
    let v = |c: u8| match c { b'A'..=b'Z' => c - b'A', b'a'..=b'z' => c - b'a' + 26, b'0'..=b'9' => c - b'0' + 52, b'+' => 62, _ => 63 } as u32;
    let b: Vec<u8> = s.trim().bytes().filter(|&c| c != b'=').collect(); let mut out = vec![];
    for ch in b.chunks(4) { let mut n = 0u32; for (i, &c) in ch.iter().enumerate() { n |= v(c) << (18 - 6 * i); }
        for i in 0..ch.len() - 1 { out.push((n >> (16 - 8 * i)) as u8); } }
    out
}
// item encoders (spec layout)
fn place_item(side: u8, flags: u8, price: u64, qty: u64, expiry: u64, band: u16) -> Vec<u8> { place_ex(side, flags, 0, price, qty, expiry, band, 0, 0, 0, 0) }
fn place_ex(side: u8, flags: u8, kind: u8, price: u64, qty: u64, expiry: u64, band: u16, cid: u64, a1: i64, a2: u64, a3: u64) -> Vec<u8> {
    let mut b = vec![side, flags, kind]; for v in [price, qty, expiry] { b.extend_from_slice(&v.to_le_bytes()); } b.extend_from_slice(&band.to_le_bytes());
    for v in [cid, a1 as u64, a2, a3] { b.extend_from_slice(&v.to_le_bytes()); }
    assert_eq!(b.len(), 61); b
}
fn cancel_item(id: u64, side: u8, price: u64) -> Vec<u8> { let mut b = id.to_le_bytes().to_vec(); b.push(side); b.extend_from_slice(&price.to_le_bytes()); b }
fn batch_data(cancels: &[Vec<u8>], places: &[Vec<u8>]) -> Vec<u8> {
    let mut d = vec![3u8, 0, 0, cancels.len() as u8, places.len() as u8];
    for c in cancels { d.extend_from_slice(c) } for p in places { d.extend_from_slice(p) } d
}
fn trade_accts(kp: &Keypair, u: &Address, m: &Address, shards: &[Address]) -> Vec<AccountMeta> {
    let mut v = vec![AccountMeta::new(kp.pubkey(), true), w(u), r(m)]; v.extend(shards.iter().map(w)); v
}

