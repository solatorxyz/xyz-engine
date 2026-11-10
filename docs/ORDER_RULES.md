# Order rules

What an order does on Solator, for integrators and market makers. The constants are in `perp-core/src/spec.rs`; every rule
below has a test in `program-tests/tests/` (`micro.rs` for the ones added on 2026-11-10).

## Priority

Price first. At one price:

1. resting orders (shards), oldest arrival slot first, then first in the shard;
2. maker quotes (FastBook), oldest quote slot first, then the earlier slab.

A resting order has waited since it was placed; a quote only since its last re-quote. Before 2026-11-10 a quote won the tie.

Inside one `batch`, cancels run before places.

## Flags (place item, byte 1)

| Bit | Name | Meaning |
|---|---|---|
| 1 | `POST_ONLY` | Never takes. If it would cross the best order that can still trade, it is skipped (not an error). |
| 2 | `REDUCE_ONLY` | Capped at the position, immediate-or-cancel. Refused with `Margin` if the fill would leave equity below zero. |
| 4 | `IOC` | Fills what it can, the rest is dropped. |
| 8 | `FOK` | Fills in full or not at all. |
| 16 | `ALO_SLIDE` | With `POST_ONLY`: instead of being skipped, rests one tick behind the best live opposite order. |
| 32, 64 | STP mode | 0 cancel the resting order, 1 cancel the incoming one, 2 decrement both. |
| 128 | `IF_CANCELLED` | The place lands only if the cancel item at the same index (first 64) removed a live order. |

`POST_ONLY` with `REDUCE_ONLY` is refused (`BadData`).

`IF_CANCELLED` is the replace a maker wants: a quote that was filled or expired between the maker's read and its transaction
is not posted again at full size.

## Cancel items (side byte)

| Bit | Meaning |
|---|---|
| 0 | side (0 bid, 1 ask) |
| 64 | `CANCEL_ALL`: every resting order of the account in the passed shards of this market, both sides. `id` is ignored. |
| 128 | `BY_CID`: `id` is the client order id |

Cancelling an order that is already gone succeeds.

## A batch during a pause or a stale oracle

Cancels land. Place items that would add risk are skipped, like post-only and full-bucket skips. A single `place`
instruction still fails with `Paused` or `Stale`. Reduce-only orders are accepted in both states.

## Stops and take-profits

- Reduce-only, executed by any keeper once the trigger price is met (oracle by default; mark, or mark confirmed by oracle,
  by flag).
- `limit_px = 0` means market: the mark minus 10% for a sell, plus 10% for a buy (`TRIGGER_MARKET_SLIP_BPS`).
- A partial fill leaves the rest armed. The keeper tip is paid in proportion to each fill, so the whole tip is paid once.
- The keeper must pass every opposite shard and the market's FastBook, and the account's own-side shards when the account
  has resting orders (a fill it has not claimed yet changes what there is to reduce).
- A stop that has protected a position is cleared when the account goes flat in that market. One set before the entry
  order waits for the position.
- The account is never left below zero equity by a stop.

## TWAP

Each slice is what is left divided by the slices left, so a slice that met a thin book is made up by the later ones. The
order ends when its slices or its quantity run out.

## Funding

Hourly, on the oracle price, accrued every slot. Rate = average premium + clamp(interest - premium, +-0.05%), interest 0.01%
per 8 hours, capped at 0.5% an hour. The period is counted in slots (9,000), so it is an hour only while slots are 400 ms.

## Not there yet

- No acknowledgement event per order (assigned id, skipped / rested / filled). A client learns a skip from the book.
- A marketable limit order sent through `batch` matches resting orders only. To hit maker quotes, send `take_fast` first and
  rest the remainder.
- Maintenance margin is flat per market, with a position cap, not tiered by notional.
- Four trigger slots per account across all markets.
