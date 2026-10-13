# vllm

**For:** a product team that serves its own open model instead of paying per token. Example: a support-tools startup with one rented 48 GB GPU, serving Qwen3-8B to its app servers.

```
subweave run vllm "serve Qwen/Qwen3-8B to our app servers on the private network" --on root@gpu1 --name gpu1
```

## Goals

| Goal | Holds when | Usual fix |
|---|---|---|
| `driver` | `nvidia-smi` lists a GPU | `install-driver` (asks, reboots) |
| `installed` | `vllm --version` runs | `install` (coder) |
| `running` | `/health` answers | `restart` |
| `serving` | `/v1/models` names `model` | `serve` (coder) |
| `gpu-healthy` | no Xid errors in 10 minutes | `restart` or `reboot` (asks) |
| `keeping-up` | under 20 queued, under 50 preemptions in 5 minutes | `lower-concurrency`, `restart` |
| `room` | model cache disk under 90% | `free-disk` (coder) |
| `quiet` | decider: no warning lines that need action | `restart`, `fix` |

## What happens when

1. **Traffic doubles at 09:00.** Preemptions climb and `keeping-up` fails. The decider picks between `lower-concurrency` and `restart` from the KV cache use and the logs.
2. **Xid 79 at night.** `gpu-healthy` fails. The `nvidia-xid` skill says reboot, and `ask before reboot` asks the person first.
3. **`subweave tell gpu1 "serve Qwen/Qwen3-14B instead"`.** `serving` fails, because `/v1/models` still names the old model, and `serve` moves the service to the new one.

## Limits

- Goals watch the queue, not the answers. A wrong chat template or a bad quantization serves wrong answers with every goal green.
- No software fixes a dead GPU. On a rented GPU, the person opens a ticket with the provider.
- A spot or preemptible GPU disappears with the daemon on it ([§7](../../LIMITS.md#7-one-box-no-failover)).
- The first install downloads the model: tens of GB, so `installed` can fail its first checks while the download runs.
- vLLM changes flags between releases, so every vLLM upgrade asks.
