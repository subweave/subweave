# Weavefile

A Weavefile (`<name>.weave`) describes a service that runs itself on one box: a VM the engine reaches over SSH.
The engine installs it, watches it, repairs it, backs it up and upgrades it, as the file says.

Two models do the judgment:

| Role | Model (OpenRouter) | Does | Called |
|---|---|---|---|
| `decider` | `typesafe/jev-1.13` | typed decisions: yes/no, one of N | every check, every gate, every tick |
| `coder` | `deepseek/deepseek-v4-flash-0731` | runs shell commands on the box toward a goal | only in `do` steps |

```
subweave run ethereum "full node holding only 1000 blocks" --on root@203.0.113.7
```

Examples: [`weaves/ethereum.weave`](weaves/ethereum.weave), [`weaves/postgres.weave`](weaves/postgres.weave).

## File

Line based. `#` starts a comment outside quotes. Blocks indent by 2 spaces.

### Header

| Line | Example | Meaning |
|---|---|---|
| `weave <name> <version>` | `weave postgres 0.1` | identity; required, once |
| `about "<text>"` | `about "PostgreSQL server"` | one line for people and models |
| `box <os>, <n> cpu, <n> GB memory, <n> GB\|TB disk` | `box ubuntu 24.04, 1 cpu, 1 GB memory, 10 GB disk` | minimum box; checked before install |
| `decider <model>` | `decider typesafe/jev-1.13` | OpenRouter id of the decision model |
| `coder <model>` | `coder deepseek/deepseek-v4-flash-0731` | OpenRouter id of the coding model |
| `budget $<n> a month` | `budget $5 a month` | model spend cap for this box |

### Wants: what a request may say

```
want history = full
  archive          "every historical state"
  full             "all blocks and receipts, recent state only"
  last <n> blocks  "prune blocks older than n"
```

- One option per line: a label, then an optional quoted meaning.
- `<slot>` in a label takes a value from the request. Slot names are unique in the file.
- `= <label>` sets the default; it may fill a slot (`= last 10064 blocks`).
- No default: the request must say it, or the engine asks.

### Rules

| Line | Checked against | Broken |
|---|---|---|
| `never "<text>"` | the request, and every coder command | request refused; command refused |
| `always "<text>"` | every `watch` tick | `heal` runs with `{why}` = the rule |
| `ask "<text>"` | the plan, and every coder command | the person says yes or no first |

### Blocks

```
<name> [every <duration>] [keep <n>]:
  <steps>
```

Durations: `30s`, `5m`, `2h`, `1d`. Any name works; five names have engine meaning:

| Block | Runs | Engine adds |
|---|---|---|
| `install` | once, on `subweave run` | box check before; resumes at the failed step |
| `watch` | `every <duration>` | its first `match` sets the box status; its reads feed every decision |
| `heal` | when a block fails or an `always` rule breaks | `{why}`; if `heal` fails, the engine asks the person |
| `backup` | `every`, `-> backup`, `subweave do <box> backup` | empty `{backup}` dir on the box; copied off the box after; `keep` newest N |
| `restore` | `subweave do <box> restore [--from <id>]` | copies a stored backup into `{backup}` first |

Other blocks run on `every`, on `->`, or on `subweave do <box> <block>`.

### Steps

| Step | Example | Done by |
|---|---|---|
| `run <shell>` | `run systemctl restart postgresql` | engine; non-zero exit fails the block |
| `do "<goal>"` | `do "install PostgreSQL {version}"` | coder, until the decider says the goal holds |
| `read <name> = <shell>` | `read disk = df --output=pcent / \| tail -1` | engine; stdout becomes `{name}`; numbers become metrics |
| `match "<question>":` | arms `<label> "<meaning>" [-> <block>]` | decider picks one arm |
| `if "<question>" -> <block>` | or `if "<question>":` with indented steps | decider, yes/no |
| `ready "<question>" within <d> [else -> <block>]` | `ready "postgres accepts connections" within 5m` | decider, re-asked until yes or timeout |
| `ask "<question>"` | `ask "Upgrade now?"` | the person; no stops the block |
| `-> <block>` | `-> backup` | engine runs that block, then continues |

### Values

`{name}` inside quotes or shell is replaced by:

- a want: the chosen label with slots filled (`last 1000 blocks`)
- a slot: its value (`1000`)
- a read: its latest output
- `{box}` the box name, `{backup}` the backup dir, `{why}` the arm, rule or failure that started this block

## How a request becomes a plan

`subweave run <weave> "<request>"` sends the decider two requests:

1. Resolve, over `{weave, request, settings}`:
   - per want: a Choice of its options plus `unsaid`
   - per slot: a Choice of the request's words plus `none`
   - per `never`: a Noul "Does `request` ask the box to <text>?"
   - one Noul: does `request` ask for something no want covers?
2. Plan, over `{weave, request, plan}` with each value's meaning:
   - per `ask`: a Noul "A rule says to ask the person <text>. Does carrying out `plan` trigger this rule?"

| Answer | Engine |
|---|---|
| want `unsaid` | default; no default: asks |
| want or slot confidence < 0.5 | asks the person to pick |
| `never` ≥ 0.5 | refuses, naming the rule |
| unknown ≥ 0.5 | asks whether to go on without it |
| `ask` ≥ 0.5 | asks before install |

Measured on `typesafe/jev-1.13`, about $0.00004 per request:

| Request | Result |
|---|---|
| `full node holding only 1000 blocks` | history `last <n> blocks` (1.00), n `1000` (1.00), network and rpc unsaid; reorg-margin `ask` 0.90 |
| `full node keeping the last 20000 blocks` | reorg-margin `ask` 0.32: no question |
| `archive node on sepolia, and run a validator too` | refused: validator `never` 0.97 |
| `a node for my rollup` | every want unsaid; unknown 0.59: asks |

## How a `do` step runs

1. The coder gets the goal, the plan, the rules, the box facts and the latest reads, and one tool: `shell`.
2. Before each command, the decider answers one Noul per `never` and per `ask` rule about that command.
   - `never` ≥ 0.5: the command is refused; the coder hears which rule.
   - `ask` ≥ 0.5: the person says yes or no.
3. The coder calls `done` with a summary. The decider checks "the goal holds" against the transcript and fresh reads.
   - ≥ 0.8: the step passes. Below: the coder continues.
4. 40 commands without a passing goal fail the step.

## Failure

A block fails when a `run` exits non-zero, a `do` gives up, or a `ready` times out with no `else`.
The engine then runs `heal` with `{why}` set to the failure. A failing `heal` stops and asks the person.

## Engine

- Runs where `subweave` runs; reaches the box with `ssh` (`--on user@host`) or locally (`--on local`).
- Keeps each box in `~/.subweave/boxes/<box>/`: plan, events, metrics, spend, backups.
- Reads `OPENROUTER_API_KEY`. Stops calling models at the `budget` and asks the person.

| Command | Does |
|---|---|
| `subweave run <weave> ["<request>"] --on <target> [--name <box>]` | plan, box check, `install`, then supervise |
| `subweave up <box>` | supervise again: `watch` and every `every` block |
| `subweave ps` | boxes, status, last tick, spend this month |
| `subweave logs <box>` | events: steps, commands, decisions, costs |
| `subweave stats <box>` | metrics from numeric reads |
| `subweave do <box> <block>` | run one block now |
| `subweave parse <weave> [--json]` | check a file; JSON is the parsed file and its tokens |

## Grammar

```
file    = { line } ;
line    = header | want | option | rule | block | step | comment | blank ;
header  = ("weave" name version | "about" str | "box" text | "decider" id | "coder" id | "budget" "$" num "a month") ;
want    = "want" name [ "=" text ] ;                     (* options follow, indented *)
option  = label [ str ] ;                                (* label: words and <slot> *)
rule    = ("never" | "always" | "ask") str ;
block   = name [ "every" dur ] [ "keep" num ] ":" ;
step    = "run" shell | "do" str | "read" name "=" shell
        | "match" str ":" | arm | "else" "->" name
        | "if" str ( "->" name | ":" )
        | "ready" str "within" dur [ "else" "->" name ]
        | "ask" str | "->" name ;
arm     = name str [ "->" name ] ;
```
