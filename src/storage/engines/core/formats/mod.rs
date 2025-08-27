//! Storage format abstractions
//! 
//! Provides row-based and columnar storage formats shared by multiple engines

pub mod row_based;
pub mod columnar;

// Row-based formats are used by SST and SWIFT
// Columnar formats are used by VIPER and NOVA

pub use row_based::{
    DataBlock, BlockMetadata, RowBasedUtilities,
    SstIOLayer,
};

pub use columnar::{
    ColumnarSchema, ParquetQueryEngine, ParquetIOLayer,
    ParquetWriter, ColumnarOperations, FooterCache,
};