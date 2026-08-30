# CODEX REVIEW HANDOFF — TD-PAXRG-1 Rebase Conflict Resolution (RDSTRAT-12 §3 r4 × Phase D v4 chunks)

**Reviewer**: Codex (independent second agentic eval)
**Branch**: `topic/pax-olap-scan-ga` (PR #1762 → develop), head `d16041003` post-rebase
**Conflict commit pair**: my `9fe42dd1a` (TD-PAXRG-1 Phase D) × develop's `c7dbbf768`/`47bef62e0` (TD-SELECTOR-1 gate 3 + TD-RDSTRAT-12 §3 r4, both merged 2026-08-28/29)

---

## 1. The conflict

`rabitq_search_segment_coalesced_allowed` in `src/storage/engines/sst/segment_format.rs` — step 5 (Region D top-k OID fetch). Two independent rewrites of the same ~120-line region landed on opposite sides:

**Develop side (TD-RDSTRAT-12 §3 round 4, keep verbatim)**: `oid_policy` → `oid_fetches = plan_coalesced_block_ranges(...)` → **peek·batch·feed**: classification per fetch via `survivor_cache.peek_memory_exact(CacheKind::Other, ...)` (`OidSlot::SiteServed` vs `OidSlot::Cold`), one bounded-concurrent `read_ranges_prefetch` wave for the cold ranges (with sequential fallback + metrics-parity `record_get(ResultPayload)` per physical GET), then per-fetch loaders fed prefetched bytes through `survivor_cache.get_or_fetch`, then per-block OID decode via `PaxBlockReader::decode_str_stripe(OID)` filling `oid_of: HashMap<usize, String>`.

**My side (TD-PAXRG-1 Phase D)**: on v4 row-group segments, the footer's per-RG stats payload addresses each RG's **OID chunk** (`FooterRowGroupStats.oid_chunk_rel_off/len/encoding_id/is_lz4`), so the top-k OIDs are fetched as byte-sliced chunks (decoded standalone via `proximadb_block_format::decode_str_chunk`) instead of whole RGs/blocks.

## 2. The resolution (what to verify)

Merged shape in `rabitq_search_segment_coalesced_allowed` (post-rebase HEAD):

```rust
let v4_chunk_fetch = topk_blocks.iter().all(|&bi| {
    footer.block_stats.get(bi)
        .is_some_and(|s| s.as_ref().is_some_and(|stats| stats.oid_chunk_len > 0))
});
if v4_chunk_fetch {
    for (&bi, locals) in &block_rows {
        [per-RG OID-chunk fetch via SurvivorRangeCache::get_or_fetch +
         decode_str_chunk(stats.oid_encoding_id, stats.oid_is_lz4, b.row_count)]
    }
} else {
    [develop's peek·batch·feed machinery, VERBATIM: oid_policy → oid_fetches →
     OidSlot classification → read_ranges_prefetch wave → loaders → decode loop]
}
```

**Key semantic decisions to double-check:**

1. **Discriminator**: `v4_chunk_fetch` = "every top-k RG carries a stats payload with `oid_chunk_len > 0`". Post-collapse there is ONE layout version, so the RG-stats payload presence *is* the v4 marker (non-RG layouts — the kill-switch legacy framing — carry zero-length/absent stats). Verify no path can reach the chunk arm on a non-v4 segment.
2. **r4 machinery preserved verbatim** in the else branch — including the sequential fallback in the `read_ranges_prefetch` error arm, the metrics-parity `record_get` discipline (baseline loader records only `ResultPayload` GETs; Cold loaders silent), and `drain_and_forward_read_ranges_metrics()`.
3. **Chunk arm deliberately does NOT batch** across RGs: chunks live at the same relative offset in *different* RGs (far apart in the file); coalescing would span the inter-RG gap and re-read the bodies the chunk fetch exists to skip. GET count is within a small constant (+1) at −34% bytes — pinned by `v4_oid_chunk_fetch_parity_and_byte_win_vs_v3`.
4. **`peek_memory_exact` is NOT used in the chunk arm**: the chunk loader still goes through `survivor_cache.get_or_fetch` (read-through, so hot queries hit the cache) but skip classification because a chunk is either in the cache or fetched whole — the peek/batch classification exists to amortize whole-block ranges, not ~KB chunks. **This is the weakest decision in the resolution** — if Codex believes hot-path chunk re-reads need the cache peek too, flag it.

## 3. Evidence the resolution is behavior-correct

- `v4_oid_chunk_fetch_parity_and_byte_win_vs_v3` — identical hits + bitwise scores vs the v3 whole-block path; ≥32 KiB bytes saved; GETs within +2. Executed green post-rebase.
- `v4_ranged_scan_prunes_row_groups_off_the_wire`, `v4_floor_kills_microgranule_amplification`, `v4_grouped_aggregates_parity_over_provider`, `v4_provider_reads_attribute_to_ambient_scope` — all green post-rebase (175-test sweep).
- Full sweep: 175 root-targeted + 586 storage-common + 76 block-format + 39 queue green on HEAD `d16041003`.

## 4. Specific questions for the independent eval

1. Is the `v4_chunk_fetch` discriminator airtight? (Could a legacy-layout segment produce footer stats payloads with `oid_chunk_len > 0`?)
2. In the chunk arm, does `decode_str_chunk`'s fail-open (`unwrap_or_default` → empty oids) risk silent OID-less hits rather than a loud failure? Is fail-open the right call there, matching the legacy `unwrap_or_default` on `decode_str_stripe`?
3. Is the r4 peek·batch·feed preservation in the else branch truly verbatim vs develop's `c7dbbf768`? (Diff the else branch against `origin/develop`'s same region.)
4. Any other caller of `plan_coalesced_block_ranges`/`oid_policy` orphaned by the v4 branch?

## 5. Reproduce

```
git fetch origin && git worktree add ../paxrg-review topic/pax-olap-scan-ga
cargo nextest run -p proximadb --lib -E 'test(v4_oid_chunk_fetch) or test(v4_ranged) or test(coalesced)'
# targeted conflict-region read: src/storage/engines/sst/segment_format.rs, step 5 of
# rabitq_search_segment_coalesced_allowed (search "v4_chunk_fetch")
```
