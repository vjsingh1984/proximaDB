/**
 * ProximaDB TypeScript SDK - Generated REST transport seam (TD-126 Phase 4)
 *
 * This is the thin, hand-written ergonomic facade over the GENERATED REST
 * transport. The wire contract — path templates, path/query parameter
 * encoding, request/response JSON shapes — is generated from
 * `docs/openapi/proximadb-openapi.yaml` into `src/generated/schema.ts`
 * (openapi-typescript, pinned in package.json; regenerate with
 * `npm run gen-sdk`). `openapi-fetch` turns those generated `paths` types into
 * a typed runtime client.
 *
 * Generators don't do ergonomics; this layer is the value-add: it injects the
 * facade's own `fetch` (which owns bearer auth, retries, and error mapping —
 * see `client.ts`) into the typed client, so the generated wire plumbing and
 * the hand-written transport policy compose without either reimplementing the
 * other. This mirrors the merged Go pilot (TD-126 Phase 2), where the generated
 * `genrest.Client` is driven through `RequestEditorFn`s + a custom `http.Client`.
 *
 * Copyright 2025 ProximaDB Contributors
 * Licensed under the Apache License, Version 2.0
 */

import createClient from "openapi-fetch";
import type { Client } from "openapi-fetch";
import type { paths } from "./generated/schema";

/**
 * The typed REST client over the generated OpenAPI `paths`.
 */
export type GeneratedClient = Client<paths>;

/**
 * A fetch implementation matching the facade's transport policy. It is given a
 * resolved `Request` (URL + method + body assembled by the generated client)
 * and must return a native `Response`. The facade implements auth/retry/error
 * mapping inside this function (see `ProximaDBClient`).
 */
export type TransportFetch = (input: Request) => Promise<Response>;

/**
 * Construct the generated typed REST client, routing every request the
 * generated client issues through the facade's transport `fetch`.
 */
export function createTransport(
  baseUrl: string,
  fetchImpl: TransportFetch,
): GeneratedClient {
  return createClient<paths>({
    baseUrl,
    fetch: fetchImpl,
  });
}
