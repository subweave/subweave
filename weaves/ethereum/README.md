# ethereum

**For:** a team that reads the chain all day and pays a hosted RPC by the request. Example: a 4-person wallet startup moving its reads to its own node on a dedicated server.

```
subweave run ethereum "full node holding only 1000 blocks, RPC on the private network" --on root@eth1
```

## Goals

| Goal | Holds when | Usual fix |
|---|---|---|
| `installed` | both binaries exist | `install` (coder) |
| `running` | both services are active | `restart` |
| `sealed` | nothing but localhost listens on port 8551 | `seal` (coder) |
| `room` | disk under 85% | `free-disk` |
| `peered` | 5 peers or more | `add-peers` |
| `synced` | lighthouse is under 64 slots behind | decider: `wait` while syncing |
| `moving` | the head grew since the last check | `restart`, `rollback`, `resync` (asks) |
| `current` | both clients run their newest release | `update` (coder) |
| `quiet` | decider: no warning lines that need action | `restart`, `fix` |

## What happens when

1. **The request says "only 1000 blocks".** The decider fills `blocks = 1000`. `ask if blocks < 10064` is code, so the person is asked before the first action ([LIMITS §1](../../LIMITS.md#1-numbers-inside-prose)).
2. **The request says "and run a validator too".** `never "run a validator or hold staking keys"` refuses the whole request.
3. **A release notes a database migration.** `current` fails. The quoted `ask` reads the notes, and the person answers before `update` runs.
4. **The coder tries `ufw allow 8551/tcp`.** The gate refuses it ([§5](../../LIMITS.md#5-gating-shell-commands-depends-on-their-stated-purpose)). If another change gets through and reth listens beyond localhost, `sealed` fails within a minute.

## Limits

- First sync takes hours to days. `synced` fails the whole time, and the decider answers `wait` every 10 minutes.
- `needs` asks for 2 TB, which fits `full` and pruned nodes. An `archive` request needs a bigger disk; the daemon checks `needs`, not the setting.
- A release note that hides a migration gets past the `ask`. `rollback` puts back binaries, not a migrated database.
- No validator, no MEV relay, no slashing protection: the weave refuses them.
