# Restore the newest wal-g backup

The daemon runs this recipe before it calls the coder. The old data directory stays on disk.

```recipe
systemctl stop postgresql@{version}-main
mv /var/lib/postgresql/{version}/main /var/lib/postgresql/{version}/main.before-restore-$(date +%Y%m%d%H%M)
sudo -u postgres wal-g --config /etc/wal-g/wal-g.json backup-fetch /var/lib/postgresql/{version}/main LATEST
echo "restore_command = 'wal-g --config /etc/wal-g/wal-g.json wal-fetch %f %p'" > /etc/postgresql/{version}/main/conf.d/restore.conf
sudo -u postgres touch /var/lib/postgresql/{version}/main/recovery.signal
systemctl start postgresql@{version}-main
```

Recovery replays every archived WAL segment, then promotes and removes `recovery.signal`. Until then `pg_isready` answers "rejecting connections"; that is progress, not failure.

When `backup-fetch` fails, the bucket or the keys are wrong: check `backup-list` before anything else.
