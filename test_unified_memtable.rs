#!/usr/bin/env rust

//! Test script for unified memtable system

use proximadb::storage::memtable::{
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
    println!("🧪 Testing Unified Memtable System...");
    
    // Test 1: Basic BTree implementation
    println!("\n1. Testing BTree implementation...");
    let mut btree: BTreeMemtable<String, i32> = BTreeMemtable::new();
    btree.insert("key1".to_string(), 100).await?;
    let value = btree.get(&"key1".to_string()).await?;
    println!("✅ BTree: inserted and retrieved value: {:?}", value);
    
    // Test 2: Basic SkipList implementation  
    println!("\n2. Testing SkipList implementation...");
    let mut skiplist: SkipListMemtable<String, i32> = SkipListMemtable::new();
    skiplist.insert("key2".to_string(), 200).await?;
    let value = skiplist.get(&"key2".to_string()).await?;
    println!("✅ SkipList: inserted and retrieved value: {:?}", value);
    
    // Test 3: WAL Memtable with BTree backend
    println!("\n3. Testing WAL Memtable wrapper...");
    let config = MemtableConfig::default();
    let wal_memtable: WalMemtable<u64, String> = SpecializedMemtableFactory::create_btree_for_wal(config);
    wal_memtable.insert(1001, "wal_entry_1".to_string()).await?;
    let value = wal_memtable.get(&1001).await?;
    println!("✅ WAL Memtable: inserted and retrieved value: {:?}", value);
    
    // Test 4: LSM Memtable with SkipList backend
    println!("\n4. Testing LSM Memtable wrapper...");
    let config = MemtableConfig::default();
    let lsm_memtable: LsmMemtable<String, i32> = SpecializedMemtableFactory::create_skiplist_for_lsm(config);
    lsm_memtable.insert("lsm_key".to_string(), 999).await?;
    let value = lsm_memtable.get(&"lsm_key".to_string()).await?;
    println!("✅ LSM Memtable: inserted and retrieved value: {:?}", value);
    
    println!("\n🎉 All basic memtable tests passed!");
    Ok(())
}