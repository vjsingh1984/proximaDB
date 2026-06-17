#!/usr/bin/env python3
"""PySpark reference for the Monte Carlo option-pricing benchmark.

Prices the same option-contract Parquet the Rust benchmark uses, via a Spark UDF over a
DataFrame — exactly the shape ProximaDB's `mc_price` UDF mirrors. Run this on the same box
and dataset to compare against the Rust-native numbers (no JVM, Arrow-native, rayon-parallel).

This is intentionally a *separate*, externally-run script: ProximaDB has no JVM/Spark
dependency, and `cargo`/pytest assert the speedup against in-repo Rust/numpy baselines instead.

Setup (in the project venv):
    pip install pyspark
Run:
    python option_pricing_spark.py --parquet /tmp/mc/options.parquet --paths 100000 --cores 8

Object storage: point --parquet at s3a://bucket/options.parquet (with hadoop-aws configured)
to compare the I/O path against ProximaDB's object-store reads.
"""
import argparse
import math
import time


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--parquet", required=True, help="path or s3a:// URL to options.parquet")
    ap.add_argument("--paths", type=int, default=100_000)
    ap.add_argument("--cores", type=int, default=0, help="0 = Spark default (all cores)")
    args = ap.parse_args()

    from pyspark.sql import SparkSession
    from pyspark.sql.functions import udf
    from pyspark.sql.types import DoubleType

    builder = SparkSession.builder.appName("proxima-mc-option-pricing")
    if args.cores > 0:
        builder = builder.master(f"local[{args.cores}]")
    spark = builder.getOrCreate()

    n_paths = args.paths

    # GBM Monte Carlo for one contract — the Spark-UDF analog of ProximaDB's mc_price.
    # (Pure-Python per-row UDF; this is the JVM/serialization-bound path Spark users hit.)
    def mc_price(spot, strike, vol, rate, t, is_call):
        import random

        rnd = random.Random(0)
        drift = (rate - 0.5 * vol * vol) * t
        diffusion = vol * math.sqrt(t)
        total = 0.0
        for _ in range(n_paths):
            z = rnd.gauss(0.0, 1.0)
            terminal = spot * math.exp(drift + diffusion * z)
            payoff = (terminal - strike) if is_call else (strike - terminal)
            if payoff > 0.0:
                total += payoff
        return math.exp(-rate * t) * (total / n_paths)

    mc_price_udf = udf(mc_price, DoubleType())

    df = spark.read.parquet(args.parquet)
    priced = df.select(
        df.id,
        mc_price_udf(df.spot, df.strike, df.vol, df.rate, df.t, df.is_call).alias("price"),
    )

    start = time.perf_counter()
    n = priced.count()  # force full evaluation
    elapsed = time.perf_counter() - start

    opts_per_sec = n / elapsed
    paths_per_sec = n * n_paths / elapsed
    print(
        f"pyspark               contracts={n} paths={n_paths} "
        f"wall={elapsed:.3f}s  options/s={opts_per_sec:>12.0}  paths/s={paths_per_sec:>15.0}"
    )
    spark.stop()


if __name__ == "__main__":
    main()
