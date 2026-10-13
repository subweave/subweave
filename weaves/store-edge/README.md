# store-edge

**For:** retail IT with a small PC in every shop and an uplink that drops. Example: a 300-shop chain; each back office has a fanless box on DSL with LTE backup.

```
while read shop host tills; do
  subweave run ./store-edge "shop $shop, tills $tills, ship to https://influx.acme.example org acme bucket shops" --on root@$host --name shop-$shop
done < shops.txt
```

This weave has no coder and no `fix`. Every goal has one action or none, so it keeps working with no uplink and no model ([LIMITS §6](../../LIMITS.md#6-offline-means-code-only)).

## Goals

| Goal | Holds when | Action |
|---|---|---|
| `installed` | `telegraf` exists | `install` |
| `configured` | `/etc/default/telegraf` matches the settings and `telegraf --test` passes | `configure` |
| `running` | the service is active | `restart` |
| `room` | disk under 90% | `trim` |
| `draining` | the uplink is down, or the buffer is small or shrinking | `restart` |
| `tills` | every till answers ping | none: tells the person |
| `quiet` | decider: no log line that needs a person | none: tells the person |

## What happens when

1. **The uplink is down for two days.** `draining` holds, and Telegraf writes to its disk buffer. `quiet` stays unknown, and a crashed Telegraf still restarts.
2. **The uplink comes back.** The buffer drains. If it stops shrinking, `draining` fails and Telegraf restarts.
3. **Someone unplugs till 10.0.4.12.** `tills` fails, and the person gets a question. With no uplink, it waits in the box's inbox.
4. **Shop 114 gets a third till.** `subweave tell shop-114 "tills 10.0.4.11,10.0.4.12,10.0.4.13"` changes the setting; `configured` fails, and `configure` rewrites the files.

## Limits

- A slot takes one word of the request, so the till list has commas and no spaces.
- Telegraf's disk buffer is marked experimental by InfluxData. A long outage can fill the disk: `trim` frees logs, never metrics, and then the person is asked.
- Budget is `$1` a month per box, `$300` for the chain. The `quiet` check every 30 minutes costs about `$0.03` a month per box at the measured `$0.000023` a decision.
- No fleet view. `subweave ps` reaches 300 boxes one SSH session at a time.
