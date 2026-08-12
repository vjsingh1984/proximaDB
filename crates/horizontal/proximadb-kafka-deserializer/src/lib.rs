// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! Kafka message deserializers, extracted from the root `streaming/kafka`
//! module (TD-DECOMP-42).
//!
//! [`deserializer`] turns raw Kafka payloads into [`deserializer::VectorMessage`]s
//! across JSON / Avro / Protobuf / Raw formats, selected by the
//! [`proximadb_kafka_config::config::DeserializationFormat`] carried in the
//! consumer config. Depends only on `serde`/`serde_json` + the sibling
//! `proximadb-kafka-config` crate — the heavy `rdkafka` consumer impl stays
//! root-side, keeping this a clean horizontal-tier leaf.

pub mod deserializer;
