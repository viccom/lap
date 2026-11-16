//! Write surface — the commands that mutate the library (serverization plan §2).
//!
//! The bodies here are plain functions with no Tauri dependency, so the same
//! code serves the desktop shell and (from phase 3) the `lapd` HTTP bridge.
//! The `#[tauri::command]` wrappers stay in the shell (`t_cmds.rs`) and only
//! forward their arguments here.
//!
//! This module is also the home of the write-surface contract test: the
//! arguments the frontend sends are a public interface, so they are pinned
//! explicitly rather than left to drift.

use serde::{Deserialize, Serialize};

use crate::t_sqlite::{
    ACollection, ACollectionOrder, AFile, AFolder, ATag, ATagFileState, Album, AlbumDisplayOrder,
};

/// Result of adding files to a collection (JS-visible: camelCase fields).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionAddResult {
    pub added: usize,
    pub skipped: usize,
    pub added_file_ids: Vec<i64>,
    pub skipped_file_ids: Vec<i64>,
}

/// Metadata changes applied to a batch of files (JS-visible: camelCase fields).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchFileMetadataUpdate {
    pub file_ids: Vec<i64>,
    pub is_favorite: Option<bool>,
    pub rating: Option<i32>,
    pub culling_flag: Option<i32>,
    pub rotate_delta: Option<i32>,
    pub comment: Option<String>,
}

// ============================================================================
// Album arrangement
// ============================================================================

/// Recount the files an album holds.
pub fn recount_album(album_id: i64) -> Result<Album, String> {
    Album::recount_album(album_id).map_err(|e| format!("Error while recounting album: {}", e))
}

/// Register a new album for `folder_path` and apply its metadata.
///
/// Authorizing the directory with the platform's filesystem scope is a
/// desktop-shell concern and stays in the command wrapper; this body only
/// touches the database.
pub fn add_album(
    folder_path: &str,
    name: &str,
    description: &str,
    file_types: i64,
    small_image_filter: i64,
    excluded_folders: &[String],
) -> Result<Album, String> {
    let album = Album::add_album_to_db(folder_path)?;
    let id = album.id.ok_or("New album has no id")?;
    if let Err(error) = Album::edit(
        id,
        name,
        description,
        file_types,
        small_image_filter,
        excluded_folders,
    ) {
        let _ = Album::delete_from_db(id);
        return Err(error);
    }
    Album::get_album_by_id(id)
}

/// Change an album's metadata.
///
/// Block new scans/syncs, then finish cancellation before changing the scope
/// used by the running scan's sweep.
pub async fn edit_album(
    id: i64,
    name: String,
    description: String,
    file_types: i64,
    small_image_filter: i64,
    excluded_folders: Vec<String>,
) -> Result<usize, String> {
    let old = Album::get_album_by_id(id)?;
    let scope_changed = old.file_types != file_types
        || old.excluded_folders != excluded_folders
        || old.small_image_filter != small_image_filter;
    let _scope_guard = scope_changed.then(|| crate::t_utils::AlbumRemovalGuard::acquire(id));
    if scope_changed {
        crate::t_state::index_cancellation()
            .0
            .lock()
            .unwrap()
            .insert(id, true);
        crate::t_utils::wait_for_album_scan_end(id).await?;
    }
    let lock = crate::t_utils::album_sync_lock(id);
    let _guard = lock
        .lock()
        .map_err(|_| "Album sync lock poisoned".to_string())?;
    Album::edit(
        id,
        &name,
        &description,
        file_types,
        small_image_filter,
        &excluded_folders,
    )
}

/// Remove an album and its thumbnail cache.
pub async fn remove_album(id: i64) -> Result<usize, String> {
    let _removal_guard = crate::t_utils::AlbumRemovalGuard::acquire(id);
    crate::t_state::index_cancellation()
        .0
        .lock()
        .unwrap()
        .insert(id, true);
    crate::t_utils::wait_for_album_scan_end(id).await?;

    let result = {
        let album_sync_lock = crate::t_utils::album_sync_lock(id);
        let _album_sync_guard = album_sync_lock
            .lock()
            .map_err(|_| format!("Album sync lock poisoned: {}", id))?;
        Album::delete_from_db(id)
            .map_err(|e| format!("Error while removing album with id {}: {}", id, e))?
    };
    crate::t_utils::release_album_sync_lock(id);

    let library_id = crate::t_config::load_app_config()
        .map(|c| c.current_library_id)
        .unwrap_or_else(|_| "default".to_string());
    if let Err(error) = crate::t_cache_storage::remove_album_cache(&library_id, id) {
        eprintln!("Album {id} was removed, but its thumbnail cache could not be cleaned: {error}");
    }

    Ok(result)
}

/// Reorder the albums in the sidebar.
pub fn reorder_albums(items: Vec<AlbumDisplayOrder>) -> Result<usize, String> {
    Album::reorder_display_order(items).map_err(|e| format!("Error while reordering albums: {}", e))
}

/// Set the file shown as an album's cover.
pub fn set_album_cover(id: i64, file_id: i64) -> Result<usize, String> {
    Album::update_column(id, "cover_file_id", &file_id)
        .map_err(|e| format!("Error while setting album cover: {}", e))
}

// ============================================================================
// Collections
// ============================================================================

/// Create a collection.
pub fn create_collection(name: &str) -> Result<ACollection, String> {
    ACollection::create(name).map_err(|e| format!("Error while creating collection: {}", e))
}

/// Rename a collection.
pub fn rename_collection(id: i64, name: &str) -> Result<usize, String> {
    ACollection::rename(id, name).map_err(|e| format!("Error while renaming collection: {}", e))
}

/// Delete a collection.
pub fn delete_collection(id: i64) -> Result<usize, String> {
    ACollection::delete(id).map_err(|e| format!("Error while deleting collection: {}", e))
}

/// Reorder the collections in the sidebar.
pub fn reorder_collections(items: Vec<ACollectionOrder>) -> Result<usize, String> {
    ACollection::reorder(items).map_err(|e| format!("Error while reordering collections: {}", e))
}

/// Add files to a collection.
pub fn add_files_to_collection(
    collection_id: i64,
    file_ids: Vec<i64>,
) -> Result<CollectionAddResult, String> {
    ACollection::add_files(collection_id, file_ids)
        .map(|(added_file_ids, skipped_file_ids)| CollectionAddResult {
            added: added_file_ids.len(),
            skipped: skipped_file_ids.len(),
            added_file_ids,
            skipped_file_ids,
        })
        .map_err(|e| format!("Error while adding files to collection: {}", e))
}

/// Remove files from a collection.
pub fn remove_files_from_collection(
    collection_id: i64,
    file_ids: Vec<i64>,
) -> Result<usize, String> {
    ACollection::remove_files(collection_id, file_ids)
        .map_err(|e| format!("Error while removing files from collection: {}", e))
}

/// Remove every file from a collection.
pub fn clear_collection(collection_id: i64) -> Result<usize, String> {
    ACollection::clear(collection_id).map_err(|e| format!("Error while clearing collection: {}", e))
}

// ============================================================================
// Favorites and ratings
// ============================================================================

/// Mark a folder as a favorite.
pub fn set_folder_favorite(folder_id: i64, is_favorite: bool) -> Result<usize, String> {
    AFolder::update_column(folder_id, "is_favorite", &is_favorite)
        .map_err(|e| format!("Error while setting folder favorite: {}", e))
}

/// Exclude a folder from search results.
pub fn set_folder_search_excluded(
    album_id: i64,
    folder_path: &str,
    is_excluded: bool,
) -> Result<usize, String> {
    let folder = AFolder::add_to_db(album_id, folder_path)
        .map_err(|e| format!("Error while ensuring folder in DB: {}", e))?;
    let folder_id = folder
        .id
        .ok_or_else(|| "Folder was saved without an id".to_string())?;
    AFolder::update_column(folder_id, "is_excluded_from_search", &is_excluded)
        .map_err(|e| format!("Error while setting folder search exclusion: {}", e))
}

/// Mark a file as a favorite.
pub fn set_file_favorite(file_id: i64, is_favorite: bool) -> Result<usize, String> {
    AFile::update_column(file_id, "is_favorite", &is_favorite)
        .map_err(|e| format!("Error while setting file favorite: {}", e))
}

/// Set a file's rating (0-5).
pub fn set_file_rating(file_id: i64, rating: i32) -> Result<usize, String> {
    let clamped = rating.clamp(0, 5);
    AFile::update_column(file_id, "rating", &clamped)
        .map_err(|e| format!("Error while setting file rating: {}", e))
}

/// Set a file's culling status (0: unreviewed, 1: pick, 2: reject).
pub fn set_file_culling_flag(file_id: i64, culling_flag: i32) -> Result<usize, String> {
    let clamped = culling_flag.clamp(0, 2);
    AFile::update_column(file_id, "culling_flag", &clamped)
        .map_err(|e| format!("Error while setting file culling flag: {}", e))
}

/// Apply several metadata changes to a batch of files.
pub fn batch_update_file_metadata(params: BatchFileMetadataUpdate) -> Result<usize, String> {
    AFile::batch_update_metadata(
        &params.file_ids,
        params.is_favorite,
        params.rating,
        params.culling_flag,
        params.rotate_delta,
        params.comment.as_deref(),
    )
    .map_err(|e| format!("Error while updating file metadata: {}", e))
}

// ============================================================================
// Tags
// ============================================================================

/// Move a tag under a new parent.
pub fn move_tag(tag_id: i64, parent_id: Option<i64>) -> Result<(), String> {
    ATag::move_tag(tag_id, parent_id)
}

/// Create a tag.
pub fn create_tag(name: &str, parent_id: Option<i64>) -> Result<ATag, String> {
    ATag::add(name, parent_id).map_err(|e| format!("Error while creating tag: {}", e))
}

/// Rename a tag.
pub fn rename_tag(tag_id: i64, new_name: &str) -> Result<usize, String> {
    ATag::rename(tag_id, new_name).map_err(|e| format!("Error while renaming tag: {}", e))
}

/// Delete a tag.
pub fn delete_tag(tag_id: i64) -> Result<usize, String> {
    ATag::delete(tag_id).map_err(|e| format!("Error while deleting tag: {}", e))
}

/// Add a tag to a file.
pub fn add_tag_to_file(file_id: i64, tag_id: i64) -> Result<(), String> {
    ATag::add_tag_to_file(file_id, tag_id)
        .map_err(|e| format!("Error while adding tag to file: {}", e))
}

/// Remove a tag from a file.
pub fn remove_tag_from_file(file_id: i64, tag_id: i64) -> Result<usize, String> {
    ATag::remove_tag_from_file(file_id, tag_id)
        .map_err(|e| format!("Error while removing tag from file: {}", e))
}

/// Add and remove tags across a batch of files.
pub fn apply_tags_to_files(
    file_ids: Vec<i64>,
    add_tag_ids: Vec<i64>,
    remove_tag_ids: Vec<i64>,
) -> Result<Vec<ATagFileState>, String> {
    ATag::apply_to_files(&file_ids, &add_tag_ids, &remove_tag_ids)
        .map_err(|e| format!("Error while applying tags to files: {}", e))
}

#[cfg(test)]
pub(crate) mod contract_tests {
    //! The write surface had **no** test coverage, so a dropped or renamed
    //! argument would ship silently: the frontend sends a key the command no
    //! longer declares, and Tauri ignores it. These tests pin the arguments
    //! the frontend sends (camelCase, as Tauri converts them) so a refactor
    //! that moves a command body cannot quietly change the contract.

    /// The arguments the frontend sends for each write command, in order.
    /// `injected` marks a parameter Tauri injects rather than the frontend
    /// sending it — those may change (that is what this batch removes), the
    /// rest may not.
    const WRITE_COMMANDS: &[(&str, &[&str], &[&str])] = &[
        // tags
        ("move_tag", &["tagId", "parentId"], &[]),
        ("create_tag", &["name", "parentId"], &[]),
        ("rename_tag", &["tagId", "newName"], &[]),
        ("delete_tag", &["tagId"], &[]),
        ("add_tag_to_file", &["fileId", "tagId"], &[]),
        ("remove_tag_from_file", &["fileId", "tagId"], &[]),
        (
            "apply_tags_to_files",
            &["fileIds", "addTagIds", "removeTagIds"],
            &[],
        ),
        // favorites and ratings
        ("set_folder_favorite", &["folderId", "isFavorite"], &[]),
        (
            "set_folder_search_excluded",
            &["albumId", "folderPath", "isExcluded"],
            &[],
        ),
        ("set_file_favorite", &["fileId", "isFavorite"], &[]),
        ("set_file_rating", &["fileId", "rating"], &[]),
        ("set_file_culling_flag", &["fileId", "cullingFlag"], &[]),
        ("batch_update_file_metadata", &["params"], &[]),
        // collections
        ("create_collection", &["name"], &[]),
        ("rename_collection", &["id", "name"], &[]),
        ("delete_collection", &["id"], &[]),
        ("reorder_collections", &["items"], &[]),
        ("add_files_to_collection", &["collectionId", "fileIds"], &[]),
        ("remove_files_from_collection", &["collectionId", "fileIds"], &[]),
        ("clear_collection", &["collectionId"], &[]),
        // album arrangement
        ("recount_album", &["albumId"], &[]),
        (
            "add_album",
            &[
                "folderPath",
                "name",
                "description",
                "fileTypes",
                "smallImageFilter",
                "excludedFolders",
            ],
            &["app_handle"],
        ),
        (
            "edit_album",
            &[
                "id",
                "name",
                "description",
                "fileTypes",
                "smallImageFilter",
                "excludedFolders",
            ],
            &[],
        ),
        ("remove_album", &["id"], &[]),
        ("reorder_albums", &["items"], &[]),
        ("set_album_cover", &["id", "fileId"], &[]),
    ];

    /// Tauri converts snake_case command arguments to camelCase for the
    /// frontend, so the contract is stated in camelCase.
    fn to_camel(name: &str) -> String {
        let mut parts = name.split('_');
        let head = parts.next().unwrap_or_default().to_string();
        let tail: Vec<String> = parts
            .map(|p| {
                let mut c = p.chars();
                match c.next() {
                    Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                    None => String::new(),
                }
            })
            .collect();
        format!("{head}{}", tail.join(""))
    }

    /// Split a parameter list on the commas that separate parameters (the
    /// types here contain no top-level commas, but be safe anyway).
    fn split_params(text: &str) -> Vec<String> {
        let mut depth = 0usize;
        let mut current = String::new();
        let mut out = Vec::new();
        for ch in text.chars() {
            match ch {
                '<' | '(' | '[' => {
                    depth += 1;
                    current.push(ch);
                }
                '>' | ')' | ']' => {
                    depth = depth.saturating_sub(1);
                    current.push(ch);
                }
                ',' if depth == 0 => {
                    out.push(current.trim().to_string());
                    current = String::new();
                }
                _ => current.push(ch),
            }
        }
        if !current.trim().is_empty() {
            out.push(current.trim().to_string());
        }
        out
    }

    /// The signature of `name` in the command shell, as (injected, js-visible)
    /// parameter names. Shared with the t_state de-injection guard, which
    /// pins the same invariant for the heavy/admin surface.
    pub(crate) fn signature_of(source: &str, name: &str) -> Option<(Vec<String>, Vec<String>)> {
        let needle = format!("fn {name}(");
        let start = source.find(&needle)?;
        let after = &source[start + needle.len()..];
        let end = after.find(')')?;
        let params = split_params(&after[..end]);
        let mut injected = Vec::new();
        let mut visible = Vec::new();
        for param in params {
            let ident = param.split(':').next().unwrap_or_default().trim().to_string();
            if ident.is_empty() {
                continue;
            }
            if param.contains("AppHandle") || param.contains("State<") {
                injected.push(ident);
            } else {
                visible.push(to_camel(&ident));
            }
        }
        Some((injected, visible))
    }

    #[test]
    fn write_commands_keep_the_arguments_the_frontend_sends() {
        let source = include_str!("t_cmds.rs");
        for (name, expected_visible, expected_injected) in WRITE_COMMANDS {
            let (injected, visible) = signature_of(source, name).unwrap_or_else(|| {
                panic!("write command {name} disappeared from the command shell")
            });
            assert_eq!(
                visible, *expected_visible,
                "{name} changed the arguments the frontend sends"
            );
            assert_eq!(
                injected, *expected_injected,
                "{name} changed its injected parameters"
            );
        }
    }

    #[test]
    fn every_write_command_is_covered_by_the_contract() {
        // A new write command must be added to the table above, otherwise the
        // contract test would not notice its arguments at all.
        let source = include_str!("t_cmds.rs");
        for (name, _, _) in WRITE_COMMANDS {
            assert!(
                signature_of(source, name).is_some(),
                "{name} is listed but not found in the command shell"
            );
        }
        assert_eq!(WRITE_COMMANDS.len(), 26, "the write surface changed size");
    }

    #[test]
    fn write_commands_take_no_injected_state() {
        // The boundary of phase 2 batch 2: a write command may not take a
        // Tauri-injected `State<T>` (the core reads process state itself), and
        // may only take an `AppHandle` where the desktop shell genuinely has
        // to act — authorizing a directory with the platform filesystem scope.
        let source = include_str!("t_cmds.rs");
        for (name, _, expected_injected) in WRITE_COMMANDS {
            let (injected, _) = signature_of(source, name).expect("write command");
            assert!(
                !injected.iter().any(|param| param == "state"),
                "{name} still takes an injected State"
            );
            let app_handles = injected.iter().filter(|p| *p == "app_handle").count();
            let allowed = if *name == "add_album" { 1 } else { 0 };
            assert_eq!(
                app_handles, allowed,
                "{name} takes an AppHandle the core does not need"
            );
            assert_eq!(&injected, expected_injected, "{name} injection changed");
        }
    }
}
