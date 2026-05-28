"""DDL helpers for agentic backing-store schemas.

The SDK owns the flexible schema authoring surface: callers describe stable
fields, JSONB payloads, vector embeddings, graph projections, and event streams.
The emitted SQL stays PostgreSQL/pgwire compatible so the Rust server can parse
and execute the relational core while catalog metadata records cross-modal
intent.
"""

from __future__ import annotations

from dataclasses import dataclass, field


@dataclass(frozen=True)
class AgenticField:
    name: str
    sql_type: str
    required: bool = False
    indexed: bool = False


@dataclass(frozen=True)
class JsonbProjection:
    column: str
    path: str = "$"
    indexed: bool = False


@dataclass(frozen=True)
class VectorProjection:
    field: str
    dimension: int
    metadata_column: str = "metadata"


@dataclass(frozen=True)
class GraphProjection:
    label: str
    id_field: str
    edge_types: tuple[str, ...] = ()


@dataclass(frozen=True)
class EventProjection:
    stream_prefix: str
    payload_column: str = "payload"
    partition_fields: tuple[str, ...] = ("tenant_id", "thread_id")


@dataclass(frozen=True)
class AgenticDDL:
    store: str
    table: str
    fields: tuple[AgenticField, ...]
    jsonb: tuple[JsonbProjection, ...] = field(default_factory=tuple)
    vectors: tuple[VectorProjection, ...] = field(default_factory=tuple)
    graph: tuple[GraphProjection, ...] = field(default_factory=tuple)
    events: tuple[EventProjection, ...] = field(default_factory=tuple)
    catalog_namespace: str | None = None
    storage_engine: str = "SST"
    physical_layout: str = "hybrid"

    @classmethod
    def default(
        cls,
        store: str,
        *,
        embedding_dimension: int = 1536,
        storage_engine: str = "SST",
        physical_layout: str = "hybrid",
    ) -> AgenticDDL:
        store_ident = _ident(store)
        table = f"{store_ident}_agent_store"
        return cls(
            store=store,
            table=table,
            catalog_namespace=f"agentic.{store_ident}",
            fields=(
                AgenticField("record_id", "TEXT", required=True),
                AgenticField("tenant_id", "TEXT", required=True, indexed=True),
                AgenticField("thread_id", "TEXT", required=True, indexed=True),
                AgenticField("namespace", "TEXT", required=True, indexed=True),
                AgenticField("key", "TEXT", required=True, indexed=True),
                AgenticField("created_at_ms", "BIGINT", required=True, indexed=True),
                AgenticField("updated_at_ms", "BIGINT", required=True, indexed=True),
            ),
            jsonb=(
                JsonbProjection("payload", "$", indexed=True),
                JsonbProjection("metadata", "$", indexed=True),
                JsonbProjection("checkpoint", "$", indexed=False),
            ),
            vectors=(VectorProjection("embedding", embedding_dimension),),
            graph=(GraphProjection("Symbol", "record_id", ("REFERENCES", "CALLS")),),
            events=(EventProjection("agent"),),
            storage_engine=storage_engine,
            physical_layout=physical_layout,
        )

    def create_table_sql(self) -> str:
        definitions = []
        for item in self.fields:
            suffix = " NOT NULL" if item.required else ""
            definitions.append(f"{_q(item.name)} {item.sql_type}{suffix}")
        definitions.extend(
            f"{_q(item.column)} JSONB NOT NULL DEFAULT '{{}}'::jsonb"
            for item in self.jsonb
        )
        definitions.extend(
            f"{_q(item.field)} VECTOR({item.dimension})" for item in self.vectors
        )
        definitions.append(f"PRIMARY KEY ({_q('record_id')})")
        options = (
            f"storage_engine = '{self.storage_engine}', "
            f"layout = '{self.physical_layout}', "
            f"xcatalog_namespace = '{self.catalog_namespace or f'agentic.{self.store}'}', "
            "schema_kind = 'agentic_mixed'"
        )
        return (
            f"CREATE TABLE IF NOT EXISTS {_q(self.table)} "
            f"({', '.join(definitions)}) WITH ({options});"
        )

    def index_sql(self) -> list[str]:
        statements: list[str] = []
        for item in self.fields:
            if item.indexed:
                statements.append(
                    f"CREATE INDEX IF NOT EXISTS {_q(f'idx_{self.table}_{item.name}')} "
                    f"ON {_q(self.table)} ({_q(item.name)});"
                )
        for item in self.jsonb:
            if item.indexed:
                statements.append(
                    f"CREATE INDEX IF NOT EXISTS {_q(f'idx_{self.table}_{item.column}_gin')} "
                    f"ON {_q(self.table)} USING GIN ({_q(item.column)});"
                )
        for item in self.vectors:
            statements.append(
                f"CREATE INDEX IF NOT EXISTS {_q(f'idx_{self.table}_{item.field}_hnsw')} "
                f"ON {_q(self.table)} USING HNSW ({_q(item.field)});"
            )
        return statements

    def xcatalog_sql(self) -> list[str]:
        namespace = self.catalog_namespace or f"agentic.{self.store}"
        statements = [
            f"COMMENT ON TABLE {_q(self.table)} IS "
            f"'xcatalog.namespace={namespace};"
            f"engine={self.storage_engine};layout={self.physical_layout}';"
        ]
        for item in self.graph:
            statements.append(
                f"COMMENT ON COLUMN {_q(self.table)}.{_q(item.id_field)} IS "
                f"'xcatalog.graph.label={item.label};edges={','.join(item.edge_types)}';"
            )
        for item in self.events:
            statements.append(
                f"COMMENT ON COLUMN {_q(self.table)}.{_q(item.payload_column)} IS "
                f"'xcatalog.event.stream_prefix={item.stream_prefix};"
                f"partitions={','.join(item.partition_fields)}';"
            )
        return statements

    def statements(self, *, include_xcatalog: bool = True) -> list[str]:
        statements = [self.create_table_sql(), *self.index_sql()]
        if include_xcatalog:
            statements.extend(self.xcatalog_sql())
        return statements


def _ident(value: str) -> str:
    cleaned = "".join(ch if ch.isalnum() or ch == "_" else "_" for ch in value.lower())
    return cleaned.strip("_") or "agent"


def _q(value: str) -> str:
    return '"' + value.replace('"', '""') + '"'
