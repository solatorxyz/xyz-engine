//! A signing lease: which one of a key's hosts may sign for a slot.
//!
//! Next-price execution rests on one promise: one price a market a slot. With one signer machine the promise is easy and
//! the machine is a single point of failure. With two machines holding the same key, a standby that takes over while
//! the primary is merely slow would sign a second price for a slot the primary already signed, and whoever executes an
//! order could pick between them. Timeouts cannot rule that out: no machine can tell "the other one is dead" from "I
//! cannot hear it".
//!
//! So the hosts do not decide between themselves. The chain does, in slots, which is the same clock the prices are
//! labelled with:
//!
//!   * the lease gives the slots `since..=expiry` to one member (`holder`);
//!   * the holder can only push `expiry` forward;
//!   * anyone else can take the lease only in a slot after `expiry`, and then holds from that slot on.
//!
//! A host signs the price for slot s only if the lease it has read from the chain names it as holder with
//! `since <= s <= expiry`. What a host has read can be old; it is never wrong in the direction that matters, because
//! the slots a member was once given are never given to anyone else. Two members' slots are therefore disjoint whatever
//! their clocks, their connections or the order their transactions land in (`lease_step_keeps_terms_apart` below, and
//! `program-tests/tests/lease.rs` against the built program).
//!
//! The lease is advice to the signers, not a check in the fill path: trading code does not read it. A key that ignores
//! its own lease can still sign what it likes, exactly as it could before; what the lease removes is the honest
//! failure, two well-behaved hosts disagreeing about who is alive.

use super::*;

/// What a claim does to a lease: `who` asks, in slot `now`, to hold through slot `until`.
/// Returns the new (holder, since, expiry), or why not. Pure, so that it can be proved (see the Kani harness).
#[inline(always)]
pub(crate) fn lease_step(
    holder: u64,
    since: u64,
    expiry: u64,
    who: u64,
    now: u64,
    until: u64,
) -> Result<(u64, u64, u64), E> {
    // A lease reaches a bounded way ahead: that bound is how long a dead holder keeps the others out.
    if until <= now || until - now > spec::LEASE_MAX_SLOTS {
        return Err(E::BadData);
    }
    if who == holder && now <= expiry {
        // The holder, still inside its term: forward only. Giving slots back would need everyone to agree on which.
        if until < expiry {
            return Err(E::BadData);
        }
        return Ok((holder, since, until));
    }
    // A new term (another member, or the old holder after it let the lease run out): only once the chain is past
    // every slot the last term covered, and only from here on.
    if now <= expiry {
        return Err(E::Locked);
    }
    Ok((who, now, until))
}

/// The key puts its signing under a lease and names the hosts. Once: the account must be new.
/// accs: key(s), lease(w, zeroed). data: tag, n u8, n x member [32]
pub(crate) fn lease_init(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 2 || d.len() < 2 {
        return Err(err(E::BadData));
    }
    let n = d[1] as usize;
    if n == 0 || n > LEASE_MEMBERS || d.len() != 2 + 32 * n {
        return Err(err(E::BadData));
    }
    if !sg(pid, &a[0]) || !a[1].owned_by(pid) || a[1].data_len() < LEASE_SIZE || disc_of(&a[1]) != 0 {
        return Err(err(E::BadAccount));
    }
    let mut members = [[0u8; 32]; LEASE_MEMBERS];
    for i in 0..n {
        members[i].copy_from_slice(&d[2 + 32 * i..34 + 32 * i]);
        // a member listed twice would hold two seats and could "take over" from itself; an empty seat is nobody
        if members[i] == [0; 32] || members[..i].contains(&members[i]) {
            return Err(err(E::BadData));
        }
    }
    let l = unsafe { &mut *(a[1].borrow_unchecked().as_ptr() as *mut Lease) };
    l.disc = DISC_LEASE;
    l.key = key(&a[0]);
    l.members = members;
    l.n = n as u64;
    // holder 0 with expiry 0: nobody holds any slot until the first claim
    Ok(())
}

/// A member takes the lease or the holder extends it. accs: member(s), lease(w). data: tag, until u64
pub(crate) fn lease_claim(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 2 || d.len() != 9 {
        return Err(err(E::BadData));
    }
    let l: &mut Lease = load(&a[1], pid, DISC_LEASE, LEASE_SIZE)?;
    if !sg(pid, &a[0]) {
        return Err(err(E::BadAccount));
    }
    let me = key(&a[0]);
    let who = l.members[..(l.n as usize).min(LEASE_MEMBERS)].iter().position(|m| *m == me).ok_or(err(E::BadAccount))?;
    let (holder, since, expiry) =
        lease_step(l.holder, l.since, l.expiry, who as u64, clock()?.slot, rd64(d, 1)?).map_err(err)?;
    if holder != l.holder || since != l.since {
        l.term += 1;
    }
    l.holder = holder;
    l.since = since;
    l.expiry = expiry;
    Ok(())
}

#[cfg(kani)]
mod proofs {
    use super::*;

    /// The whole safety argument, for every state and every claim (full 64-bit slots, no bounds):
    ///   * `expiry` never moves back, so it is always the last slot anyone was ever given;
    ///   * a renewal changes nothing but `expiry`;
    ///   * a new term starts in the current slot, strictly after that last slot.
    /// By induction over the claims, the slots of two different terms never overlap.
    #[kani::proof]
    fn lease_step_keeps_terms_apart() {
        let (holder, since, expiry): (u64, u64, u64) = (kani::any(), kani::any(), kani::any());
        let (who, now, until): (u64, u64, u64) = (kani::any(), kani::any(), kani::any());
        kani::assume(since <= expiry);
        if let Ok((h, s, e)) = lease_step(holder, since, expiry, who, now, until) {
            assert!(e >= expiry && s <= e && e > now && e - now <= spec::LEASE_MAX_SLOTS);
            if h == holder && s == since {
                assert!(who == holder && now <= expiry);
            } else {
                assert!(h == who && s == now && s > expiry);
            }
        }
    }

    /// Two members cannot both come out of the same slot holding it: with the lease in any state, a claim by someone
    /// who is not the holder succeeds only after the holder's last slot.
    #[kani::proof]
    fn lease_step_refuses_a_second_holder() {
        let (holder, since, expiry): (u64, u64, u64) = (kani::any(), kani::any(), kani::any());
        let (who, now, until): (u64, u64, u64) = (kani::any(), kani::any(), kani::any());
        kani::assume(who != holder && now <= expiry);
        assert!(lease_step(holder, since, expiry, who, now, until).is_err());
    }
}
