# PULSAR Status

PULSAR has been retired as a separate graph engine.

The useful ideas from PULSAR are now requirements for the relational distributed
substrate:

- partition placement,
- shard-aware routing,
- traversal fanout/fanin,
- hot-partition detection,
- rebalance signals,
- distributed execution metadata in `EXPLAIN`.

ORION remains the graph runtime. It may provide graph-specific planning hints,
but it does not own distributed coordination or durable graph authority.

Use ORION for graph workloads. Configure scale-out behavior through the
relational distributed planner and xCatalog policy.
