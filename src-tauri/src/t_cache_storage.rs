//! Relocatable thumbnail storage. Other application/WebView caches stay in their default location.
use crate::t_config;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Component, Path, PathBuf},
    sync::{
        Arc, Mutex, RwLock, RwLockReadGuard,
        atomic::{AtomicU8, Ordering},
    },
    time::{Duration, Instant},
};
use uuid::Uuid;
use walkdir::WalkDir;

static CACHE_IO: RwLock<()> = RwLock::new(());
static ROOT_CACHE: RootCache = RootCache(Mutex::new(None));
const DIRECTORY_NAME: &str = "lap-thumbnail-cache";
static MIGRATION: Mutex<Option<(String, Arc<MigrationCancellation>)>> = Mutex::new(None);

#[derive(Default)]
pub(crate) struct MigrationCancellation(AtomicU8);

impl MigrationCancellation {
    pub(crate) fn check(&self) -> Result<(), CacheChangeError> {
        if self.0.load(Ordering::SeqCst) == 1 {
            Err(CacheChangeError {
                code: CacheChangeErrorCode::Cancelled,
                message: "Cache migration cancelled.".into(),
            })
        } else {
            Ok(())
        }
    }

    // Once commit begins, cancellation can no longer be accepted.
    fn seal(&self) -> Result<(), CacheChangeError> {
        match self
            .0
            .compare_exchange(0, 2, Ordering::SeqCst, Ordering::SeqCst)
        {
            Ok(_) => Ok(()),
            Err(_) => self.check(),
        }
    }

    fn cancel(&self) -> bool {
        self.0
            .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }
}

pub(crate) struct MigrationOperation {
    pub cancellation: Arc<MigrationCancellation>,
}

impl MigrationOperation {
    pub fn begin(id: String) -> Result<Self, CacheChangeError> {
        let mut active = MIGRATION
            .lock()
            .map_err(|_| "Cache migration lock is unavailable.")?;
        if active.is_some() {
            return Err("A cache migration is already running.".into());
        }
        let cancellation = Arc::new(MigrationCancellation::default());
        *active = Some((id, cancellation.clone()));
        Ok(Self { cancellation })
    }
}

impl Drop for MigrationOperation {
    fn drop(&mut self) {
        if let Ok(mut active) = MIGRATION.lock() {
            *active = None;
        }
    }
}

pub(crate) fn cancel_migration(id: &str) -> Result<bool, String> {
    let active = MIGRATION
        .lock()
        .map_err(|_| "Cache migration lock is unavailable.")?;
    Ok(active
        .as_ref()
        .is_some_and(|(active_id, token)| active_id == id && token.cancel()))
}

// Outer None means uninitialized; inner None means the system default root.
struct RootCache(Mutex<Option<Option<PathBuf>>>);

impl RootCache {
    fn get(
        &self,
        load: impl FnOnce() -> Result<Option<PathBuf>, String>,
    ) -> Result<Option<PathBuf>, String> {
        let mut cached = self
            .0
            .lock()
            .map_err(|_| "Thumbnail root lock is unavailable.")?;
        if cached.is_none() {
            *cached = Some(load()?);
        }
        Ok(cached.as_ref().unwrap().clone())
    }

    // Called with CACHE_IO held exclusively: readers see the new root only after commit.
    fn commit(
        &self,
        root: Option<PathBuf>,
        save: impl FnOnce() -> Result<(), String>,
    ) -> Result<(), String> {
        let mut cached = self
            .0
            .lock()
            .map_err(|_| "Thumbnail root lock is unavailable.")?;
        save()?;
        *cached = Some(root);
        Ok(())
    }
}

/// Hold across path resolution and filesystem access. Migration cannot switch roots mid-operation.
pub(crate) fn access() -> Result<RwLockReadGuard<'static, ()>, String> {
    CACHE_IO
        .read()
        .map_err(|_| "Thumbnail cache lock is unavailable.".into())
}

pub(crate) fn root() -> Result<PathBuf, String> {
    match ROOT_CACHE.get(|| {
        Ok(t_config::load_app_config()?
            .thumbnail_cache_dir
            .map(PathBuf::from))
    })? {
        Some(path) => {
            if !path.is_dir() {
                return Err("The custom thumbnail cache location is unavailable. Reconnect the drive before using or moving this cache.".into());
            }
            Ok(path)
        }
        None => t_config::get_app_cache_dir(),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheLocation {
    path: String,
    custom: bool,
    available: bool,
}

pub fn location() -> Result<CacheLocation, String> {
    let custom = t_config::load_app_config()?.thumbnail_cache_dir;
    let available = custom.as_ref().is_none_or(|path| Path::new(path).is_dir());
    Ok(CacheLocation {
        available,
        path: custom
            .clone()
            .map(PathBuf::from)
            .map(Ok)
            .unwrap_or_else(t_config::get_app_cache_dir)?
            .to_string_lossy()
            .into_owned(),
        custom: custom.is_some(),
    })
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheProgress {
    pub phase: &'static str,
    pub completed: u64,
    pub total: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheChangeResult {
    pub path: String,
    pub custom: bool,
    pub cleanup_warnings: Vec<String>,
}

fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && Path::new(id).components().count() == 1
        && matches!(
            Path::new(id).components().next(),
            Some(Component::Normal(_))
        )
}

fn reject_overlap(source: &Path, target: &Path) -> Result<(), String> {
    if source != target && (source.starts_with(target) || target.starts_with(source)) {
        return Err(
            "The new cache location cannot contain or be inside the current cache location.".into(),
        );
    }
    Ok(())
}

/// Copy only known library/album trees; never move raw-previews, motion videos, or user files.
fn album_trees(source: &Path, library_ids: &[String]) -> Result<Vec<PathBuf>, String> {
    let mut trees = Vec::new();
    for id in library_ids {
        if !safe_id(id) {
            return Err("Invalid library ID".into());
        }
        let library = source.join(id);
        if !library.try_exists().map_err(|e| e.to_string())? {
            continue;
        }
        if fs::symlink_metadata(&library)
            .map_err(|e| e.to_string())?
            .file_type()
            .is_symlink()
        {
            return Err("Cannot migrate a symlinked thumbnail library directory.".into());
        }
        for entry in fs::read_dir(&library).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            if entry.file_name().to_string_lossy().parse::<i64>().is_ok() {
                let kind = entry.file_type().map_err(|e| e.to_string())?;
                if kind.is_symlink() {
                    return Err("Cannot migrate a symlinked thumbnail album directory.".into());
                }
                if kind.is_dir() {
                    trees.push(PathBuf::from(id).join(entry.file_name()));
                }
            }
        }
    }
    Ok(trees)
}

fn walk_files(
    directory: &Path,
    mut visit: impl FnMut(&Path) -> Result<(), String>,
) -> Result<(), String> {
    for entry in WalkDir::new(directory).follow_links(false) {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.file_type().is_symlink() {
            return Err("Cannot migrate symlinked cache files.".into());
        }
        if entry.file_type().is_file() {
            visit(entry.path())?;
        }
    }
    Ok(())
}

/// Read-only preview using exactly the same library/album selection as migration.
pub(crate) fn migration_file_count() -> Result<u64, CacheChangeError> {
    let _guard = access()?;
    let config = t_config::load_app_config()?;
    let source = match config.thumbnail_cache_dir {
        Some(path) if !Path::new(&path).is_dir() => {
            return Err(CacheChangeError {
                code: CacheChangeErrorCode::SourceUnavailable,
                message: "The custom thumbnail cache location is unavailable.".into(),
            });
        }
        Some(path) => PathBuf::from(path),
        None => t_config::get_app_cache_dir()?,
    };
    count_library_files(
        &source,
        &config
            .libraries
            .into_iter()
            .map(|lib| lib.id)
            .collect::<Vec<_>>(),
    )
    .map_err(Into::into)
}

fn count_library_files(source: &Path, ids: &[String]) -> Result<u64, String> {
    let mut total = 0;
    for tree in album_trees(source, ids)? {
        walk_files(&source.join(tree), |_| {
            total += 1;
            Ok(())
        })?;
    }
    Ok(total)
}

const MIGRATION_JOURNAL: &str = ".lap-thumbnail-migration.json";
const MIGRATION_PENDING: &str = ".lap-thumbnail-migration.pending";

#[derive(Serialize, Deserialize)]
struct MigrationJournal {
    source: PathBuf,
    target: PathBuf,
    stage: String,
    trees: Vec<PathBuf>,
}

fn write_journal(journal: &MigrationJournal) -> Result<(), String> {
    use std::io::Write;
    let path = journal.target.join(MIGRATION_PENDING);
    // create_new never overwrites a previous migration or an unrelated file.
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|e| format!("Cannot create cache migration record: {e}"))?;
    let bytes = serde_json::to_vec(journal).map_err(|e| e.to_string())?;
    if let Err(error) = file.write_all(&bytes).and_then(|_| file.sync_all()) {
        drop(file);
        let _ = fs::remove_file(&path);
        return Err(error.to_string());
    }
    drop(file);
    fs::rename(&path, journal.target.join(MIGRATION_JOURNAL)).map_err(|e| e.to_string())?;
    sync_directory(&journal.target);
    Ok(())
}

fn sync_directory(path: &Path) {
    if let Ok(file) = fs::File::open(path) {
        let _ = file.sync_all();
    }
}

/// Recover only paths declared before copying began. The active root is the commit marker:
/// after config commit, installed copies must survive even if the process never returned.
fn recover_interrupted(target: &Path, active_root: &Path) -> Result<Vec<String>, String> {
    // The pending record is published atomically before staging or copying starts.
    // A crash while writing it therefore leaves no installed files to roll back.
    let pending = target.join(MIGRATION_PENDING);
    if pending.try_exists().map_err(|e| e.to_string())? {
        if !fs::symlink_metadata(&pending)
            .map_err(|e| e.to_string())?
            .file_type()
            .is_file()
        {
            return Err("Invalid pending cache migration record.".into());
        }
        fs::remove_file(pending).map_err(|e| e.to_string())?;
    }
    let record = target.join(MIGRATION_JOURNAL);
    if !record.try_exists().map_err(|e| e.to_string())? {
        return Ok(Vec::new());
    }
    if fs::symlink_metadata(&record)
        .map_err(|e| e.to_string())?
        .file_type()
        .is_symlink()
    {
        return Err("The cache migration record must not be a symbolic link.".into());
    }
    let journal: MigrationJournal =
        serde_json::from_slice(&fs::read(&record).map_err(|e| e.to_string())?).map_err(|e| {
            format!("Invalid cache migration record; original cache is unchanged: {e}")
        })?;
    if journal.target != target
        || !journal.source.is_absolute()
        || (journal.source == journal.target && !journal.trees.is_empty())
        || !safe_id(&journal.stage)
        || !journal
            .stage
            .strip_prefix(".lap-cache-migration-")
            .is_some_and(|id| Uuid::parse_str(id).is_ok())
        || journal.trees.iter().any(|tree| {
            let parts = tree.components().collect::<Vec<_>>();
            parts.len() != 2
                || parts
                    .iter()
                    .any(|part| !matches!(part, Component::Normal(_)))
                || tree
                    .file_name()
                    .and_then(|name| name.to_str())
                    .and_then(|name| name.parse::<i64>().ok())
                    .is_none()
        })
    {
        return Err("Unsafe cache migration record; no cache files were removed.".into());
    }
    reject_overlap(&journal.source, target)?;
    let stage = target.join(&journal.stage);
    if fs::symlink_metadata(&stage).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err("The migration staging directory must not be a symbolic link.".into());
    }
    if active_root != target {
        // If the album still exists in staging, it was never installed in the target.
        for tree in &journal.trees {
            if !stage.join(tree).try_exists().map_err(|e| e.to_string())? {
                let directory = target.join(tree);
                if directory.parent().is_some_and(|parent| {
                    fs::symlink_metadata(parent).is_ok_and(|m| m.file_type().is_symlink())
                }) {
                    return Err("The migration destination contains a symbolic link.".into());
                }
                remove_cache_directory(&directory)?;
                if let Some(parent) = directory.parent() {
                    let _ = fs::remove_dir(parent);
                }
            }
        }
    }
    let mut warnings = Vec::new();
    if active_root == target && journal.source != target {
        for tree in &journal.trees {
            let old = journal.source.join(tree);
            // Never traverse a replaced source root or library symlink during cleanup.
            if fs::symlink_metadata(&journal.source).is_ok_and(|m| m.file_type().is_symlink())
                || old.parent().is_some_and(|parent| {
                    fs::symlink_metadata(parent).is_ok_and(|m| m.file_type().is_symlink())
                })
            {
                warnings.push(format!(
                    "{}: source became a symbolic link; cache retained",
                    old.display()
                ));
                continue;
            }
            if let Err(error) = remove_cache_directory(&old) {
                warnings.push(format!("{}: {error}", old.display()));
            }
            if let Some(parent) = old.parent() {
                let _ = fs::remove_dir(parent);
            }
        }
    }
    remove_cache_directory(&stage)?;
    fs::remove_file(record).map_err(|e| e.to_string())?;
    sync_directory(target);
    Ok(warnings)
}

/// Stage everything before installing or saving the new root. On failure the old root stays active.
#[cfg(test)]
fn transfer(
    source: &Path,
    target: &Path,
    trees: &[PathBuf],
    progress: impl FnMut(CacheProgress),
    commit: impl FnOnce() -> Result<(), String>,
) -> Result<Vec<String>, CacheChangeError> {
    transfer_cancellable(
        source,
        target,
        trees,
        &MigrationCancellation::default(),
        progress,
        commit,
    )
}

fn transfer_cancellable(
    source: &Path,
    target: &Path,
    trees: &[PathBuf],
    cancellation: &MigrationCancellation,
    mut progress: impl FnMut(CacheProgress),
    commit: impl FnOnce() -> Result<(), String>,
) -> Result<Vec<String>, CacheChangeError> {
    cancellation.check()?;
    for tree in trees {
        if target.join(tree).try_exists().map_err(|e| e.to_string())? {
            return Err(format!("The destination already contains a thumbnail album: {}. Choose an empty cache location.", target.join(tree).display()).into());
        }
        // Reject symlinked parent directories even when the album itself does not exist.
        if let Some(parent) = target.join(tree).parent() {
            if parent.exists()
                && fs::symlink_metadata(parent)
                    .map_err(|e| e.to_string())?
                    .file_type()
                    .is_symlink()
            {
                return Err("The destination contains a symlinked directory.".into());
            }
        }
    }
    progress(CacheProgress {
        phase: "counting",
        completed: 0,
        total: 0,
    });
    let mut total = 0;
    for tree in trees {
        walk_files(&source.join(tree), |_| {
            cancellation.check().map_err(|e| e.message)?;
            total += 1;
            Ok(())
        })
        .map_err(|error| cancellation.check().err().unwrap_or_else(|| error.into()))?;
    }
    let stage_name = format!(".lap-cache-migration-{}", Uuid::new_v4());
    let stage = target.join(&stage_name);
    write_journal(&MigrationJournal {
        source: source.to_path_buf(),
        target: target.to_path_buf(),
        stage: stage_name,
        trees: trees.to_vec(),
    })?;
    if let Err(error) = fs::create_dir(&stage) {
        let _ = fs::remove_file(target.join(MIGRATION_JOURNAL));
        return Err(format!("Cannot write to the cache location: {error}").into());
    }
    let result: Result<(), CacheChangeError> = (|| {
        let mut completed = 0;
        let mut last_report = Instant::now();
        progress(CacheProgress {
            phase: "copying",
            completed,
            total,
        });
        for tree in trees {
            cancellation.check()?;
            fs::create_dir_all(stage.join(tree)).map_err(|e| e.to_string())?;
            walk_files(&source.join(tree), |path| {
                cancellation.check().map_err(|e| e.message)?;
                let relative = path.strip_prefix(source).map_err(|e| e.to_string())?;
                let destination = stage.join(relative);
                fs::create_dir_all(destination.parent().unwrap()).map_err(|e| e.to_string())?;
                let copied = fs::copy(path, &destination)
                    .map_err(|e| format!("Failed to copy '{}': {e}", path.display()))?;
                if copied != fs::metadata(path).map_err(|e| e.to_string())?.len() {
                    return Err("Cache copy was incomplete.".into());
                }
                fs::OpenOptions::new()
                    .write(true)
                    .open(&destination)
                    .and_then(|f| f.sync_all())
                    .map_err(|e| e.to_string())?;
                completed += 1;
                if last_report.elapsed() >= Duration::from_millis(150) {
                    progress(CacheProgress {
                        phase: "copying",
                        completed,
                        total,
                    });
                    last_report = Instant::now();
                }
                Ok(())
            })
            .map_err(|error| cancellation.check().err().unwrap_or_else(|| error.into()))?;
        }
        progress(CacheProgress {
            phase: "finishing",
            completed,
            total,
        });
        for tree in trees {
            cancellation.check()?;
            let destination = target.join(tree);
            fs::create_dir_all(destination.parent().unwrap()).map_err(|e| e.to_string())?;
            fs::rename(stage.join(tree), &destination).map_err(|e| e.to_string())?;
            sync_directory(destination.parent().unwrap());
        }
        sync_directory(target);
        cancellation.seal()?;
        commit().map_err(Into::into)
    })();
    if let Err(error) = result {
        progress(CacheProgress {
            phase: "rolling_back",
            completed: 0,
            total,
        });
        // Leave the record intact if rollback fails, so the next attempt can retry it.
        if let Err(cleanup) = recover_interrupted(target, source) {
            return Err(format!(
                "{}. Cache migration rollback needs retry: {cleanup}",
                error.message
            )
            .into());
        }
        progress(CacheProgress {
            phase: "rolled_back",
            completed: 0,
            total,
        });
        return Err(error);
    }
    let mut warnings = Vec::new();
    // Configuration now points to target: recovery must retain the installed cache.
    match recover_interrupted(target, target) {
        Ok(cleanup_warnings) => warnings.extend(cleanup_warnings),
        Err(error) => warnings.push(format!("Cache migration record cleanup: {error}")),
    }
    Ok(warnings)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheChangeError {
    code: CacheChangeErrorCode,
    message: String,
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum CacheChangeErrorCode {
    SourceUnavailable,
    Cancelled,
    OperationFailed,
}

impl From<String> for CacheChangeError {
    fn from(message: String) -> Self {
        Self {
            code: CacheChangeErrorCode::OperationFailed,
            message,
        }
    }
}

impl From<&str> for CacheChangeError {
    fn from(message: &str) -> Self {
        message.to_string().into()
    }
}

fn migration_source(
    custom: Option<&str>,
    default: &Path,
    allow_missing: bool,
) -> Result<Option<PathBuf>, CacheChangeError> {
    let source = custom
        .map(PathBuf::from)
        .unwrap_or_else(|| default.to_path_buf());
    if custom.is_none() {
        fs::create_dir_all(&source).map_err(|e| e.to_string())?;
    } else if !source.is_dir() {
        return if allow_missing {
            Ok(None)
        } else {
            Err(CacheChangeError {
                code: CacheChangeErrorCode::SourceUnavailable,
                message: "The custom thumbnail cache location is unavailable.".into(),
            })
        };
    }
    fs::canonicalize(source)
        .map(Some)
        .map_err(|e| e.to_string().into())
}

/// Cleanup is supplementary: a missing cache must never reverse a successful catalog removal.
pub(crate) fn remove_album_cache(library_id: &str, album_id: i64) -> Result<(), String> {
    let _guard = access()?;
    let directory = root()?.join(library_id).join(album_id.to_string());
    remove_cache_directory(&directory)
}

/// Cache cleanup must not turn an already committed library removal into a failure.
pub(crate) fn remove_library_cache(library_id: &str) -> Vec<String> {
    let _guard = match access() {
        Ok(guard) => guard,
        Err(error) => return vec![error],
    };
    // The configured root is already the full lap-thumbnail-cache path.
    // Motion videos remain under the system root, so clean both locations.
    remove_library_cache_from_roots(library_id, [root(), t_config::get_app_cache_dir()])
}

fn remove_library_cache_from_roots(
    library_id: &str,
    roots: impl IntoIterator<Item = Result<PathBuf, String>>,
) -> Vec<String> {
    roots
        .into_iter()
        .filter_map(|root| {
            root.and_then(|root| remove_cache_directory(&root.join(library_id)))
                .err()
        })
        .collect()
}

fn remove_cache_directory(directory: &Path) -> Result<(), String> {
    match fs::remove_dir_all(directory) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

/// Empty directories and regular OS metadata files may remain after a migration.
/// Inspect their contents without following links or modifying any existing files.
fn destination_has_content(directory: &Path) -> Result<bool, String> {
    for entry in WalkDir::new(directory).follow_links(false).min_depth(1) {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.file_type().is_dir() {
            continue;
        }
        if entry.file_type().is_file()
            && matches!(
                entry.file_name().to_str(),
                Some(".DS_Store" | "Thumbs.db" | "desktop.ini")
            )
        {
            continue;
        }
        return Ok(true);
    }
    Ok(false)
}

pub fn change(
    selected: Option<String>,
    allow_missing_source: bool,
    cancellation: &MigrationCancellation,
    progress: impl FnMut(CacheProgress),
) -> Result<CacheChangeResult, CacheChangeError> {
    let _guard = CACHE_IO.try_write().map_err(|_| {
        "Thumbnail cache is busy. Please retry after current operations finish.".to_string()
    })?;
    cancellation.check()?;
    let config = t_config::load_app_config()?;
    let default_root = t_config::get_app_cache_dir()?;
    // Only an explicit recovery confirmation may abandon an unavailable source.
    let source = migration_source(
        config.thumbnail_cache_dir.as_deref(),
        &default_root,
        allow_missing_source,
    )?;
    let previous_root = source
        .clone()
        .or_else(|| config.thumbnail_cache_dir.as_ref().map(PathBuf::from));
    let target = if let Some(selected) = selected.as_ref() {
        let parent = fs::canonicalize(selected)
            .map_err(|e| format!("Cache destination unavailable: {e}"))?;
        parent.join(DIRECTORY_NAME)
    } else {
        t_config::get_app_cache_dir()?
    };
    if fs::symlink_metadata(&target).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err("The cache destination must not be a symbolic link.".into());
    }
    // Validate before creating anything under the existing cache.
    if let Some(previous) = previous_root.as_ref() {
        reject_overlap(previous, &target)?;
    }
    fs::create_dir_all(&target).map_err(|e| e.to_string())?;
    let target = fs::canonicalize(target).map_err(|e| e.to_string())?;
    if let Some(previous) = previous_root.as_ref() {
        reject_overlap(previous, &target)?;
    }
    let active_root = source
        .as_deref()
        .or(previous_root.as_deref())
        .unwrap_or(&target);
    let mut recovery_warnings = Vec::new();
    if let Some(source) = source.as_ref() {
        recovery_warnings.extend(recover_interrupted(source, active_root)?);
    }
    recovery_warnings.extend(recover_interrupted(&target, active_root)?);
    let path = target.to_string_lossy().into_owned();
    if source.as_ref() == Some(&target) {
        return Ok(CacheChangeResult {
            path,
            custom: config.thumbnail_cache_dir.is_some(),
            cleanup_warnings: recovery_warnings,
        });
    }
    if selected.is_some() && destination_has_content(&target)? {
        return Err(
            "Choose an empty lap-thumbnail-cache directory to avoid overwriting existing files."
                .into(),
        );
    }
    let ids = config
        .libraries
        .iter()
        .map(|l| l.id.clone())
        .collect::<Vec<_>>();
    let trees = source
        .as_ref()
        .map(|source| album_trees(source, &ids))
        .transpose()?
        .unwrap_or_default();
    let custom = selected.is_some();
    let mut warnings = transfer_cancellable(
        source
            .as_deref()
            .or(previous_root.as_deref())
            .unwrap_or(&target),
        &target,
        &trees,
        cancellation,
        progress,
        || {
            ROOT_CACHE.commit(custom.then(|| target.clone()), || {
                t_config::set_thumbnail_cache_dir(custom.then(|| path.clone()))
            })
        },
    )?;
    warnings.extend(recovery_warnings);
    Ok(CacheChangeResult {
        path,
        custom,
        cleanup_warnings: warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_only_destination_can_be_reused_after_round_trip() {
        let source = TestDir::new();
        let target = TestDir::new();
        source.put("default/1/ab/a.jpg", b"thumbnail");
        target.put(".DS_Store", b"finder");
        target.put("default/.DS_Store", b"finder");
        target.put("unused/Thumbs.db", b"windows");
        target.put("unused/desktop.ini", b"windows");
        assert!(!destination_has_content(&target.0).unwrap());
        let trees = vec![PathBuf::from("default/1")];
        transfer(&source.0, &target.0, &trees, |_| {}, || Ok(())).unwrap();
        assert!(destination_has_content(&target.0).unwrap());
        transfer(&target.0, &source.0, &trees, |_| {}, || Ok(())).unwrap();
        assert!(!destination_has_content(&target.0).unwrap());
        transfer(&source.0, &target.0, &trees, |_| {}, || Ok(())).unwrap();
        assert_eq!(
            fs::read(target.0.join("default/1/ab/a.jpg")).unwrap(),
            b"thumbnail"
        );
        assert_eq!(
            fs::read(target.0.join("default/.DS_Store")).unwrap(),
            b"finder"
        );
    }

    #[test]
    fn destination_check_preserves_protection_for_real_and_hidden_files() {
        let target = TestDir::new();
        target.put("empty/.DS_Store", b"finder");
        target.put("empty/.hidden", b"user file");
        assert!(destination_has_content(&target.0).unwrap());
        fs::remove_file(target.0.join("empty/.hidden")).unwrap();
        target.put("empty/nested/photo.jpg", b"user file");
        assert!(destination_has_content(&target.0).unwrap());
        assert_eq!(
            fs::read(target.0.join("empty/nested/photo.jpg")).unwrap(),
            b"user file"
        );
    }

    #[cfg(unix)]
    #[test]
    fn metadata_named_symlinks_are_not_treated_as_empty() {
        let target = TestDir::new();
        let outside = TestDir::new();
        outside.put("keep", b"user file");
        std::os::unix::fs::symlink(outside.0.join("keep"), target.0.join(".DS_Store")).unwrap();
        assert!(destination_has_content(&target.0).unwrap());
        assert_eq!(fs::read(outside.0.join("keep")).unwrap(), b"user file");
    }

    #[test]
    fn migration_preview_counts_only_managed_album_files_across_libraries() {
        let source = TestDir::new();
        source.put("default/1/ab/a.jpg", b"one");
        source.put("default/2/cd/b.png", b"two");
        source.put("second/3/ef/c.jpg", b"three");
        source.put("default/motion/movie.mp4", b"motion");
        source.put("raw-previews/raw.jpg", b"raw");
        source.put("unknown/4/gh/d.jpg", b"unmanaged");
        assert_eq!(
            count_library_files(&source.0, &["default".into(), "second".into()]).unwrap(),
            3
        );
        assert_eq!(
            count_library_files(&source.0.join("missing"), &["default".into()]).unwrap(),
            0
        );
    }

    #[test]
    fn cancelling_before_commit_preserves_source_and_rolls_back_target() {
        for phase in ["counting", "copying", "finishing"] {
            let source = TestDir::new();
            let target = TestDir::new();
            source.put("default/1/ab/abc.jpg", b"original");
            let cancellation = MigrationCancellation::default();
            let mut committed = false;
            let mut rollback_events = Vec::new();
            let error = transfer_cancellable(
                &source.0,
                &target.0,
                &["default/1".into()],
                &cancellation,
                |progress| {
                    if progress.phase == phase {
                        assert!(cancellation.cancel());
                    }
                    if progress.phase == "rolling_back" {
                        assert!(target.0.join(MIGRATION_JOURNAL).exists());
                        rollback_events.push(progress.phase);
                    }
                    if progress.phase == "rolled_back" {
                        assert_eq!(fs::read_dir(&target.0).unwrap().count(), 0);
                        rollback_events.push(progress.phase);
                    }
                },
                || {
                    committed = true;
                    Ok(())
                },
            )
            .unwrap_err();
            assert_eq!(error.code, CacheChangeErrorCode::Cancelled);
            if phase != "counting" {
                assert_eq!(rollback_events, vec!["rolling_back", "rolled_back"]);
            }
            assert!(!committed);
            assert_eq!(
                fs::read(source.0.join("default/1/ab/abc.jpg")).unwrap(),
                b"original"
            );
            assert_eq!(fs::read_dir(&target.0).unwrap().count(), 0);
        }
    }

    #[test]
    fn cancellation_cannot_be_accepted_after_commit_begins() {
        let source = TestDir::new();
        let target = TestDir::new();
        source.put("default/1/ab/abc.jpg", b"original");
        let cancellation = MigrationCancellation::default();
        transfer_cancellable(
            &source.0,
            &target.0,
            &["default/1".into()],
            &cancellation,
            |_| {},
            || {
                assert!(!cancellation.cancel());
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(
            fs::read(target.0.join("default/1/ab/abc.jpg")).unwrap(),
            b"original"
        );
        assert!(!source.0.join("default/1").exists());
    }

    #[test]
    fn cancellation_is_scoped_to_the_active_operation() {
        let operation = MigrationOperation::begin("current".into()).unwrap();
        assert!(!cancel_migration("previous").unwrap());
        operation.cancellation.check().unwrap();
        assert!(cancel_migration("current").unwrap());
        assert_eq!(
            operation.cancellation.check().unwrap_err().code,
            CacheChangeErrorCode::Cancelled
        );
        drop(operation);
        let retry = MigrationOperation::begin("retry".into()).unwrap();
        assert!(!cancel_migration("current").unwrap());
        retry.cancellation.check().unwrap();
    }

    #[test]
    fn cache_change_errors_expose_stable_codes_independent_of_messages() {
        let error = CacheChangeError {
            code: CacheChangeErrorCode::SourceUnavailable,
            message: "Drive disconnected".into(),
        };
        assert_eq!(
            serde_json::to_value(error).unwrap(),
            serde_json::json!({
                "code": "SOURCE_UNAVAILABLE", "message": "Drive disconnected"
            })
        );
        let error = CacheChangeError::from("SOURCE_UNAVAILABLE in an unrelated message");
        assert_eq!(error.code, CacheChangeErrorCode::OperationFailed);
    }

    #[test]
    fn root_cache_loads_once_and_changes_only_after_successful_commit() {
        let cache = RootCache(Mutex::new(None));
        assert!(cache.get(|| Err("temporary config error".into())).is_err());
        assert_eq!(cache.get(|| Ok(None)).unwrap(), None);
        assert_eq!(
            cache.get(|| panic!("default root must be cached")).unwrap(),
            None
        );
        let custom = PathBuf::from("/custom/lap-thumbnail-cache");
        assert!(
            cache
                .commit(Some(custom.clone()), || Err("save failed".into()))
                .is_err()
        );
        assert_eq!(cache.get(|| panic!("must retain old root")).unwrap(), None);
        cache.commit(Some(custom.clone()), || Ok(())).unwrap();
        assert_eq!(
            cache.get(|| panic!("must use committed root")).unwrap(),
            Some(custom)
        );
        cache.commit(None, || Ok(())).unwrap();
        assert_eq!(cache.get(|| panic!("reset must be cached")).unwrap(), None);
    }

    struct TestDir(PathBuf);
    impl TestDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("lap-cache-test-{}", Uuid::new_v4()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn put(&self, path: &str, content: &[u8]) {
            let path = self.0.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, content).unwrap();
        }
    }
    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn interrupted_copy_and_installation_can_be_retried() {
        let source = TestDir::new();
        let target = TestDir::new();
        source.put("default/1/ab/abc.jpg", b"original");
        let trees = vec![PathBuf::from("default/1")];
        // Crash after staging is created but before copying finishes.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            transfer(
                &source.0,
                &target.0,
                &trees,
                |p| {
                    if p.phase == "copying" {
                        panic!("simulated process interruption");
                    }
                },
                || Ok(()),
            )
            .unwrap();
        }));
        assert!(target.0.join(MIGRATION_JOURNAL).exists());
        recover_interrupted(&target.0, &source.0).unwrap();
        assert_eq!(fs::read_dir(&target.0).unwrap().count(), 0);
        // Crash after installing copies, before config commit.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            transfer(
                &source.0,
                &target.0,
                &trees,
                |_| {},
                || panic!("before config commit"),
            )
            .unwrap();
        }));
        assert!(target.0.join("default/1/ab/abc.jpg").exists());
        recover_interrupted(&target.0, &source.0).unwrap();
        assert_eq!(fs::read_dir(&target.0).unwrap().count(), 0);
        assert_eq!(
            fs::read(source.0.join("default/1/ab/abc.jpg")).unwrap(),
            b"original"
        );
        transfer(&source.0, &target.0, &trees, |_| {}, || Ok(())).unwrap();
        assert_eq!(
            fs::read(target.0.join("default/1/ab/abc.jpg")).unwrap(),
            b"original"
        );
    }

    #[test]
    fn recovery_preserves_copies_when_config_was_committed() {
        let source = TestDir::new();
        let target = TestDir::new();
        source.put("default/1/ab/abc.jpg", b"original");
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            transfer(
                &source.0,
                &target.0,
                &["default/1".into()],
                |_| {},
                || panic!("after config commit"),
            )
            .unwrap();
        }));
        recover_interrupted(&target.0, &target.0).unwrap();
        assert_eq!(
            fs::read(target.0.join("default/1/ab/abc.jpg")).unwrap(),
            b"original"
        );
        assert!(!target.0.join(MIGRATION_JOURNAL).exists());
        assert!(!source.0.join("default/1/ab/abc.jpg").exists());
        // The original location can immediately be selected again.
        transfer(
            &target.0,
            &source.0,
            &["default/1".into()],
            |_| {},
            || Ok(()),
        )
        .unwrap();
    }

    #[test]
    fn incomplete_record_and_unsafe_journal_are_handled_without_touching_user_files() {
        let target = TestDir::new();
        target.put(MIGRATION_PENDING, b"{partial-json");
        target.put("keep.txt", b"unrelated");
        recover_interrupted(&target.0, Path::new("/old-cache")).unwrap();
        assert!(!target.0.join(MIGRATION_PENDING).exists());
        let journal = MigrationJournal {
            source: PathBuf::from("/old-cache"),
            target: target.0.clone(),
            stage: format!(".lap-cache-migration-{}", Uuid::new_v4()),
            trees: vec![PathBuf::from("../outside")],
        };
        write_journal(&journal).unwrap();
        assert!(recover_interrupted(&target.0, Path::new("/old-cache")).is_err());
        assert_eq!(fs::read(target.0.join("keep.txt")).unwrap(), b"unrelated");
    }

    #[test]
    fn unavailable_source_requires_confirmation_and_never_recreates_old_directory() {
        let parent = TestDir::new();
        let missing = parent.0.join("disconnected-drive/cache");
        let default = TestDir::new();
        let custom = missing.to_str().unwrap();
        assert_eq!(
            migration_source(Some(custom), &default.0, false)
                .unwrap_err()
                .code,
            CacheChangeErrorCode::SourceUnavailable
        );
        assert!(!missing.exists());
        let source = migration_source(Some(custom), &default.0, true).unwrap();
        assert!(source.is_none());
        let target = TestDir::new();
        target.put("raw-previews/keep.jpg", b"unrelated");
        let mut committed = false;
        transfer(
            &target.0,
            &target.0,
            &[],
            |_| {},
            || {
                committed = true;
                Ok(())
            },
        )
        .unwrap();
        assert!(committed);
        assert!(!missing.exists());
        assert_eq!(
            fs::read(target.0.join("raw-previews/keep.jpg")).unwrap(),
            b"unrelated"
        );
    }

    #[test]
    fn recovery_still_migrates_files_if_the_drive_reconnects() {
        let source = TestDir::new();
        let default = TestDir::new();
        source.put("default/1/ab/abc.jpg", b"thumbnail");
        assert_eq!(
            migration_source(source.0.to_str(), &default.0, true).unwrap(),
            Some(fs::canonicalize(&source.0).unwrap())
        );
    }

    #[test]
    fn absent_cache_cleanup_is_successful_and_real_errors_are_reported() {
        let directory = TestDir::new();
        assert!(remove_cache_directory(&directory.0.join("missing")).is_ok());
        directory.put("file", b"not a directory");
        assert!(remove_cache_directory(&directory.0.join("file")).is_err());
        assert!(directory.0.join("file").exists());
    }

    #[test]
    fn migration_blocks_cache_access_until_it_finishes() {
        let guard = CACHE_IO.write().unwrap();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let _access = access().unwrap();
            ready_tx.send(()).unwrap();
        });
        assert!(ready_rx.recv_timeout(Duration::from_millis(50)).is_err());
        drop(guard);
        ready_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        worker.join().unwrap();
    }

    #[test]
    fn old_configs_default_to_system_cache() {
        let config: t_config::AppConfig =
            serde_json::from_str(r#"{"current_library_id":"default","libraries":[]}"#).unwrap();
        assert!(config.thumbnail_cache_dir.is_none());
        let mut config = config;
        config.thumbnail_cache_dir = Some("/custom/lap-thumbnail-cache".into());
        let loaded: t_config::AppConfig =
            serde_json::from_str(&serde_json::to_string(&config).unwrap()).unwrap();
        assert_eq!(loaded.thumbnail_cache_dir, config.thumbnail_cache_dir);
    }

    #[test]
    fn migrates_all_libraries_and_restores_without_moving_other_caches() {
        let source = TestDir::new();
        let target = TestDir::new();
        source.put("default/1/ab/abc.jpg", b"jpeg-one");
        source.put("second/2/cd/cde.png", b"png-two");
        source.put("default/motion/movie.mp4", b"motion");
        source.put("raw-previews/preview.jpg", b"raw");
        source.put("unknown/3/ef/efg.jpg", b"unmanaged");
        let trees = album_trees(&source.0, &["default".into(), "second".into()]).unwrap();
        assert_eq!(trees.len(), 2);
        let mut events = Vec::new();
        let warnings = transfer(
            &source.0,
            &target.0,
            &trees,
            |p| events.push(p),
            || {
                // Commit happens only after all files are installed, with source still intact.
                assert_eq!(
                    fs::read(target.0.join("second/2/cd/cde.png")).unwrap(),
                    b"png-two"
                );
                assert!(source.0.join("default/1/ab/abc.jpg").exists());
                Ok(())
            },
        )
        .unwrap();
        assert!(warnings.is_empty());
        assert!(!source.0.join("default/1").exists());
        assert!(source.0.join("default/motion/movie.mp4").exists());
        assert!(source.0.join("raw-previews/preview.jpg").exists());
        assert!(source.0.join("unknown/3/ef/efg.jpg").exists());
        assert_eq!(events.last().unwrap().completed, 2);
        assert_eq!(events.last().unwrap().total, 2);
        transfer(&target.0, &source.0, &trees, |_| {}, || Ok(())).unwrap();
        assert_eq!(
            fs::read(source.0.join("default/1/ab/abc.jpg")).unwrap(),
            b"jpeg-one"
        );
        assert!(!target.0.join("second/2").exists());
        assert_eq!(fs::read_dir(&target.0).unwrap().count(), 0);
        // Selecting the same custom location again must work without hidden records.
        transfer(&source.0, &target.0, &trees, |_| {}, || Ok(())).unwrap();
        assert!(!target.0.join(MIGRATION_JOURNAL).exists());
        assert!(!target.0.join(MIGRATION_PENDING).exists());
        assert_eq!(
            fs::read(target.0.join("default/1/ab/abc.jpg")).unwrap(),
            b"jpeg-one"
        );
    }

    #[test]
    fn library_cleanup_removes_custom_thumbnails_and_system_motion_only_for_that_library() {
        let custom = TestDir::new();
        let system = TestDir::new();
        custom.put("lap-thumbnail-cache/default/1/ab/abc.jpg", b"thumbnail");
        custom.put("lap-thumbnail-cache/second/2/cd/cde.jpg", b"keep");
        system.put("default/motion/movie.mp4", b"motion");
        system.put("second/motion/movie.mp4", b"keep");
        let warnings = remove_library_cache_from_roots(
            "default",
            [Ok(custom.0.join(DIRECTORY_NAME)), Ok(system.0.clone())],
        );
        assert!(warnings.is_empty());
        assert!(!custom.0.join("lap-thumbnail-cache/default").exists());
        assert!(!system.0.join("default").exists());
        assert!(
            custom
                .0
                .join("lap-thumbnail-cache/second/2/cd/cde.jpg")
                .exists()
        );
        assert!(system.0.join("second/motion/movie.mp4").exists());
        // An unavailable thumbnail root must not prevent system-cache cleanup.
        let warnings = remove_library_cache_from_roots(
            "second",
            [Err("Drive unavailable".into()), Ok(system.0.clone())],
        );
        assert_eq!(warnings, vec!["Drive unavailable"]);
        assert!(!system.0.join("second").exists());
    }

    #[test]
    fn config_commit_failure_rolls_back_installed_copies() {
        let source = TestDir::new();
        let target = TestDir::new();
        source.put("default/1/ab/abc.jpg", b"original");
        target.put("unrelated.txt", b"keep");
        let trees = vec![PathBuf::from("default/1")];
        assert!(
            transfer(
                &source.0,
                &target.0,
                &trees,
                |_| {},
                || Err("config save failed".into())
            )
            .is_err()
        );
        assert_eq!(
            fs::read(source.0.join("default/1/ab/abc.jpg")).unwrap(),
            b"original"
        );
        assert!(!target.0.join("default/1").exists());
        assert_eq!(fs::read(target.0.join("unrelated.txt")).unwrap(), b"keep");
        assert!(!fs::read_dir(&target.0).unwrap().any(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".lap-cache-migration")
        }));
    }

    #[test]
    fn copy_failure_preserves_source_and_does_not_commit() {
        let source = TestDir::new();
        let target = TestDir::new();
        source.put("default/1/ab/abc.jpg", b"original");
        let trees = vec![PathBuf::from("default/1")];
        let mut committed = false;
        let result = transfer(
            &source.0,
            &target.0,
            &trees,
            |p| {
                // Simulate losing access to the destination after counting.
                if p.phase == "copying" {
                    for e in fs::read_dir(&target.0).unwrap() {
                        let e = e.unwrap();
                        if e.file_type().unwrap().is_dir() {
                            fs::remove_dir_all(e.path()).unwrap();
                        }
                    }
                    fs::write(target.0.join("default"), b"conflict").unwrap();
                }
            },
            || {
                committed = true;
                Ok(())
            },
        );
        assert!(result.is_err());
        assert!(!committed);
        assert!(source.0.join("default/1/ab/abc.jpg").exists());
    }

    #[test]
    fn rejects_existing_album_without_overwriting_it() {
        let source = TestDir::new();
        let target = TestDir::new();
        source.put("default/1/ab/abc.jpg", b"new");
        target.put("default/1/ab/abc.jpg", b"old");
        assert!(
            transfer(
                &source.0,
                &target.0,
                &["default/1".into()],
                |_| {},
                || panic!("must not commit")
            )
            .is_err()
        );
        assert_eq!(
            fs::read(target.0.join("default/1/ab/abc.jpg")).unwrap(),
            b"old"
        );
    }

    #[test]
    fn rejects_nested_paths_and_unsafe_library_ids() {
        assert!(reject_overlap(Path::new("/cache"), Path::new("/cache/inside")).is_err());
        assert!(reject_overlap(Path::new("/cache/inside"), Path::new("/cache")).is_err());
        assert!(reject_overlap(Path::new("/cache"), Path::new("/other")).is_ok());
        for id in ["", "..", "../outside", "/absolute"] {
            assert!(!safe_id(id));
        }
        assert!(safe_id("default"));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinks_without_touching_their_targets() {
        let source = TestDir::new();
        let target = TestDir::new();
        let external = TestDir::new();
        external.put("private.jpg", b"private");
        source.put("default/1/ab/abc.jpg", b"thumbnail");
        std::os::unix::fs::symlink(&external.0, source.0.join("default/1/link")).unwrap();
        assert!(
            transfer(
                &source.0,
                &target.0,
                &["default/1".into()],
                |_| {},
                || panic!("must not commit")
            )
            .is_err()
        );
        assert_eq!(
            fs::read(external.0.join("private.jpg")).unwrap(),
            b"private"
        );
    }
}
