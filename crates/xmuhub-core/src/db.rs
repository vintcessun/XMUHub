//! redb persistence. The database is the single source of truth; everything else
//! (in-memory catalog, search index) is rebuilt from it at boot.

use std::path::Path;

use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use serde::{Serialize, de::DeserializeOwned};

use crate::error::{Error, Result};
use crate::model::*;

const SCHEMA_VERSION: u8 = 1;

pub const COURSES: TableDefinition<u64, &[u8]> = TableDefinition::new("courses");
pub const RESOURCES: TableDefinition<u64, &[u8]> = TableDefinition::new("resources");
pub const TOKENS: TableDefinition<u64, &[u8]> = TableDefinition::new("tokens");
pub const UPLOADS: TableDefinition<u64, &[u8]> = TableDefinition::new("uploads");
pub const BLOBS: TableDefinition<&str, &[u8]> = TableDefinition::new("blobs");
/// Free-form small state: id sequences, storage bucket cursors, settings.
pub const META: TableDefinition<&str, &[u8]> = TableDefinition::new("meta");

pub fn encode<T: Serialize>(v: &T) -> Vec<u8> {
    let mut out = vec![SCHEMA_VERSION];
    out.extend(postcard::to_stdvec(v).expect("postcard encoding of in-memory record"));
    out
}

pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    match bytes.split_first() {
        Some((&SCHEMA_VERSION, body)) => Ok(postcard::from_bytes(body)?),
        Some((v, _)) => Err(Error::Internal(format!("unknown record version {v}"))),
        None => Err(Error::Internal("empty record".into())),
    }
}

pub struct Db {
    pub inner: Database,
}

/// Everything loaded at boot.
#[derive(Default)]
pub struct Snapshot {
    pub courses: Vec<Course>,
    pub resources: Vec<Resource>,
    pub tokens: Vec<Token>,
    pub uploads: Vec<Upload>,
    pub blobs: Vec<Blob>,
    pub meta: Vec<(String, Vec<u8>)>,
}

impl Db {
    pub fn open(path: &Path, cache_bytes: usize) -> Result<Db> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        // redb defaults to a 1 GiB page cache; the host has well under that free.
        let inner = Database::builder().set_cache_size(cache_bytes).create(path)?;
        let txn = inner.begin_write()?;
        txn.open_table(COURSES)?;
        txn.open_table(RESOURCES)?;
        txn.open_table(TOKENS)?;
        txn.open_table(UPLOADS)?;
        txn.open_table(BLOBS)?;
        txn.open_table(META)?;
        txn.commit()?;
        Ok(Db { inner })
    }

    pub fn load(&self) -> Result<Snapshot> {
        let txn = self.inner.begin_read()?;
        let mut snap = Snapshot::default();
        for row in txn.open_table(COURSES)?.iter()? {
            snap.courses.push(decode(row?.1.value())?);
        }
        for row in txn.open_table(RESOURCES)?.iter()? {
            snap.resources.push(decode(row?.1.value())?);
        }
        for row in txn.open_table(TOKENS)?.iter()? {
            snap.tokens.push(decode(row?.1.value())?);
        }
        for row in txn.open_table(UPLOADS)?.iter()? {
            snap.uploads.push(decode(row?.1.value())?);
        }
        for row in txn.open_table(BLOBS)?.iter()? {
            snap.blobs.push(decode(row?.1.value())?);
        }
        for row in txn.open_table(META)?.iter()? {
            let (k, v) = row?;
            snap.meta.push((k.value().to_string(), v.value().to_vec()));
        }
        Ok(snap)
    }

    /// Runs `f` inside one write transaction and commits it.
    pub fn write<R>(&self, f: impl FnOnce(&Tx<'_>) -> Result<R>) -> Result<R> {
        let txn = self.inner.begin_write()?;
        let r = f(&Tx { txn: &txn })?;
        txn.commit()?;
        Ok(r)
    }

    /// Dumps every table into one self-describing blob for off-site backup.
    pub fn export(&self) -> Result<Vec<u8>> {
        let snap = self.load()?;
        let dump = Dump {
            version: SCHEMA_VERSION,
            courses: snap.courses,
            resources: snap.resources,
            tokens: snap.tokens,
            blobs: snap.blobs,
            meta: snap.meta,
        };
        Ok(serde_json::to_vec(&dump).map_err(|e| Error::Internal(e.to_string()))?)
    }
}

#[derive(Serialize, serde::Deserialize)]
pub struct Dump {
    pub version: u8,
    pub courses: Vec<Course>,
    pub resources: Vec<Resource>,
    pub tokens: Vec<Token>,
    pub blobs: Vec<Blob>,
    pub meta: Vec<(String, Vec<u8>)>,
}

pub struct Tx<'a> {
    txn: &'a redb::WriteTransaction,
}

impl Tx<'_> {
    pub fn put_course(&self, c: &Course) -> Result<()> {
        self.txn.open_table(COURSES)?.insert(c.id, encode(c).as_slice())?;
        Ok(())
    }
    pub fn put_resource(&self, r: &Resource) -> Result<()> {
        self.txn.open_table(RESOURCES)?.insert(r.id, encode(r).as_slice())?;
        Ok(())
    }
    pub fn put_token(&self, t: &Token) -> Result<()> {
        self.txn.open_table(TOKENS)?.insert(t.id, encode(t).as_slice())?;
        Ok(())
    }
    pub fn put_upload(&self, u: &Upload) -> Result<()> {
        self.txn.open_table(UPLOADS)?.insert(u.id, encode(u).as_slice())?;
        Ok(())
    }
    pub fn del_upload(&self, id: Id) -> Result<()> {
        self.txn.open_table(UPLOADS)?.remove(id)?;
        Ok(())
    }
    pub fn put_blob(&self, b: &Blob) -> Result<()> {
        self.txn.open_table(BLOBS)?.insert(b.key.as_str(), encode(b).as_slice())?;
        Ok(())
    }
    pub fn del_blob(&self, key: &str) -> Result<()> {
        self.txn.open_table(BLOBS)?.remove(key)?;
        Ok(())
    }
    pub fn put_meta(&self, key: &str, value: &[u8]) -> Result<()> {
        self.txn.open_table(META)?.insert(key, value)?;
        Ok(())
    }
}
