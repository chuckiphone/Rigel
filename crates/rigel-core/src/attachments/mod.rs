//! Attachment storage with per-project quotas (roadmap 101).

pub mod store;

pub use store::{Attachment, AttachmentError, AttachmentStore};
