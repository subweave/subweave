# store-edge

**For:** retail IT with a small PC in every shop and an uplink that drops. Example: a 300-shop chain; each back office has a fanless box on DSL with LTE backup.

```
while read shop host tills; do
  subweave run ./store-edge "shop $shop, tills $tills, ship to https://influx.acme.example org acme bucket shops" --on root@$host --name shop-$shop
done < shops.txt
```

This weave has no coder, no `fix`, no quoted fact and no `DO` step. Every goal has exactly one action, so it keeps working with no uplink and no model ([LIMITS §8](../../LIMITS.md#8-offline-means-code-only)).

| ARG | Level | Default | Says |
|---|---|---|---|
| `shop` | MUST | — | the shop number, the `shop` tag on every metric |
| `url` | MUST | — | the central InfluxDB v2 server |
| `org` | MUST | — | its organization |
| `bucket` | MUST | — | the bucket for every shop's metrics |
| `tills` | SHOULD | empty: `tills-up` off | till addresses, commas and no spaces |
| `interval` | MAY | `10s` | how often Telegraf collects and sends; `NEVER IF interval < 1s` |

## Goals

```
installed
├─ configured
│  └─ running
│     ├─ draining
│     ├─ quiet
│     └─ tills-up   IF tills
├─ room
├─ fenced
└─ patched
```

## What happens when

1. **The uplink is down for two days.** `draining` holds, and Telegraf writes to its disk buffer. `quiet` stays unknown, and a crashed Telegraf still restarts within 2 minutes, in code.
2. **The uplink comes back.** The buffer drains. If it stops shrinking, `draining` fails and Telegraf restarts.
3. **Someone unplugs till 10.0.4.12.** `tills-up` fails. `recheck-tills` pings five times and writes the neighbour entry to the journal; with `TRIES 1 PER 1d`, the next failure asks the person on their channels. With no uplink, the ask waits in the box's inbox.
4. **The token is revoked.** Telegraf logs `E!` lines on every flush. `quiet` fails, `restart` runs once, and the next failure that day asks the person.
5. **Shop 114 gets a third till.** `subweave tell shop-114 "tills 10.0.4.11,10.0.4.12,10.0.4.13"` changes the ARG; `configured` fails, and `configure` rewrites the files.

## Limits

- A value takes one word of the request, so the till list has commas and no spaces.
- Telegraf's disk buffer is marked experimental by InfluxData. A long outage can fill the disk: `trim` frees logs, never metrics, and then the person is asked.
- The `quiet` check every 30 minutes is the only model call: about `$0.03` a month per box, `$10` for the chain, at the measured `$0.000022` a decision ([§12](../../LIMITS.md#12-speed-and-cost)).
- No fleet view. `subweave ps` reaches 300 boxes one SSH session at a time.
- A dead box sends nothing: pair each with a `heartbeat` channel ([§9](../../LIMITS.md#9-one-box-no-failover)).
