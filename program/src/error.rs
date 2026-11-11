//! The program's error codes. Every refusal a client can see is one of these; the numbers are part of the spec
//! (`spec::ERR_*`, checked at compile time in `lib.rs`).

use super::*;

#[repr(u32)]
pub enum E {
    BadAccount = 1,
    BadData,
    Margin,
    Stale,
    NotLiquidatable,
    ShardFull,
    NoShard,
    Oracle,
    /// Retired (expired orders are skipped, not refused); the number stays so the ones after it keep theirs.
    #[allow(dead_code)]
    Expired,
    NotBankrupt,
    Math,
    Cap,
    Ledger,
    FokUnfilled,
    Paused,
    NotTriggered,
    Vault,
    TokenProgram,
    MissingShards,
    Rank,
    Timelock,
    Isolated,
    UseAdl,
    Breaker,
    NoFill,
    Locked,
    Halted,
    Assert,
}

#[inline(always)]
pub(crate) fn err(e: E) -> ProgramError {
    ProgramError::Custom(e as u32)
}
