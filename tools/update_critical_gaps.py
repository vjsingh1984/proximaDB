#!/usr/bin/env python3
import re
import sys
from pathlib import Path


def parse_overall_completion(text: str) -> int:
    m = re.search(r"^=== Overall Completion:\s*(\d+)%\s*$", text, re.M)
    if not m:
        raise SystemExit("Could not find 'Overall Completion' line")
    return int(m.group(1))


def extract_block(text: str, start_pat: str, end_pat: str) -> tuple[int, int, str]:
    sm = re.search(start_pat, text, re.M)
    if not sm:
        raise SystemExit(f"Start pattern not found: {start_pat}")
    em = re.search(end_pat, text[sm.start():], re.M)
    if not em:
        raise SystemExit(f"End pattern not found: {end_pat}")
    s = sm.start()
    e = sm.start() + em.start()
    return s, e, text[s:e]


def count_phase_rows(block: str, phase_title_substring: str, label_filter: str | None = None) -> tuple[int, int]:
    # Count non-complete rows (TODO/MISSING/IN PROGRESS/PARTIAL) in a specific phase table block
    # Returns (non_complete_count, total_rows)
    # Find the phase header row within the table
    lines = block.splitlines()
    start = None
    for i, ln in enumerate(lines):
        if phase_title_substring in ln:
            start = i
            break
    if start is None:
        return (0, 0)
    # Collect until next phase header within the same table or end of table
    non_complete = 0
    total = 0
    for ln in lines[start + 1 :]:
        if ln.strip() == '|===':
            break
        if not ln.startswith('| '):
            continue
        # Optional label filter
        if label_filter and not re.search(label_filter, ln):
            continue
        # Status is the third column; detect not complete
        if '✅ COMPLETE' in ln or '✅ DONE' in ln:
            # complete
            total += 1
        elif ln.strip().startswith('| **'):  # header row inside table
            continue
        else:
            total += 1
            non_complete += 1
    return non_complete, total


def proportion_to_ints(total_percent: int, weights: list[float]) -> list[int]:
    raw = [w * total_percent for w in weights]
    ints = [int(x) for x in map(lambda v: int(v), [r // 1 for r in raw])]
    ints = [int(r) for r in [int(x) for x in [int(v) for v in ints]]]
    # recompute with floor and distribute remainder
    floors = [int(r) for r in [int(v) for v in [int(x) for x in raw]]]
    floors = [int(x) for x in [int(v) for v in floors]]
    sum_floor = sum([int(v) for v in floors])
    remainder = total_percent - sum_floor
    fracs = [(raw[i] - floors[i], i) for i in range(len(raw))]
    fracs.sort(reverse=True)
    out = floors[:]
    for j in range(remainder):
        out[fracs[j % len(out)][1]] += 1
    return out


def main():
    path = Path(sys.argv[1] if len(sys.argv) > 1 else 'docs/09-roadmap/planned/graph_database_requirements_spec.adoc')
    text = path.read_text(encoding='utf-8')

    overall = parse_overall_completion(text)
    remaining = max(0, 100 - overall)

    # Extract the big status section block from after Overall Completion table to before Critical Gaps
    start_pat = r"^\[cols=.*?$"  # first table after Overall Completion
    end_pat = r"^=== Critical Gaps Remaining .*?$"
    s, e, status_block = extract_block(text, start_pat, end_pat)

    # Category counts
    # 1) Performance Benchmarks: lines with Benchmarks not complete
    perf_bench = len(re.findall(r"^\|\s*Benchmarks\s*\|.*\|\s*(?!✅ COMPLETE).*$", status_block, re.M))

    # 2) Phase 1 addendum: non-complete rows in PHASE 1 table
    p1_non_complete, _ = count_phase_rows(status_block, '**PHASE 1', None)

    # 3) Phase 2 addendum: non-complete rows in PHASE 2 table (excluding Mode Config)
    p2_non_complete, _ = count_phase_rows(status_block, '**PHASE 2', None)
    # subtract any fully complete neutral rows if needed via pattern
    # Keep as total non-complete from that section.

    # 4) Phase 3 addendum: rows specifically for Shortest Paths and Traversal Controls
    p3_non_complete, _ = count_phase_rows(status_block, '**PHASE 3', r"Shortest Paths|Traversal Controls")

    # 5) Phase 4 addendum: rows for Unique Constraints, TTL/Expiry, Full-Text Index, Stats Maintenance
    p4_non_complete, _ = count_phase_rows(status_block, '**PHASE 4', r"Unique Constraints|TTL/Expiry|Full-Text Index|Stats Maintenance")

    # 6) Property Index verification: if Property Index is not complete
    prop_index_non_complete = len(re.findall(r"^\|\s*Property Index\s*\|.*\|\s*(?!✅ COMPLETE).*$", status_block, re.M))
    prop_index_non_complete = 1 if prop_index_non_complete > 0 else 0

    counts = [
        ('Performance Benchmarks', perf_bench),
        ('Phase 1 Addendum', p1_non_complete),
        ('Phase 2 Addendum', p2_non_complete),
        ('Phase 3 Addendum', p3_non_complete),
        ('Phase 4 Addendum', p4_non_complete),
        ('Property Index Verification', prop_index_non_complete),
    ]

    total_counts = sum(c for _, c in counts) or 1
    weights = [c / total_counts for _, c in counts]
    percents = proportion_to_ints(remaining, weights)

    # Build new Critical Gaps section
    new_header = f"=== Critical Gaps Remaining ({remaining}%):"
    lines = [new_header, ""]
    # Preserve descriptions as in doc
    descs = {
        'Performance Benchmarks': 'Implement benchmark suite to validate 1M+ edges/sec and planner costs.',
        'Phase 1 Addendum': 'ULID adoption, edge `weight/bidirectional/validity`, RI checks, delete modes.',
        'Phase 2 Addendum': 'Batch/upsert endpoints, pagination tokens, request timeouts, streaming traversal in gRPC.',
        'Phase 3 Addendum': 'Traversal resource guards and weighted shortest paths (Dijkstra/A*), k-shortest.',
        'Phase 4 Addendum': 'Unique constraints, TTL/expiry, initial full‑text index module, stats maintenance loop.',
        'Property Index Verification': 'Prove correctness/perf; finalize partial implementation.',
    }
    for (name, _), pct in zip(counts, percents):
        lines.append(f"1. **{name} ({pct}%)** – {descs[name]}")

    new_section = "\n".join(lines) + "\n"

    # Replace existing Critical Gaps section
    cg_start = re.search(r"^=== Critical Gaps Remaining .*?$", text, re.M)
    if not cg_start:
        raise SystemExit("Critical Gaps section start not found")
    # Find end: next '=== ' or end of file
    end_match = re.search(r"^===\s+Key Achievements:\s*$", text[cg_start.start():], re.M)
    if not end_match:
        raise SystemExit("Key Achievements section not found after Critical Gaps")
    cg_s = cg_start.start()
    cg_e = cg_start.start() + end_match.start()
    updated = text[:cg_s] + new_section + text[cg_e:]

    path.write_text(updated, encoding='utf-8')
    print(f"Updated Critical Gaps to {remaining}% with auto-derived weights.")


if __name__ == '__main__':
    main()
