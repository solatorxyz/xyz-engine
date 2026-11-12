// The addresses everything else is built on: the program, the token program, the collateral. Nothing but constants,
// so the command line (cli/solator.ts, published with the kernel) and the site share one copy.
// (the command line reads SOLATOR_PROGRAM and SOLATOR_MINT; the site, whose pages can only see NEXT_PUBLIC_ names, those)
export const PROGRAM_ID =
  process.env.SOLATOR_PROGRAM ??
  process.env.NEXT_PUBLIC_PROGRAM_ID ??
  "7AY437YN1dupEhyahmKAQ6KTRm4nVx52BdyVzTcJbuRZ";
export const TOKEN_PROGRAM = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
export const ATA_PROGRAM = "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL";
/** The collateral of every listed market: the test dollar the faucet hands out (6 decimals). Another deployment (a
 *  local chain, another cluster) names its own with NEXT_PUBLIC_MINT. */
export const MINT =
  process.env.SOLATOR_MINT ?? process.env.NEXT_PUBLIC_MINT ?? "6hNBNCXegZxr2drnSydntdt5JYyv4m2Qtp23iM6teW6a";
