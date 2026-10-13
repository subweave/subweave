# Settings for the box's size

Write them to `/etc/postgresql/{version}/main/conf.d/tuning.conf`, one file the weave owns.

| Setting | Value |
|---|---|
| `shared_buffers` | 25% of memory |
| `effective_cache_size` | 75% of memory |
| `maintenance_work_mem` | 5% of memory, at most 2GB |
| `work_mem` | (memory − shared_buffers) / (max_connections × 4), at least 4MB |
| `max_connections` | 100; more only with a pooler such as pgbouncer |
| `max_wal_size` | 10% of the data disk, at most 16GB |
| `random_page_cost` | 1.1 on SSD |

`shared_buffers` and `max_connections` need a restart. The rest need `select pg_reload_conf()`.
