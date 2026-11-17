//! Metadata source adapters share field definitions in `schema`.
pub mod apple_sidecar;
pub mod embedded;
pub(crate) mod files;
pub(crate) mod import_count;
pub(crate) mod keywords;
mod merge;
mod model;
pub(crate) mod packets;
pub(crate) mod schema;
pub(crate) mod system;
pub mod xmp;
