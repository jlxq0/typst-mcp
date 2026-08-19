//! On-disk storage for rendered documents and uploaded assets.
//!
//! Two properties this module exists to guarantee:
//!
//! * **Tenants cannot see each other's data.** Every lookup takes a `(TenantId, id)`
//!   pair and builds its path from both. There is deliberately no `resolve(id)` — not
//!   a private one, not a convenience helper — so there is nothing to forget to guard.
//! * **Disk cannot grow without bound.** A TTL alone does not achieve that: nothing
//!   stops a caller writing 50 GB inside one window. Retention, a per-tenant ceiling
//!   and a global ceiling are three independent guards, and the volume's own size
//!   limit sits behind all of them.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::principal::TenantId;

/// What is being stored. Each kind gets its own retention and its own subdirectory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// A rendered document and its previews.
    Output,
    /// An uploaded image, font or data file.
    Asset,
    /// A caller-supplied template.
    Template,
}

impl Kind {
    fn dir(self) -> &'static str {
        match self {
            Self::Output => "out",
            Self::Asset => "assets",
            Self::Template => "tpl",
        }
    }

    /// The id prefix, which makes an id self-describing and keeps the namespaces
    /// disjoint — a template id can never be mistaken for an output id.
    pub fn prefix(self) -> &'static str {
        match self {
            Self::Output => "job_",
            Self::Asset => "ast_",
            Self::Template => "tpl_",
        }
    }
}

/// Retention and size limits.
#[derive(Debug, Clone)]
pub struct Limits {
    pub output_ttl: Duration,
    pub asset_ttl: Duration,
    pub template_ttl: Duration,
    /// Ceiling per tenant. Matters as much as the global one: with only a global cap,
    /// one busy caller evicts everyone else's data.
    pub max_tenant_bytes: u64,
    pub max_store_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            output_ttl: Duration::from_secs(2 * 60 * 60),
            asset_ttl: Duration::from_secs(24 * 60 * 60),
            template_ttl: Duration::from_secs(7 * 24 * 60 * 60),
            max_tenant_bytes: 512 * 1024 * 1024,
            max_store_bytes: 2 * 1024 * 1024 * 1024,
        }
    }
}

impl Limits {
    fn ttl(&self, kind: Kind) -> Duration {
        match kind {
            Kind::Output => self.output_ttl,
            Kind::Asset => self.asset_ttl,
            Kind::Template => self.template_ttl,
        }
    }
}

/// Why a store operation failed.
#[derive(Debug, Error)]
pub enum StoreError {
    #[error("{kind:?} {id:?} was not found")]
    NotFound { kind: Kind, id: String },
    /// Distinct from `NotFound` on purpose: a caller — especially a model — reads a
    /// 404 as "wrong id" and retries the same one forever, where "expired" tells it to
    /// produce the thing again.
    #[error("{kind:?} {id:?} has expired; create it again")]
    Expired { kind: Kind, id: String },
    #[error("{id:?} is not a valid {kind:?} id")]
    BadId { kind: Kind, id: String },
    #[error("storing {size} bytes would exceed the {limit} byte limit for this tenant")]
    TenantFull { size: u64, limit: u64 },
    #[error("io error at {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
}

/// Optional metadata recorded alongside a stored file.
///
/// Grouped rather than passed as two adjacent `Option<String>` parameters, which are
/// trivially swapped at a call site and would silently record a content type as a
/// filename.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Meta {
    pub filename: Option<String>,
    pub content_type: Option<String>,
}

impl Meta {
    pub fn new(filename: impl Into<String>, content_type: impl Into<String>) -> Self {
        Self {
            filename: Some(filename.into()),
            content_type: Some(content_type.into()),
        }
    }
}

/// One stored item.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub kind: Kind,
    pub bytes: u64,
    /// Unix seconds.
    pub created_at: u64,
    pub expires_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
}

impl Entry {
    fn is_expired(&self, now: u64) -> bool {
        self.expires_at <= now
    }
}

/// Tenant-partitioned storage on a local filesystem.
pub struct Store {
    root: PathBuf,
    limits: Limits,
    /// Accounting, rebuilt from disk at startup. A lock rather than a lock-free
    /// structure because eviction has to see a consistent total.
    index: Mutex<Index>,
}

#[derive(Debug, Default)]
struct Index {
    /// (tenant, kind, id) -> entry
    entries: HashMap<(TenantId, Kind, String), Entry>,
    per_tenant: HashMap<TenantId, u64>,
    total: u64,
}

impl Store {
    /// Open (or create) a store rooted at `root`, rebuilding the index from disk.
    pub fn open(root: impl Into<PathBuf>, limits: Limits) -> Result<Self, StoreError> {
        let root = root.into();
        create_dir(&root)?;
        create_dir(&root.join("tmp"))?;
        let store = Self {
            root,
            limits,
            index: Mutex::new(Index::default()),
        };
        store.rebuild_index()?;
        Ok(store)
    }

    /// Store `bytes` for `tenant` and return the entry.
    ///
    /// Writes to a temporary file and renames into place, so a reader can never observe
    /// a half-written document; a crash mid-write leaves a stray temp file rather than
    /// a corrupt one that looks complete.
    pub fn put(
        &self,
        tenant: &TenantId,
        kind: Kind,
        name: &str,
        bytes: &[u8],
        meta: Meta,
    ) -> Result<Entry, StoreError> {
        let id = new_id(kind);
        self.put_with_id(tenant, kind, &id, name, bytes, meta)
    }

    /// Store under an existing id, e.g. a second file belonging to one output.
    pub fn put_with_id(
        &self,
        tenant: &TenantId,
        kind: Kind,
        id: &str,
        name: &str,
        bytes: &[u8],
        meta: Meta,
    ) -> Result<Entry, StoreError> {
        let id = validate_id(kind, id)?;
        let size = bytes.len() as u64;

        // Check the tenant ceiling before writing. Global pressure is relieved by
        // eviction; a single tenant over its own budget is simply refused, because
        // evicting their own data to make room for their next upload is a loop.
        {
            let index = self.lock();
            let used = index.per_tenant.get(tenant).copied().unwrap_or(0);
            if used + size > self.limits.max_tenant_bytes {
                return Err(StoreError::TenantFull {
                    size,
                    limit: self.limits.max_tenant_bytes,
                });
            }
        }

        let dir = self.entry_dir(tenant, kind, &id);
        create_dir(&dir)?;
        let path = dir.join(safe_name(name));
        self.write_atomic(&path, bytes)?;

        let now = now();
        let entry = Entry {
            id: id.clone(),
            kind,
            bytes: size,
            created_at: now,
            expires_at: now + self.limits.ttl(kind).as_secs(),
            filename: meta.filename,
            content_type: meta.content_type,
        };
        self.write_meta(&dir, &entry)?;

        {
            let mut index = self.lock();
            let key = (tenant.clone(), kind, id);
            // A second file under the same id adds to its total rather than replacing it.
            if let Some(existing) = index.entries.get_mut(&key) {
                existing.bytes += size;
                existing.expires_at = entry.expires_at;
            } else {
                index.entries.insert(key, entry.clone());
            }
            *index.per_tenant.entry(tenant.clone()).or_insert(0) += size;
            index.total += size;
        }

        self.evict_if_over_budget()?;
        Ok(entry)
    }

    /// Read one file belonging to `(tenant, kind, id)`.
    ///
    /// Both halves of the key are used to build the path, so a valid id belonging to
    /// another tenant simply resolves to a path that does not exist.
    pub fn get(
        &self,
        tenant: &TenantId,
        kind: Kind,
        id: &str,
        name: &str,
    ) -> Result<Vec<u8>, StoreError> {
        let id = validate_id(kind, id)?;
        self.check_live(tenant, kind, &id)?;
        let path = self.entry_dir(tenant, kind, &id).join(safe_name(name));
        std::fs::read(&path).map_err(|source| {
            if source.kind() == io::ErrorKind::NotFound {
                StoreError::NotFound {
                    kind,
                    id: id.clone(),
                }
            } else {
                StoreError::Io { path, source }
            }
        })
    }

    /// Metadata for one entry.
    pub fn entry(&self, tenant: &TenantId, kind: Kind, id: &str) -> Result<Entry, StoreError> {
        let id = validate_id(kind, id)?;
        self.check_live(tenant, kind, &id)
    }

    /// Every live entry of `kind` for `tenant`, newest first.
    pub fn list(&self, tenant: &TenantId, kind: Kind) -> Vec<Entry> {
        let now = now();
        let index = self.lock();
        let mut entries: Vec<Entry> = index
            .entries
            .iter()
            .filter(|((t, k, _), e)| t == tenant && *k == kind && !e.is_expired(now))
            .map(|(_, e)| e.clone())
            .collect();
        entries.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(a.id.cmp(&b.id)));
        entries
    }

    /// Delete one live entry belonging to `tenant`.
    ///
    /// The scoped key is checked before removal, so an id from another tenant is
    /// indistinguishable from a missing id and an expired entry still reports 410.
    pub fn delete(&self, tenant: &TenantId, kind: Kind, id: &str) -> Result<Entry, StoreError> {
        let id = validate_id(kind, id)?;
        let entry = self.check_live(tenant, kind, &id)?;
        self.remove(tenant, kind, &id);
        Ok(entry)
    }

    /// Delete everything that has passed its TTL. Returns the number removed.
    pub fn reap(&self) -> usize {
        let now = now();
        let expired: Vec<_> = {
            let index = self.lock();
            index
                .entries
                .iter()
                .filter(|(_, e)| e.is_expired(now))
                .map(|(key, _)| key.clone())
                .collect()
        };
        let count = expired.len();
        for (tenant, kind, id) in expired {
            self.remove(&tenant, kind, &id);
        }
        count
    }

    /// Bytes currently stored.
    pub fn used_bytes(&self) -> u64 {
        self.lock().total
    }

    pub fn tenant_bytes(&self, tenant: &TenantId) -> u64 {
        self.lock().per_tenant.get(tenant).copied().unwrap_or(0)
    }

    pub fn len(&self) -> usize {
        self.lock().entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    // -- internals ---------------------------------------------------------------

    fn lock(&self) -> std::sync::MutexGuard<'_, Index> {
        // A poisoned lock means a previous holder panicked mid-update. The index is
        // rebuildable from disk, so carrying on with slightly stale accounting beats
        // taking the process down.
        self.index.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Both key halves go into the path — this is the tenant isolation.
    fn entry_dir(&self, tenant: &TenantId, kind: Kind, id: &str) -> PathBuf {
        self.root
            .join(format!("t_{tenant}"))
            .join(kind.dir())
            .join(id)
    }

    fn check_live(&self, tenant: &TenantId, kind: Kind, id: &str) -> Result<Entry, StoreError> {
        let index = self.lock();
        let entry = index
            .entries
            .get(&(tenant.clone(), kind, id.to_owned()))
            .ok_or_else(|| StoreError::NotFound {
                kind,
                id: id.to_owned(),
            })?;
        if entry.is_expired(now()) {
            return Err(StoreError::Expired {
                kind,
                id: id.to_owned(),
            });
        }
        Ok(entry.clone())
    }

    fn write_atomic(&self, path: &Path, bytes: &[u8]) -> Result<(), StoreError> {
        let tmp = self
            .root
            .join("tmp")
            .join(format!("{}.part", new_id(Kind::Output)));
        std::fs::write(&tmp, bytes).map_err(|source| StoreError::Io {
            path: tmp.clone(),
            source,
        })?;
        std::fs::rename(&tmp, path).map_err(|source| {
            let _ = std::fs::remove_file(&tmp);
            StoreError::Io {
                path: path.to_owned(),
                source,
            }
        })
    }

    fn write_meta(&self, dir: &Path, entry: &Entry) -> Result<(), StoreError> {
        let json = serde_json::to_vec(entry).unwrap_or_else(|_| b"{}".to_vec());
        self.write_atomic(&dir.join("meta.json"), &json)
    }

    fn remove(&self, tenant: &TenantId, kind: Kind, id: &str) {
        let _ = std::fs::remove_dir_all(self.entry_dir(tenant, kind, id));
        let mut index = self.lock();
        if let Some(entry) = index.entries.remove(&(tenant.clone(), kind, id.to_owned())) {
            index.total = index.total.saturating_sub(entry.bytes);
            if let Some(used) = index.per_tenant.get_mut(tenant) {
                *used = used.saturating_sub(entry.bytes);
            }
        }
    }

    /// Evict oldest-first until the global ceiling is respected.
    fn evict_if_over_budget(&self) -> Result<(), StoreError> {
        loop {
            let victim = {
                let index = self.lock();
                if index.total <= self.limits.max_store_bytes {
                    return Ok(());
                }
                index
                    .entries
                    .iter()
                    .min_by_key(|(_, e)| e.created_at)
                    .map(|(key, _)| key.clone())
            };
            match victim {
                Some((tenant, kind, id)) => self.remove(&tenant, kind, &id),
                // Nothing left to evict; the accounting must be wrong rather than the
                // disk genuinely full of nothing.
                None => return Ok(()),
            }
        }
    }

    /// Rebuild the index by walking `root`.
    ///
    /// Runs at startup so a restart does not lose track of what is on disk — otherwise
    /// the quotas would read as empty while the volume stayed full.
    fn rebuild_index(&self) -> Result<(), StoreError> {
        let mut index = Index::default();
        let Ok(tenants) = std::fs::read_dir(&self.root) else {
            return Ok(());
        };

        for tenant_dir in tenants.flatten() {
            let Some(name) = tenant_dir.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let Some(tenant) = name.strip_prefix("t_").and_then(TenantId::parse) else {
                continue;
            };

            for kind in [Kind::Output, Kind::Asset, Kind::Template] {
                let dir = tenant_dir.path().join(kind.dir());
                let Ok(entries) = std::fs::read_dir(&dir) else {
                    continue;
                };
                for entry_dir in entries.flatten() {
                    let Some(id) = entry_dir.file_name().to_str().map(str::to_owned) else {
                        continue;
                    };
                    let Ok(entry) = read_meta(&entry_dir.path()) else {
                        // No readable metadata means an interrupted write. Remove it
                        // rather than leave bytes the accounting cannot see.
                        let _ = std::fs::remove_dir_all(entry_dir.path());
                        continue;
                    };
                    index.total += entry.bytes;
                    *index.per_tenant.entry(tenant.clone()).or_insert(0) += entry.bytes;
                    index.entries.insert((tenant.clone(), kind, id), entry);
                }
            }
        }

        *self.lock() = index;
        Ok(())
    }
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store")
            .field("root", &self.root)
            .field("entries", &self.len())
            .field("used_bytes", &self.used_bytes())
            .finish()
    }
}

fn read_meta(dir: &Path) -> Result<Entry, ()> {
    let bytes = std::fs::read(dir.join("meta.json")).map_err(|_| ())?;
    serde_json::from_slice(&bytes).map_err(|_| ())
}

fn create_dir(path: &Path) -> Result<(), StoreError> {
    std::fs::create_dir_all(path).map_err(|source| StoreError::Io {
        path: path.to_owned(),
        source,
    })
}

/// Mint an id: a kind prefix plus a ULID.
///
/// Random rather than sequential, so possessing one id tells you nothing about any
/// other and a leaked URL is not the start of an enumeration.
fn new_id(kind: Kind) -> String {
    format!("{}{}", kind.prefix(), ulid::Ulid::new())
}

/// Accept an id that came from outside.
///
/// Checked against the exact expected shape *before* it is used to build a path, so
/// traversal is impossible rather than merely unlikely.
fn validate_id(kind: Kind, id: &str) -> Result<String, StoreError> {
    let bad = || StoreError::BadId {
        kind,
        id: id.to_owned(),
    };
    let body = id.strip_prefix(kind.prefix()).ok_or_else(bad)?;
    if body.len() != 26 || !body.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return Err(bad());
    }
    Ok(id.to_owned())
}

/// Reduce a filename to something that cannot escape its directory.
///
/// The names here are ours (`doc.pdf`, `page-1.png`), but this is the last point
/// before a path is built, and a defence that only works when the input is trusted is
/// not a defence.
fn safe_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        .collect();
    if cleaned.is_empty() || cleaned.starts_with('.') {
        "file".to_owned()
    } else {
        cleaned
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tenant(seed: &str) -> TenantId {
        TenantId::derive(b"test-salt", seed.as_bytes())
    }

    fn store(limits: Limits) -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(dir.path(), limits).expect("opens");
        (dir, store)
    }

    #[test]
    fn stores_and_reads_back() {
        let (_dir, store) = store(Limits::default());
        let alice = tenant("alice");
        let entry = store
            .put(
                &alice,
                Kind::Output,
                "doc.pdf",
                b"%PDF-1.7",
                Meta::default(),
            )
            .expect("stores");

        assert!(entry.id.starts_with("job_"));
        assert_eq!(entry.bytes, 8);
        assert_eq!(
            store
                .get(&alice, Kind::Output, &entry.id, "doc.pdf")
                .expect("reads"),
            b"%PDF-1.7"
        );
    }

    #[test]
    fn one_tenant_cannot_reach_anothers_data() {
        // The property the whole module is arranged around.
        let (_dir, store) = store(Limits::default());
        let (alice, bob) = (tenant("alice"), tenant("bob"));
        let entry = store
            .put(&alice, Kind::Output, "doc.pdf", b"secret", Meta::default())
            .expect("stores");

        // Bob has a perfectly valid id — it just is not his.
        assert!(matches!(
            store.get(&bob, Kind::Output, &entry.id, "doc.pdf"),
            Err(StoreError::NotFound { .. })
        ));
        assert!(matches!(
            store.entry(&bob, Kind::Output, &entry.id),
            Err(StoreError::NotFound { .. })
        ));
        assert!(store.list(&bob, Kind::Output).is_empty());
    }

    #[test]
    fn kinds_are_separate_namespaces() {
        let (_dir, store) = store(Limits::default());
        let alice = tenant("alice");
        let asset = store
            .put(&alice, Kind::Asset, "logo.png", b"\x89PNG", Meta::default())
            .expect("stores");
        // The prefix makes the id self-describing, so it cannot be used as another kind.
        assert!(matches!(
            store.get(&alice, Kind::Output, &asset.id, "logo.png"),
            Err(StoreError::BadId { .. })
        ));
    }

    #[test]
    fn hostile_ids_are_refused_before_a_path_is_built() {
        let (_dir, store) = store(Limits::default());
        let alice = tenant("alice");
        for hostile in [
            "job_../../../../etc/passwd",
            "../../etc/passwd",
            "job_",
            "job_short",
            "job_0123456789012345678901234567890",
            "job_../0000000000000000000000000",
            "",
        ] {
            assert!(
                matches!(
                    store.get(&alice, Kind::Output, hostile, "doc.pdf"),
                    Err(StoreError::BadId { .. })
                ),
                "{hostile:?} must be refused"
            );
        }
    }

    #[test]
    fn a_hostile_filename_cannot_escape_its_directory() {
        let (_dir, store) = store(Limits::default());
        let alice = tenant("alice");
        let entry = store
            .put(
                &alice,
                Kind::Output,
                "../../escaped.pdf",
                b"x",
                Meta::default(),
            )
            .expect("stores");
        // It was stored under a flattened name inside the entry directory.
        let escaped = store.get(&alice, Kind::Output, &entry.id, "....escaped.pdf");
        assert!(
            escaped.is_ok(),
            "expected the sanitised name to be readable"
        );
    }

    #[test]
    fn expiry_is_reported_as_expired_not_missing() {
        // A model reads 404 as "wrong id" and retries forever; "expired" tells it to
        // render again.
        let limits = Limits {
            output_ttl: Duration::ZERO,
            ..Default::default()
        };
        let (_dir, store) = store(limits);
        let alice = tenant("alice");
        let entry = store
            .put(&alice, Kind::Output, "doc.pdf", b"x", Meta::default())
            .expect("stores");

        assert!(matches!(
            store.entry(&alice, Kind::Output, &entry.id),
            Err(StoreError::Expired { .. })
        ));
        assert!(store.list(&alice, Kind::Output).is_empty());
    }

    #[test]
    fn reaping_frees_space_and_forgets_the_entry() {
        let limits = Limits {
            output_ttl: Duration::ZERO,
            ..Default::default()
        };
        let (_dir, store) = store(limits);
        let alice = tenant("alice");
        store
            .put(
                &alice,
                Kind::Output,
                "doc.pdf",
                &[0u8; 1024],
                Meta::default(),
            )
            .expect("stores");
        assert_eq!(store.used_bytes(), 1024);

        assert_eq!(store.reap(), 1);
        assert_eq!(store.used_bytes(), 0);
        assert!(store.is_empty());
    }

    #[test]
    fn a_tenant_over_its_own_ceiling_is_refused() {
        // Refused rather than evicted: evicting a tenant's own data to make room for
        // their next write is a loop, not a policy.
        let limits = Limits {
            max_tenant_bytes: 100,
            ..Default::default()
        };
        let (_dir, store) = store(limits);
        let alice = tenant("alice");
        store
            .put(&alice, Kind::Asset, "a.bin", &[0u8; 80], Meta::default())
            .expect("fits");
        let err = store
            .put(&alice, Kind::Asset, "b.bin", &[0u8; 80], Meta::default())
            .expect_err("over budget");
        assert!(
            matches!(err, StoreError::TenantFull { limit: 100, .. }),
            "{err:?}"
        );
    }

    #[test]
    fn one_tenants_ceiling_does_not_constrain_another() {
        let limits = Limits {
            max_tenant_bytes: 100,
            ..Default::default()
        };
        let (_dir, store) = store(limits);
        store
            .put(
                &tenant("alice"),
                Kind::Asset,
                "a.bin",
                &[0u8; 90],
                Meta::default(),
            )
            .expect("alice fits");
        store
            .put(
                &tenant("bob"),
                Kind::Asset,
                "b.bin",
                &[0u8; 90],
                Meta::default(),
            )
            .expect("bob has his own budget");
    }

    #[test]
    fn the_global_ceiling_evicts_oldest_first() {
        let limits = Limits {
            max_tenant_bytes: 10_000,
            max_store_bytes: 250,
            ..Default::default()
        };
        let (_dir, store) = store(limits);
        let alice = tenant("alice");

        let first = store
            .put(&alice, Kind::Asset, "a.bin", &[0u8; 100], Meta::default())
            .expect("stores");
        // Same-second timestamps would make eviction order ambiguous.
        std::thread::sleep(Duration::from_millis(1100));
        let second = store
            .put(&alice, Kind::Asset, "b.bin", &[0u8; 100], Meta::default())
            .expect("stores");
        std::thread::sleep(Duration::from_millis(1100));
        let third = store
            .put(&alice, Kind::Asset, "c.bin", &[0u8; 100], Meta::default())
            .expect("stores");

        assert!(
            store.used_bytes() <= 250,
            "over budget: {}",
            store.used_bytes()
        );
        assert!(
            matches!(
                store.entry(&alice, Kind::Asset, &first.id),
                Err(StoreError::NotFound { .. })
            ),
            "the oldest entry should have been evicted"
        );
        assert!(store.entry(&alice, Kind::Asset, &second.id).is_ok());
        assert!(store.entry(&alice, Kind::Asset, &third.id).is_ok());
    }

    #[test]
    fn several_files_can_share_one_id() {
        let (_dir, store) = store(Limits::default());
        let alice = tenant("alice");
        let entry = store
            .put(&alice, Kind::Output, "doc.pdf", b"%PDF-", Meta::default())
            .expect("stores");
        store
            .put_with_id(
                &alice,
                Kind::Output,
                &entry.id,
                "page-1.png",
                b"\x89PNG",
                Meta::default(),
            )
            .expect("stores alongside");

        assert_eq!(
            store
                .get(&alice, Kind::Output, &entry.id, "doc.pdf")
                .unwrap(),
            b"%PDF-"
        );
        assert_eq!(
            store
                .get(&alice, Kind::Output, &entry.id, "page-1.png")
                .unwrap(),
            b"\x89PNG"
        );
        // One logical entry, both files counted.
        assert_eq!(store.list(&alice, Kind::Output).len(), 1);
        assert_eq!(store.used_bytes(), 9);
    }

    #[test]
    fn the_index_survives_a_restart() {
        // Without this the quotas would read empty after a restart while the volume
        // stayed full.
        let dir = tempfile::tempdir().expect("tempdir");
        let alice = tenant("alice");
        let id = {
            let store = Store::open(dir.path(), Limits::default()).expect("opens");
            let entry = store
                .put(&alice, Kind::Asset, "a.bin", &[0u8; 512], Meta::default())
                .expect("stores");
            entry.id
        };

        let reopened = Store::open(dir.path(), Limits::default()).expect("reopens");
        assert_eq!(reopened.used_bytes(), 512);
        assert_eq!(reopened.tenant_bytes(&alice), 512);
        assert!(reopened.entry(&alice, Kind::Asset, &id).is_ok());
    }

    #[test]
    fn an_interrupted_write_is_cleaned_up_on_restart() {
        let dir = tempfile::tempdir().expect("tempdir");
        let alice = tenant("alice");
        {
            let store = Store::open(dir.path(), Limits::default()).expect("opens");
            let entry = store
                .put(&alice, Kind::Asset, "a.bin", b"data", Meta::default())
                .expect("stores");
            // Simulate a crash between writing the payload and its metadata.
            let meta = dir
                .path()
                .join(format!("t_{alice}"))
                .join("assets")
                .join(&entry.id)
                .join("meta.json");
            std::fs::remove_file(meta).expect("removes meta");
        }

        let reopened = Store::open(dir.path(), Limits::default()).expect("reopens");
        assert!(
            reopened.is_empty(),
            "an entry with no metadata must not be indexed"
        );
        assert_eq!(reopened.used_bytes(), 0);
    }

    #[test]
    fn ids_are_unique_and_unguessable() {
        let ids: std::collections::BTreeSet<_> = (0..1000).map(|_| new_id(Kind::Output)).collect();
        assert_eq!(ids.len(), 1000, "ids collided");
    }

    #[test]
    fn listings_are_newest_first_and_exclude_other_tenants() {
        let (_dir, store) = store(Limits::default());
        let (alice, bob) = (tenant("alice"), tenant("bob"));
        store
            .put(&alice, Kind::Asset, "a", b"1", Meta::default())
            .unwrap();
        store
            .put(&bob, Kind::Asset, "b", b"2", Meta::default())
            .unwrap();
        store
            .put(&alice, Kind::Asset, "c", b"3", Meta::default())
            .unwrap();

        let listed = store.list(&alice, Kind::Asset);
        assert_eq!(listed.len(), 2);
        assert!(
            listed
                .windows(2)
                .all(|w| w[0].created_at >= w[1].created_at)
        );
    }

    #[test]
    fn delete_is_tenant_scoped_and_frees_the_exact_entry() {
        let (_dir, store) = store(Limits::default());
        let (alice, bob) = (tenant("alice"), tenant("bob"));
        let entry = store
            .put(
                &alice,
                Kind::Template,
                "template.tar",
                b"draft",
                Meta::default(),
            )
            .expect("stores");

        assert!(matches!(
            store.delete(&bob, Kind::Template, &entry.id),
            Err(StoreError::NotFound { .. })
        ));
        assert_eq!(store.used_bytes(), 5);
        store
            .delete(&alice, Kind::Template, &entry.id)
            .expect("owner deletes");
        assert_eq!(store.used_bytes(), 0);
        assert!(matches!(
            store.entry(&alice, Kind::Template, &entry.id),
            Err(StoreError::NotFound { .. })
        ));
    }
}
