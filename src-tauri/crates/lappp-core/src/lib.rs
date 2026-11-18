//! The Tauri-free core of Lap (serverization plan §3.1).
//!
//! The desktop shell (the workspace root package `Lap`) keeps only Tauri
//! glue and thin command wrappers; everything a future `lapd` server
//! shares lives here. A module must have zero `tauri::` references to
//! move in — the boundary guard in `guard.rs` keeps it that way.

pub mod t_af_area;
pub mod t_affinity;
pub mod t_ai;
pub mod t_ai_png;
pub mod t_batch_rename;
pub mod t_cache_storage;
pub mod t_cluster;
pub mod t_cmds;
pub mod t_common;
pub mod t_config;
pub mod t_dedup;
pub mod t_embed;
pub mod t_embedded_jpeg;
pub mod t_events;
pub mod t_face;
pub mod t_fts;
pub mod t_heif;
pub mod t_http;
pub mod t_image;
pub mod t_jpeg;
pub mod t_jxl;
pub mod t_lens;
pub mod t_libraw;
pub mod t_migration;
pub mod t_montage;
pub mod t_motion_photo;
pub mod t_paths;
pub mod t_raw_display;
pub mod t_rt;
pub mod t_similar;
pub mod t_shutdown;
pub mod t_sqlite;
pub mod t_startup;
pub mod t_state;
pub mod t_storage;
pub mod t_tags;
pub mod t_utils;
pub mod t_video;
pub mod t_vl;
pub mod t_vl_protocol;
pub mod t_vl_schedule;
pub mod t_vl_tags;
pub mod t_write;
pub mod metadata;

#[cfg(test)]
mod guard;
