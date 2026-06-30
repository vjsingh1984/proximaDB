//! Hardware capability detection — extracted to the `proximadb-hardware-caps`
//! foundation crate (decomposition contracts; issue #162 cycle-break landed first).
//! Re-exported here so existing `crate::core::hardware_capabilities::*` paths
//! (and the 53 consumers) resolve unchanged.
pub use proximadb_hardware_caps::*;
