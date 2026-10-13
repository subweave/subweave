# Major upgrade with postgresql-common

1. Run the `backup` action first.
2. Install the new release's packages for every extension the old cluster uses: `postgresql-<new>-<extension>`.
3. Installing `postgresql-<new>` creates an empty `<new>/main` cluster. Drop it: `pg_dropcluster --stop <new> main`.
4. Upgrade: `pg_upgradecluster -v <new> -m upgrade <old> main`. The new cluster takes the old port; the old cluster stops and stays on disk.
5. Check: `pg_lsclusters`, then `select version()` and a count on the app's largest table.
6. Run the `backup` action again: archived WAL from the old cluster cannot restore the new one.

Never drop the old cluster: the person decides when, with `pg_dropcluster <old> main`.
