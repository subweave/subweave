# reth pruning

| `history` | reth flags |
|---|---|
| `archive` | none |
| `full` | `--full` |
| `last <blocks> blocks` | the flags below, each set to `<blocks>` |

```
--prune.minimum-distance <blocks>
--prune.sender-recovery.distance <blocks>
--prune.transaction-lookup.distance <blocks>
--prune.receipts.distance <blocks>
--prune.account-history.distance <blocks>
--prune.storage-history.distance <blocks>
--prune.bodies.distance <blocks>
```

- reth keeps 10064 blocks for reorgs and manual unwinds by default (`MINIMUM_UNWIND_SAFE_DISTANCE`). `--prune.minimum-distance` below that lowers the margin; the weave's `ask if blocks < 10064` rule has already asked the person.
- Flag names change between releases. Check them with `reth node --help | grep prune` for the installed version before writing the unit file.
- Going from pruned to `full` or `archive` needs a resync: pruned data does not come back.
