# Independent oracles for snapshot reconstruction

Both checks compare `vsc-core` with pyvshadow (libvshadow-python), an
independent implementation. Neither dataset is committed.

## dfVFS `vss.raw` (real shadow copies)

`tests/oracle_dfvfs.rs` runs when `VSC_ORACLE_IMAGE` points at `vss.raw` from
<https://github.com/log2timeline/dfvfs> at commit
`0778d3fb20728cbf5c60e14b702527eab0b1c227` (SHA-256
`e633f0be5fb9ee9a07d44ba5223b786012ce89e9f0024f93d743568e6d052f16`). It is an
NTFS volume with two shadow copies whose stores hold plain and overlay
descriptors, and the newer store has a previous bitmap. The expected per-store
digests were produced with libvshadow-python 20260714.

```
VSC_ORACLE_IMAGE=/path/to/vss.raw just test-vsc --test oracle_dfvfs
```

## Random multi-store volumes (forwarders, overlays, not-used, bitmaps)

The real image has no forwarder or not-used descriptors, so a differential run
covers them:

```
python -m venv venv && venv/bin/pip install libvshadow-python==20260714
venv/bin/python tests/oracle/random_images.py out 300 20261003 /path/to/vss.raw
just compare-vsc /absolute/path/to/out 300
```

`random_images.py` writes NTFS-headed volumes (the boot sector and VSS
identifier are taken from `vss.raw`) with one to four stores in shuffled catalog
order. Their block lists mix plain, overlay, forwarder (including self and
chained) and not-used descriptors, re-use forwarder targets to trigger reverse
swaps, and carry random current and previous bitmaps. pyvshadow dumps every
store, and the example compares whole volumes plus 500 random unaligned ranges
per store. On 2026-10-03 seeds 20261003 (300 images) and 7 (500 images) gave
1,984 stores and 992,000 ranges identical, with no rejections on either side.

Run `just` recipes from the repository root. On 2026-10-13 the pinned dfVFS
image was downloaded, its source SHA-256 verified, and both whole-store hashes
were exercised by the reader oracle, `just probe-vsc ... --blocks --hash`, and
the application's env-gated VSS/NTFS tests. These use the existing independent
20260714 oracle digests; neither the current libvshadow source nor the random
corpus was rebuilt/rerun on that date. See `docs/formats/VSS.md` for API parity,
synthetic regressions and remaining limits.
