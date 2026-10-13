# valkey

**For:** an agency or managed service provider that runs one small box per client. Example: a 5-person agency with 40 client apps, each with its own cache or session store on a 2 GB VM.

```
subweave run valkey "cache for acme-shop, 2048 MB, private network" --on root@acme-cache --name acme-cache
subweave run valkey "session store for bolt-bikes, 512 MB, private network, copies to backup@10.0.0.9:/srv/valkey/bolt" --on root@bolt-kv --name bolt-kv
subweave inbox
```

## Goals

| Goal | Holds when | Usual fix |
|---|---|---|
| `installed` | `valkey-server` exists | `install` |
| `running` | `PING` answers `PONG` | `restart` |
| `sized` | `maxmemory`, eviction and AOF match `role` and `memory` | `size` |
| `guarded` | local: only loopback listens; private: a password is set | `guard` (coder) |
| `room` | a store uses under 90% of `maxmemory` | `fix` (coder) |
| `persisted` | the last AOF write succeeded | `rewrite-aof` |
| `accepting` | no new rejected connections | `raise-clients`, `restart` |
| `copied` | a copy under 25 hours old, when `copies` is set | `backup` |
| `quiet` | decider: no warning lines that need action | `fix` (coder) |

Most goals are code, and most actions are `run` lines: Valkey reports its state in numbers.

## What happens when

1. **A client app leaks connections.** `rejected_connections` grows and `accepting` fails. The decider picks between `raise-clients` and `restart`.
2. **The decider picks `restart` for a 1.8 GB cache.** `ask before restart if role == "cache" and used > 1073741824` asks first: a cold cache sends every read to the client's database.
3. **A store with no copies.** `ask if role == "store" and copies == "none"` asks at `run`, before anything is installed.
4. **Monday morning.** `subweave inbox` lists the open questions from all 40 boxes in one place.

## Limits

- An answer covers one service. `always` on one client's box does not carry to the other 39.
- No replica. A store loses up to about one second of writes in a crash (`appendfsync everysec`), and up to a day when the box is lost.
- The Valkey version is Ubuntu's: 7.2 on 24.04. Newer releases need another package source.
- `room` for a store has no safe action: deleting keys is a `never`. The coder can only report, so the person decides.
