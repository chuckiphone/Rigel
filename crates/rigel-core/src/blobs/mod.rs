//! Blob storage with content-addressed integrity (roadmap 184).
//!
//! Blobs are addressed by their SHA-256, so identical bytes are stored once.
//! Every read re-hashes the bytes and refuses to serve a corrupted blob, and an
//! orphan sweep removes blobs no remaining session references.

pub mod store;

pub use store::{BlobError, BlobStore};
