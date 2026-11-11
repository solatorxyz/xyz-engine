// The signing lease (program/src/lease.rs), against the built program.
//
// What must hold: of the hosts that share a signing key, at most one is ever given a slot. A host signs the price for
// slot s only while the lease it read from the chain says `holder == me && since <= s <= expiry`; the tests keep, for
// every member, the set of slots it was ever told it holds, and those sets must never meet.
#[path = "../../program/src/state.rs"]
#[allow(dead_code)]
mod state;
include!("common.rs");

const IX_LEASE_INIT: u8 = 85;
const IX_LEASE_CLAIM: u8 = 86;
const MAX: u64 = 300; // spec::LEASE_MAX_SLOTS
const BAD_ACCOUNT: u32 = 1;
const BAD_DATA: u32 = 2;
const LOCKED: u32 = 26;

struct Hosts {
    e: Env,
    lease: Address,
    key: Keypair,
    members: Vec<Keypair>,
}

fn init_data(members: &[Address]) -> Vec<u8> {
    let mut d = vec![IX_LEASE_INIT, members.len() as u8];
    for m in members {
        d.extend_from_slice(m.as_ref());
    }
    d
}

impl Hosts {
    /// A signing key that has put itself under a lease shared by `n` hosts.
    fn new(n: usize) -> Hosts {
        let mut e = Env::new();
        let (lease, key) = (Address::new_unique(), Keypair::new());
        let members: Vec<Keypair> = (0..n).map(|_| Keypair::new()).collect();
        e.prog_acct(lease, LEASE_SIZE);
        let keys: Vec<Address> = members.iter().map(|m| m.pubkey()).collect();
        let accs = vec![AccountMeta::new_readonly(key.pubkey(), true), w(&lease)];
        e.send("lease_init", accs, init_data(&keys), &[&key], true);
        Hosts { e, lease, key, members }
    }
    fn state(&self) -> Lease {
        unsafe { std::ptr::read_unaligned(self.e.data(&self.lease).as_ptr() as *const Lease) }
    }
    /// Member `i` asks to hold through slot `until`. Ok, or the program's error code.
    fn claim_by(&mut self, who: &Keypair, until: u64) -> Result<(), u32> {
        let mut d = vec![IX_LEASE_CLAIM];
        d.extend_from_slice(&until.to_le_bytes());
        let ix = Instruction {
            program_id: self.e.pid,
            accounts: vec![AccountMeta::new_readonly(who.pubkey(), true), w(&self.lease)],
            data: d,
        };
        let payer = &self.e.admin;
        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&payer.pubkey()),
            &[payer, who],
            self.e.svm.latest_blockhash(),
        );
        let res = self.e.svm.send_transaction(tx);
        self.e.svm.expire_blockhash();
        match res {
            Ok(_) => Ok(()),
            Err(f) => match f.err {
                solana_transaction::TransactionError::InstructionError(
                    _,
                    solana_instruction::error::InstructionError::Custom(c),
                ) => Err(c),
                other => panic!("unexpected failure: {other:?}"),
            },
        }
    }
    fn claim(&mut self, i: usize, until: u64) -> Result<(), u32> {
        let who = self.members[i].insecure_clone();
        self.claim_by(&who, until)
    }
}

#[test]
fn only_the_key_itself_puts_its_signing_under_a_lease() {
    let mut h = Hosts::new(2);
    let l = h.state();
    assert_eq!((l.key, l.n, l.expiry, l.term), (h.key.pubkey().to_bytes(), 2, 0, 0));

    // someone else's signature does not bind this key, and an initialised lease cannot be re-made with other hosts
    let (other, fresh) = (Keypair::new(), Address::new_unique());
    h.e.prog_acct(fresh, LEASE_SIZE);
    let keys = [other.pubkey()];
    let unsigned = vec![AccountMeta::new_readonly(h.key.pubkey(), false), w(&fresh)];
    h.e.send("", unsigned, init_data(&keys), &[], false);
    let again = vec![AccountMeta::new_readonly(h.key.pubkey(), true), w(&h.lease)];
    let key = h.key.insecure_clone();
    h.e.send("", again, init_data(&keys), &[&key], false);
    assert_eq!(h.state().n, 2);

    // no members, too many, the same host twice, an empty seat: all refused
    let signed = |fresh: &Address| vec![AccountMeta::new_readonly(key.pubkey(), true), w(fresh)];
    let five: Vec<Address> = (0..5).map(|_| Address::new_unique()).collect();
    for bad in [vec![], five, vec![keys[0], keys[0]], vec![keys[0], Address::default()]] {
        h.e.send("", signed(&fresh), init_data(&bad), &[&key], false);
    }
}

#[test]
fn a_member_takes_the_lease_and_keeps_it_by_renewing() {
    let mut h = Hosts::new(2);
    h.e.set_time(2000);
    assert_eq!(h.claim(0, 2100), Ok(()));
    let l = h.state();
    assert_eq!((l.holder, l.since, l.expiry, l.term), (0, 2000, 2100, 1));

    // renewing moves the end and nothing else; it cannot give slots back or reach further than the limit
    h.e.set_time(2050);
    assert_eq!(h.claim(0, 2200), Ok(()));
    let l = h.state();
    assert_eq!((l.holder, l.since, l.expiry, l.term), (0, 2000, 2200, 1));
    assert_eq!(h.claim(0, 2199), Err(BAD_DATA));
    assert_eq!(h.claim(0, 2050 + MAX + 1), Err(BAD_DATA));
    assert_eq!(h.claim(0, 2050), Err(BAD_DATA)); // a lease that ends now covers nothing ahead
    assert_eq!(h.state().expiry, 2200);
}

#[test]
fn nobody_else_takes_it_before_it_runs_out() {
    let mut h = Hosts::new(3);
    h.e.set_time(2000);
    assert_eq!(h.claim(0, 2100), Ok(()));

    // the standby cannot hear the holder and tries to take over: refused through the holder's last slot
    for slot in [2000, 2050, 2100] {
        h.e.set_time(slot);
        assert_eq!(h.claim(1, slot + 100), Err(LOCKED));
    }
    // one slot later it can, and it holds from that slot, not from any slot the first holder had
    h.e.set_time(2101);
    assert_eq!(h.claim(1, 2201), Ok(()));
    let l = h.state();
    assert_eq!((l.holder, l.since, l.expiry, l.term), (1, 2101, 2201, 2));

    // the old holder comes back: its renewal is now a claim against the new holder, and waits like anyone's
    assert_eq!(h.claim(0, 2300), Err(LOCKED));
    // a key that is not a member, and a member that did not sign, get nowhere
    let stranger = Keypair::new();
    assert_eq!(h.claim_by(&stranger, 2300), Err(BAD_ACCOUNT));
}

#[test]
fn a_holder_that_let_it_lapse_starts_a_new_term() {
    let mut h = Hosts::new(2);
    h.e.set_time(2000);
    assert_eq!(h.claim(0, 2010), Ok(()));
    h.e.set_time(2500);
    assert_eq!(h.claim(0, 2600), Ok(()));
    let l = h.state();
    // the gap 2011..2499 belongs to nobody: the new term starts now
    assert_eq!((l.holder, l.since, l.expiry, l.term), (0, 2500, 2600, 2));
}

/// Hosts with bad clocks and bad timing: each picks a moment and a reach at random, and believes whatever the chain last
/// told it. No slot is ever given to two of them, over thousands of claims in any order.
#[test]
fn two_hosts_are_never_given_the_same_slot() {
    let mut h = Hosts::new(3);
    let mut rng = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = move |n: u64| {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng % n
    };
    let mut owner: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
    let (mut slot, mut taken, mut refused, mut terms) = (2000u64, 0u32, 0u32, 0u64);
    for _ in 0..4000 {
        slot += next(40); // sometimes the same slot again, sometimes far past the holder's end
        h.e.set_time(slot);
        let who = next(3) as usize;
        let until = slot + next(MAX + 20); // sometimes not ahead at all, sometimes beyond the limit
        match h.claim(who, until) {
            Ok(()) => {
                taken += 1;
                let l = h.state();
                assert_eq!(l.holder as usize, who);
                assert!(l.since <= slot && slot < l.expiry && l.expiry - slot <= MAX && l.term >= terms);
                terms = l.term;
                // this is everything the host may now sign for
                for s in l.since..=l.expiry {
                    let was = *owner.entry(s).or_insert(who);
                    assert_eq!(was, who, "slot {s} was given to host {was} and now to host {who}");
                }
            }
            Err(code) => {
                refused += 1;
                assert!(code == LOCKED || code == BAD_DATA);
            }
        }
    }
    // the run must have exercised both outcomes and several changes of hands to mean anything
    assert!(taken > 500 && refused > 500 && terms > 50, "taken {taken} refused {refused} terms {terms}");
}
