//! Storage format abstractions
//! 
//! Provides FastLanes block-columnar and pure columnar storage formats shared by multiple engines

pub mod fastlanes_blocks;
pub mod columnar;

// FastLanes block-columnar formats are used by SST and SWIFT (vectors are columnar-encoded within blocks)
// Pure columnar formats are used by VIPER and NOVA (Parquet-based)

pub use fastlanes_blocks::{
    FastLanesDataBlock, FastLanesBlockMetadata,
    SstIOLayer,
};

pub use columnar::{
    ColumnarSchema, ParquetQueryEngine, ParquetIOLayer,
    ParquetWriter, ColumnarOperations, FooterCache,
};