//! Per-project versioned prompt templates and immutable transcript snapshots.

mod library;

pub use library::{Prompt, PromptError, PromptLibrary, PromptPin};
