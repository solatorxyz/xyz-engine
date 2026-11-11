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
fn house(e: &mut Env) -> H { house_p(e, 0) }
fn house_p(e: &mut Env, perf: u64) -> H {
    e.oracle_update("", 1000_00000000, None);
    let (a, m) = (e.admin.pubkey(), e.market);
    let f = Address::new_unique(); e.prog_acct(f, FAST_SIZE);
    let mut d = vec![61u8]; d.extend_from_slice(&0u64.to_le_bytes());
    e.send("", vec![AccountMeta::new(a, true), w(&m), w(&f)], d, &[], true);
    let (v, sub, sub1, slab) = (Address::new_unique(), Address::new_unique(), Address::new_unique(), Address::new_unique());
    e.prog_acct(v, VAULT_SIZE); e.prog_acct(sub, USER_SIZE); e.prog_acct(sub1, USER_SIZE); e.prog_acct(slab, SLAB_SIZE);
    let mut d = vec![50u8]; for x in [100u64, 2000, perf, 2500] { d.extend_from_slice(&x.to_le_bytes()); }
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

#[test] fn the_performance_fee_goes_to_the_protocol_not_to_the_other_holders() {
    let mut e = Env::new(); let h = house_p(&mut e, 1000); // 10% of a holder's gain
    let (a, av, asrc) = holder(&mut e, 10_000_000_000);
    vdep(&mut e, &h, &a, &av, &asrc, 10_000_000_000, &[], true);
    vfund(&mut e, &h, None, 2_000_000_000, None, true);
    vquote(&mut e, &h, &h.op, &[PEGGED[0], PEGGED[1], (ASK, peg(10), 1_000), (0, peg(-10), 1_000)], true);
    let (tp, tu) = e.user(100_000_000_000);
    take(&mut e, &h, &tp, &tu, 0, PX + 100, 1_000); // the vault sells 1,000 lots at 100,010
    let s = e.slot; e.set_time(s + 101); e.oracle_update("", 990_00000000, None); // -1%: the vault's short is in profit
    settle(&mut e, &h); crank(&mut e, &h, &[h.f], true);
    let nav = vst(&e, &h.v).nav as i128; assert!(nav > 10_000_000_000);
    let (fees0, before) = (e.market_state().fees as i128, token_bal(&e, &asrc));
    let gross = 1_000_000_000i128 * (nav + 1_000_000) / (10_000_000_000 + 1_000_000);
    let fee = (gross - 1_000_000_000) / 10; assert!(fee > 0);
    // a fee is due, so the market's fee ledger is written: with the market read-only the withdrawal is refused
    vwd(&mut e, &h, &a, &av, &asrc, 1_000_000_000, &[h.f], false);
    let (m, tv, pda) = (e.market, e.vault, e.vault_pda); let mut d = vec![54u8]; d.extend_from_slice(&1_000_000_000u64.to_le_bytes());
    e.send("", vec![AccountMeta::new(a.pubkey(), true), w(&h.v), w(&av), w(&h.sub), w(&m), w(&tv), w(&asrc), r(&TOKEN), r(&pda), w(&h.f)], d, &[&a], true);
    assert_eq!((token_bal(&e, &asrc) - before) as i128, gross - fee, "the holder is paid the gain less the fee");
    assert_eq!(e.market_state().fees as i128 - fees0, fee, "the fee is the protocol's");
    assert_eq!(vst(&e, &h.v).nav as i128, nav - gross, "and none of it stays behind for the other holders");
    // no gain, no fee: a holder who bought in at the current value and leaves at it pays nothing and needs no writable market
    let (b, bv, bsrc) = holder(&mut e, 1_000_000_000);
    vdep(&mut e, &h, &b, &bv, &bsrc, 1_000_000_000, &[h.f], true);
    let s = e.slot; e.set_time(s + 101); e.oracle_update("", 990_00000000, None);
    let (fees1, sh) = (e.market_state().fees, vds(&e, &bv).shares);
    vwd(&mut e, &h, &b, &bv, &bsrc, sh, &[h.f], true);
    assert_eq!(e.market_state().fees, fees1);
}

// ---------- next-price execution ----------
const ED_ID: Address = Address::new_from_array([3, 125, 70, 214, 124, 147, 251, 190, 18, 249, 66, 143, 131, 141, 64, 255, 5, 112, 116, 73, 39, 244, 138, 100, 252, 202, 112, 68, 128, 0, 0, 0]);
const IXS_ID: Address = Address::new_from_array([6, 167, 213, 23, 24, 123, 209, 102, 53, 218, 212, 4, 85, 253, 194, 192, 193, 36, 198, 143, 33, 86, 117, 165, 219, 186, 203, 95, 8, 0, 0, 0]);
const NEXT: [(u16, u32, u32); 2] = [(0xC000, 0, 3), (0xC000 | ASK_LAD, 0, 3)]; // pegged, next-price only
fn msg(magic: &[u8; 8], m: &Address, px: u64, slot: u64) -> Vec<u8> { let mut v = magic.to_vec(); v.extend_from_slice(m.as_ref()); for x in [px, slot, slot * 400] { v.extend_from_slice(&x.to_le_bytes()); } v }
fn ed_ix(kp: &Keypair, m: &[u8]) -> Instruction {
    let base = 2 + 14; let (mut d, mut body) = (vec![1u8, 0], vec![]);
    let pk = base + body.len(); body.extend_from_slice(kp.pubkey().as_ref());
    let so = base + body.len(); body.extend_from_slice(kp.sign_message(m).as_ref());
    let mo = base + body.len(); body.extend_from_slice(m);
    for x in [so as u16, u16::MAX, pk as u16, u16::MAX, mo as u16, m.len() as u16, u16::MAX] { d.extend_from_slice(&x.to_le_bytes()); }
    d.extend(body); Instruction { program_id: ED_ID, accounts: vec![], data: d }
}
fn send_ixs(e: &mut Env, ixs: Vec<Instruction>, signers: &[&Keypair], ok: bool) {
    let mut s: Vec<&Keypair> = vec![&e.admin]; s.extend_from_slice(signers);
    let tx = Transaction::new_signed_with_payer(&ixs, Some(&e.admin.pubkey()), &s, e.svm.latest_blockhash());
    let res = e.svm.send_transaction(tx); e.svm.expire_blockhash();
    match (res, ok) { (Ok(_), true) | (Err(_), false) => {}, (Ok(_), false) => panic!("expected failure"), (Err(f), true) => panic!("{:?}\n{}", f.err, f.meta.logs.join("\n")) }
}
/// A vault whose rules allow next-price ladders only, quoting 100 lots each side 10 off the price; the book has a price signer.
fn house_next(e: &mut Env) -> (H, Keypair) {
    let h = house(e); let signer = Keypair::new(); let (a, m) = (e.admin.pubkey(), e.market);
    // a live market's signer goes through the timelock
    let mut d = vec![28u8, 12]; d.extend_from_slice(&0u64.to_le_bytes()); d.extend_from_slice(signer.pubkey().as_ref());
    e.send("", vec![AccountMeta::new(a, true), w(&m)], d, &[], true);
    let s = e.slot + 216_000; e.set_time(s); e.send("", vec![w(&m), w(&h.f)], vec![29], &[], true); e.oracle_update("", 1000_00000000, None);
    rules(e, &h, 0, 0, 2, true);
    let (dk, dv, dsrc) = holder(e, 10_000_000_000); vdep(e, &h, &dk, &dv, &dsrc, 10_000_000_000, &[], true);
    vfund(e, &h, None, 2_000_000_000, None, true);
    vquote(e, &h, &h.op, &[PEGGED[0], PEGGED[1], (ASK, peg(10), 100), (0, peg(-10), 100)], false); // the rules ask for next-price ladders
    vquote(e, &h, &h.op, &[NEXT[0], NEXT[1], (ASK, peg(10), 100), (0, peg(-10), 100)], true);
    (h, signer)
}
fn set_trig(e: &mut Env, kp: &Keypair, u: &Address, slot: u8, kind: u8, side: u8, v: [u64; 6], ok: bool) {
    let m = e.market; let mut d = vec![20u8, slot, kind, side]; for x in v { d.extend_from_slice(&x.to_le_bytes()); }
    e.send("", vec![AccountMeta::new(kp.pubkey(), true), w(u), r(&m)], d, &[kp], ok);
}
/// exec_trigger by keeper `kp` on `tu`'s trigger `si`; `arm`: commit a stop instead of filling it; `sig`: the signed price before it
fn exec(e: &mut Env, h: &H, kp: &Keypair, ku: &Address, tu: &Address, si: u8, arm: bool, sig: Option<Instruction>, ok: bool) {
    let m = e.market; let mut d = vec![21u8, si]; if arm { d.push(1); }
    let ix = Instruction { program_id: e.pid, accounts: vec![AccountMeta::new(kp.pubkey(), true), w(ku), w(tu), r(&m), w(&h.f), r(&h.slab), r(&IXS_ID)], data: d };
    send_ixs(e, sig.into_iter().chain([ix]).collect(), &[kp], ok);
}
fn to_slot(e: &mut Env, slot: u64, px: i64) { e.set_time(slot); e.oracle_update("", px, None); }
const NX: &[u8; 8] = b"etnextpx";

#[test] fn a_committed_order_fills_at_the_price_signed_for_its_slot_whatever_the_taker_knew() {
    let mut e = Env::new(); let (h, signer) = house_next(&mut e); let m = e.market;
    let (tp, tu) = e.user(10_000_000_000); let (kk, ku) = e.user(1_000_000);
    // an immediate take finds nothing: the vault's quotes are for committed orders only
    take(&mut e, &h, &tp, &tu, 0, PX + 500, 40);
    assert_eq!(e.user_state(&tu).pos[0].base, 0);
    // the taker has seen something the signed price has not (say the true price is already 0.5% up) and commits a buy at slot s
    let s = e.slot;
    set_trig(&mut e, &tp, &tu, 0, 4, 0, [0, PX + 2_000, 40, 0, 0, 0], true);
    // not before its pricing slot, even with a price in hand
    exec(&mut e, &h, &kk, &ku, &tu, 0, false, Some(ed_ix(&signer, &msg(NX, &m, 100_000, s + 2))), false);
    to_slot(&mut e, s + 2, 1005_00000000); // two slots on the signer has caught up: its price for slot s + 2 is 100,500
    // only that slot's price, in that message type, from the book's signer
    exec(&mut e, &h, &kk, &ku, &tu, 0, false, None, false);
    exec(&mut e, &h, &kk, &ku, &tu, 0, false, Some(ed_ix(&signer, &msg(NX, &m, 100_000, s + 1))), false);
    exec(&mut e, &h, &kk, &ku, &tu, 0, false, Some(ed_ix(&signer, &msg(NX, &m, 100_000, s + 3))), false);
    exec(&mut e, &h, &kk, &ku, &tu, 0, false, Some(ed_ix(&signer, &msg(b"etfastpx", &m, 100_500, s + 2))), false);
    exec(&mut e, &h, &kk, &ku, &tu, 0, false, Some(ed_ix(&kk, &msg(NX, &m, 100_500, s + 2))), false);
    exec(&mut e, &h, &kk, &ku, &tu, 0, false, Some(ed_ix(&signer, &msg(NX, &m, 103_000, s + 2))), false); // a price far from the oracle
    exec(&mut e, &h, &kk, &ku, &tu, 0, false, Some(ed_ix(&signer, &msg(NX, &m, 100_500, s + 2))), true);
    // filled 10 above the price of its own slot: what the taker knew at the commitment bought it nothing
    let b = fb(&e, &h.f); assert_eq!((b.makers[0].base, b.makers[0].quote), (-40, 100_510 * 40));
    assert_eq!(e.user_state(&tu).pos[0].base, 40);
    assert_eq!(e.user_state(&tu).triggers[0].kind, 0, "one shot");
    // late execution still uses the order's own slot price: nobody picks a better moment
    let s2 = e.slot;
    set_trig(&mut e, &tp, &tu, 0, 4, 1, [0, 0, 40, 0, 0, 0], true); // sell 40 at the next price
    to_slot(&mut e, s2 + 6, 1000_00000000);
    exec(&mut e, &h, &kk, &ku, &tu, 0, false, Some(ed_ix(&signer, &msg(NX, &m, 100_000, s2 + 6))), false);
    exec(&mut e, &h, &kk, &ku, &tu, 0, false, Some(ed_ix(&signer, &msg(NX, &m, 100_450, s2 + 2))), true);
    assert_eq!(e.user_state(&tu).pos[0].base, 0);
    assert_eq!(fb(&e, &h.f).makers[0].quote, 100_510 * 40 - 100_440 * 40);
}

#[test] fn a_commitment_cannot_be_taken_back_and_lapses_on_its_own() {
    let mut e = Env::new(); let (h, signer) = house_next(&mut e); let m = e.market;
    let (tp, tu) = e.user(10_000_000_000); let (kk, ku) = e.user(1_000_000);
    let s = e.slot;
    set_trig(&mut e, &tp, &tu, 0, 4, 0, [0, PX + 2_000, 40, 0, 0, 0], true);
    // no replacing it, no clearing it, no pulling the margin out from under it
    set_trig(&mut e, &tp, &tu, 0, 4, 1, [0, 0, 40, 0, 0, 0], false);
    set_trig(&mut e, &tp, &tu, 0, 0, 0, [0; 6], false);
    let dst = Address::new_unique(); let mint = Address::from(e.market_state().mint);
    e.svm.set_account(dst, Account { lamports: 1_000_000_000, data: token_acct(&mint, &tp.pubkey(), 0), owner: TOKEN, executable: false, rent_epoch: 0 }).unwrap();
    let wd = |e: &mut Env, ok: bool| { let (tv, pda) = (e.vault, e.vault_pda); let mut d = vec![14u8]; d.extend_from_slice(&1_000_000u64.to_le_bytes());
        e.send("", vec![AccountMeta::new(tp.pubkey(), true), w(&tu), r(&m), w(&tv), w(&dst), r(&TOKEN), r(&pda)], d, &[&tp], ok); };
    wd(&mut e, false);
    // another slot of the same account is free
    set_trig(&mut e, &tp, &tu, 1, 4, 0, [0, PX + 2_000, 10, 0, 0, 0], true);
    // forty-one slots after its pricing slot the price is too old: anyone clears it, nothing fills
    to_slot(&mut e, s + 2 + 41, 1000_00000000);
    exec(&mut e, &h, &kk, &ku, &tu, 0, false, Some(ed_ix(&signer, &msg(NX, &m, 100_000, s + 2))), true);
    assert_eq!((e.user_state(&tu).triggers[0].kind, e.user_state(&tu).pos[0].base), (0, 0));
    exec(&mut e, &h, &kk, &ku, &tu, 1, false, None, true);
    wd(&mut e, true);
    set_trig(&mut e, &tp, &tu, 0, 4, 1, [0, 0, 40, 0, 0, 0], true);
}

#[test] fn a_stop_is_committed_by_a_keeper_and_filled_at_the_next_price() {
    let mut e = Env::new(); let (h, signer) = house_next(&mut e); let m = e.market;
    let (tp, tu) = e.user(10_000_000_000); let (kk, ku) = e.user(1_000_000);
    let s = e.slot;
    set_trig(&mut e, &tp, &tu, 0, 4, 0, [0, PX + 2_000, 40, 0, 0, 0], true);
    to_slot(&mut e, s + 2, 1000_00000000);
    exec(&mut e, &h, &kk, &ku, &tu, 0, false, Some(ed_ix(&signer, &msg(NX, &m, 100_000, s + 2))), true);
    assert_eq!(e.user_state(&tu).pos[0].base, 40);
    // a stop-loss at 99,500 (market), tip 1,000
    set_trig(&mut e, &tp, &tu, 1, 2, 1, [99_500, 0, 40, 1_000, 0, 0], true);
    exec(&mut e, &h, &kk, &ku, &tu, 1, true, None, false); // not fired: nothing to commit
    to_slot(&mut e, s + 5, 990_00000000);
    // fired. Filling it on the spot is refused (the vault quotes for the next price only): the keeper commits it ...
    exec(&mut e, &h, &kk, &ku, &tu, 1, false, None, false);
    exec(&mut e, &h, &kk, &ku, &tu, 1, true, None, true);
    assert_ne!(e.user_state(&tu).triggers[1].kind & 1024, 0);
    set_trig(&mut e, &tp, &tu, 1, 2, 1, [90_000, 0, 40, 0, 0, 0], false); // ... and the owner can't move it out of the way now
    exec(&mut e, &h, &kk, &ku, &tu, 1, false, Some(ed_ix(&signer, &msg(NX, &m, 99_000, s + 7))), false); // too early
    to_slot(&mut e, s + 7, 985_00000000);
    let k0 = e.user_state(&ku).cash;
    exec(&mut e, &h, &kk, &ku, &tu, 1, false, Some(ed_ix(&signer, &msg(NX, &m, 98_500, s + 7))), true);
    let t = e.user_state(&tu); assert_eq!((t.pos[0].base, t.triggers[1].kind), (0, 0));
    assert_eq!(e.user_state(&ku).cash - k0, 1_000, "the keeper is paid for the fill");
    assert_eq!(fb(&e, &h.f).makers[0].quote, 100_010 * 40 - 98_490 * 40, "sold 10 under the price of the stop's own slot");
}
