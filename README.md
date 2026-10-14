# Subweave

A Weavefile says what must stay true on a Linux box. The Subweave daemon keeps it true: it installs, watches, repairs, backs up and upgrades one service, and asks a person when it should not decide alone.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/img/hero-dark.svg">
  <img alt="A shortened postgres Weavefile beside its goals: running, archiving after running, backed after archiving" src="docs/img/hero-light.svg">
</picture>

```
subweave run postgres "postgres 18 for our app, private network, backups to s3://acme-pg/db1"
```

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/img/loop-dark.svg">
  <img alt="The loop: check the goals, a goal fails, the decider picks an action, the rules gate it, the steps run, the goal must hold within its time, else ask the person" src="docs/img/loop-light.svg">
</picture>

- **Code** checks numbers, runs learned commands and enforces named rules.
- **The decider** (default [Jev](https://docs.typesafe.ai), a System One model) checks the request against the weave's `ARG`s, picks the next action and judges text. Each decision costs about $0.00004.
- **The coder** (default DeepSeek) writes each box's commands once, and does the open-ended tasks, one gated shell command at a time.
- **Channels** take the daemon's asks and messages to the person: [Steery](https://steery.dev) (signed answers), Telegram, Slack, email, webhooks.

The run picks both models; a Weavefile only hints. One [OpenRouter](https://openrouter.ai) key runs every weave here ([Models](WEAVEFILE.md#models)).

The format is in [WEAVEFILE.md](WEAVEFILE.md): capital keywords, as in a Dockerfile, and one file per service. A weave says what, and keeps commands for when the exact command is the knowledge:

```weavefile
ARG bucket  SHOULD "an S3 URL for base backups and WAL, like s3://acme-pg/db1"

FACT disk  "percent used of the filesystem that holds /var/lib/postgresql, a number"
GOAL room  EVERY 5m  disk < 85

ACTION free-disk FOR room WITHIN 10m
  WHEN "old logs, not data or WAL, fill the disk"
  DO   "delete compressed PostgreSQL logs older than 7 days and shrink the systemd journal to 200 MB"
```

`subweave plan` checks a request before anything runs, and shows each finding as an error, an ask, a warning or info:

```
$ subweave plan postgres "postgres 18, private network, backups to acme-pg, keep 14"

error    bucket    `acme-pg` is not an S3 URL for base backups and WAL, like s3://acme-pg/db1   0.04
info     version   18   0.73
info     access    private: connections from the private network, with a password and TLS   0.95
```

## Example weaves

| Weave | For | Box | Shows |
|---|---|---|---|
| [postgres](weaves/postgres) | a 6-person SaaS team with no DBA | Hetzner VM, object storage | the reference: skills, restore drills, a major upgrade by sentence |
| [ethereum](weaves/ethereum) | a wallet startup leaving hosted RPC | dedicated server, 2 TB | a `MUST` network, thresholds as code, gated coder commands |
| [vllm](weaves/vllm) | a team serving its own open model | rented GPU | a `MUST` model, faults the box cannot fix |
| [store-edge](weaves/store-edge) | a 300-shop retail chain | fanless PC on DSL | no coder, works offline, a heartbeat |
| [valkey](weaves/valkey) | an agency with 40 client boxes | small VMs | a `MUST` role, one inbox for every box |
| [minecraft](weaves/minecraft) | a teacher with a coding club | one VPS | untrusted text, update and rollback |

Each weave's README says what it cannot do. [LIMITS.md](LIMITS.md) says what Subweave does badly, with the decider calls that show it in [evidence/](evidence).

## Status

Design. The format and the six weaves are written; the engine is not. The decider is measured on 77 calls, including seven request checks of the postgres weave; no weave has run on a box yet.

## License

[MIT](LICENSE)
