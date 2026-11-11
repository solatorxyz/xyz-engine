//! Reading accounts and deciding who signed.
//!
//! `load` is the one place the program turns account bytes into a typed reference. It checks owner, size and
//! discriminator, then hands out `&'static mut T` straight into the account's data: zero-copy, and outside the borrow
//! checker. The rule that keeps this sound is in the callers: no path keeps a `&mut` to an account while it loads the
//! same account again (docs/ARCHITECTURE.md, "Zero-copy"); `program-tests/diff-build.sh` reruns the suites at other
//! optimisation levels to catch any behaviour that would lean on the optimiser's view of those references.

use super::*;

#[inline(always)]
pub(crate) fn load<T>(a: &AccountView, pid: &Address, disc: u64, size: usize) -> Result<&'static mut T, ProgramError> {
    if !a.owned_by(pid) || a.data_len() < size {
        return Err(err(E::BadAccount));
    }
    let p = unsafe { a.borrow_unchecked() }.as_ptr() as *mut T;
    if unsafe { *(p as *const u64) } != disc {
        return Err(err(E::BadAccount));
    }
    Ok(unsafe { &mut *p })
}

#[inline(always)]
pub(crate) fn disc_of(a: &AccountView) -> u64 {
    if a.data_len() < 8 {
        return 0;
    }
    unsafe { *(a.borrow_unchecked().as_ptr() as *const u64) }
}

#[inline(always)]
pub(crate) fn key(a: &AccountView) -> Key {
    a.address().to_bytes()
}

pub(crate) fn rd64(d: &[u8], o: usize) -> Result<u64, ProgramError> {
    d.get(o..o + 8).map(|b| u64::from_le_bytes(b.try_into().unwrap())).ok_or(err(E::BadData))
}

/// The admin a pool / vault is created with: the creating signer, or (data) a Multisig account that is among the accounts.
pub(crate) fn admin_key(
    pid: &Address,
    a: &[AccountView],
    signer: &Key,
    wanted: Option<&[u8]>,
) -> Result<Key, ProgramError> {
    let Some(w) = wanted else { return Ok(*signer) };
    let mut k = [0u8; 32];
    k.copy_from_slice(w);
    if k == *signer {
        return Ok(k);
    }
    let x = a.iter().find(|x| key(x) == k).ok_or(err(E::BadAccount))?;
    load::<Multisig>(x, pid, DISC_MULTISIG, MULTISIG_SIZE)?;
    Ok(k)
}

/// Admin authorization: a signer equal to `admin`, or `admin` is a Multisig account (passed) and at least m of
/// its distinct keys signed.
/// A signature counts only from an account the program doesn't own. Program-owned accounts (vaults, multisigs, users, ...) are
/// created from keypairs, so whoever kept such a keypair could otherwise sign as that account: as a vault sub-account's
/// authority, or as a multisig acting alone.
#[inline(always)]
pub(crate) fn sg(pid: &Address, x: &AccountView) -> bool {
    x.is_signer() && !x.owned_by(pid)
}

pub(crate) fn auth(pid: &Address, admin: &Key, a: &[AccountView]) -> bool {
    if a.iter().any(|x| sg(pid, x) && key(x) == *admin) {
        return true;
    }
    let Some(ms) = a.iter().find(|x| key(x) == *admin) else { return false };
    let Ok(ms) = load::<Multisig>(ms, pid, DISC_MULTISIG, MULTISIG_SIZE) else { return false };
    let mut got = 0u8;
    let mut cnt = 0u64;
    for x in a {
        if !sg(pid, x) {
            continue;
        }
        let k = key(x);
        for i in 0..(ms.n as usize).min(8) {
            if ms.keys[i] == k && got & (1 << i) == 0 {
                got |= 1 << i;
                cnt += 1;
            }
        }
    }
    cnt >= ms.m
}

/// `admin` is a Multisig account (passed) and at least one of its keys signed.
pub(crate) fn any_member(pid: &Address, admin: &Key, a: &[AccountView]) -> bool {
    let Some(ms) = a.iter().find(|x| key(x) == *admin) else { return false };
    let Ok(ms) = load::<Multisig>(ms, pid, DISC_MULTISIG, MULTISIG_SIZE) else { return false };
    a.iter().any(|x| {
        sg(pid, x) && {
            let k = key(x);
            (0..(ms.n as usize).min(8)).any(|i| ms.keys[i] == k)
        }
    })
}
