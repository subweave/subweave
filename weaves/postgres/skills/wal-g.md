# wal-g

One config file, `/etc/wal-g/wal-g.json`, owner `postgres`, mode 600. Every wal-g command passes `--config /etc/wal-g/wal-g.json`.

Write it from the environment, so the keys never appear in a command:

```
install -d -o postgres -m 700 /etc/wal-g
jq -n --arg prefix "{bucket}" --arg id "$AWS_ACCESS_KEY_ID" --arg key "$AWS_SECRET_ACCESS_KEY" \
  '{WALG_S3_PREFIX: $prefix, AWS_ACCESS_KEY_ID: $id, AWS_SECRET_ACCESS_KEY: $key, PGHOST: "/var/run/postgresql", WALG_COMPRESSION_METHOD: "zstd"}' \
  > /etc/wal-g/wal-g.json
```

For an endpoint other than `aws`, add `AWS_ENDPOINT` (the HTTPS URL), `AWS_REGION` and `AWS_S3_FORCE_PATH_STYLE: "true"`.

Archiving, in `/etc/postgresql/{version}/main/conf.d/archive.conf`:

```
archive_mode = on
archive_command = 'wal-g --config /etc/wal-g/wal-g.json wal-push %p'
archive_timeout = 60
```

`archive_mode` needs a restart; `archive_command` needs only a reload.

Check:

```
sudo -u postgres wal-g --config /etc/wal-g/wal-g.json backup-list --detail --json
sudo -u postgres psql -Atc "select archived_count, failed_count, last_failed_wal from pg_stat_archiver"
```

A first base backup is needed before any restore can work: run the `backup` action after archiving starts.
