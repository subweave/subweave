# NVIDIA Xid errors

The kernel log line `NVRM: Xid (PCI:0000:3b:00): 79, …` carries the Xid number after the address.

| Xid | Usual cause | Software fix |
|---|---|---|
| 13, 31, 43 | the application: a bad kernel or memory access | restart vllm |
| 45 | cleanup after a killed process | none needed |
| 48, 94, 95 | uncorrectable ECC memory error | reboot; repeated: replace the GPU |
| 63, 64 | ECC page retirement or row remapping | reboot to apply |
| 74 | NVLink error | reboot; repeated: hardware |
| 79 | the GPU fell off the bus | reboot; repeated: hardware |

Check after a reboot: `nvidia-smi -q -d ECC,PAGE_RETIREMENT,ROW_REMAPPER`.

Hardware errors go to the person with the Xid lines: on a rented GPU, only the provider can replace it.
