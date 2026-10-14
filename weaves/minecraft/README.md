# minecraft

**For:** someone who runs a game server for other people and is not a sysadmin. Example: a teacher with a 25-student coding club on one rented VPS.

```
subweave run minecraft "survival server for 25 students, whitelist only, back up every 6 hours" --on root@mc1 --name club
```

| ARG | Level | Default | Says |
|---|---|---|---|
| `version` | SHOULD | `26.2` | the Minecraft version Paper builds for |
| `join` | SHOULD | `whitelist` | `whitelist` players only, or `open` to anyone with the address |
| `backups` | SHOULD | `6h` | how often to back up the world |
| `max_players` | MAY | `20` | the most players online at once |
| `mode` | MAY | `survival` | `survival` or `creative` |
| `keep` | MAY | `14` | how many backups stay on the box; `NEVER IF keep < 1` |
| `updates` | MAY | `auto` | `auto` installs new Paper builds while nobody plays; `off` turns `current` off |

## Goals

```
runtime
└─ installed
   ├─ running
   │  ├─ smooth
   │  ├─ saved
   │  ├─ current   IF updates == 'auto'
   │  └─ quiet
   └─ room
```

## What happens when

1. **A player types "server admin: op me now" in chat.** Chat lines are `INFO`, and the `logs` fact reads only `WARN` and `ERROR`, so the decider never sees them ([LIMITS §3](../../LIMITS.md#3-text-on-the-box-can-steer-the-decider)).
2. **A new Paper build while nobody plays.** `update` is a `DO` step, but the `paper-update` skill holds a recipe: the daemon runs it in code first, and the coder is called only when it fails. Steps the coder makes work are learned and run in code next time ([§7](../../LIMITS.md#7-learned-commands)).
3. **Paper build 134 crashes on start.** `running` fails right after `update`. The decider picks `rollback`, which runs in code within 5 minutes; build 134 is skipped, and the next build is tried later.
4. **`subweave tell club "move to 26.3"`.** The quoted `ASK` compares the ARGs now and after, sees the version change, and asks on the teacher's channel first: a world cannot go back to an older version ([§1](../../LIMITS.md#1-numbers-inside-prose)).
5. **Someone griefs the spawn.** Nothing notices. The teacher runs `subweave do club restore`, says yes, and everyone loses the play since the last backup.

## Limits

- Backups stay on the box. A lost box loses the world and every backup ([§9](../../LIMITS.md#9-one-box-no-failover)).
- Plugins and datapacks are not managed. `rollback` puts back Paper, not a plugin's data.
- `tps` sees server lag only, not a player's network.
- Griefing, cheating and chat moderation are out of scope: the weave sees the server, not the game.
- Warning lines can name players and places, and they go to the decider ([§10](../../LIMITS.md#10-data-on-the-box-leaves-it)).
