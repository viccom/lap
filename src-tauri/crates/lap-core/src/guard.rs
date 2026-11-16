//! Boundary guard (serverization plan §2): a module that has been
//! de-Tauri'd must not reach back into `tauri::`. The module list is
//! every source file of lap-core; new files must be added here.

const SOURCES: &[&str] = &[
    include_str!("t_af_area.rs"),
    include_str!("t_ai.rs"),
    include_str!("t_ai_png.rs"),
    include_str!("t_batch_rename.rs"),
    include_str!("t_cache_storage.rs"),
    include_str!("t_cluster.rs"),
    include_str!("t_cmds.rs"),
    include_str!("t_common.rs"),
    include_str!("t_config.rs"),
    include_str!("t_dedup.rs"),
    include_str!("t_embed.rs"),
    include_str!("t_embedded_jpeg.rs"),
    include_str!("t_events.rs"),
    include_str!("t_face.rs"),
    include_str!("t_fts.rs"),
    include_str!("t_heif.rs"),
    include_str!("t_http.rs"),
    include_str!("t_image.rs"),
    include_str!("t_jpeg.rs"),
    include_str!("t_jxl.rs"),
    include_str!("t_lens.rs"),
    include_str!("t_libraw.rs"),
    include_str!("t_migration.rs"),
    include_str!("t_montage.rs"),
    include_str!("t_motion_photo.rs"),
    include_str!("t_paths.rs"),
    include_str!("t_raw_display.rs"),
    include_str!("t_rt.rs"),
    include_str!("t_similar.rs"),
    include_str!("t_shutdown.rs"),
    include_str!("t_sqlite.rs"),
    include_str!("t_startup.rs"),
    include_str!("t_state.rs"),
    include_str!("t_storage.rs"),
    include_str!("t_tags.rs"),
    include_str!("t_utils.rs"),
    include_str!("t_video.rs"),
    include_str!("t_vl.rs"),
    include_str!("t_vl_protocol.rs"),
    include_str!("t_vl_schedule.rs"),
    include_str!("t_vl_tags.rs"),
    include_str!("t_write.rs"),
    include_str!("metadata/apple_sidecar.rs"),
    include_str!("metadata/embedded.rs"),
    include_str!("metadata/files.rs"),
    include_str!("metadata/import_count.rs"),
    include_str!("metadata/keywords.rs"),
    include_str!("metadata/merge.rs"),
    include_str!("metadata/mod.rs"),
    include_str!("metadata/model.rs"),
    include_str!("metadata/packets.rs"),
    include_str!("metadata/schema.rs"),
    include_str!("metadata/system.rs"),
    include_str!("metadata/xmp.rs"),
];

const NAMES: &[&str] = &[
    "t_af_area.rs",
    "t_ai.rs",
    "t_ai_png.rs",
    "t_batch_rename.rs",
    "t_cache_storage.rs",
    "t_cluster.rs",
    "t_cmds.rs",
    "t_common.rs",
    "t_config.rs",
    "t_dedup.rs",
    "t_embed.rs",
    "t_embedded_jpeg.rs",
    "t_events.rs",
    "t_face.rs",
    "t_fts.rs",
    "t_heif.rs",
    "t_http.rs",
    "t_image.rs",
    "t_jpeg.rs",
    "t_jxl.rs",
    "t_lens.rs",
    "t_libraw.rs",
    "t_migration.rs",
    "t_montage.rs",
    "t_motion_photo.rs",
    "t_paths.rs",
    "t_raw_display.rs",
    "t_rt.rs",
    "t_similar.rs",
    "t_shutdown.rs",
    "t_sqlite.rs",
    "t_startup.rs",
    "t_state.rs",
    "t_storage.rs",
    "t_tags.rs",
    "t_utils.rs",
    "t_video.rs",
    "t_vl.rs",
    "t_vl_protocol.rs",
    "t_vl_schedule.rs",
    "t_vl_tags.rs",
    "t_write.rs",
    "metadata/apple_sidecar.rs",
    "metadata/embedded.rs",
    "metadata/files.rs",
    "metadata/import_count.rs",
    "metadata/keywords.rs",
    "metadata/merge.rs",
    "metadata/mod.rs",
    "metadata/model.rs",
    "metadata/packets.rs",
    "metadata/schema.rs",
    "metadata/system.rs",
    "metadata/xmp.rs",
];

/// Line comments are stripped first so doc comments that *describe* the
/// old `tauri::async_runtime` name do not trip the check; the needle is
/// assembled from parts so this assertion's own text cannot match.
#[test]
fn core_modules_do_not_reference_tauri() {
    assert_eq!(NAMES.len(), SOURCES.len(), "lists must stay in sync");
    let needle = ["tauri", "::"].concat();
    let use_needle = ["use ", "tauri"].concat();
    for (name, source) in NAMES.iter().zip(SOURCES) {
        let code: String = source
            .lines()
            .map(|line| match line.find("//") {
                Some(at) => &line[..at],
                None => line,
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !code.contains(&needle) && !code.contains(&use_needle),
            "{name} references tauri; a lap-core module must stay Tauri-free"
        );
    }
}
