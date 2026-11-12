// The addresses everything else is built on: the program, the token program, the collateral. Nothing but constants,
// so the command line (cli/solator.ts, published with the kernel) and the site share one copy.
// (the command line reads SOLATOR_PROGRAM and SOLATOR_MINT; the site, whose pages can only see NEXT_PUBLIC_ names, those)
export const PROGRAM_ID =
  process.env.SOLATOR_PROGRAM ??
  process.env.NEXT_PUBLIC_PROGRAM_ID ??
  "7AY437YN1dupEhyahmKAQ6KTRm4nVx52BdyVzTcJbuRZ";
export const TOKEN_PROGRAM = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
export const ATA_PROGRAM = "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL";
/** The collateral of the devnet deployment's listed markets: the test dollar the site's faucet hands out (6 decimals).
 *  A market opened with any other token works on chain, but the site lists only markets on this one. Another
 *  deployment (a local chain, another cluster) names its own. */
export const MINT =
  process.env.SOLATOR_MINT ?? process.env.NEXT_PUBLIC_MINT ?? "65Y9os6EE8hWreCtd38QvFA4A8thtmVp9NsBwRG9Hsrd";
