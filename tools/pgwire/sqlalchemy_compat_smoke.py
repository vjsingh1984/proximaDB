#!/usr/bin/env python3
"""SQLAlchemy smoke test for ProximaDB pgwire compatibility."""

from __future__ import annotations

import os
import sys


def main() -> int:
    try:
        import sqlalchemy as sa
        from sqlalchemy import inspect, text
    except ImportError:
        print("SKIP: SQLAlchemy is not installed in this Python environment")
        return 77

    url = os.environ.get(
        "PROXIMADB_PGWIRE_URL",
        "postgresql+psycopg://postgres@127.0.0.1:5433/benchbase?sslmode=disable",
    )
    engine = sa.create_engine(url, future=True)

    with engine.begin() as conn:
        conn.execute(text("DROP TABLE IF EXISTS sqlalchemy_smoke_items"))
        conn.execute(
            text(
                "CREATE TABLE sqlalchemy_smoke_items ("
                "id INT PRIMARY KEY, "
                "name VARCHAR(64) NOT NULL, "
                "qty INT DEFAULT 0, "
                "price DECIMAL(12,2)"
                ")"
            )
        )
        conn.execute(
            text(
                "INSERT INTO sqlalchemy_smoke_items (id, name, qty, price) "
                "VALUES (1, 'widget', 3, 12.50)"
            )
        )
        row = conn.execute(
            text(
                "SELECT id, name, qty, price "
                "FROM sqlalchemy_smoke_items WHERE id = 1"
            )
        ).one()
        assert str(row._mapping["name"]) == "widget", row

    inspector = inspect(engine)
    table_names = inspector.get_table_names(schema="public")
    assert "sqlalchemy_smoke_items" in table_names, table_names

    columns = inspector.get_columns("sqlalchemy_smoke_items", schema="public")
    column_names = {column["name"] for column in columns}
    assert {"id", "name", "qty", "price"}.issubset(column_names), columns

    print("SQLAlchemy compatibility smoke passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
