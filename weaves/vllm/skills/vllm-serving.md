# vllm serve

```
/opt/vllm/bin/vllm serve {model} --host <127.0.0.1 or the private address> --port 8000 \
  --gpu-memory-utilization 0.90 --max-model-len <fits the KV cache> --max-num-seqs <concurrency>
```

- `VLLM_API_KEY` in the environment turns on bearer auth for the API routes (`/v1`, `/v2`, `/inference`). `/health` and `/metrics` stay open: keep port 8000 off the internet.
- Startup logs print the KV cache size in tokens and the maximum concurrency for `--max-model-len`. Read them before changing either flag.
- Out of memory at startup: lower `--max-model-len` first, then `--gpu-memory-utilization`.

| Metric on `/metrics` | Means |
|---|---|
| `vllm:num_requests_running` | requests in the current batch |
| `vllm:num_requests_waiting` | requests queued for a slot |
| `vllm:kv_cache_usage_perc` | KV cache in use; 1 is full |
| `vllm:num_preemptions_total` | requests evicted from the KV cache and recomputed |

A queue with an empty KV cache means too few slots: raise `--max-num-seqs`. Preemptions with a full KV cache mean too many: lower it.
