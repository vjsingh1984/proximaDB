"""Notebook-style lazy Python facade for embedded ProximaDB.

This module intentionally keeps Python as a plan builder. Execution is delegated
to the embedded Rust database through the existing native methods exposed by
``ProximaDB``.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from typing import Any, Mapping, Sequence

from ._proximadb_embedded import ProximaDB

_MASTER_RE = re.compile(r"^proxima-local\[(?P<workers>[1-9][0-9]*)\]$")
_IDENT_RE = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")


def col(name: str) -> Column:
    """Create a column expression for notebook/DataFrame-style filters."""
    return Column(name)


def _quote_ident(name: str) -> str:
    if not name:
        raise ValueError("identifier must not be empty")
    return '"' + name.replace('"', '""') + '"'


def _table_ident(name: str) -> str:
    if not name:
        raise ValueError("table name must not be empty")
    parts = name.split(".")
    if any(not part for part in parts):
        raise ValueError("table name contains an empty identifier segment")
    return ".".join(
        part if _IDENT_RE.match(part) else _quote_ident(part) for part in parts
    )


def _literal(value: Any) -> str:
    if value is None:
        return "NULL"
    if isinstance(value, bool):
        return "TRUE" if value else "FALSE"
    if isinstance(value, (int, float)):
        return str(value)
    text = str(value).replace("'", "''")
    return f"'{text}'"


@dataclass(frozen=True)
class Predicate:
    """SQL predicate fragment built by the Python facade."""

    sql: str

    def __and__(self, other: Predicate) -> Predicate:
        return Predicate(f"({self.sql}) AND ({other.sql})")

    def __or__(self, other: Predicate) -> Predicate:
        return Predicate(f"({self.sql}) OR ({other.sql})")

    def __invert__(self) -> Predicate:
        return Predicate(f"NOT ({self.sql})")


@dataclass(frozen=True)
class Column:
    """Column expression used by ``where`` and ``filter``."""

    name: str

    def _compare(self, op: str, value: Any) -> Predicate:
        if value is None and op == "=":
            return Predicate(f"{_quote_ident(self.name)} IS NULL")
        if value is None and op in ("!=", "<>"):
            return Predicate(f"{_quote_ident(self.name)} IS NOT NULL")
        return Predicate(f"{_quote_ident(self.name)} {op} {_literal(value)}")

    def __eq__(self, value: Any) -> Predicate:  # type: ignore[override]
        return self._compare("=", value)

    def __ne__(self, value: Any) -> Predicate:  # type: ignore[override]
        return self._compare("!=", value)

    def __gt__(self, value: Any) -> Predicate:
        return self._compare(">", value)

    def __ge__(self, value: Any) -> Predicate:
        return self._compare(">=", value)

    def __lt__(self, value: Any) -> Predicate:
        return self._compare("<", value)

    def __le__(self, value: Any) -> Predicate:
        return self._compare("<=", value)

    def contains(self, value: str) -> Predicate:
        return Predicate(
            f"{_quote_ident(self.name)} LIKE {_literal('%' + value + '%')}"
        )

    def startswith(self, value: str) -> Predicate:
        return Predicate(f"{_quote_ident(self.name)} LIKE {_literal(value + '%')}")


@dataclass(frozen=True)
class _Plan:
    source_kind: str
    source: str
    operations: tuple[Mapping[str, Any], ...] = field(default_factory=tuple)

    def append(self, op: Mapping[str, Any]) -> _Plan:
        return _Plan(
            source_kind=self.source_kind,
            source=self.source,
            operations=self.operations + (dict(op),),
        )


class ProximaSessionBuilder:
    """Builder for ``ProximaSession``."""

    def __init__(self) -> None:
        self._data_dirs: Any = "./data"
        self._cache_size_mb = 512
        self._default_engine = "sst"
        self._master = "proxima-local[1]"
        self._memory_limit: str | None = None
        self._batch_size = 10000

    def data_dir(self, path: str) -> ProximaSessionBuilder:
        self._data_dirs = path
        return self

    def data_dirs(self, paths: Any) -> ProximaSessionBuilder:
        self._data_dirs = paths
        return self

    def cache_size_mb(self, value: int) -> ProximaSessionBuilder:
        self._cache_size_mb = int(value)
        return self

    def default_engine(self, value: str) -> ProximaSessionBuilder:
        self._default_engine = value
        return self

    def master(self, value: str) -> ProximaSessionBuilder:
        if value != "local" and not _MASTER_RE.match(value):
            raise ValueError("master must be 'local' or 'proxima-local[n]'")
        self._master = "proxima-local[1]" if value == "local" else value
        return self

    def memory_limit(self, value: str) -> ProximaSessionBuilder:
        self._memory_limit = value
        return self

    def batch_size(self, value: int) -> ProximaSessionBuilder:
        if value <= 0:
            raise ValueError("batch_size must be positive")
        self._batch_size = int(value)
        return self

    def get_or_create(self) -> ProximaSession:
        db = ProximaDB(
            data_dirs=self._data_dirs,
            cache_size_mb=self._cache_size_mb,
            default_engine=self._default_engine,
        )
        return ProximaSession(
            db,
            master=self._master,
            memory_limit=self._memory_limit,
            batch_size=self._batch_size,
        )


class ProximaSession:
    """Notebook/procedural session backed by an embedded Rust database."""

    def __init__(
        self,
        db: ProximaDB,
        *,
        master: str = "proxima-local[1]",
        memory_limit: str | None = None,
        batch_size: int = 10000,
    ) -> None:
        self.db = db
        self.master = master
        self.memory_limit = memory_limit
        self.batch_size = batch_size

    @classmethod
    def builder(cls) -> ProximaSessionBuilder:
        return ProximaSessionBuilder()

    @property
    def worker_count(self) -> int:
        match = _MASTER_RE.match(self.master)
        return int(match.group("workers")) if match else 1

    def table(self, name: str) -> ProximaFrame:
        return ProximaFrame(self, _Plan("table", name))

    def sql(self, query: str) -> ProximaFrame:
        return ProximaFrame(self, _Plan("sql", query))

    def explain_plan(self, plan: _Plan) -> Mapping[str, Any]:
        if hasattr(self.db, "explain_notebook_plan"):
            try:
                return self.db.explain_notebook_plan(self._plan_envelope(plan))
            except RuntimeError:
                pass
        frame = ProximaFrame(self, plan)
        compiled_sql: str | None
        try:
            compiled_sql = frame.compile_sql()
        except NotImplementedError:
            compiled_sql = None
        partition_plan = self._partition_plan(plan)

        unsupported = [
            op["type"]
            for op in plan.operations
            if op["type"] not in {"select", "where", "limit", "group_count"}
        ]
        return {
            "source_surface": "python_notebook",
            "execution_scope": "local_process",
            "master": self.master,
            "workers": self.worker_count,
            "memory_limit": self.memory_limit,
            "batch_size": self.batch_size,
            "authority_mode": "ProximaAuthoritative",
            "policy_boundary": "engine-enforced",
            "compute_route": "DataFusionLocal" if compiled_sql else "Native",
            "status": "phase1_python_facade",
            "compiled_sql": compiled_sql,
            "partition_plan": partition_plan,
            "effective_parallelism": partition_plan["effective_read_partitions"],
            "unsupported_operations": unsupported,
            "plan": {
                "source_kind": plan.source_kind,
                "source": plan.source,
                "operations": list(plan.operations),
            },
        }

    def _plan_envelope(self, plan: _Plan) -> Mapping[str, Any]:
        return {
            "source_surface": "python_notebook",
            "session": {
                "master": self.master,
                "workers": self.worker_count,
                "memory_limit": self.memory_limit,
                "batch_size": self.batch_size,
            },
            "plan": {
                "source_kind": plan.source_kind,
                "source": plan.source,
                "operations": list(plan.operations),
            },
        }

    def _partition_plan(self, plan: _Plan) -> Mapping[str, Any]:
        if plan.source_kind != "table":
            return {
                "requested_partitions": self.worker_count,
                "planned_partitions": 1,
                "effective_read_partitions": 1,
                "planner": "sql_source_fallback",
                "execution_scope": "local_process",
            }
        if hasattr(self.db, "plan_partitions"):
            try:
                return self.db.plan_partitions(plan.source, self.worker_count)
            except RuntimeError as exc:
                return self._fallback_partition_plan(
                    plan.source,
                    "native partition planning rejected this source; "
                    f"using one safe logical partition: {exc}",
                )
        return self._fallback_partition_plan(
            plan.source,
            "native partition diagnostics unavailable in this extension build; "
            "using one safe whole-collection partition",
        )

    def _fallback_partition_plan(
        self,
        source: str,
        rejected_reason: str,
    ) -> Mapping[str, Any]:
        return {
            "collection": source,
            "requested_partitions": self.worker_count,
            "planned_partitions": 1,
            "effective_read_partitions": 1,
            "planner": "whole_collection_fallback",
            "execution_scope": "local_process",
            "safe_parallelism": 1,
            "rejected_parallelism_reason": rejected_reason,
            "partitions": [
                {
                    "partition_id": 0,
                    "preferred_locations": [],
                    "estimated_rows": None,
                    "estimated_bytes": None,
                    "splits": [
                        {
                            "split_id": f"collection:{source}:whole",
                            "file_path": f"collection://{source}",
                            "offset": 0,
                            "length": 0,
                            "estimated_rows": None,
                            "estimated_bytes": None,
                        }
                    ],
                }
            ],
        }


class ProximaFrame:
    """Lazy frame with a PySpark-style subset of operations."""

    def __init__(self, session: ProximaSession, plan: _Plan) -> None:
        self.session = session
        self._plan = plan

    @property
    def plan(self) -> Mapping[str, Any]:
        return {
            "source_kind": self._plan.source_kind,
            "source": self._plan.source,
            "operations": list(self._plan.operations),
        }

    def select(self, *columns: Any) -> ProximaFrame:
        if len(columns) == 1 and isinstance(columns[0], (list, tuple)):
            columns = tuple(columns[0])
        names = [c.name if isinstance(c, Column) else str(c) for c in columns]
        if not names:
            raise ValueError("select requires at least one column")
        return ProximaFrame(
            self.session, self._plan.append({"type": "select", "columns": names})
        )

    def where(self, predicate: Predicate) -> ProximaFrame:
        if not isinstance(predicate, Predicate):
            raise TypeError("where expects a Predicate built with col(...)")
        return ProximaFrame(
            self.session,
            self._plan.append({"type": "where", "predicate": predicate.sql}),
        )

    filter = where

    def limit(self, count: int) -> ProximaFrame:
        if count < 0:
            raise ValueError("limit must be non-negative")
        return ProximaFrame(
            self.session, self._plan.append({"type": "limit", "count": int(count)})
        )

    def group_by(self, *columns: Any) -> GroupedProximaFrame:
        if len(columns) == 1 and isinstance(columns[0], (list, tuple)):
            columns = tuple(columns[0])
        names = [c.name if isinstance(c, Column) else str(c) for c in columns]
        if not names:
            raise ValueError("group_by requires at least one column")
        return GroupedProximaFrame(self, names)

    def vector_search(
        self,
        *,
        column: str,
        query: Sequence[float],
        top_k: int = 10,
    ) -> ProximaFrame:
        return ProximaFrame(
            self.session,
            self._plan.append(
                {
                    "type": "vector_search",
                    "column": column,
                    "query": list(query),
                    "top_k": int(top_k),
                }
            ),
        )

    def compile_sql(self) -> str:
        if self._plan.source_kind == "sql":
            if self._plan.operations:
                raise NotImplementedError(
                    "operations on raw SQL frames are not supported yet"
                )
            return self._plan.source.strip()
        if self._plan.source_kind != "table":
            raise NotImplementedError(
                f"unsupported source kind {self._plan.source_kind!r}"
            )

        select_columns: Sequence[str] | None = None
        predicates = []
        limit_count: int | None = None
        group_columns: Sequence[str] | None = None
        aggregate_count = False

        for op in self._plan.operations:
            op_type = op["type"]
            if op_type == "select":
                select_columns = op["columns"]  # type: ignore[assignment]
            elif op_type == "where":
                predicates.append(str(op["predicate"]))
            elif op_type == "limit":
                limit_count = int(op["count"])
            elif op_type == "group_count":
                group_columns = op["columns"]  # type: ignore[assignment]
                aggregate_count = True
            else:
                raise NotImplementedError(f"{op_type} is not SQL-backed in phase 1")

        if aggregate_count:
            assert group_columns is not None
            projection = ", ".join(_quote_ident(c) for c in group_columns)
            select_clause = f"{projection}, COUNT(*) AS count"
            group_clause = " GROUP BY " + projection
        else:
            select_clause = (
                ", ".join(_quote_ident(c) for c in select_columns)
                if select_columns
                else "*"
            )
            group_clause = ""

        sql = f"SELECT {select_clause} FROM {_table_ident(self._plan.source)}"
        if predicates:
            sql += " WHERE " + " AND ".join(f"({p})" for p in predicates)
        sql += group_clause
        if limit_count is not None:
            sql += f" LIMIT {limit_count}"
        return sql

    def _operation(self, op_type: str) -> Mapping[str, Any] | None:
        for op in self._plan.operations:
            if op.get("type") == op_type:
                return op
        return None

    def _operations(self, op_type: str) -> list[Mapping[str, Any]]:
        return [op for op in self._plan.operations if op.get("type") == op_type]

    def _has_vector_search(self) -> bool:
        return self._operation("vector_search") is not None

    def explain(self, format: str = "json") -> Any:
        explanation = dict(self.session.explain_plan(self._plan))
        if format == "json":
            return explanation
        if format == "text":
            compiled = explanation.get("compiled_sql") or "<not SQL-backed>"
            return (
                "Proxima notebook plan\n"
                f"  route: {explanation['compute_route']}\n"
                f"  scope: {explanation['execution_scope']}\n"
                f"  workers: {explanation['workers']}\n"
                f"  effective_parallelism: {explanation['effective_parallelism']}\n"
                f"  sql: {compiled}"
            )
        raise ValueError("format must be 'json' or 'text'")

    def collect(self, limit: int | None = None) -> list[Mapping[str, Any]]:
        frame = self.limit(limit) if limit is not None else self
        if frame._has_vector_search():
            return frame._collect_vector_search()
        result = self.session.db.execute_sql(frame.compile_sql())
        return list(result.get("rows", []))

    def _collect_vector_search(self) -> list[Mapping[str, Any]]:
        if self._plan.source_kind != "table":
            raise NotImplementedError(
                "vector_search requires a table/collection source"
            )
        vector_ops = self._operations("vector_search")
        if len(vector_ops) != 1:
            raise NotImplementedError(
                "exactly one vector_search operation is supported"
            )
        unsupported = [
            op.get("type")
            for op in self._plan.operations
            if op.get("type") not in {"vector_search", "select", "limit"}
        ]
        if unsupported:
            raise NotImplementedError(
                "vector_search phase 1 supports select and limit only; "
                f"unsupported operations: {unsupported}"
            )

        vector_op = vector_ops[0]
        top_k = int(vector_op.get("top_k", 10))
        limits = [int(op["count"]) for op in self._operations("limit")]
        if limits:
            top_k = min(top_k, *limits)
        results = self.session.db.search(
            self._plan.source,
            query=vector_op["query"],
            top_k=top_k,
        )
        rows = []
        for result in results:
            metadata = dict(result.metadata)
            row = {
                "id": result.id,
                "score": result.score,
                "metadata": metadata,
            }
            for key, value in metadata.items():
                row.setdefault(key, value)
            rows.append(row)

        select = self._operation("select")
        if select is not None:
            columns = list(select["columns"])
            rows = [{column: row.get(column) for column in columns} for row in rows]
        return rows

    def to_arrow(self) -> Any:
        try:
            import pyarrow as pa
            import pyarrow.ipc as ipc
        except ImportError as exc:
            raise ImportError(
                "to_arrow requires pyarrow. Install proximadb_embedded[arrow]."
            ) from exc
        if self._has_vector_search():
            return pa.Table.from_pylist(self.collect())
        if hasattr(self.session.db, "execute_notebook_plan_arrow_ipc"):
            data = self.session.db.execute_notebook_plan_arrow_ipc(
                self.session._plan_envelope(self._plan)
            )
            with ipc.open_stream(pa.BufferReader(data)) as reader:
                return reader.read_all()
        if hasattr(self.session.db, "execute_sql_arrow_ipc"):
            data = self.session.db.execute_sql_arrow_ipc(self.compile_sql())
            with ipc.open_stream(pa.BufferReader(data)) as reader:
                return reader.read_all()
        return pa.Table.from_pylist(self.collect())

    def to_pandas(self) -> Any:
        return self.to_arrow().to_pandas()


class GroupedProximaFrame:
    """Grouped frame returned by ``ProximaFrame.group_by``."""

    def __init__(self, frame: ProximaFrame, columns: Sequence[str]) -> None:
        self._frame = frame
        self._columns = list(columns)

    def count(self) -> ProximaFrame:
        return ProximaFrame(
            self._frame.session,
            self._frame._plan.append({"type": "group_count", "columns": self._columns}),
        )


__all__ = [
    "Column",
    "GroupedProximaFrame",
    "Predicate",
    "ProximaFrame",
    "ProximaSession",
    "ProximaSessionBuilder",
    "col",
]
