# Checkpoint sync for lighthouse

```
lighthouse bn --network {network} --datadir /var/lib/eth/lighthouse \
  --execution-endpoint http://127.0.0.1:8551 --execution-jwt /etc/eth/jwt.hex \
  --checkpoint-sync-url <url> --http
```

| Network | `<url>` |
|---|---|
| mainnet | `https://mainnet.checkpoint.sigp.io` |
| hoodi | `https://hoodi.checkpoint.sigp.io` |
| sepolia | `https://checkpoint-sync.sepolia.ethpandaops.io` |

More providers: https://eth-clients.github.io/checkpoint-sync-endpoints/

- Check the provider first: `curl -s <url>/eth/v1/beacon/states/finalized/finality_checkpoints`.
- The checkpoint applies only to an empty beacon database. To start again, stop lighthouse and remove `/var/lib/eth/lighthouse/beacon`.
- Lighthouse backfills old blocks after it follows the head; `sync_distance` is near 0 before backfill ends.
