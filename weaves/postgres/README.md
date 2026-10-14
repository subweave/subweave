# postgres

**For:** a small product team with one database and no DBA. Example: 6 engineers, one Hetzner CPX31, backups in Hetzner Object Storage.

```
subweave run postgres "postgres 18 for our app, private network, backups to s3://acme-pg/db1 on https://fsn1.your-objectstorage.com, keep 14" --on root@db1 --name db
```

The daemon asks for the app's password and the two bucket keys once, then installs.

| ARG | Level | Default | Says |
|---|---|---|---|
| `version` | SHOULD | `18` | the PostgreSQL major release; `NEVER IF version < 14` |
| `access` | SHOULD | `local` | `local` or `private` (password and TLS) |
| `bucket` | SHOULD | — | an S3 URL for base backups and WAL; unsaid turns backups off |
| `endpoint` | MAY | — | an S3-compatible store other than Amazon S3 |
| `keep` | MAY | `7` | daily base backups to keep |
| `database` | MAY | `app` | the app's database and role |
| `updates` | MAY | `auto` | `auto`, or `ask` before each minor release |

```
installed
├─ configured
│  └─ running
│     ├─ archiving   IF bucket
│     │  └─ backed
│     │     └─ drilled
│     ├─ connections
│     ├─ short-tx
│     ├─ frozen
│     ├─ patched
│     ├─ current
│     └─ quiet
└─ room
```

One `RUN` step in the file, `systemctl restart`. Every other step is a `DO` task that the box learns once, and 16 of the 18 facts are quoted.

## What happens when

1. **The request says "backups to acme-pg".** The plan stops with an error: `acme-pg` is not an S3 URL ([LIMITS §6](../../LIMITS.md#6-checking-a-request)).
2. **The kernel kills postgres for memory.** `running` fails. The kernel log names the OOM killer, and the decider picks `restart` ([§2](../../LIMITS.md#2-the-action-description-is-the-decision)). A `warning` goes to the channels, then an `info` when it holds again.
3. **The first night.** `backed` fails, because no backup ever passed. The coder pushes one and the box learns the commands; every later night runs them as code. `drilled` then fails the same way: the coder restores the backup beside the live cluster, on port 5433, with archiving off.
4. **The bucket key is revoked.** `archiving` fails. `archive` and `fix` fail, the decision drops under 0.5, and the person gets an ask on Steery ([§4](../../LIMITS.md#4-the-decider-rarely-asks-on-its-own)).
5. **Transaction IDs age past 1 billion.** `frozen` fails. The coder finds what holds the oldest ID back before it vacuums; a stale replication slot is the person's to drop.
6. **`subweave tell db "move to postgres 18"` on a box on 17.** `current` fails, and `upgrade` asks first. `tell db "postgres 16"` is refused: `NEVER IF version < major`.
7. **The box is lost.** Run the same request `--on` a new box, then `subweave do db restore`.

## Limits

- No replica. A lost box costs the writes since the last WAL switch, up to `archive_timeout` (60 s), and hours of restore for a large database ([§9](../../LIMITS.md#9-one-box-no-failover)).
- Log lines can hold query values, and they go to the model providers ([§10](../../LIMITS.md#10-data-on-the-box-leaves-it)).
- `configured` is a model's reading of `pg_hba.conf` and the firewall. A wrong yes leaves a wrong rule in place.
- A full data disk runs the log cleanup once before the person is asked ([§4](../../LIMITS.md#4-the-decider-rarely-asks-on-its-own)).
- Extensions such as PostGIS are not an `ARG`: the request check warns, and major upgrades of them are the coder's.
- No pooler: `end-idle` ends sessions that an app may still hold.
