# valkey

**For:** an agency or managed service provider that runs one small box per client. Example: a 5-person agency with 40 client apps, each with its own cache or session store on a 2 GB VM.

```
subweave run valkey "session store for bolt-bikes, 512 MB, private network, copies to backup@10.0.0.9:/srv/valkey/bolt" --on root@bolt-kv --name bolt-kv
```

| ARG | Level | Default | Says |
|---|---|---|---|
| `role` | MUST | — | `cache` evicts and keeps nothing; `store` keeps an AOF and never evicts |
| `memory` | SHOULD | `1024` | megabytes for data; `NEVER IF memory < 64` |
| `access` | SHOULD | `local` | `local` only, or `private` network with a password (`VALKEYCLI_AUTH`, asked only then) |
| `copies` | MAY | empty: `copied` off | an scp destination for nightly RDB copies |

## Goals

```
installed
└─ running
   ├─ sized
   ├─ guarded
   ├─ room   IF role == 'store'
   ├─ accepting
   ├─ persisted   IF role == 'store'
   ├─ copied   IF copies
   └─ quiet
```

Most facts are `valkey-cli` commands: Valkey reports its state in numbers, and the exact command is the knowledge.

## What happens when

1. **The request says "a Valkey for acme-shop".** `role` is MUST: a cache and a store differ in persistence and eviction, so `plan` shows an error and nothing runs.
2. **A store with no copies.** `ASK IF role == 'store' AND NOT copies` asks at `run`, before anything is installed.
3. **A client app leaks connections.** `rejected_connections` grows and `accepting` fails. The decider picks between `raise-clients` and `restart`; the `valkey-clients` skill tells the coder to name the leaking address.
4. **The decider picks `restart` for a 1.8 GB cache.** `ASK BEFORE restart IF role == 'cache' AND used > 1073741824` asks first, on the agency's channel: a cold cache sends every read to the client's database.
5. **Someone runs `CONFIG SET maxmemory-policy noeviction` on a cache by hand.** `sized` fails within a minute. `size` runs the commands this box learned when it first passed, in code, with no coder call ([LIMITS §7](../../LIMITS.md#7-learned-commands)). On Monday, `subweave inbox` lists the open asks from all 40 boxes in one place.

## Limits

- An answer covers one service. `always` on one client's box does not carry to the other 39.
- No replica. A store loses up to about one second of writes in a crash (`appendfsync everysec`), and up to a day when the box is lost ([§9](../../LIMITS.md#9-one-box-no-failover)).
- The Valkey version is Ubuntu's: 7.2 on 24.04. Newer releases need another package source.
- `room` for a store has no safe action: deleting keys is a `NEVER`. The coder can only report, so the person decides ([§4](../../LIMITS.md#4-the-decider-rarely-asks-on-its-own)).
- Warning lines can hold client addresses, and they go to the decider ([§10](../../LIMITS.md#10-data-on-the-box-leaves-it)).
