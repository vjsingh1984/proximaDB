#!/usr/bin/env python3
"""Single-process numpy Monte Carlo option-pricing baseline.

Prices the same option-contract Parquet that the Rust benchmark and the PySpark reference
use, so the three can be compared side-by-side on the same box and data. This is the
"vectorized Python / numpy" reference — fast per-core but single-process and GIL-bound.

Usage:
    python option_pricing_baseline.py --parquet /tmp/mc/options.parquet --paths 100000

Generate the Parquet with the Rust benchmark (it writes options.parquet to --data-dir):
    cargo run --release --example montecarlo_bench --features datafusion-integration -- \
        --contracts 1000000 --paths 100000 --data-dir /tmp/mc
"""
import argparse
import time

import numpy as np
import pyarrow.parquet as pq


def mc_price(spot, strike, vol, rate, t, is_call, n_paths, rng):
    """Vectorized GBM Monte Carlo for one contract (numpy)."""
    z = rng.standard_normal(n_paths)
    drift = (rate - 0.5 * vol * vol) * t
    diffusion = vol * np.sqrt(t)
    terminal = spot * np.exp(drift + diffusion * z)
    if is_call:
        payoff = np.maximum(terminal - strike, 0.0)
    else:
        payoff = np.maximum(strike - terminal, 0.0)
    return float(np.exp(-rate * t) * payoff.mean())


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--parquet", required=True, help="path to options.parquet")
    ap.add_argument("--paths", type=int, default=100_000)
    args = ap.parse_args()

    table = pq.read_table(args.parquet)
    cols = {name: table.column(name).to_numpy(zero_copy_only=False) for name in table.column_names}
    n = table.num_rows

    rng = np.random.default_rng(12345)
    start = time.perf_counter()
    prices = [
        mc_price(
            cols["spot"][i],
            cols["strike"][i],
            cols["vol"][i],
            cols["rate"][i],
            cols["t"][i],
            bool(cols["is_call"][i]),
            args.paths,
            rng,
        )
        for i in range(n)
    ]
    elapsed = time.perf_counter() - start

    opts_per_sec = n / elapsed
    paths_per_sec = n * args.paths / elapsed
    print(
        f"numpy-baseline        contracts={n} paths={args.paths} "
        f"wall={elapsed:.3f}s  options/s={opts_per_sec:>12.0}  paths/s={paths_per_sec:>15.0}  "
        f"(sum={sum(prices):.2f})"
    )


if __name__ == "__main__":
    main()
