# solator: a market from a terminal

Anyone can open a market on the kernel: `init_market` called without a pool makes the signer the market's admin and
gives the market a vault and an insurance account of its own (`program/src/init.rs`). A trading account's cash is a
claim on exactly one vault, so whatever happens in such a market stays in it.

There is no screen for this. A market's creator is its price source and its operator: what they do is send signed
instructions, on a schedule, from a machine that stays on. This directory is the script for it, and for the one job
that anyone may do on anyone's market: liquidating.

**Research software. Unaudited. Devnet.** A market cannot be closed and its rent (about 0.13 SOL) is not returned.

```sh
cd ts && npm install
npm run solator -- help
```

## A creator's day

```sh
# the price feed, the market with its own vault and insurance, the order book: three transactions
npm run solator -- market create GOLD --dex xyz --price 2650.5 --cap 250000 --bond 12500

# keep it open: read a price, post it, book filled resting orders, liquidate
npm run solator -- price run <market> --from url:https://example.com/gold.json --json data.price
npm run solator -- price run <market> --from cmd:"./my-price.sh"

npm run solator -- market show <market>
npm run solator -- fees claim <market>          # half of the net trading fees; the other half stays as insurance
npm run solator -- bond add <market> 2500       # one way
npm run solator -- market cap <market> 500000
npm run solator -- market pause <market>
```

Every command takes `--url <rpc>` (default: devnet) and `--keypair <file>` (default: `~/.config/solana/id.json`).
`SOLATOR_PROGRAM` and `SOLATOR_MINT` in the environment name another deployment of the program or another collateral.

What the numbers mean:

- **The price.** The market takes what its creator's key posts. A post that moves the price more than 5% is held until
  the next post confirms it. When no price has arrived for as long as the market says a price is good (`--fresh`:
  1 minute, 10 minutes or 1 hour), it takes no orders and judges nobody until one does.
- **Margin.** 10% to open, 5% to keep (10x), and the program refuses a change below that.
- **The cap and the bond.** `--cap` limits open interest. The bond is money the creator pays into the market's
  insurance; it is the first money taken when a position loses more than its margin. A bond of a twentieth of the cap
  covers every position on one side losing one more full step of the price (5%) after its margin is gone. The program
  does not require a bond: `market show` says whether the market has one and whether it covers the cap.
- **Fees.** Takers pay 5 bps, makers are paid 2. Of what is left, half is the creator's (`fees claim`) and half goes
  into the market's insurance.

## Liquidating, for anyone

```sh
npm run solator -- liquidate run <market> [--earn]
```

No key of the market is involved. The program judges every account itself; this asks it (by simulating the
instruction) and sends what it accepts.

1. An account worth less than its maintenance margin has a quarter of its position at a time sold into (or bought
   from) the orders resting in the book: within 5% of the mark first, then 10%, then 20%. The fee is 0.25% of what is
   closed, twice that per wider step; with `--earn` a fifth of it is paid to the sender's trading account in that
   market, the rest goes to the market's insurance.
2. An account worth less than nothing is closed at the mark against the holders of the other side with the largest
   gain for their size. The loss is taken from the market's insurance first, then from those holders' gain on the lots
   closed; what still remains is written down as the market's bad debt.

An account between the two (under maintenance, still worth something, and no order within 20% of the mark) waits.
The creator's `price run` does the same work each turn, so a market whose creator is running it needs nobody else.

## Checking it

The scripts run the built program in a local VM behind the JSON-RPC methods the CLI uses, and judge every command by
what it prints and by what the chain holds afterwards.

```sh
(cd ../program && cargo build-sbf)
npm test          # the CLI end to end; then a long and a short through every state of a liquidation
```

| Path | What it is |
|---|---|
| `cli/solator.ts` | The command line. |
| `lib/createMarket.ts` | The instructions that open and run a market, and the arithmetic of its tick, lot, cap and bond. |
| `lib/keeper.ts` | Liquidation and auto-deleveraging: which accounts to ask about, and the instructions. |
| `lib/percolator.ts` | Finding such markets on chain. |
| `lib/sdk/` | Account layouts and constants generated from `perp-core/src/spec.rs`, and the encoders. |
| `scripts/` | The local chain and the two end-to-end tests. |
