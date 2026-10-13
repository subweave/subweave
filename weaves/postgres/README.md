# postgres

**For:** a small product team with one database and no DBA. Example: 6 engineers, one Hetzner CPX31, backups in Hetzner Object Storage.

```
subweave run postgres "postgres 18 for our app, private network, backups to s3://acme-pg/db1 on https://fsn1.your-objectstorage.com, keep 14" --on root@db1 --name db
```

The daemon asks for the two bucket keys once, then installs.

## Goals

| Goal | Holds when | Usual fix |
|---|---|---|
| `installed` | PostgreSQL and wal-g binaries exist | `install` |
| `configured` | decider: `listen_addresses` and `pg_hba.conf` match `access` | `configure` (coder) |
| `running` | `pg_isready` | `restart` |
| `archiving` | the newest WAL archive attempt succeeded | `fix` (coder) |
| `backed` | a base backup under 25 hours old | `backup` |
| `room` | disk under 85% | `free-disk` |
| `connections` | under 80% of `max_connections` | `end-idle` |
| `patched` | no pending minor release | `patch` |
| `current` | the running major is `version` | `upgrade` (asks first) |
| `quiet` | decider: no warning lines that need action | `fix` (coder) |

## What happens when

1. **The kernel kills postgres for memory.** `running` fails. The kernel log names the OOM killer, and the decider picks `restart` ([LIMITS §2](../../LIMITS.md#2-the-action-description-is-the-decision)).
2. **The bucket key is revoked.** `archiving` and `backed` fail. Two backups and one `fix` fail, `tries` runs out, and the person gets a question ([§4](../../LIMITS.md#4-the-decider-rarely-asks-on-its-own)).
3. **A box on 17 gets `subweave tell db "move to postgres 18"`.** `installed` fails until 18 is installed; `current` fails until `upgrade` runs, after the person says yes.
4. **The box is lost.** Run the same request `--on` a new box, then `subweave do db restore`.

## Limits

- No replica. A lost box costs the writes since the last WAL switch, up to `archive_timeout` (60 s), and hours of restore for a large database.
- Log lines can hold query values, and they go to the model providers ([§8](../../LIMITS.md#8-data-on-the-box-goes-to-the-model-providers)).
- `configured` is a model's reading of `pg_hba.conf`. A wrong yes leaves a wrong file in place.
- Extensions the skills do not name, such as PostGIS or TimescaleDB, leave major upgrades to the coder.
- No pooler: `end-idle` ends sessions that an app may still hold.
