//! Storage format abstractions
//!
//! Provides FastLanes block-columnar and pure columnar storage formats shared by multiple engines

pub mod columnar;
pub mod fastlanes_blocks;

// FastLanes block-columnar formats are used by SST and SWIFT (vectors are columnar-encoded within blocks)
// Pure columnar formats are used by VIPER and NOVA (Parquet-based)

pub use fastlanes_blocks::{FastLanesBlockMetadata, FastLanesDataBlock, SstIOLayer};

pub use columnar::{
    ColumnarOperations, ColumnarSchema, FooterCache, ParquetIOLayer, ParquetQueryEngine,
    ParquetWriter,
};
