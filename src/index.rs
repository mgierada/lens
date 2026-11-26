//! Whole-repository fact index with a per-blob SQLite cache in `.git/lens/index.db`.
//!
//! Facts are keyed by (blob SHA, language, extractor version), so unchanged files are parsed once
//! and shared between revisions and runs.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use globset::GlobSet;
use rayon::prelude::*;
use rusqlite::{Connection, OptionalExtension, params};

use crate::facts::{Facts, normalize};
use crate::git::Git;
use crate::lang::{Language, Registry};

/// Bump when the [`Facts`] layout or normalisation changes.
const FACTS_VERSION: u32 = 2;
/// Blobs read and parsed per batch, bounding memory on cold indexes of large repos.
const BATCH_SIZE: usize = 2000;

/// Which files get indexed.
pub struct IndexOptions {
    pub excludes: GlobSet,
    pub max_file_size: u64,
    pub use_cache: bool,
}

/// A blob to index: what it is and which language parses it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BlobKey {
    pub blob: String,
    pub lang: &'static str,
}

/// An indexed file at a revision.
pub struct IndexedFile {
    pub path: String,
    pub lang: &'static str,
    pub facts: Arc<Facts>,
}

/// Facts store backed by an optional on-disk cache.
pub struct Index<'a> {
    git: &'a Git,
    registry: &'a Registry,
    cache: Option<Connection>,
    facts: HashMap<BlobKey, Arc<Facts>>,
}

impl<'a> Index<'a> {
    /// Open the index; the cache lives in the repository's common git dir.
    pub fn open(git: &'a Git, registry: &'a Registry, use_cache: bool) -> Result<Self> {
        let cache = if use_cache {
            Some(open_cache(&git.common_dir()?)?)
        } else {
            None
        };
        Ok(Self {
            git,
            registry,
            cache,
            facts: HashMap::new(),
        })
    }

    /// Index every supported file at `rev`.
    pub fn snapshot(&mut self, rev: &str, options: &IndexOptions) -> Result<Vec<IndexedFile>> {
        let entries = self.git.ls_tree(rev)?;
        let selected: Vec<(String, BlobKey)> = entries
            .into_iter()
            .filter(|e| e.size <= options.max_file_size && !options.excludes.is_match(&e.path))
            .filter_map(|e| {
                let lang = self.registry.for_path(&e.path)?.id();
                Some((e.path, BlobKey { blob: e.blob, lang }))
            })
            .collect();
        self.ensure(selected.iter().map(|(_, key)| key.clone()).collect())?;
        Ok(selected
            .into_iter()
            .filter_map(|(path, key)| {
                let facts = self.facts.get(&key)?.clone();
                Some(IndexedFile {
                    path,
                    lang: key.lang,
                    facts,
                })
            })
            .collect())
    }

    /// Facts for a single blob, indexing it if needed.
    pub fn facts_for(&mut self, key: &BlobKey) -> Result<Option<Arc<Facts>>> {
        self.ensure(vec![key.clone()])?;
        Ok(self.facts.get(key).cloned())
    }

    /// Make sure facts for all `keys` are loaded, parsing whatever the cache lacks.
    fn ensure(&mut self, keys: Vec<BlobKey>) -> Result<()> {
        let unique: HashSet<BlobKey> = keys.into_iter().collect();
        let unknown: Vec<BlobKey> = unique
            .into_iter()
            .filter(|k| !self.facts.contains_key(k))
            .collect();
        let mut missing: Vec<BlobKey> = Vec::new();
        for key in unknown {
            match self.load_cached(&key)? {
                Some(facts) => {
                    self.facts.insert(key, Arc::new(facts));
                }
                None => missing.push(key),
            }
        }
        for batch in missing.chunks(BATCH_SIZE) {
            self.index_batch(batch)?;
        }
        Ok(())
    }

    fn index_batch(&mut self, batch: &[BlobKey]) -> Result<()> {
        let shas: Vec<String> = batch.iter().map(|k| k.blob.clone()).collect();
        let blobs = self.git.read_blobs(&shas)?;
        let registry = self.registry;
        let parsed: Vec<(BlobKey, Facts)> = batch
            .par_iter()
            .filter_map(|key| {
                let source = blobs.get(&key.blob)?;
                let lang = registry.by_id(key.lang)?;
                Some((key.clone(), extract(lang, source)))
            })
            .collect();
        self.store(&parsed)?;
        for (key, facts) in parsed {
            self.facts.insert(key, Arc::new(facts));
        }
        Ok(())
    }

    fn load_cached(&self, key: &BlobKey) -> Result<Option<Facts>> {
        let Some(conn) = &self.cache else {
            return Ok(None);
        };
        let version = self.version(key.lang);
        let data: Option<Vec<u8>> = conn
            .prepare_cached(
                "SELECT data FROM facts WHERE blob = ?1 AND lang = ?2 AND version = ?3",
            )?
            .query_row(params![key.blob, key.lang, version], |row| row.get(0))
            .optional()?;
        // A corrupt or outdated entry is treated as a miss and re-indexed.
        Ok(data.and_then(|bytes| serde_json::from_slice(&bytes).ok()))
    }

    fn store(&mut self, parsed: &[(BlobKey, Facts)]) -> Result<()> {
        let versions: HashMap<&str, String> = parsed
            .iter()
            .map(|(k, _)| (k.lang, self.version(k.lang)))
            .collect();
        let Some(conn) = self.cache.as_mut() else {
            return Ok(());
        };
        let tx = conn.transaction()?;
        {
            let mut insert = tx.prepare_cached(
                "INSERT OR REPLACE INTO facts (blob, lang, version, data) VALUES (?1, ?2, ?3, ?4)",
            )?;
            for (key, facts) in parsed {
                let data = serde_json::to_vec(facts)?;
                insert.execute(params![key.blob, key.lang, versions[key.lang], data])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    fn version(&self, lang: &str) -> String {
        let lang_version = self.registry.by_id(lang).map_or(0, |l| l.version());
        format!("{FACTS_VERSION}.{lang_version}")
    }
}

/// Parse one blob. Unparseable input yields empty facts flagged with errors, never a failure.
fn extract(lang: &dyn Language, source: &[u8]) -> Facts {
    match lang.extract(source) {
        Ok(raw) => normalize(raw, source),
        Err(_) => Facts {
            has_errors: true,
            ..Facts::default()
        },
    }
}

fn open_cache(common_dir: &Path) -> Result<Connection> {
    let dir = common_dir.join("lens");
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let conn = Connection::open(dir.join("index.db")).context("opening index cache")?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         CREATE TABLE IF NOT EXISTS facts (
             blob TEXT NOT NULL,
             lang TEXT NOT NULL,
             version TEXT NOT NULL,
             data BLOB NOT NULL,
             PRIMARY KEY (blob, lang, version)
         );",
    )?;
    Ok(conn)
}
