#!/usr/bin/env bash
set -u

PGHOST="${PGHOST:-127.0.0.1}"
PGPORT="${PGPORT:-5433}"
PGUSER="${PGUSER:-postgres}"
PGDATABASE="${PGDATABASE:-benchbase}"

PSQL=(psql -X -v ON_ERROR_STOP=1 -h "$PGHOST" -p "$PGPORT" -U "$PGUSER" -d "$PGDATABASE")

pass=0
fail=0
gap=0

run_sql() {
  local label="$1"
  local sql="$2"
  echo "== $label"
  if "${PSQL[@]}" -c "$sql"; then
    pass=$((pass + 1))
  else
    echo "FAIL: $label"
    fail=$((fail + 1))
  fi
}

run_sql_contains() {
  local label="$1"
  local sql="$2"
  local expected="$3"
  local output
  echo "== $label"
  if output=$("${PSQL[@]}" -c "$sql" 2>&1); then
    echo "$output"
    if grep -Fq "$expected" <<<"$output"; then
      pass=$((pass + 1))
    else
      echo "FAIL: $label (missing expected output: $expected)"
      fail=$((fail + 1))
    fi
  else
    echo "$output"
    echo "FAIL: $label"
    fail=$((fail + 1))
  fi
}

known_gap() {
  local label="$1"
  local sql="$2"
  echo "== known gap: $label"
  if "${PSQL[@]}" -c "$sql"; then
    echo "SEMANTIC GAP STILL PRESENT: $label"
    gap=$((gap + 1))
  else
    echo "EXPECTED GAP: $label"
    gap=$((gap + 1))
  fi
}

known_gap_until_contains() {
  local label="$1"
  local sql="$2"
  local expected="$3"
  local output
  echo "== known gap: $label"
  if output=$("${PSQL[@]}" -c "$sql" 2>&1); then
    echo "$output"
    if grep -Fq "$expected" <<<"$output"; then
      echo "GAP CLOSED: $label"
      pass=$((pass + 1))
    else
      echo "EXPECTED GAP: $label"
      gap=$((gap + 1))
    fi
  else
    echo "$output"
    echo "EXPECTED GAP: $label"
    gap=$((gap + 1))
  fi
}

run_session_sql_contains() {
  local label="$1"
  local sql="$2"
  local expected="$3"
  local output
  echo "== $label"
  if output=$(printf "%s\n" "$sql" | "${PSQL[@]}" 2>&1); then
    echo "$output"
    if grep -Fq "$expected" <<<"$output"; then
      pass=$((pass + 1))
    else
      echo "FAIL: $label (missing expected output: $expected)"
      fail=$((fail + 1))
    fi
  else
    echo "$output"
    echo "FAIL: $label"
    fail=$((fail + 1))
  fi
}

run_sql "metadata scalar current_schema" "SELECT current_schema();"
run_sql "metadata scalar current_database" "SELECT current_database();"
run_sql "metadata scalar current_user" "SELECT current_user;"
run_sql "pg_settings max_index_keys" "SELECT setting FROM pg_catalog.pg_settings WHERE name = 'max_index_keys';"
run_sql "constant select" "SELECT 1 AS one;"

run_sql "drop smoke order table" "DROP TABLE IF EXISTS pgwire_smoke_orders;"
run_sql "drop smoke customer table" "DROP TABLE IF EXISTS pgwire_smoke_customer;"
run_sql "create customer table" "CREATE TABLE pgwire_smoke_customer (c_id INT PRIMARY KEY, c_name VARCHAR(32) NOT NULL, c_balance DECIMAL(12,2) DEFAULT 0, c_active BOOLEAN DEFAULT true);"
run_sql "create order table with constraints" "CREATE TABLE pgwire_smoke_orders (o_id INT PRIMARY KEY, c_id INT NOT NULL, o_status CHAR(1) DEFAULT 'N', o_total DECIMAL(12,2), CONSTRAINT fk_customer FOREIGN KEY (c_id) REFERENCES pgwire_smoke_customer(c_id), CONSTRAINT uq_order_customer UNIQUE (o_id, c_id), CONSTRAINT ck_total CHECK (o_total >= 0));"
run_sql "create secondary index" "CREATE INDEX idx_pgwire_smoke_orders_customer ON pgwire_smoke_orders(c_id);"

run_sql "insert customer" "INSERT INTO pgwire_smoke_customer (c_id, c_name, c_balance, c_active) VALUES (1, 'alice', 42.50, true);"
run_sql "upsert customer" "INSERT INTO pgwire_smoke_customer (c_id, c_name, c_balance, c_active) VALUES (1, 'alice updated', 50.00, true) ON CONFLICT (c_id) DO UPDATE SET c_name = EXCLUDED.c_name, c_balance = EXCLUDED.c_balance;"
run_sql "insert order" "INSERT INTO pgwire_smoke_orders (o_id, c_id, o_status, o_total) VALUES (100, 1, 'N', 125.75);"
run_sql "point select customer" "SELECT c_id, c_name, c_balance, c_active FROM pgwire_smoke_customer WHERE c_id = 1;"
run_sql "point select order" "SELECT * FROM pgwire_smoke_orders WHERE o_id = 100;"
run_sql "update customer" "UPDATE pgwire_smoke_customer SET c_balance = 75.25 WHERE c_id = 1;"
run_sql "point select after update" "SELECT c_id, c_name, c_balance FROM pgwire_smoke_customer WHERE c_id = 1;"
run_sql_contains "non primary key predicate select" "SELECT c_id, c_name, c_balance FROM pgwire_smoke_customer WHERE c_name = 'alice updated' AND c_active = true;" "alice updated"
run_sql_contains "in predicate select" "SELECT c_id, c_name FROM pgwire_smoke_customer WHERE c_id IN (1, 2);" "alice updated"
run_sql_contains "like predicate select" "SELECT c_id, c_name FROM pgwire_smoke_customer WHERE c_name LIKE 'alice%';" "alice updated"
run_session_sql_contains "write intent session hint reaches EXPLAIN" "SET proximadb.write.row_count_hint = '100000';
EXPLAIN (FORMAT JSON) INSERT INTO pgwire_smoke_customer SELECT * FROM pgwire_smoke_customer;" "\"row_count_hint\": 100000"
run_sql "delete order" "DELETE FROM pgwire_smoke_orders WHERE o_id = 100;"

run_sql "information_schema tables" "SELECT table_schema, table_name FROM information_schema.tables WHERE table_name = 'pgwire_smoke_customer';"
run_sql "information_schema columns" "SELECT table_name, column_name, data_type FROM information_schema.columns WHERE table_name = 'pgwire_smoke_customer';"
run_sql "pg_catalog jdbc tables" "SELECT NULL AS TABLE_CAT, n.nspname AS TABLE_SCHEM, c.relname AS TABLE_NAME, CASE c.relkind WHEN 'r' THEN 'TABLE' ELSE NULL END AS TABLE_TYPE FROM pg_catalog.pg_namespace n, pg_catalog.pg_class c WHERE c.relnamespace = n.oid AND c.relname = 'pgwire_smoke_customer' ORDER BY TABLE_TYPE,TABLE_SCHEM,TABLE_NAME;"
run_sql "pg_catalog jdbc columns" "SELECT NULL AS TABLE_CAT, n.nspname AS TABLE_SCHEM, c.relname AS TABLE_NAME, a.attname AS COLUMN_NAME, a.atttypid AS DATA_TYPE, a.attlen AS COLUMN_SIZE, CASE WHEN a.attnotnull THEN 'NO' ELSE 'YES' END AS IS_NULLABLE FROM pg_catalog.pg_namespace n, pg_catalog.pg_class c, pg_catalog.pg_attribute a WHERE c.relnamespace = n.oid AND a.attrelid = c.oid AND c.relname = 'pgwire_smoke_customer' ORDER BY TABLE_SCHEM,TABLE_NAME,a.attnum;"

run_sql_contains "full table scan select returns inserted customer" "SELECT * FROM pgwire_smoke_customer;" "alice updated"
run_sql_contains "full table scan after update shows updated balance" "SELECT c_id, c_balance FROM pgwire_smoke_customer WHERE c_id = 1;" "75.25"
run_sql "insert second customer for delete test" "INSERT INTO pgwire_smoke_customer (c_id, c_name, c_balance, c_active) VALUES (2, 'bob', 10.00, true);"
run_sql "delete second customer" "DELETE FROM pgwire_smoke_customer WHERE c_id = 2;"
run_sql_contains "scan after delete — only alice remains" "SELECT c_id, c_name FROM pgwire_smoke_customer;" "alice"
run_sql_contains "or predicate select returns correct row" "SELECT c_id, c_name FROM pgwire_smoke_customer WHERE c_id = 1 OR c_id = 2;" "alice"

echo "psql compatibility smoke: pass=$pass fail=$fail known_gap=$gap"
if [ "$fail" -ne 0 ]; then
  exit 1
fi
