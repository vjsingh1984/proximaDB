#!/usr/bin/env rust

//! Test script for unified memtable system

use proximadb::storage::memtable::{
use tracing::{debug, error, info, warn};
    core::{MemtableCore, MemtableConfig},
    implementations::{
        btree::BTreeMemtable,
        skiplist::SkipListMemtable,
        hashmap::HashMapMemtable,
    },
    specialized::{
        WalMemtable,
        LsmMemtable,
        SpecializedMemtableFactory,
    },
    MemtableFactory,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    debug!("🧪 Testing Unified Memtable System...");
    
    // Test 1: Basic BTree implementation
    debug!("\n1. Testing BTree implementation...");
    let mut btree: BTreeMemtable<String, i32> = BTreeMemtable::new();
    btree.insert("key1".to_string(), 100).await?;
    let value = btree.get(key)).await?;
    debug!("✅ BTree: inserted and retrieved value: {:?}", value);
    
    // Test 2: Basic SkipList implementation  
    debug!("\n2. Testing SkipList implementation...");
    let mut skiplist: SkipListMemtable<String, i32> = SkipListMemtable::new();
    skiplist.insert("key2".to_string(), 200).await?;
    let value = skiplist.get(key)).await?;
    debug!("✅ SkipList: inserted and retrieved value: {:?}", value);
    
    // Test 3: WAL Memtable with BTree backend
    debug!("\n3. Testing WAL Memtable wrapper...");
    let config = MemtableConfig::default();
    let wal_memtable: WalMemtable<u64, String> = SpecializedMemtableFactory::create_btree_for_wal(config);
    wal_memtable.insert(1001, "wal_entry_1".to_string()).await?;
    let value = wal_memtable.get(key).await?;
    debug!("✅ WAL Memtable: inserted and retrieved value: {:?}", value);
    
    // Test 4: LSM Memtable with SkipList backend
    debug!("\n4. Testing LSM Memtable wrapper...");
    let config = MemtableConfig::default();
    let lsm_memtable: LsmMemtable<String, i32> = SpecializedMemtableFactory::create_skiplist_for_lsm(config);
    lsm_memtable.insert("lsm_key".to_string(), 999).await?;
    let value = lsm_memtable.get(key)).await?;
    debug!("✅ LSM Memtable: inserted and retrieved value: {:?}", value);
    
    debug!("\n🎉 All basic memtable tests passed!");
    Ok(())
}