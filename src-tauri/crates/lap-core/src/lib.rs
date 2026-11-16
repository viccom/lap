//! The Tauri-free core of Lap (serverization plan §3.1).
//!
//! Phase 3 moves the core modules here one domain at a time; the desktop
//! shell (the root `Lap` package) keeps only Tauri glue and thin command
//! wrappers. A module is admitted once it has zero `tauri::` references —
//! the boundary guard in `t_paths` enforces that it stays that way.
