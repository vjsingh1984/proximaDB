"""MLOps and AutoML catalog contracts for ProximaDB.

These helpers keep ML metadata SQL-queryable through pgwire while leaving
training orchestration, artifacts, and long-running jobs to SDK/REST/gRPC
surfaces. Optional integrations such as MLflow, AutoGluon, and PyCaret are
detected lazily so the base SDK remains dependency-light.
"""

from __future__ import annotations

import importlib.util
from dataclasses import dataclass, field
from typing import Any


@dataclass(frozen=True)
class FeatureTableDDL:
    name: str
    entity_key: str = "entity_id"
    event_time: str = "event_time"
    features_column: str = "features"
    embedding_dimension: int | None = None
    storage_engine: str = "VIPER"
    physical_layout: str = "columnar"
    catalog_namespace: str | None = None

    def create_table_sql(self) -> str:
        columns = [
            f"{_q(self.entity_key)} TEXT NOT NULL",
            f"{_q(self.event_time)} TIMESTAMP NOT NULL",
            f"{_q(self.features_column)} JSONB NOT NULL DEFAULT '{{}}'::jsonb",
        ]
        if self.embedding_dimension is not None:
            columns.append(f"{_q('embedding')} VECTOR({self.embedding_dimension})")
        columns.append(f"PRIMARY KEY ({_q(self.entity_key)}, {_q(self.event_time)})")
        return f"CREATE TABLE IF NOT EXISTS {_q(_ident(self.name))} ({', '.join(columns)});"

    def xcatalog_sql(self) -> list[str]:
        namespace = self.catalog_namespace or f"features.{_ident(self.name)}"
        return [
            f"COMMENT ON TABLE {_q(_ident(self.name))} IS "
            f"'xcatalog.namespace={namespace};"
            f"engine={self.storage_engine};layout={self.physical_layout};"
            "kind=feature_table';"
        ]

    def statements(self, *, include_xcatalog: bool = True) -> list[str]:
        statements = [self.create_table_sql()]
        if include_xcatalog:
            statements.extend(self.xcatalog_sql())
        return statements


@dataclass(frozen=True)
class ExperimentTrackerDDL:
    prefix: str = "mlops"
    storage_engine: str = "SST"
    physical_layout: str = "hybrid"

    def statements(self) -> list[str]:
        runs = _ident(f"{self.prefix}_runs")
        metrics = _ident(f"{self.prefix}_metrics")
        params = _ident(f"{self.prefix}_params")
        registry = _ident(f"{self.prefix}_models")
        predictions = _ident(f"{self.prefix}_predictions")
        return [
            (
                f"CREATE TABLE IF NOT EXISTS {_q(runs)} ("
                '"run_id" TEXT NOT NULL, "experiment_name" TEXT NOT NULL, '
                '"status" TEXT NOT NULL, "framework" TEXT, "started_at_ms" BIGINT, '
                '"ended_at_ms" BIGINT, "metadata" JSONB NOT NULL DEFAULT \'{}\'::jsonb, '
                'PRIMARY KEY ("run_id"));'
            ),
            (
                f"CREATE TABLE IF NOT EXISTS {_q(metrics)} ("
                '"run_id" TEXT NOT NULL, "metric_name" TEXT NOT NULL, '
                '"step" BIGINT NOT NULL, "metric_value" DOUBLE PRECISION NOT NULL, '
                '"recorded_at_ms" BIGINT, PRIMARY KEY ("run_id", "metric_name", "step"));'
            ),
            (
                f"CREATE TABLE IF NOT EXISTS {_q(params)} ("
                '"run_id" TEXT NOT NULL, "param_name" TEXT NOT NULL, '
                '"param_value" TEXT NOT NULL, PRIMARY KEY ("run_id", "param_name"));'
            ),
            (
                f"CREATE TABLE IF NOT EXISTS {_q(registry)} ("
                '"model_name" TEXT NOT NULL, "version" TEXT NOT NULL, "stage" TEXT, '
                '"run_id" TEXT, "artifact_uri" TEXT, "signature" JSONB NOT NULL DEFAULT \'{}\'::jsonb, '
                '"metrics" JSONB NOT NULL DEFAULT \'{}\'::jsonb, '
                'PRIMARY KEY ("model_name", "version"));'
            ),
            (
                f"CREATE TABLE IF NOT EXISTS {_q(predictions)} ("
                '"prediction_id" TEXT NOT NULL, "model_name" TEXT NOT NULL, '
                '"model_version" TEXT, "entity_id" TEXT, "event_time" TIMESTAMP, '
                '"inputs" JSONB NOT NULL DEFAULT \'{}\'::jsonb, '
                '"prediction" JSONB NOT NULL DEFAULT \'{}\'::jsonb, '
                '"labels" JSONB NOT NULL DEFAULT \'{}\'::jsonb, '
                'PRIMARY KEY ("prediction_id"));'
            ),
            (
                f"COMMENT ON TABLE {_q(runs)} IS "
                f"'xcatalog.namespace=mlops.{_ident(self.prefix)};"
                f"engine={self.storage_engine};layout={self.physical_layout};kind=experiment_runs';"
            ),
        ]


@dataclass(frozen=True)
class AutoMLRunSpec:
    framework: str
    target: str
    feature_table: str
    label_column: str
    problem_type: str | None = None
    eval_metric: str | None = None
    presets: str | None = None
    time_limit_seconds: int | None = None
    extra: dict[str, Any] = field(default_factory=dict)

    def metadata(self) -> dict[str, Any]:
        return {
            "framework": self.framework,
            "target": self.target,
            "feature_table": self.feature_table,
            "label_column": self.label_column,
            "problem_type": self.problem_type,
            "eval_metric": self.eval_metric,
            "presets": self.presets,
            "time_limit_seconds": self.time_limit_seconds,
            "extra": dict(self.extra),
        }


def optional_integrations() -> dict[str, bool]:
    """Return installed optional ML integration availability."""
    return {
        "mlflow": importlib.util.find_spec("mlflow") is not None,
        "autogluon": importlib.util.find_spec("autogluon") is not None,
        "pycaret": importlib.util.find_spec("pycaret") is not None,
    }


def _ident(value: str) -> str:
    cleaned = "".join(ch if ch.isalnum() or ch == "_" else "_" for ch in value.lower())
    return cleaned.strip("_") or "mlops"


def _q(value: str) -> str:
    return '"' + value.replace('"', '""') + '"'
