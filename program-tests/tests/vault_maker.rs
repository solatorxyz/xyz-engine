// The vault as a maker: depositors' money quotes the market through a sub-account of the vault. Real SBF program.
// What must hold: the vault's value counts the margin it has locked for quoting and waits for its fills to be settled (so a
// deposit can't buy it cheap and a withdrawal can't dodge a loss); the operator's key can write quotes only inside the vault's
// rules and can never move value out; an ordinary maker instruction never works on a vault slab.
#[path = "../../program/src/state.rs"]
#[allow(dead_code)]
mod state;
include!("common.rs");
const PX: u64 = ORACLE;
const ASK: u16 = 8; // idx 0, ask side, level 0
const ASK_LAD: u16 = 1; // ladder of idx 0, ask side
const PEGGED: [(u16, u32, u32); 2] = [(0xC000, 0, 1), (0xC000 | ASK_LAD, 0, 1)];
fn peg(off: i64) -> u32 { (off + (1i64 << 31)) as u32 }
fn items(tag: u8, v: &[(u16, u32, u32)]) -> Vec<u8> { let mut d = vec![tag]; for (i, p, q) in v { d.extend_from_slice(&i.to_le_bytes()); d.extend_from_slice(&p.to_le_bytes()); d.extend_from_slice(&q.to_le_bytes()); } d }
fn fb(e: &Env, f: &Address) -> Box<FastBook> { let d = e.data(f); let mut b: Box<std::mem::MaybeUninit<FastBook>> = Box::new_uninit(); unsafe { std::ptr::copy_nonoverlapping(d.as_ptr(), b.as_mut_ptr() as *mut u8, FAST_SIZE); b.assume_init() } }
fn vst(e: &Env, k: &Address) -> Vault { unsafe { std::ptr::read_unaligned(e.data(k).as_ptr() as *const Vault) } }
fn vds(e: &Env, k: &Address) -> VaultDeposit { unsafe { std::ptr::read_unaligned(e.data(k).as_ptr() as *const VaultDeposit) } }
fn sig(k: &Keypair) -> AccountMeta { AccountMeta::new_readonly(k.pubkey(), true) }
struct H { v: Address, sub: Address, sub1: Address, slab: Address, f: Address, op: Keypair }
/// A vault with two sub-accounts, an operator, and a slab for subs[0] approved on the market's book.
fn house(e: &mut Env) -> H {
    e.oracle_update("", 1000_00000000, None);
    let (a, m) = (e.admin.pubkey(), e.market);
    let f = Address::new_unique(); e.prog_acct(f, FAST_SIZE);
    let mut d = vec![61u8]; d.extend_from_slice(&0u64.to_le_bytes());
    e.send("", vec![AccountMeta::new(a, true), w(&m), w(&f)], d, &[], true);
    let (v, sub, sub1, slab) = (Address::new_unique(), Address::new_unique(), Address::new_unique(), Address::new_unique());
    e.prog_acct(v, VAULT_SIZE); e.prog_acct(sub, USER_SIZE); e.prog_acct(sub1, USER_SIZE); e.prog_acct(slab, SLAB_SIZE);
    let mut d = vec![50u8]; for x in [100u64, 2000, 0, 2500] { d.extend_from_slice(&x.to_le_bytes()); }
    e.send("", vec![AccountMeta::new(a, true), w(&v), r(&m)], d, &[], true);
    e.send("", vec![AccountMeta::new(a, true), w(&v), w(&sub)], vec![51], &[], true);
    e.send("", vec![AccountMeta::new(a, true), w(&v), w(&sub1)], vec![51], &[], true);
    let op = Keypair::new(); e.svm.airdrop(&op.pubkey(), 10_000_000_000).unwrap();
    let h = H { v, sub, sub1, slab, f, op };
    rules(e, &h, 0, 0, 0, true);
    e.send("vault_slab_init", vec![AccountMeta::new(a, true), w(&v), r(&sub), w(&slab)], vec![58], &[], true);
    e.send("vault_join", vec![AccountMeta::new(a, true), r(&m), w(&f), w(&slab), r(&v)], vec![59], &[], true);
    h
}
fn rules(e: &mut Env, h: &H, edge: u64, tol: u64, fresh: u64, ok: bool) {
    let mut d = vec![82u8]; d.extend_from_slice(h.op.pubkey().as_ref()); for x in [edge, tol, fresh] { d.extend_from_slice(&x.to_le_bytes()); }
    let a = e.admin.pubkey();
    e.send("", vec![AccountMeta::new(a, true), w(&h.v)], d, &[], ok);
}
fn vquote(e: &mut Env, h: &H, by: &Keypair, v: &[(u16, u32, u32)], ok: bool) -> Option<u64> {
    e.send(if ok { "vault_quote" } else { "" }, vec![sig(by), r(&h.v), w(&h.slab)], items(79, v), &[by], ok)
}
/// by = None: the vault admin (the payer) signs
fn vfund(e: &mut Env, h: &H, by: Option<&Keypair>, amt: i64, tgt: Option<i64>, ok: bool) {
    let mut d = vec![80u8]; d.extend_from_slice(&amt.to_le_bytes()); if let Some(t) = tgt { d.extend_from_slice(&t.to_le_bytes()); }
    let (m, a) = (e.market, e.admin.pubkey());
    let (s0, sg): (AccountMeta, Vec<&Keypair>) = match by { Some(k) => (sig(k), vec![k]), None => (AccountMeta::new(a, true), vec![]) };
    e.send("", vec![s0, w(&h.sub), r(&m), w(&h.f), w(&h.v)], d, &sg, ok);
}
fn vkill(e: &mut Env, h: &H, by: &Keypair, on: u8, ok: bool) { e.send("", vec![sig(by), r(&h.v), w(&h.slab)], vec![81, on], &[by], ok); }
fn vmove(e: &mut Env, h: &H, by: &Keypair, from: &Address, to: &Address, amt: u64, ok: bool) {
    let mut d = vec![83u8]; d.extend_from_slice(&amt.to_le_bytes()); let m = e.market;
    e.send("", vec![sig(by), w(&h.v), w(from), w(to), r(&m), r(&h.f)], d, &[by], ok);
}
fn settle(e: &mut Env, h: &H) { let m = e.market; e.send("", vec![w(&h.sub), r(&m), w(&h.f)], vec![65], &[], true); }
fn crank(e: &mut Env, h: &H, rest: &[Address], ok: bool) {
    let m = e.market; let mut a = vec![w(&h.v), w(&h.sub), r(&m)]; a.extend(rest.iter().map(w));
    e.send("", a, vec![56], &[], ok);
}
fn holder(e: &mut Env, amt: u64) -> (Keypair, Address, Address) {
    let kp = Keypair::new(); e.svm.airdrop(&kp.pubkey(), 10_000_000_000).unwrap();
    let vd = Address::new_unique(); e.prog_acct(vd, VDEP_SIZE);
    let src = Address::new_unique(); let mint = Address::from(e.market_state().mint);
    e.svm.set_account(src, Account { lamports: 1_000_000_000, data: token_acct(&mint, &kp.pubkey(), amt), owner: TOKEN, executable: false, rent_epoch: 0 }).unwrap();
    (kp, vd, src)
}
fn vdep(e: &mut Env, h: &H, kp: &Keypair, vd: &Address, src: &Address, amt: u64, rest: &[Address], ok: bool) {
    let (m, tv) = (e.market, e.vault); let mut d = vec![53u8]; d.extend_from_slice(&amt.to_le_bytes());
    let mut a = vec![AccountMeta::new(kp.pubkey(), true), w(&h.v), w(vd), w(&h.sub), r(&m), w(&tv), w(src), r(&TOKEN)]; a.extend(rest.iter().map(w));
    e.send("", a, d, &[kp], ok);
}
fn vwd(e: &mut Env, h: &H, kp: &Keypair, vd: &Address, dst: &Address, sh: u64, rest: &[Address], ok: bool) {
    let (m, tv, pda) = (e.market, e.vault, e.vault_pda); let mut d = vec![54u8]; d.extend_from_slice(&sh.to_le_bytes());
    let mut a = vec![AccountMeta::new(kp.pubkey(), true), w(&h.v), w(vd), w(&h.sub), r(&m), w(&tv), w(dst), r(&TOKEN), r(&pda)]; a.extend(rest.iter().map(w));
    e.send("", a, d, &[kp], ok);
}
fn take(e: &mut Env, h: &H, kp: &Keypair, u: &Address, side: u8, limit: u64, qty: u64) {
    let m = e.market; let mut d = vec![66u8, side, 0, 1]; d.extend_from_slice(&limit.to_le_bytes()); d.extend_from_slice(&qty.to_le_bytes());
    e.send("", vec![AccountMeta::new(kp.pubkey(), true), w(u), r(&m), w(&h.f), r(&h.slab)], d, &[kp], true);
}
fn token_bal(e: &Env, k: &Address) -> u64 { u64::from_le_bytes(e.data(k)[64..72].try_into().unwrap()) }
/// What a sub-account is worth: its equity plus the margin it has locked in the book and fills not settled yet.
fn worth(e: &Env, h: &H, sub: &Address) -> i128 {
    let (u, m, b) = (e.user_state(sub), e.market_state(), fb(e, &h.f));
    let pos: i128 = u.pos.iter().filter(|p| p.market != [0; 32]).map(|p| p.base as i128 * m.mark_px as i128).sum();
    let mk = b.makers[..b.n as usize].iter().find(|x| x.user == sub.to_bytes());
    u.cash as i128 + pos + mk.map_or(0, |x| x.margin as i128 + x.quote as i128 + x.base as i128 * m.mark_px as i128)
}

#[test] fn the_vault_quotes_and_its_value_counts_locked_margin_and_settled_fills() {
    let mut e = Env::new(); let h = house(&mut e);
    let (a, av, asrc) = holder(&mut e, 10_000_000_000);
    vdep(&mut e, &h, &a, &av, &asrc, 10_000_000_000, &[], true);
    assert_eq!((vst(&e, &h.v).shares, vst(&e, &h.v).nav), (10_000_000_000, 10_000_000_000));
    // the admin locks $1,000 of the vault's cash as quoting margin: it leaves the account's cash for the book
    vfund(&mut e, &h, None, 1_000_000_000, None, true);
    assert_eq!(e.user_state(&h.sub).cash, 9_000_000_000);
    assert_eq!((fb(&e, &h.f).makers[0].margin, fb(&e, &h.f).makers[0].budget), (1_000_000_000, 10_000_000_000));
    // from now on the vault can't be valued without the book ...
    crank(&mut e, &h, &[], false);
    crank(&mut e, &h, &[h.f], true);
    assert_eq!(vst(&e, &h.v).nav, 10_000_000_000, "locked margin is still the vault's");
    // ... so a second depositor pays the full value: the same money buys the same shares, not 11% more
    let (b, bv, bsrc) = holder(&mut e, 10_000_000_000);
    vdep(&mut e, &h, &b, &bv, &bsrc, 10_000_000_000, &[], false);
    vdep(&mut e, &h, &b, &bv, &bsrc, 10_000_000_000, &[h.f], true);
    assert_eq!(vds(&e, &bv).shares, 10_000_000_000);
    assert_eq!(vst(&e, &h.v).nav, 20_000_000_000);
    // the operator posts a pegged two-sided quote; a taker buys 40 lots from the vault at the price + 10
    vquote(&mut e, &h, &h.op, &[PEGGED[0], PEGGED[1], (ASK, peg(10), 100), (0, peg(-10), 100)], true);
    let (tp, tu) = e.user(10_000_000_000);
    take(&mut e, &h, &tp, &tu, 0, PX + 100, 40);
    assert_eq!(e.user_state(&tu).pos[0].base, 40);
    let bk = fb(&e, &h.f); assert_eq!((bk.makers[0].base, bk.makers[0].quote), (-40, 100_010 * 40));
    // unsettled fills: no valuation, so neither a deposit nor a withdrawal can trade on a value that leaves them out
    crank(&mut e, &h, &[h.f], false);
    let (c, cv, csrc) = holder(&mut e, 1_000_000_000);
    vdep(&mut e, &h, &c, &cv, &csrc, 1_000_000_000, &[h.f], false);
    settle(&mut e, &h);
    crank(&mut e, &h, &[h.f], true);
    let nav = vst(&e, &h.v).nav as i128;
    assert_eq!(nav, worth(&e, &h, &h.sub));
    assert_eq!(nav, 20_000_000_000 + 10 * 40, "sold 40 lots 10 above the mark");
    assert_eq!(e.user_state(&h.sub).pos[0].base, -40);
}

#[test] fn the_operator_can_only_quote_inside_the_rules() {
    let mut e = Env::new(); let h = house(&mut e);
    let (a, av, asrc) = holder(&mut e, 10_000_000_000);
    vdep(&mut e, &h, &a, &av, &asrc, 10_000_000_000, &[], true);
    vfund(&mut e, &h, None, 1_000_000_000, None, true);
    let stranger = Keypair::new(); e.svm.airdrop(&stranger.pubkey(), 10_000_000_000).unwrap();
    let ok = [PEGGED[0], PEGGED[1], (ASK, peg(10), 100), (0, peg(-10), 100)];
    // only the operator's key, and only through the vault's instruction
    vquote(&mut e, &h, &stranger, &ok, false);
    let (m, opk) = (e.market, h.op.pubkey());
    e.send("", vec![AccountMeta::new(opk, true), w(&h.slab)], items(63, &ok), &[&h.op], false); // slab_quote
    e.send("", vec![AccountMeta::new(opk, true), w(&h.slab)], vec![67, 1], &[&h.op], false); // slab_kill
    let mut d = vec![64u8]; d.extend_from_slice(&(-1i64).to_le_bytes());
    e.send("", vec![AccountMeta::new(opk, true), w(&h.sub), r(&m), w(&h.f)], d, &[&h.op], false); // slab_fund
    // a fixed price is refused: a vault quote always moves with the price
    vquote(&mut e, &h, &h.op, &[(ASK, 100_010, 100)], false);
    vquote(&mut e, &h, &h.op, &[(0xC000 | ASK_LAD, 0, 0), (ASK, 100_010, 100)], false);
    // through the price is refused on either side: the vault never sells below it or buys above it
    vquote(&mut e, &h, &h.op, &[PEGGED[0], PEGGED[1], (ASK, peg(-1), 100)], false);
    vquote(&mut e, &h, &h.op, &[PEGGED[0], PEGGED[1], (0, peg(1), 100)], false);
    vquote(&mut e, &h, &h.op, &[PEGGED[0], PEGGED[1], (ASK, peg(0), 100), (0, peg(0), 100)], true);
    // a refused write leaves nothing behind, and settings alone can't unpeg a ladder that is offering
    vquote(&mut e, &h, &h.op, &[(0xC000 | ASK_LAD, 0, 0)], false);
    vquote(&mut e, &h, &h.op, &[(0xC000, 0, 0)], false);
    vquote(&mut e, &h, &h.op, &[(ASK + 1, peg(-3), 5)], false);
    vquote(&mut e, &h, &h.op, &ok, true);
    vquote(&mut e, &h, &h.op, &[(0xC000 | ASK_LAD, 5_000, 1)], true); // settings alone are fine while the ladder stays inside the rules
    // the admin demands 10 bps of edge (100 price units here), measured on a reference within 5% of the price
    rules(&mut e, &h, 10, 500, 0, true);
    vquote(&mut e, &h, &h.op, &[(ASK, peg(100), 100)], false); // no reference / tolerance on the ladder yet
    let refs = [(0x8000u16, PX as u32, 100u32), (0x8000 | ASK_LAD, PX as u32, 100)];
    vquote(&mut e, &h, &h.op, &[refs[0], refs[1], (ASK, peg(99), 100), (0, peg(-100), 100)], false);
    vquote(&mut e, &h, &h.op, &[refs[0], refs[1], (ASK, peg(100), 100), (0, peg(-99), 100)], false);
    vquote(&mut e, &h, &h.op, &[(0x8000, PX as u32, 600), (0x8000 | ASK_LAD, PX as u32, 600), (ASK, peg(100), 100), (0, peg(-100), 100)], false); // tolerance over the cap
    vquote(&mut e, &h, &h.op, &[(0x8000, 1_000, 100), (0x8000 | ASK_LAD, 1_000, 100), (ASK, peg(1), 100), (0, peg(-1), 100)], true); // a far-off reference passes the check ...
    let (tp, tu) = e.user(10_000_000_000);
    take(&mut e, &h, &tp, &tu, 0, PX + 1_000, 10);
    assert_eq!(e.user_state(&tu).pos[0].base, 0, "... and fills nothing: the ladder is outside its own tolerance");
    vquote(&mut e, &h, &h.op, &[refs[0], refs[1], (ASK, peg(100), 100), (0, peg(-100), 100)], true);
    take(&mut e, &h, &tp, &tu, 0, PX + 1_000, 10);
    assert_eq!(e.user_state(&tu).pos[0].base, 10);
    assert_eq!(fb(&e, &h.f).makers[0].quote, 100_100 * 10);
    // the admin demands a signed price: a ladder without the fresh setting is refused
    rules(&mut e, &h, 10, 500, 1, true);
    vquote(&mut e, &h, &h.op, &[(ASK, peg(100), 100)], false);
    vquote(&mut e, &h, &h.op, &[(0xC000, 0, 1 | 2 << 8), (0xC000 | ASK_LAD, 0, 1 | 2 << 8), (ASK, peg(100), 100), (0, peg(-100), 100)], true);
    // the rules themselves: an edge needs a tolerance cap, both are bounded, and only the admin sets them
    rules(&mut e, &h, 10, 0, 0, false); rules(&mut e, &h, 1001, 500, 0, false); rules(&mut e, &h, 10, 2001, 0, false);
    let mut d = vec![82u8]; d.extend_from_slice(stranger.pubkey().as_ref()); for x in [0u64, 0, 0] { d.extend_from_slice(&x.to_le_bytes()); }
    e.send("", vec![sig(&stranger), w(&h.v)], d, &[&stranger], false);
}

#[test] fn margin_cash_and_the_stop_follow_the_roles() {
    let mut e = Env::new(); let h = house(&mut e);
    let (a, av, asrc) = holder(&mut e, 10_000_000_000);
    vdep(&mut e, &h, &a, &av, &asrc, 10_000_000_000, &[], true);
    let stranger = Keypair::new(); e.svm.airdrop(&stranger.pubkey(), 10_000_000_000).unwrap();
    // more margin: the admin only. Less: the operator too. Nobody else, either way.
    vfund(&mut e, &h, Some(&h.op), 1_000_000_000, None, false);
    vfund(&mut e, &h, Some(&h.op), 0, Some(500_000_000), false);
    vfund(&mut e, &h, Some(&stranger), -1, None, false);
    vfund(&mut e, &h, None, 2_000_000_000, Some(2_000_000_000), true);
    vfund(&mut e, &h, Some(&stranger), -1_000_000_000, None, false);
    vfund(&mut e, &h, Some(&h.op), 0, Some(3_000_000_000), false);
    vfund(&mut e, &h, Some(&h.op), -500_000_000, Some(1_500_000_000), true);
    assert_eq!((fb(&e, &h.f).makers[0].margin, fb(&e, &h.f).makers[0].target), (1_500_000_000, 1_500_000_000));
    assert_eq!(e.user_state(&h.sub).cash, 8_500_000_000);
    // idle cash moves between the vault's own accounts and nowhere else; the receiving account then counts in the value
    let (_, outsider) = e.user(1_000_000);
    vmove(&mut e, &h, &stranger, &h.sub, &h.sub1, 1_000_000_000, false);
    vmove(&mut e, &h, &h.op, &h.sub, &outsider, 1_000_000_000, false);
    vmove(&mut e, &h, &h.op, &h.sub, &h.sub, 1_000_000_000, false);
    vmove(&mut e, &h, &h.op, &h.sub, &h.sub1, 9_000_000_000, false); // more than it has
    // the giving account is judged with its book: without it (it has margin locked there) nothing moves
    let mut d = vec![83u8]; d.extend_from_slice(&1_000_000_000u64.to_le_bytes()); let m = e.market;
    e.send("", vec![sig(&h.op), w(&h.v), w(&h.sub), w(&h.sub1), r(&m)], d, &[&h.op], false);
    vmove(&mut e, &h, &h.op, &h.sub, &h.sub1, 1_000_000_000, true);
    assert_eq!((e.user_state(&h.sub).cash, e.user_state(&h.sub1).cash), (7_500_000_000, 1_000_000_000));
    assert_eq!(vst(&e, &h.v).active, 0b11);
    crank(&mut e, &h, &[h.f], false); // sub1 is part of the vault now and must be passed
    crank(&mut e, &h, &[h.sub1, h.f], true);
    assert_eq!(vst(&e, &h.v).nav, 10_000_000_000, "moving cash inside the vault changes nothing");
    // the stop: admin or operator at any time; a stranger never while the vault runs
    let q = [PEGGED[0], PEGGED[1], (ASK, peg(10), 100), (0, peg(-10), 100)];
    vquote(&mut e, &h, &h.op, &q, true);
    vkill(&mut e, &h, &stranger, 1, false);
    vkill(&mut e, &h, &h.op, 1, true);
    let (tp, tu) = e.user(10_000_000_000);
    take(&mut e, &h, &tp, &tu, 0, PX + 100, 10);
    assert_eq!(e.user_state(&tu).pos[0].base, 0, "stopped quotes fill nothing");
    vkill(&mut e, &h, &h.op, 0, true);
    take(&mut e, &h, &tp, &tu, 0, PX + 100, 10);
    assert_eq!(e.user_state(&tu).pos[0].base, 10);
    // a halted vault: no new quotes, no more margin, and anyone may stop what is still posted (but not restart it)
    let adm = e.admin.pubkey(); let mut d = vec![57u8]; for x in [100u64, 2000, 2500, 1] { d.extend_from_slice(&x.to_le_bytes()); }
    e.send("", vec![AccountMeta::new(adm, true), w(&h.v)], d, &[], true);
    vquote(&mut e, &h, &h.op, &q, false);
    settle(&mut e, &h);
    vfund(&mut e, &h, None, 1, None, false);
    vfund(&mut e, &h, Some(&h.op), -1_000_000, None, true); // releasing still works
    vkill(&mut e, &h, &stranger, 0, false);
    vkill(&mut e, &h, &stranger, 1, true);
    take(&mut e, &h, &tp, &tu, 0, PX + 100, 10);
    assert_eq!(e.user_state(&tu).pos[0].base, 10);
}

#[test] fn depositors_carry_the_vaults_loss_and_are_paid_from_idle_cash() {
    let mut e = Env::new(); let h = house(&mut e);
    let (a, av, asrc) = holder(&mut e, 10_000_000_000);
    vdep(&mut e, &h, &a, &av, &asrc, 10_000_000_000, &[], true);
    vfund(&mut e, &h, None, 9_500_000_000, None, true); // nearly everything quotes
    vquote(&mut e, &h, &h.op, &[PEGGED[0], PEGGED[1], (ASK, peg(10), 1_000), (0, peg(-10), 1_000)], true);
    let (tp, tu) = e.user(100_000_000_000);
    take(&mut e, &h, &tp, &tu, 0, PX + 100, 1_000); // the vault is now short 1,000 lots at 100,010
    let s = e.slot; e.set_time(s + 101); e.oracle_update("", 1010_00000000, None); // +1% against it
    settle(&mut e, &h);
    crank(&mut e, &h, &[h.f], true);
    let nav = vst(&e, &h.v).nav as i128;
    assert_eq!(nav, worth(&e, &h, &h.sub));
    let mark = e.market_state().mark_px as i128;
    assert!((nav - (10_000_000_000 - (mark - 100_010) * 1_000)).abs() <= 100, "the loss on 1,000 lots sold at 100,010 (funding aside)");
    assert!(nav < 10_000_000_000);
    // a withdrawal is paid at the lower value, from idle cash only: with the cash locked in the book it waits ...
    let before = token_bal(&e, &asrc);
    vwd(&mut e, &h, &a, &av, &asrc, 1_000_000_000, &[h.f], false);
    // ... until margin is released (the operator can do that; it can't take it anywhere else)
    vfund(&mut e, &h, Some(&h.op), -2_000_000_000, None, true);
    vwd(&mut e, &h, &a, &av, &asrc, 1_000_000_000, &[], false); // and never without the book
    vwd(&mut e, &h, &a, &av, &asrc, 1_000_000_000, &[h.f], true);
    let got = (token_bal(&e, &asrc) - before) as i128;
    assert_eq!(got, 1_000_000_000 * (nav + 1_000_000) / (10_000_000_000 + 1_000_000), "a tenth of the shares, a tenth of what is left");
    // a new depositor buys in at the lower value too: more shares for the same money
    let (b, bv, bsrc) = holder(&mut e, 1_000_000_000);
    vdep(&mut e, &h, &b, &bv, &bsrc, 1_000_000_000, &[h.f], true);
    assert!(vds(&e, &bv).shares > 1_000_000_000);
}

#[test] fn a_vault_slab_and_an_ordinary_slab_never_mix() {
    let mut e = Env::new(); let h = house(&mut e);
    let (a, m) = (e.admin.pubkey(), e.market);
    // an ordinary maker with its own slab on the same book
    let (kp, u) = e.user(10_000_000_000);
    let own = Address::new_unique(); e.prog_acct(own, SLAB_SIZE);
    e.send("", vec![AccountMeta::new(kp.pubkey(), true), r(&u), w(&own)], vec![60], &[&kp], true);
    e.send("", vec![AccountMeta::new(a, true), r(&m), w(&h.f), w(&own), sig(&kp)], vec![62], &[&kp], true);
    let q = [PEGGED[0], PEGGED[1], (ASK, peg(10), 100), (0, peg(-10), 100)];
    // the operator can't write it, stop it, or have it approved as the vault's
    e.send("", vec![sig(&h.op), r(&h.v), w(&own)], items(79, &q), &[&h.op], false);
    e.send("", vec![sig(&h.op), r(&h.v), w(&own)], vec![81, 1], &[&h.op], false);
    e.send("", vec![AccountMeta::new(a, true), r(&m), w(&h.f), w(&own), r(&h.v)], vec![59], &[], false);
    // nobody makes an ordinary slab for a vault account, and a vault slab is only for the vault's own accounts
    let s2 = Address::new_unique(); e.prog_acct(s2, SLAB_SIZE);
    e.send("", vec![AccountMeta::new(kp.pubkey(), true), r(&h.sub1), w(&s2)], vec![60], &[&kp], false);
    e.send("", vec![AccountMeta::new(a, true), w(&h.v), r(&u), w(&s2)], vec![58], &[], false);
    e.send("", vec![sig(&kp), w(&h.v), r(&h.sub1), w(&s2)], vec![58], &[&kp], false); // not the vault's admin
    e.send("", vec![AccountMeta::new(a, true), w(&h.v), r(&h.sub1), w(&s2)], vec![58], &[], true);
    // a second vault's admin and operator have no say over the first vault's accounts
    let (v2, x2) = (Address::new_unique(), Address::new_unique()); e.prog_acct(v2, VAULT_SIZE); e.prog_acct(x2, USER_SIZE);
    let adm2 = Keypair::new(); e.svm.airdrop(&adm2.pubkey(), 10_000_000_000).unwrap();
    let mut d = vec![50u8]; for x in [100u64, 2000, 0, 2500] { d.extend_from_slice(&x.to_le_bytes()); }
    e.send("", vec![AccountMeta::new(adm2.pubkey(), true), w(&v2), r(&m)], d, &[&adm2], true);
    e.send("", vec![AccountMeta::new(adm2.pubkey(), true), w(&v2), w(&x2)], vec![51], &[&adm2], true);
    let mut d = vec![82u8]; d.extend_from_slice(adm2.pubkey().as_ref()); for x in [0u64, 0, 0] { d.extend_from_slice(&x.to_le_bytes()); }
    e.send("", vec![AccountMeta::new(adm2.pubkey(), true), w(&v2)], d, &[&adm2], true); // adm2 is also v2's operator
    e.send("", vec![sig(&adm2), r(&v2), w(&h.slab)], items(79, &q), &[&adm2], false); // v2's operator on v1's slab
    let mut d = vec![80u8]; d.extend_from_slice(&1_000i64.to_le_bytes());
    e.send("", vec![AccountMeta::new(adm2.pubkey(), true), w(&h.sub), r(&m), w(&h.f), w(&v2)], d.clone(), &[&adm2], false); // v1's account, v2's rules
    e.send("", vec![AccountMeta::new(adm2.pubkey(), true), w(&h.sub), r(&m), w(&h.f), w(&h.v)], d, &[&adm2], false);
    let mut d = vec![83u8]; d.extend_from_slice(&1u64.to_le_bytes());
    e.send("", vec![sig(&adm2), w(&v2), w(&h.sub), w(&x2), r(&m)], d, &[&adm2], false); // can't pull v1's cash into v2
}
