from __future__ import annotations

from proximadb_sdk.integrations.agentic_ddl import AgenticDDL


def test_agentic_ddl_emits_pgwire_compatible_mixed_schema() -> None:
    ddl = AgenticDDL.default(
        "Victor Repo",
        embedding_dimension=384,
        storage_engine="VIPER",
        physical_layout="columnar",
    )

    statements = ddl.statements()

    assert statements[0].startswith('CREATE TABLE IF NOT EXISTS "victor_repo_agent_store"')
    assert '"tenant_id" TEXT NOT NULL' in statements[0]
    assert '"payload" JSONB NOT NULL DEFAULT \'{}\'::jsonb' in statements[0]
    assert '"embedding" VECTOR(384)' in statements[0]
    assert 'PRIMARY KEY ("record_id")' in statements[0]
    assert any("USING GIN" in statement and '"payload"' in statement for statement in statements)
    assert any("USING HNSW" in statement and '"embedding"' in statement for statement in statements)
    assert any(
        "xcatalog.namespace=agentic.victor_repo;engine=VIPER;layout=columnar" in statement
        for statement in statements
    )
    assert any("xcatalog.graph.label=Symbol" in statement for statement in statements)
    assert any("xcatalog.event.stream_prefix=agent" in statement for statement in statements)


def test_agentic_ddl_can_emit_relational_core_without_catalog_comments() -> None:
    ddl = AgenticDDL.default("agent", embedding_dimension=16)

    statements = ddl.statements(include_xcatalog=False)

    assert statements
    assert all("COMMENT ON" not in statement for statement in statements)
    assert statements[0] == (
        'CREATE TABLE IF NOT EXISTS "agent_agent_store" ('
        '"record_id" TEXT NOT NULL, '
        '"tenant_id" TEXT NOT NULL, '
        '"thread_id" TEXT NOT NULL, '
        '"namespace" TEXT NOT NULL, '
        '"key" TEXT NOT NULL, '
        '"created_at_ms" BIGINT NOT NULL, '
        '"updated_at_ms" BIGINT NOT NULL, '
        '"payload" JSONB NOT NULL DEFAULT \'{}\'::jsonb, '
        '"metadata" JSONB NOT NULL DEFAULT \'{}\'::jsonb, '
        '"checkpoint" JSONB NOT NULL DEFAULT \'{}\'::jsonb, '
        '"embedding" VECTOR(16), '
        'PRIMARY KEY ("record_id"));'
    )
