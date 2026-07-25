// Copyright (C) 2026 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! # `proximadb-cks-local` — the generation-fenced local-WAL `ConditionalKeyStore`
//!
//! F1b (ADR-072 D5): the **hot, correctness-carrying default** implementation of
//! [`ConditionalKeyStore`]. It generalizes the ledger's proven CAS core from
//! `str → version` to `Identity → Oid`: an in-memory authoritative map fronted by
//! an **append-only, CRC-framed WAL** so claimed keys survive restart, with
//! torn-tail-tolerant replay (a crash mid-append is discarded, not recovered as a
//! phantom claim).
//!
//! Reclaim is **append-only** (D11/D14): [`ConditionalKeyStore::tombstone`] marks
//! the live version deleted without reusing the slot; a later `put_if_absent` of
//! the same key is a *new* claim. Physical WAL compaction / GC is the
//! background-plane concern (F6), out of scope here.
//!
//! Frame layout matches the ledger WAL: `[len u32-le][crc32 u32-le][payload]`.
//! Payload: `op u8 | generation u64-le | key_len u32-le | key | oid_len u32-le | oid`.

use anyhow::{Context, Result};
use proximadb_storage_ports::{
    AtomicScope, ConditionalKeyStore, Generation, Identity, Oid, PutOutcome,
};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Read, Write};
use std::path::Path;
use std::sync::Mutex;

/// WAL fsync policy (mirrors the ledger's lever).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncPolicy {
    /// `fsync` after every append — strict durability (the default).
    PerOp,
    /// Buffer appends; the caller flushes at a checkpoint (throughput).
    Deferred,
}

const OP_PUT: u8 = 0;
const OP_TOMBSTONE: u8 = 1;

/// IEEE CRC-32 (reflected, poly `0xEDB88320`). Inline so the crate needs no
/// checksum dependency; identical to the ledger WAL's framing.
fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

#[derive(Clone)]
struct Entry {
    oid: Oid,
    generation: Generation,
    live: bool,
}

struct Inner {
    map: HashMap<Identity, Entry>,
    log: BufWriter<File>,
    sync: SyncPolicy,
}

/// A durable, generation-fenced [`ConditionalKeyStore`] backed by a local WAL.
pub struct LocalWalKeyStore {
    inner: Mutex<Inner>,
}

impl LocalWalKeyStore {
    /// Open (or create) the store at `path`, replaying any existing WAL.
    pub fn open(path: impl AsRef<Path>, sync: SyncPolicy) -> Result<Self> {
        let path = path.as_ref();
        let map = replay(path).with_context(|| format!("replaying WAL at {}", path.display()))?;
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .read(true)
            .open(path)
            .with_context(|| format!("opening WAL at {}", path.display()))?;
        Ok(Self {
            inner: Mutex::new(Inner {
                map,
                log: BufWriter::new(file),
                sync,
            }),
        })
    }

    /// Flush buffered appends to stable storage (a no-op under [`SyncPolicy::PerOp`]).
    pub fn flush(&self) -> Result<()> {
        let mut g = self.inner.lock().unwrap();
        g.log.flush()?;
        g.log.get_ref().sync_all()?;
        Ok(())
    }
}

impl Inner {
    fn append(&mut self, op: u8, generation: Generation, key: &[u8], oid: &[u8]) -> io::Result<()> {
        let mut payload = Vec::with_capacity(1 + 8 + 4 + key.len() + 4 + oid.len());
        payload.push(op);
        payload.extend_from_slice(&generation.0.to_le_bytes());
        payload.extend_from_slice(&(key.len() as u32).to_le_bytes());
        payload.extend_from_slice(key);
        payload.extend_from_slice(&(oid.len() as u32).to_le_bytes());
        payload.extend_from_slice(oid);

        let len = u32::try_from(payload.len()).map_err(io::Error::other)?;
        self.log.write_all(&len.to_le_bytes())?;
        self.log.write_all(&crc32(&payload).to_le_bytes())?;
        self.log.write_all(&payload)?;
        if self.sync == SyncPolicy::PerOp {
            self.log.flush()?;
            self.log.get_ref().sync_all()?;
        }
        Ok(())
    }
}

/// Rebuild the in-memory map from the WAL, stopping at the first torn/short/bad-CRC
/// frame (an un-acked write is discarded, never recovered as a live claim).
fn replay(path: &Path) -> Result<HashMap<Identity, Entry>> {
    let mut map = HashMap::new();
    let mut file = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(map),
        Err(e) => return Err(e.into()),
    };
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;

    let mut pos = 0usize;
    while pos + 8 <= buf.len() {
        let len = u32::from_le_bytes(buf[pos..pos + 4].try_into().unwrap()) as usize;
        let crc = u32::from_le_bytes(buf[pos + 4..pos + 8].try_into().unwrap());
        let start = pos + 8;
        if start + len > buf.len() {
            break; // torn tail
        }
        let payload = &buf[start..start + len];
        if crc32(payload) != crc {
            break; // corrupt frame
        }
        match decode(payload) {
            Some((op, generation, key, oid)) => {
                let id = Identity::from_bytes(key);
                match op {
                    OP_PUT => match String::from_utf8(oid) {
                        Ok(s) => {
                            map.insert(
                                id,
                                Entry {
                                    oid: Oid(s),
                                    generation,
                                    live: true,
                                },
                            );
                        }
                        Err(_) => break, // non-UTF-8 oid -> treat as corrupt
                    },
                    OP_TOMBSTONE => {
                        if let Some(e) = map.get_mut(&id) {
                            e.generation = generation;
                            e.live = false;
                        }
                    }
                    _ => break, // unknown op -> stop
                }
            }
            None => break, // undecodable -> torn/corrupt
        }
        pos = start + len;
    }
    Ok(map)
}

fn decode(p: &[u8]) -> Option<(u8, Generation, Vec<u8>, Vec<u8>)> {
    let mut i = 0usize;
    let op = *p.get(i)?;
    i += 1;
    let gen_bytes: [u8; 8] = p.get(i..i + 8)?.try_into().ok()?;
    let generation = Generation(u64::from_le_bytes(gen_bytes));
    i += 8;
    let key_len = u32::from_le_bytes(p.get(i..i + 4)?.try_into().ok()?) as usize;
    i += 4;
    let key = p.get(i..i + key_len)?.to_vec();
    i += key_len;
    let oid_len = u32::from_le_bytes(p.get(i..i + 4)?.try_into().ok()?) as usize;
    i += 4;
    let oid = p.get(i..i + oid_len)?.to_vec();
    i += oid_len;
    if i != p.len() {
        return None; // trailing bytes -> reject
    }
    Some((op, generation, key, oid))
}

#[async_trait::async_trait]
impl ConditionalKeyStore for LocalWalKeyStore {
    fn atomic_scope(&self) -> AtomicScope {
        AtomicScope::PerKey
    }

    async fn put_if_absent(
        &self,
        key: &Identity,
        oid: &Oid,
        fence: Generation,
    ) -> Result<PutOutcome> {
        // Blocking WAL I/O under a std mutex; no `.await` is held across the lock.
        let mut g = self.inner.lock().unwrap();
        if let Some(e) = g.map.get(key).filter(|e| e.live) {
            return Ok(PutOutcome::Conflict {
                holder: e.oid.clone(),
                generation: e.generation,
            });
        }
        g.append(OP_PUT, fence, key.as_bytes(), oid.0.as_bytes())?;
        g.map.insert(
            key.clone(),
            Entry {
                oid: oid.clone(),
                generation: fence,
                live: true,
            },
        );
        Ok(PutOutcome::Committed { generation: fence })
    }

    async fn get(&self, key: &Identity) -> Result<Option<Oid>> {
        let g = self.inner.lock().unwrap();
        Ok(g.map.get(key).filter(|e| e.live).map(|e| e.oid.clone()))
    }

    async fn tombstone(&self, key: &Identity, fence: Generation) -> Result<()> {
        let mut g = self.inner.lock().unwrap();
        let present = g.map.get(key).map(|e| e.live).unwrap_or(false);
        if present {
            g.append(OP_TOMBSTONE, fence, key.as_bytes(), &[])?;
            if let Some(e) = g.map.get_mut(key) {
                e.generation = fence;
                e.live = false;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> Identity {
        Identity::from_bytes(s.as_bytes().to_vec())
    }
    fn tmp() -> std::path::PathBuf {
        // A unique-enough path without Date/rand: process id + a static counter.
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("cks-{}-{}.wal", std::process::id(), n))
    }

    #[tokio::test]
    async fn lifecycle_and_durability() {
        let path = tmp();
        let a = Oid("row-a".into());
        let b = Oid("row-b".into());
        {
            let s = LocalWalKeyStore::open(&path, SyncPolicy::PerOp).unwrap();
            assert_eq!(
                s.put_if_absent(&id("k1"), &a, Generation(1)).await.unwrap(),
                PutOutcome::Committed {
                    generation: Generation(1)
                }
            );
            // Conflict returns the holder.
            assert_eq!(
                s.put_if_absent(&id("k1"), &b, Generation(2)).await.unwrap(),
                PutOutcome::Conflict {
                    holder: a.clone(),
                    generation: Generation(1)
                }
            );
            s.tombstone(&id("k1"), Generation(3)).await.unwrap();
            assert_eq!(s.get(&id("k1")).await.unwrap(), None);
            // Re-insert after tombstone is a new claim (append-only reclaim).
            assert_eq!(
                s.put_if_absent(&id("k1"), &b, Generation(4)).await.unwrap(),
                PutOutcome::Committed {
                    generation: Generation(4)
                }
            );
        }
        // Reopen: the WAL-rebuilt state must survive restart.
        {
            let s = LocalWalKeyStore::open(&path, SyncPolicy::PerOp).unwrap();
            assert_eq!(s.get(&id("k1")).await.unwrap(), Some(b));
            // The claimed key still conflicts after restart.
            match s.put_if_absent(&id("k1"), &a, Generation(9)).await.unwrap() {
                PutOutcome::Conflict { holder, .. } => assert_eq!(holder, Oid("row-b".into())),
                other => panic!("expected conflict after restart, got {other:?}"),
            }
        }
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn torn_tail_is_discarded() {
        let path = tmp();
        {
            let s = LocalWalKeyStore::open(&path, SyncPolicy::PerOp).unwrap();
            s.put_if_absent(&id("good"), &Oid("v".into()), Generation(1))
                .await
                .unwrap();
            s.flush().unwrap();
        }
        // Corrupt the tail by appending a bogus (short) frame header.
        {
            use std::io::Write as _;
            let mut f = OpenOptions::new().append(true).open(&path).unwrap();
            f.write_all(&999u32.to_le_bytes()).unwrap(); // claims 999 bytes that aren't there
            f.write_all(&0u32.to_le_bytes()).unwrap();
            f.flush().unwrap();
        }
        // Replay stops at the torn frame; the good record survives.
        let s = LocalWalKeyStore::open(&path, SyncPolicy::PerOp).unwrap();
        assert_eq!(s.get(&id("good")).await.unwrap(), Some(Oid("v".into())));
        let _ = std::fs::remove_file(&path);
    }
}
