# Business PoV Demos

Purpose: Demonstrate business applicability of ProximaDB through concise, runnable Point‑of‑View demos. Each script creates a small dataset, runs a business‑framed query, and prints interpretable results.

Prerequisites
- Server: `make server-start` (REST :5678). Health: `curl http://localhost:5678/health`.
- SDK: `cd clients/python && pip install -e .`. Also `pip install sentence-transformers`.
- Export `PYTHONPATH=./clients/python/src` when running from repo root.

How To Run
- E‑commerce: `python3 demo/showcases/business/ecommerce_pov.py`
- Fraud Detection: `python3 demo/showcases/business/fraud_pov.py`
- Customer 360: `python3 demo/showcases/business/customer360_pov.py`
- Hybrid Entity Store: `python3 demo/showcases/business/hybrid_pov.py`

Notes
- Scripts auto‑clean collections on start and exit.
- Graph usage (fraud PoV) is attempted via REST; if unavailable, the demo still shows vector‑side risk surfacing.
- All data is synthetic and generated in‑memory for quick runs (<5s each).
- Hybrid PoV uses entity endpoints under `/api/v2/collections/<id>/entities` and `/entities/search` for unified vector+relation workflows.
