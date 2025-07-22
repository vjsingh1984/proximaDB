//! WAL unit tests module

#[cfg(test)]
pub mod config_tests;

#[cfg(test)]
pub mod batch_strategy_tests;

#[cfg(test)]
pub mod batch_factory_tests;

#[cfg(test)]
pub mod avro_serialization_tests;

#[cfg(test)]
pub mod bincode_serialization_tests;

#[cfg(test)]
pub mod proto_serialization_tests;

#[cfg(test)]
pub mod flush_coordinator_tests;
