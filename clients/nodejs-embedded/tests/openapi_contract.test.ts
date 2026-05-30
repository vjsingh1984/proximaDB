/**
 * OpenAPI contract gate for the TypeScript REST SDK.
 *
 * For every covered SDK method this test:
 *   1. Stubs `globalThis.fetch` to capture the outbound request and return a
 *      programmed minimal-valid response.
 *   2. Calls the SDK method with a sample input.
 *   3. Asserts the captured URL/method/body against the corresponding
 *      OpenAPI operation defined in
 *      `docs/openapi/proximadb-openapi.yaml`.
 *
 * No server is started; drift between the SDK and the contract therefore
 * fails this test deterministically. Mirrors the Python contract gate at
 * `clients/python/tests/unit/test_openapi_contract.py`.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as fs from "node:fs";
import * as path from "node:path";
import * as YAML from "yaml";
import { ProximaDBClient } from "../src/client";

// ---------------------------------------------------------------------------
// Spec loading
// ---------------------------------------------------------------------------

const REPO_ROOT = path.resolve(__dirname, "..", "..", "..");
const SPEC_PATH = path.join(
  REPO_ROOT,
  "docs",
  "openapi",
  "proximadb-openapi.yaml",
);

type AnyObj = Record<string, unknown>;

function loadSpec(): AnyObj {
  if (!fs.existsSync(SPEC_PATH)) {
    throw new Error(`OpenAPI spec missing at ${SPEC_PATH}`);
  }
  return YAML.parse(fs.readFileSync(SPEC_PATH, "utf-8")) as AnyObj;
}

const SPEC: AnyObj = loadSpec();

function resolveRef(spec: AnyObj, ref: string): AnyObj {
  if (!ref.startsWith("#/")) {
    throw new Error(`Unsupported $ref form: ${ref}`);
  }
  let node: unknown = spec;
  for (const part of ref.slice(2).split("/")) {
    node = (node as AnyObj)[part];
    if (node === undefined) {
      throw new Error(`Cannot resolve ref ${ref} (broke at "${part}")`);
    }
  }
  return node as AnyObj;
}

function operationFor(
  spec: AnyObj,
  pathTemplate: string,
  method: string,
): AnyObj {
  const paths = spec.paths as AnyObj;
  const pathItem = paths[pathTemplate] as AnyObj | undefined;
  if (!pathItem) {
    throw new Error(`Path ${pathTemplate} not in OpenAPI spec`);
  }
  const op = pathItem[method.toLowerCase()] as AnyObj | undefined;
  if (!op) {
    throw new Error(`${method} ${pathTemplate} not defined in spec`);
  }
  return op;
}

function requestBodySchema(spec: AnyObj, operation: AnyObj): AnyObj | null {
  const body = operation.requestBody as AnyObj | undefined;
  if (!body) return null;
  const content = body.content as AnyObj;
  const json = content["application/json"] as AnyObj;
  let schema = json.schema as AnyObj;
  if (schema.$ref) {
    schema = resolveRef(spec, schema.$ref as string);
  }
  return schema;
}

/**
 * Lightweight schema-level check: confirm every top-level `required` key
 * present in the (resolved, possibly allOf-merged) request schema is also
 * present in the actual payload.
 */
function collectRequiredKeys(spec: AnyObj, schema: AnyObj): string[] {
  const keys: string[] = [];
  const visit = (s: AnyObj): void => {
    if (s.$ref) {
      visit(resolveRef(spec, s.$ref as string));
      return;
    }
    if (Array.isArray(s.allOf)) {
      for (const sub of s.allOf as AnyObj[]) {
        visit(sub);
      }
    }
    if (Array.isArray(s.required)) {
      for (const k of s.required as string[]) {
        if (!keys.includes(k)) keys.push(k);
      }
    }
  };
  visit(schema);
  return keys;
}

// ---------------------------------------------------------------------------
// Capturing fetch stub
// ---------------------------------------------------------------------------

interface Captured {
  url: string | null;
  method: string | null;
  body: AnyObj | null;
}

function installFetchStub(responseBody: unknown, status = 200): Captured {
  const captured: Captured = { url: null, method: null, body: null };

  const stub = vi.fn(async (url: string, init?: RequestInit) => {
    captured.url = url;
    captured.method = (init?.method ?? "GET").toUpperCase();
    captured.body =
      init?.body !== undefined && init?.body !== null
        ? (JSON.parse(String(init.body)) as AnyObj)
        : null;
    return {
      ok: status >= 200 && status < 300,
      status,
      statusText: "OK",
      json: async () => responseBody,
      text: async () => JSON.stringify(responseBody),
    };
  });

  // The client captures `globalThis.fetch` at construction time, so we must
  // patch the global before constructing the client in each test.
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  (globalThis as any).fetch = stub;

  return captured;
}

function makeClient(): ProximaDBClient {
  return new ProximaDBClient({
    url: "http://contract.test",
    maxRetries: 0,
  });
}

const ORIGINAL_FETCH = globalThis.fetch;

beforeEach(() => {
  // ensure a clean slate each test
});

afterEach(() => {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  (globalThis as any).fetch = ORIGINAL_FETCH;
  vi.restoreAllMocks();
});

// ---------------------------------------------------------------------------
// Per-operation contract checks (6 new v2 ops)
// ---------------------------------------------------------------------------

describe("OpenAPI contract gate", () => {
  it("healthLive() matches GET /health/live (getLiveness)", async () => {
    const captured = installFetchStub({ status: "ok" });
    const client = makeClient();

    const probe = await client.healthLive();

    const op = operationFor(SPEC, "/health/live", "get");
    expect(op.operationId).toBe("getLiveness");
    expect(captured.method).toBe("GET");
    expect(captured.url).toBe("http://contract.test/health/live");
    expect(captured.body).toBeNull();
    expect(probe.status).toBe("ok");
  });

  it("healthReady() matches GET /health/ready (getReadiness)", async () => {
    const captured = installFetchStub({ status: "ready" });
    const client = makeClient();

    const probe = await client.healthReady();

    const op = operationFor(SPEC, "/health/ready", "get");
    expect(op.operationId).toBe("getReadiness");
    expect(captured.method).toBe("GET");
    expect(captured.url).toBe("http://contract.test/health/ready");
    expect(captured.body).toBeNull();
    expect(probe.status).toBe("ready");
  });

  it("getCollectionSchema() matches GET /api/v2/collections/{id}/schema (getCollectionSchema)", async () => {
    const captured = installFetchStub({
      schema_id: "sch_1",
      schema_version: "v1",
      collection_id: "col_abc",
      schema: { columns: [{ name: "title", data_type: "text" }] },
      created_at: "2026-05-23T00:00:00Z",
    });
    const client = makeClient();

    const resp = await client.getCollectionSchema("col_abc");

    const op = operationFor(
      SPEC,
      "/api/v2/collections/{collection_id}/schema",
      "get",
    );
    expect(op.operationId).toBe("getCollectionSchema");
    expect(captured.method).toBe("GET");
    expect(captured.url).toBe(
      "http://contract.test/api/v2/collections/col_abc/schema",
    );
    expect(captured.body).toBeNull();
    expect(resp.schema_id).toBe("sch_1");
  });

  it("updateCollectionSchema() matches PUT /api/v2/collections/{id}/schema (updateCollectionSchema)", async () => {
    const captured = installFetchStub({
      schema_id: "sch_2",
      schema_version: "v2",
      previous_schema_id: "sch_1",
      changes: [],
      warnings: [],
      updated_at: "2026-05-23T00:00:00Z",
    });
    const client = makeClient();

    const requestBody = {
      columns: [{ name: "title", data_type: "text" }],
      enforcement: "strict" as const,
      force: true,
    };
    const resp = await client.updateCollectionSchema("col_abc", requestBody);

    const op = operationFor(
      SPEC,
      "/api/v2/collections/{collection_id}/schema",
      "put",
    );
    expect(op.operationId).toBe("updateCollectionSchema");
    expect(captured.method).toBe("PUT");
    expect(captured.url).toBe(
      "http://contract.test/api/v2/collections/col_abc/schema",
    );

    // Body must satisfy the spec's required top-level keys.
    const reqSchema = requestBodySchema(SPEC, op);
    expect(reqSchema).not.toBeNull();
    const requiredKeys = collectRequiredKeys(SPEC, reqSchema!);
    expect(requiredKeys).toContain("columns");
    for (const key of requiredKeys) {
      expect(captured.body).not.toBeNull();
      expect(Object.keys(captured.body!)).toContain(key);
    }
    expect(captured.body!.force).toBe(true);
    expect(resp.schema_id).toBe("sch_2");
  });

  it("executeQuery() matches POST /api/v2/query (executeQuery)", async () => {
    const captured = installFetchStub({
      records: [],
      total_count: 0,
    });
    const client = makeClient();

    const resp = await client.executeQuery({
      language: "aql",
      query: "MATCH (n) RETURN n LIMIT 1",
      collection: "col_abc",
    });

    const op = operationFor(SPEC, "/api/v2/query", "post");
    expect(op.operationId).toBe("executeQuery");
    expect(captured.method).toBe("POST");
    expect(captured.url).toBe("http://contract.test/api/v2/query");

    const reqSchema = requestBodySchema(SPEC, op);
    expect(reqSchema).not.toBeNull();
    const requiredKeys = collectRequiredKeys(SPEC, reqSchema!);
    expect(requiredKeys).toContain("language");
    expect(requiredKeys).toContain("query");
    for (const key of requiredKeys) {
      expect(captured.body).not.toBeNull();
      expect(Object.keys(captured.body!)).toContain(key);
    }
    expect(resp).toBeDefined();
  });

  it("explainQuery() matches POST /api/v2/query/explain (explainQuery)", async () => {
    const captured = installFetchStub({
      plan: { node: "Scan" },
    });
    const client = makeClient();

    const resp = await client.explainQuery({
      language: "uql",
      query: "SELECT * FROM col_abc LIMIT 1",
    });

    const op = operationFor(SPEC, "/api/v2/query/explain", "post");
    expect(op.operationId).toBe("explainQuery");
    expect(captured.method).toBe("POST");
    expect(captured.url).toBe("http://contract.test/api/v2/query/explain");

    const reqSchema = requestBodySchema(SPEC, op);
    expect(reqSchema).not.toBeNull();
    const requiredKeys = collectRequiredKeys(SPEC, reqSchema!);
    expect(requiredKeys).toContain("language");
    expect(requiredKeys).toContain("query");
    for (const key of requiredKeys) {
      expect(captured.body).not.toBeNull();
      expect(Object.keys(captured.body!)).toContain(key);
    }
    expect(resp).toBeDefined();
  });
});
