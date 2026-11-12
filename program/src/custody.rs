//! Money in and out. Collateral sits in one SPL token account per market group (`Market.vault`); a user's `cash` is a
//! claim on it. A withdrawal is the only instruction that pays out to a user, and it requires the full, fresh view of
//! the account: every market it holds, every book it quotes on, every shard it rests orders in.

use super::*;

/// accs: authority(s), user(w), market, vault(w), src token(w), token program. data: tag, amount
pub(crate) fn deposit(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 6 {
        return Err(err(E::BadData));
    }
    let amt = rd64(d, 1)?;
    let u: &mut User = load(&a[1], pid, DISC_USER, USER_SIZE)?;
    let m: &Market = load(&a[2], pid, DISC_MARKET, MARKET_SIZE)?;
    if !sg(pid, &a[0]) || u.authority != key(&a[0]) || m.vault != key(&a[3]) {
        return Err(err(E::BadAccount));
    }
    if a[5].address() != &pinocchio_token::ID {
        return Err(err(E::TokenProgram));
    } // AUDIT C-1
    if u.home_vault == [0; 32] {
        u.home_vault = m.vault;
    } else if u.home_vault != m.vault {
        return Err(err(E::Vault));
    } // AUDIT C-2
    if m.paused != 0 {
        return Err(err(E::Paused));
    }
    if m.tvl_cap != 0 {
        let vd = unsafe { a[3].borrow_unchecked() };
        if rd64(vd, 64)?.saturating_add(amt) > m.tvl_cap {
            return Err(err(E::Cap));
        }
    }
    pinocchio_token::instructions::Transfer::new(&a[4], &a[3], &a[0], amt).invoke_with_program(a[5].address())?;
    u.cash = u.cash.checked_add(i64::try_from(amt).map_err(|_| err(E::Math))?).ok_or(err(E::Math))?;
    Ok(())
}

/// accs: authority(s), user(w), market, vault(w), dst token(w), token program, vault_pda, [shards to claim / extra markets]
pub(crate) fn withdraw(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 7 {
        return Err(err(E::BadData));
    }
    let amt = rd64(d, 1)?;
    let u: &mut User = load(&a[1], pid, DISC_USER, USER_SIZE)?;
    let m: &Market = load(&a[2], pid, DISC_MARKET, MARKET_SIZE)?;
    if !sg(pid, &a[0]) || u.authority != key(&a[0]) || m.vault != key(&a[3]) {
        return Err(err(E::BadAccount));
    }
    if a[5].address() != &pinocchio_token::ID {
        return Err(err(E::TokenProgram));
    } // AUDIT C-1
    home(u, m)?; // AUDIT C-2
    let uk = key(&a[1]);
    let clk = clock()?;
    let slot = clk.slot;
    let nw: Now = (slot, clk.unix_timestamp);
    // a committed next-price order can't be starved of its margin once its price is known
    if u.triggers.iter().any(|t| committed(t, slot)) {
        return Err(err(E::Locked));
    }
    claim_all(pid, u, &uk, &a[7..])?;
    complete_view(pid, u, &uk, &a[2..], None)?;
    settle_funding_all(pid, u, &a[2..])?;
    // Withdrawable = equity without positive unrealized PnL, minus the initial requirement (percolator Eq_withdraw).
    // Cash may go negative (a long's cash already paid its cost): what matters is the lane below, which is the same for longs
    // and shorts. Without positions it is exactly amt <= cash.
    u.cash = i64c(u.cash as i128 - amt as i128)?;
    let (eq, im) = health_np(pid, u, &a[2..], nw)?;
    if eq < im || eq < 0 {
        return Err(err(E::Margin));
    }
    let bump = [m.vault_bump as u8];
    let seeds = [Seed::from(b"vault".as_slice()), Seed::from(&m.vault_seed), Seed::from(&bump)];
    pinocchio_token::instructions::Transfer::new(&a[3], &a[4], &a[6], amt)
        .invoke_signed_with_program(&[Signer::from(&seeds)], a[5].address())?;
    Ok(())
}

/// The bond. Collateral paid into a market's insurance: the tokens go to the market's vault and the insurance balance
/// rises by the same amount, so the vault still holds exactly what it owes. Nothing takes it out but a loss. The
/// deposit cap (tvl_cap) is a limit on traders' money and does not apply.
/// accs: payer(s), market, insurance(w), vault(w), src token(w), token program. data: tag, amount
pub(crate) fn ins_fund(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 6 {
        return Err(err(E::BadData));
    }
    let amt = rd64(d, 1)?;
    let m: &Market = load(&a[1], pid, DISC_MARKET, MARKET_SIZE)?;
    let ins: &mut Insurance = load(&a[2], pid, DISC_INS, INS_SIZE)?;
    if !sg(pid, &a[0]) || m.insurance != key(&a[2]) || m.vault != key(&a[3]) {
        return Err(err(E::BadAccount));
    }
    if a[5].address() != &pinocchio_token::ID {
        return Err(err(E::TokenProgram));
    }
    if amt == 0 {
        return Err(err(E::BadData));
    }
    pinocchio_token::instructions::Transfer::new(&a[4], &a[3], &a[0], amt).invoke_with_program(a[5].address())?;
    ins.balance = i64c(ins.balance as i128 + amt as i128)?;
    Ok(())
}

/// A market without a pool pays out its fees. `Market.fees` is what takers paid less what makers were rebated, folded
/// in from the books by `aggregate`: tokens in the vault that are nobody's cash. FEE_CREATOR_BPS of it leaves the vault
/// for the token account the admin names; the rest becomes insurance and stays. A pooled market's fees sit in a vault
/// it shares, and are not paid out here.
/// accs: admin(s), market(w), insurance(w), vault(w), dst token(w), token program, vault_pda
pub(crate) fn fee_claim(pid: &Address, a: &mut [AccountView]) -> ProgramResult {
    if a.len() < 7 {
        return Err(err(E::BadData));
    }
    let m: &mut Market = load(&a[1], pid, DISC_MARKET, MARKET_SIZE)?;
    let ins: &mut Insurance = load(&a[2], pid, DISC_INS, INS_SIZE)?;
    if !auth(pid, &m.admin, a) || m.pool != [0; 32] || m.insurance != key(&a[2]) || m.vault != key(&a[3]) {
        return Err(err(E::BadAccount));
    }
    if a[5].address() != &pinocchio_token::ID {
        return Err(err(E::TokenProgram));
    }
    if m.fees <= 0 {
        return Err(err(E::BadData));
    }
    let fees = m.fees as u64;
    let out = (fees as u128 * spec::FEE_CREATOR_BPS as u128 / 10000) as u64; // rounds toward the insurance
    m.fees = 0;
    ins.balance = i64c(ins.balance as i128 + (fees - out) as i128)?;
    if out == 0 {
        return Ok(());
    }
    let bump = [m.vault_bump as u8];
    let seeds = [Seed::from(b"vault".as_slice()), Seed::from(&m.vault_seed), Seed::from(&bump)];
    pinocchio_token::instructions::Transfer::new(&a[3], &a[4], &a[6], out)
        .invoke_signed_with_program(&[Signer::from(&seeds)], a[5].address())
}

/// AUDIT C-2: cash is a claim on exactly one vault; every market an account touches must use that vault.
#[inline(always)]
pub(crate) fn home(u: &User, m: &Market) -> ProgramResult {
    if u.home_vault != m.vault {
        Err(err(E::Vault))
    } else {
        Ok(())
    }
}
