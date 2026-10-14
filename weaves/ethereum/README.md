# ethereum

**For:** a team that reads the chain all day and pays a hosted RPC by the request. Example: a 4-person wallet startup moving its reads to its own node on a dedicated server.

```
subweave run ethereum "mainnet full node holding only 20000 blocks, RPC on the private network" --on root@eth1
```

| ARG | Level | Default | Says |
|---|---|---|---|
| `network` | MUST | — | `mainnet`, `sepolia` or `hoodi` |
| `history` | SHOULD | `full` | `archive`, `full`, `minimal`, or `last <blocks> blocks`; reth keeps at least 10064 |
| `rpc` | SHOULD | `local` | JSON-RPC on `local`host only, or on the `private` network |
| `updates` | MAY | `auto` | `auto` installs new releases; `ask` asks before each update |

## Goals

```
installed
├─ running
│  ├─ sealed
│  ├─ fenced
│  ├─ peered
│  ├─ synced
│  │  ├─ moving
│  │  └─ current
│  └─ quiet
└─ room
```

## What happens when

1. **The request says "only 1000 blocks".** The decider fills `blocks = 1000`. `ASK IF blocks < 10064` is code, so the person is asked at `run`, before the first action; reth keeps 10064 blocks of history either way ([LIMITS §1](../../LIMITS.md#1-numbers-inside-prose)).
2. **The request does not name a network.** `network` is MUST: `plan` shows an error, and nothing runs.
3. **The request says "and run a validator too".** `NEVER "run a validator or hold staking keys"` refuses the whole request.
4. **First sync.** `install` passes within 30 minutes, then `synced` fails for hours to days; the decider answers `wait` every 10 minutes. `resync` gets `WITHIN 2d` for the same reason.
5. **A release notes a database migration.** `current` fails. The quoted `ASK` reads the notes, and the person answers on their channel, such as a Steery card, before `update` runs. With `updates ask`, every update asks.
6. **The coder tries `ufw allow 8551/tcp`.** The gate refuses it ([§5](../../LIMITS.md#5-gating-shell-commands-depends-on-their-stated-purpose)). If another change gets through and reth listens beyond localhost, `sealed` fails within a minute.
7. **The journal fills the disk again a month later.** `room` fails, and `free-disk` runs the commands this box learned the first time, in code, with no coder call ([§7](../../LIMITS.md#7-learned-commands)).

## Limits

- `NEEDS` asks for 2 TB, which fits `full` and pruned nodes. An `archive` request needs a bigger disk; the daemon checks `NEEDS`, not the ARG.
- A release note that hides a migration gets past the `ASK`. `rollback` puts back binaries, not a migrated database.
- No second node: a lost box is a full resync somewhere else ([§9](../../LIMITS.md#9-one-box-no-failover)).
- Log lines and release notes go to the decider; a crafted note can try to steer it ([§3](../../LIMITS.md#3-text-on-the-box-can-steer-the-decider)).
- No validator, no MEV relay, no slashing protection: the weave refuses them.
