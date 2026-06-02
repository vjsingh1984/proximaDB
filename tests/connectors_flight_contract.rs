//! Arrow Flight contract gate for the Rust **connectors** that speak Flight
//! (today: Trino + DuckDB-bulk).
//!
//! Unlike the OpenAPI gate (which validates against the YAML spec), the
//! Flight contract is the union of:
//!
//!   - The `arrow-flight` crate's generated `FlightService` proto trait
//!     (pinned via `Cargo.toml`).
//!   - Custom JSON-encoded ticket shapes ProximaDB stamps into
//!     `Ticket.ticket`, e.g. [`ArrowFileTicket`] at
//!     `src/network/arrow_ipc/file_export.rs:639`.
//!   - `FlightDescriptor` path patterns the server's routing logic at
//!     `src/network/arrow_ipc/service.rs:313-342` recognizes
//!     (`["relational", table_fqn]`, `["vectors", collection_id]`, …).
//!
//! Per-method TDD tests for live Flight calls land in C7 (Trino pilot).
//! This file's smoke gate just proves the ticket-shape contract surfaces
//! we'll validate against.

use arrow_flight::{FlightDescriptor, Ticket};
use proximadb::network::arrow_ipc::file_export::ArrowFileTicket;

// ---------------------------------------------------------------------------
// Smoke test — proves the ArrowFileTicket detector recognizes a known-good
// JSON blob. Per-shape contract tests (descriptor patterns, ticket round
// trip) land in C7.
// ---------------------------------------------------------------------------

#[test]
fn helpers_arrow_file_ticket_detector_recognizes_known_shape() {
    let raw = br#"{"type":"arrow_file","collection_id":"c1","file_path":"/tmp/x.arrow"}"#;
    let ticket = Ticket {
        ticket: raw.to_vec().into(),
    };
    assert!(ArrowFileTicket::is_arrow_file_ticket(&ticket));
}

#[test]
fn helpers_arrow_file_ticket_detector_rejects_other_shapes() {
    let raw = br#"{"type":"sql","statement":"SELECT 1"}"#;
    let ticket = Ticket {
        ticket: raw.to_vec().into(),
    };
    assert!(!ArrowFileTicket::is_arrow_file_ticket(&ticket));
}

// ---------------------------------------------------------------------------
// ArrowFileTicket roundtrip — the JSON shape Trino's DoGet codepath needs
// when the server returns split tickets from GetFlightInfo.
// ---------------------------------------------------------------------------

#[test]
fn arrow_file_ticket_json_roundtrips() {
    let original = ArrowFileTicket {
        ticket_type: "arrow_file".to_string(),
        collection_id: "trino_col".to_string(),
        file_path: "/data/segment-001.arrow".to_string(),
        compression: None,
    };
    let bytes = serde_json::to_vec(&original).expect("serialize");
    let ticket = Ticket {
        ticket: bytes.into(),
    };
    assert!(ArrowFileTicket::is_arrow_file_ticket(&ticket));
    let parsed = ArrowFileTicket::from_ticket(&ticket).expect("parse");
    assert_eq!(parsed.collection_id, "trino_col");
    assert_eq!(parsed.file_path, "/data/segment-001.arrow");
}

// ---------------------------------------------------------------------------
// FlightDescriptor path patterns. The server at
// `src/network/arrow_ipc/service.rs:313-342` recognizes two `path` shapes
// when routing a GetSchema / GetFlightInfo request:
//
//   ["relational" | "table" | "sql", <table_fqn>]
//   ["vectors", <collection_id>]
//
// Trino's metadata calls (flight_get_table_schema, flight_get_splits) must
// build descriptors in one of these shapes. The contract gate constructs
// each shape and asserts the proto-level descriptor matches expectation.
// ---------------------------------------------------------------------------

#[test]
fn flight_descriptor_relational_path_shape() {
    let desc = FlightDescriptor::new_path(vec!["relational".into(), "tenant1.users".into()]);
    assert_eq!(
        desc.path,
        vec!["relational".to_string(), "tenant1.users".to_string()]
    );
    // First element must be one of the model-router prefixes the server recognizes.
    let head = desc.path.first().map(String::as_str);
    assert!(matches!(head, Some("relational")));
}

#[test]
fn flight_descriptor_vectors_path_shape() {
    let desc = FlightDescriptor::new_path(vec!["vectors".into(), "embeddings".into()]);
    assert_eq!(
        desc.path,
        vec!["vectors".to_string(), "embeddings".to_string()]
    );
    let head = desc.path.first().map(String::as_str);
    assert!(matches!(head, Some("vectors")));
}

// ---------------------------------------------------------------------------
// TD-098 pilot — in-process tonic Flight server fixture + live call test for
// `flight_list_schemas`. The mock implements `FlightService::list_flights`
// to return canned `FlightInfo` rows; every other RPC returns `Unimplemented`
// (the connector method only uses `list_flights`). Other 6 Trino flight_*
// helpers stay scaffolded; their tests land when the methods become live.
// ---------------------------------------------------------------------------

mod trino_flight_pilot {
    use std::pin::Pin;
    use std::sync::Arc;

    use arrow_flight::flight_service_server::{FlightService, FlightServiceServer};
    use arrow_flight::{
        Action, ActionType, Criteria, Empty, FlightData, FlightDescriptor, FlightInfo,
        HandshakeRequest, HandshakeResponse, PollInfo, PutResult, SchemaResult, Ticket,
    };
    use futures::Stream;
    use arrow::datatypes::{DataType, Field, Schema as ArrowSchema};
    use arrow_flight::SchemaAsIpc;
    use proximadb::connectors::trino::{
        TrinoConnectorConfig, TrinoSplit, TrinoTupleDomain, flight_get_splits,
        flight_get_table_schema, flight_list_schemas, flight_list_tables,
    };
    use proximadb::storage::formats::{
        FileSplit, SplitLocality, SplitStatistics, SplitType,
    };
    use tokio::sync::oneshot;
    use tokio_stream::wrappers::TcpListenerStream;
    use tonic::{Request, Response, Status, Streaming};

    type RespStream<T> = Pin<Box<dyn Stream<Item = Result<T, Status>> + Send + 'static>>;

    /// Minimal `FlightService` impl. Only `list_flights`, `get_schema`,
    /// and `get_flight_info` are wired; every other RPC returns
    /// Unimplemented to keep the trait satisfied without 300+ lines of
    /// mocks.
    struct MockFlight {
        canned: Vec<FlightInfo>,
        canned_schema: Option<Arc<ArrowSchema>>,
        canned_flight_info: Option<FlightInfo>,
    }

    #[tonic::async_trait]
    impl FlightService for MockFlight {
        type HandshakeStream = RespStream<HandshakeResponse>;
        type ListFlightsStream = RespStream<FlightInfo>;
        type DoGetStream = RespStream<FlightData>;
        type DoPutStream = RespStream<PutResult>;
        type DoActionStream = RespStream<arrow_flight::Result>;
        type ListActionsStream = RespStream<ActionType>;
        type DoExchangeStream = RespStream<FlightData>;

        async fn handshake(
            &self,
            _request: Request<Streaming<HandshakeRequest>>,
        ) -> Result<Response<Self::HandshakeStream>, Status> {
            Err(Status::unimplemented("handshake"))
        }

        async fn list_flights(
            &self,
            _request: Request<Criteria>,
        ) -> Result<Response<Self::ListFlightsStream>, Status> {
            let items: Vec<Result<FlightInfo, Status>> =
                self.canned.iter().cloned().map(Ok).collect();
            let s = futures::stream::iter(items);
            Ok(Response::new(Box::pin(s)))
        }

        async fn get_flight_info(
            &self,
            _request: Request<FlightDescriptor>,
        ) -> Result<Response<FlightInfo>, Status> {
            let Some(info) = self.canned_flight_info.as_ref() else {
                return Err(Status::unimplemented("get_flight_info"));
            };
            Ok(Response::new(info.clone()))
        }

        async fn poll_flight_info(
            &self,
            _request: Request<FlightDescriptor>,
        ) -> Result<Response<PollInfo>, Status> {
            Err(Status::unimplemented("poll_flight_info"))
        }

        async fn get_schema(
            &self,
            _request: Request<FlightDescriptor>,
        ) -> Result<Response<SchemaResult>, Status> {
            let Some(schema) = self.canned_schema.as_ref() else {
                return Err(Status::unimplemented("get_schema"));
            };
            let options = arrow::ipc::writer::IpcWriteOptions::default();
            let result: SchemaResult = SchemaAsIpc::new(schema.as_ref(), &options)
                .try_into()
                .map_err(|e: arrow::error::ArrowError| Status::internal(e.to_string()))?;
            Ok(Response::new(result))
        }

        async fn do_get(
            &self,
            _request: Request<Ticket>,
        ) -> Result<Response<Self::DoGetStream>, Status> {
            Err(Status::unimplemented("do_get"))
        }

        async fn do_put(
            &self,
            _request: Request<Streaming<FlightData>>,
        ) -> Result<Response<Self::DoPutStream>, Status> {
            Err(Status::unimplemented("do_put"))
        }

        async fn do_action(
            &self,
            _request: Request<Action>,
        ) -> Result<Response<Self::DoActionStream>, Status> {
            Err(Status::unimplemented("do_action"))
        }

        async fn list_actions(
            &self,
            _request: Request<Empty>,
        ) -> Result<Response<Self::ListActionsStream>, Status> {
            Err(Status::unimplemented("list_actions"))
        }

        async fn do_exchange(
            &self,
            _request: Request<Streaming<FlightData>>,
        ) -> Result<Response<Self::DoExchangeStream>, Status> {
            Err(Status::unimplemented("do_exchange"))
        }
    }

    /// Build a `FlightInfo` whose `flight_descriptor.path` is the given
    /// path segments. Trino's `flight_list_schemas` projects path[0] →
    /// schema name and path[1..] → table name.
    fn flight_info(path: &[&str]) -> FlightInfo {
        FlightInfo {
            schema: Default::default(),
            flight_descriptor: Some(FlightDescriptor::new_path(
                path.iter().map(|s| s.to_string()).collect(),
            )),
            endpoint: vec![],
            total_records: -1,
            total_bytes: -1,
            ordered: false,
            app_metadata: Default::default(),
        }
    }

    /// Spin up a mock FlightService on `127.0.0.1:0` and return the
    /// chosen port + a shutdown signal sender so the test can stop the
    /// server cleanly. The server task self-terminates when the
    /// shutdown receiver fires.
    async fn start_mock_flight_server(canned: Vec<FlightInfo>) -> (u16, oneshot::Sender<()>) {
        start_mock_flight_server_full(canned, None, None).await
    }

    /// Variant used by tests that need `get_schema` wired (Trino
    /// `flight_get_table_schema` pilot). `canned_schema` is encoded as
    /// Arrow IPC into the `SchemaResult.schema` bytes by the mock.
    async fn start_mock_flight_server_with_schema(
        canned: Vec<FlightInfo>,
        canned_schema: Arc<ArrowSchema>,
    ) -> (u16, oneshot::Sender<()>) {
        start_mock_flight_server_full(canned, Some(canned_schema), None).await
    }

    /// Variant used by tests that need `get_flight_info` wired (Trino
    /// `flight_get_splits` pilot). The mock returns `canned_flight_info`
    /// verbatim from `get_flight_info`; its endpoints carry the
    /// JSON-encoded splits.
    async fn start_mock_flight_server_with_flight_info(
        canned: Vec<FlightInfo>,
        canned_flight_info: FlightInfo,
    ) -> (u16, oneshot::Sender<()>) {
        start_mock_flight_server_full(canned, None, Some(canned_flight_info)).await
    }

    async fn start_mock_flight_server_full(
        canned: Vec<FlightInfo>,
        canned_schema: Option<Arc<ArrowSchema>>,
        canned_flight_info: Option<FlightInfo>,
    ) -> (u16, oneshot::Sender<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind port 0");
        let port = listener.local_addr().expect("local_addr").port();
        let incoming = TcpListenerStream::new(listener);

        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let svc = MockFlight {
            canned,
            canned_schema,
            canned_flight_info,
        };
        tokio::spawn(async move {
            let _ = tonic::transport::Server::builder()
                .add_service(FlightServiceServer::new(svc))
                .serve_with_incoming_shutdown(incoming, async {
                    let _ = shutdown_rx.await;
                })
                .await;
        });

        // tiny delay so the server is ready when the client dials
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        (port, shutdown_tx)
    }

    #[tokio::test]
    async fn trino_flight_list_schemas_buckets_by_first_path_segment() {
        let canned = vec![
            flight_info(&["sales", "orders"]),
            flight_info(&["sales", "customers"]),
            flight_info(&["analytics", "events"]),
            flight_info(&["analytics"]), // no table — schema-only entry
        ];
        let (port, _shutdown) = start_mock_flight_server(canned).await;

        let config = TrinoConnectorConfig {
            flight_endpoint: format!("grpc://127.0.0.1:{port}"),
            ..TrinoConnectorConfig::default()
        };

        let schemas = flight_list_schemas(&config.flight_endpoint, "proximadb").await;

        // BTreeMap inside flight_list_schemas → schemas come back sorted.
        assert_eq!(schemas.len(), 2, "expected analytics + sales: {schemas:?}");
        let analytics = schemas
            .iter()
            .find(|s| s.name == "analytics")
            .expect("analytics schema");
        assert_eq!(analytics.tables, vec!["events"]);
        let sales = schemas
            .iter()
            .find(|s| s.name == "sales")
            .expect("sales schema");
        // sales has two tables — order is BTreeMap insertion-driven (Vec push)
        assert_eq!(sales.tables.len(), 2);
        assert!(sales.tables.iter().any(|t| t == "orders"));
        assert!(sales.tables.iter().any(|t| t == "customers"));
    }

    #[tokio::test]
    async fn trino_flight_list_schemas_returns_empty_on_unreachable_endpoint() {
        // Dial a port that nothing's listening on — must NOT panic; must
        // degrade to empty schema list.
        let schemas = flight_list_schemas("grpc://127.0.0.1:1", "proximadb").await;
        assert!(schemas.is_empty(), "unreachable endpoint must yield empty");
    }

    #[tokio::test]
    async fn trino_flight_list_tables_filters_by_schema() {
        // Mock returns paths across two schemas + a deduplicate of
        // ["sales","orders"]. flight_list_tables("sales") should pick
        // exactly the two distinct sales tables.
        let canned = vec![
            flight_info(&["sales", "orders"]),
            flight_info(&["analytics", "events"]),
            flight_info(&["sales", "customers"]),
            flight_info(&["sales", "orders"]), // duplicate — must dedup
        ];
        let (port, _shutdown) = start_mock_flight_server(canned).await;

        let endpoint = format!("grpc://127.0.0.1:{port}");
        let tables = flight_list_tables(&endpoint, "proximadb", "sales").await;
        assert_eq!(tables.len(), 2, "expected 2 distinct sales tables: {tables:?}");
        assert!(tables.iter().any(|t| t == "orders"));
        assert!(tables.iter().any(|t| t == "customers"));

        // Schema not in canned list → empty Vec.
        let none = flight_list_tables(&endpoint, "proximadb", "missing").await;
        assert!(none.is_empty(), "missing schema must yield empty");
    }

    #[tokio::test]
    async fn trino_flight_list_tables_returns_empty_on_unreachable_endpoint() {
        let tables = flight_list_tables("grpc://127.0.0.1:1", "proximadb", "any").await;
        assert!(tables.is_empty(), "unreachable endpoint must yield empty");
    }

    #[tokio::test]
    async fn trino_flight_get_table_schema_returns_arrow_schema() {
        // Mock get_schema returns a known 2-column Arrow schema; the
        // connector must dial GetSchema and round-trip the IPC bytes
        // back into an ArrowSchema with the same fields.
        let canned_schema = Arc::new(ArrowSchema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("name", DataType::Utf8, true),
        ]));
        let (port, _shutdown) =
            start_mock_flight_server_with_schema(vec![], canned_schema.clone()).await;

        let endpoint = format!("grpc://127.0.0.1:{port}");
        let got = flight_get_table_schema(&endpoint, "proximadb", "sales", "orders")
            .await
            .expect("schema should be returned");
        assert_eq!(got.fields().len(), 2, "expected 2 fields: {got:?}");
        assert_eq!(got.field(0).name(), "id");
        assert_eq!(got.field(0).data_type(), &DataType::Int64);
        assert!(!got.field(0).is_nullable());
        assert_eq!(got.field(1).name(), "name");
        assert_eq!(got.field(1).data_type(), &DataType::Utf8);
        assert!(got.field(1).is_nullable());
    }

    #[tokio::test]
    async fn trino_flight_get_table_schema_returns_none_on_unreachable_endpoint() {
        let result =
            flight_get_table_schema("grpc://127.0.0.1:1", "proximadb", "any", "any").await;
        assert!(result.is_none(), "unreachable endpoint must yield None");
    }

    /// Build a one-segment FileSplit suitable for round-tripping through
    /// `TrinoSplit::from_file_split` + JSON serde.
    fn sample_file_split(split_id: &str, file_path: &str) -> FileSplit {
        FileSplit {
            split_id: split_id.to_string(),
            file_path: file_path.to_string(),
            offset: 0,
            length: 1024,
            split_type: SplitType::Block {
                block_id: 0,
                record_count: 100,
            },
            statistics: SplitStatistics::default(),
            locality: SplitLocality::default(),
        }
    }

    #[tokio::test]
    async fn trino_flight_get_splits_decodes_endpoint_tickets() {
        // Mock returns a FlightInfo with two endpoints; each endpoint's
        // ticket bytes are a JSON-encoded TrinoSplit. The connector
        // must decode each ticket back into a TrinoSplit.
        let split_a = TrinoSplit::from_file_split(
            "proximadb".into(),
            "sales".into(),
            "orders".into(),
            sample_file_split("split-a", "s3://orders/part-0.parquet"),
        );
        let split_b = TrinoSplit::from_file_split(
            "proximadb".into(),
            "sales".into(),
            "orders".into(),
            sample_file_split("split-b", "s3://orders/part-1.parquet"),
        );

        let endpoints = vec![
            arrow_flight::FlightEndpoint {
                ticket: Some(arrow_flight::Ticket {
                    ticket: serde_json::to_vec(&split_a).unwrap().into(),
                }),
                location: vec![],
                expiration_time: None,
                app_metadata: Default::default(),
            },
            arrow_flight::FlightEndpoint {
                ticket: Some(arrow_flight::Ticket {
                    ticket: serde_json::to_vec(&split_b).unwrap().into(),
                }),
                location: vec![],
                expiration_time: None,
                app_metadata: Default::default(),
            },
        ];
        let canned_flight_info = FlightInfo {
            schema: Default::default(),
            flight_descriptor: Some(FlightDescriptor::new_path(vec![
                "sales".into(),
                "orders".into(),
            ])),
            endpoint: endpoints,
            total_records: -1,
            total_bytes: -1,
            ordered: false,
            app_metadata: Default::default(),
        };

        let (port, _shutdown) =
            start_mock_flight_server_with_flight_info(vec![], canned_flight_info).await;

        let endpoint = format!("grpc://127.0.0.1:{port}");
        let splits = flight_get_splits(
            &endpoint,
            "proximadb",
            "sales",
            "orders",
            &TrinoTupleDomain::all(),
        )
        .await;
        assert_eq!(splits.len(), 2, "expected 2 splits: {splits:?}");
        let ids: Vec<&str> = splits.iter().map(|s| s.split_id.as_str()).collect();
        assert!(ids.contains(&"split-a"));
        assert!(ids.contains(&"split-b"));
        let a = splits.iter().find(|s| s.split_id == "split-a").unwrap();
        assert_eq!(a.file_split.file_path, "s3://orders/part-0.parquet");
        assert_eq!(a.schema, "sales");
        assert_eq!(a.table, "orders");
    }

    #[tokio::test]
    async fn trino_flight_get_splits_returns_empty_on_unreachable_endpoint() {
        let splits = flight_get_splits(
            "grpc://127.0.0.1:1",
            "proximadb",
            "any",
            "any",
            &TrinoTupleDomain::all(),
        )
        .await;
        assert!(splits.is_empty(), "unreachable endpoint must yield empty");
    }
}
