# Subweave

A Weavefile says what must stay true on a Linux box. The Subweave daemon keeps it true: it installs, watches, repairs, backs up and upgrades one service, and asks a person when it should not decide alone.

```
subweave run postgres "postgres 18 for our app, private network, backups to s3://acme-pg/db1"
```

```
check the goals ──► a goal fails ──► the decider picks an action ──► the rules gate it ──► the action runs
       ▲                                                                                        │
       └──────────────────────────── the journal records every step ◄───────────────────────────┘
```

- **Code** checks numbers, runs commands and enforces named rules.
- **The decider** ([Jev](https://docs.typesafe.ai), a System One model) turns the request into settings, picks the next action and judges text. Each decision costs about $0.00002.
- **The coder** (DeepSeek) does the open-ended tasks, one gated shell command at a time.

Both models run through [OpenRouter](https://openrouter.ai). The format is in [WEAVEFILE.md](WEAVEFILE.md).

## Example weaves

| Weave | For | Box | Shows |
|---|---|---|---|
| [postgres](weaves/postgres) | a 6-person SaaS team with no DBA | Hetzner VM, object storage | backups, restore, a major upgrade by sentence |
| [ethereum](weaves/ethereum) | a wallet startup leaving hosted RPC | dedicated server, 2 TB | thresholds as code, gated coder commands |
| [vllm](weaves/vllm) | a team serving its own open model | rented GPU | faults the box cannot fix |
| [store-edge](weaves/store-edge) | a 300-shop retail chain | fanless PC on DSL | no coder, works offline, alerts |
| [valkey](weaves/valkey) | an agency with 40 client boxes | small VMs | goals in code, one inbox for every box |
| [minecraft](weaves/minecraft) | a teacher with a coding club | one VPS | untrusted text, update and rollback |

Each weave's README says what it cannot do. [LIMITS.md](LIMITS.md) says what Subweave does badly, with the decider calls that show it in [evidence/](evidence).

## Status

Design. The format and the six weaves are written; the engine is not. The decider is measured on 21 hand-made cases; no weave has run on a box yet.
