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

#[cfg(test)]
mod contract_tests {
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
            &["state"],
        ),
        ("remove_album", &["id"], &["state"]),
        ("reorder_albums", &["items"], &[]),
        ("set_album_cover", &["id", "fileId"], &[]),
    ];

    /// Tauri converts snake_case command arguments to camelCase for the
    /// frontend, so the contract is stated in camelCase.
    fn to_camel(name: &str) -> String {
        let mut parts = name.split('_');
        let head = parts.next().unwrap_or_default().to_string();
        let tail: Vec<String> = parts.map(|p| {
            let mut c = p.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        }).collect();
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
    /// parameter names.
    fn signature_of(source: &str, name: &str) -> Option<(Vec<String>, Vec<String>)> {
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
}
