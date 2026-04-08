//! WAL unit tests module

// TODO: Fix compilation errors - proto field changes

// TODO: Fix compilation errors - proto field changes
// #[cfg(test)]
// pub mod batch_strategy_tests;

#[cfg(test)]
pub mod avro_serialization_tests;

#[cfg(test)]
pub mod bincode_serialization_tests;

#[cfg(test)]
pub mod flush_coordinator_tests;

// TODO: Fix compilation errors - WalManifest renamed to GlobalManifest, WriteBufferDiskManager removed
// #[cfg(test)]
// pub mod durability_tests;

#[cfg(test)]
pub mod background_flush_optimization_tests;
