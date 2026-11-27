//! Safe mode (roadmap 82).
//!
//! An untrusted project can be opened with tools, network and scripts turned off.
//! Every turn row records the mode it ran under, and leaving safe mode requires an
//! explicit action rather than a settings flip that could be triggered by accident.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    Normal,
    Safe,
}

impl Mode {
    pub fn is_safe(self) -> bool {
        matches!(self, Mode::Safe)
    }

    pub fn tools_enabled(self) -> bool {
        !self.is_safe()
    }

    pub fn network_enabled(self) -> bool {
        !self.is_safe()
    }

    pub fn scripts_enabled(self) -> bool {
        !self.is_safe()
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Normal => "normal",
            Mode::Safe => "safe",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SafeModeState {
    pub mode: Mode,
    /// Explicit confirmation phrase required to leave safe mode.
    pub leave_phrase: String,
}

impl Default for SafeModeState {
    fn default() -> Self {
        Self {
            mode: Mode::Normal,
            leave_phrase: "leave safe mode".to_string(),
        }
    }
}

impl SafeModeState {
    pub fn new_safe() -> Self {
        Self {
            mode: Mode::Safe,
            leave_phrase: "leave safe mode".to_string(),
        }
    }

    pub fn enter(&mut self) {
        self.mode = Mode::Safe;
    }

    /// Leave safe mode only when the caller supplies the exact confirmation phrase.
    pub fn leave(&mut self, confirmation: &str) -> Result<(), SafeModeError> {
        if self.mode != Mode::Safe {
            return Err(SafeModeError::NotInSafeMode);
        }
        if confirmation != self.leave_phrase {
            return Err(SafeModeError::BadConfirmation);
        }
        self.mode = Mode::Normal;
        Ok(())
    }

    /// Record the mode on a turn row (returns the string to store).
    pub fn turn_mode(&self) -> &'static str {
        self.mode.as_str()
    }

    /// One file per session, under `.rigel`, so the mode survives a reload but never enters
    /// the checkpointed tree. The id is one path component and cannot escape that directory.
    pub fn state_path(project_root: &Path, session_id: &str) -> Result<PathBuf, SafeModeError> {
        if session_id.is_empty()
            || session_id == "."
            || session_id == ".."
            || session_id.contains(['/', '\\', ':', '\0'])
        {
            return Err(SafeModeError::UnsafeSession(session_id.to_string()));
        }
        Ok(project_root
            .join(".rigel")
            .join("sessions")
            .join(format!("{session_id}.safe-mode.json")))
    }

    /// A project with no state file opens normally; an unreadable file is an error, not a quiet
    /// downgrade to normal mode.
    pub fn load(project_root: &Path, session_id: &str) -> Result<Self, SafeModeError> {
        let path = Self::state_path(project_root, session_id)?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let text =
            fs::read_to_string(&path).map_err(|error| SafeModeError::Io(error.to_string()))?;
        serde_json::from_str(&text).map_err(|error| SafeModeError::Corrupt(error.to_string()))
    }

    pub fn save(&self, project_root: &Path, session_id: &str) -> Result<(), SafeModeError> {
        let path = Self::state_path(project_root, session_id)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| SafeModeError::Io(error.to_string()))?;
        }
        let text = serde_json::to_string_pretty(self)
            .map_err(|error| SafeModeError::Corrupt(error.to_string()))?;
        fs::write(&path, text).map_err(|error| SafeModeError::Io(error.to_string()))
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SafeModeError {
    #[error("not in safe mode")]
    NotInSafeMode,
    #[error("confirmation phrase does not match")]
    BadConfirmation,
    #[error("session id {0} is not a safe path component")]
    UnsafeSession(String),
    #[error("safe mode state could not be read or written: {0}")]
    Io(String),
    #[error("safe mode state is not valid: {0}")]
    Corrupt(String),
}

/// A turn record carrying the mode it ran under.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnMode {
    pub turn_id: String,
    pub mode: Mode,
}

impl TurnMode {
    pub fn new(turn_id: impl Into<String>, mode: Mode) -> Self {
        Self {
            turn_id: turn_id.into(),
            mode,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_mode_disables_tools_network_scripts() {
        let state = SafeModeState::new_safe();
        assert!(state.mode.is_safe());
        assert!(!state.mode.tools_enabled());
        assert!(!state.mode.network_enabled());
        assert!(!state.mode.scripts_enabled());
    }

    #[test]
    fn normal_mode_enables_everything() {
        let state = SafeModeState::default();
        assert!(!state.mode.is_safe());
        assert!(state.mode.tools_enabled());
        assert!(state.mode.network_enabled());
        assert!(state.mode.scripts_enabled());
    }

    #[test]
    fn leaving_requires_the_exact_phrase() {
        let mut state = SafeModeState::new_safe();
        assert!(state.leave("wrong").is_err());
        assert!(state.mode.is_safe());
        assert!(state.leave("leave safe mode").is_ok());
        assert!(!state.mode.is_safe());
    }

    #[test]
    fn leaving_when_not_safe_is_refused() {
        let mut state = SafeModeState::default();
        assert_eq!(
            state.leave("leave safe mode"),
            Err(SafeModeError::NotInSafeMode)
        );
    }

    #[test]
    fn turn_rows_carry_the_mode() {
        let state = SafeModeState::new_safe();
        let turn = TurnMode::new("turn-1", state.mode);
        assert_eq!(turn.mode, Mode::Safe);
        assert_eq!(state.turn_mode(), "safe");
    }

    #[test]
    fn state_persists_per_session_and_refuses_an_escaping_id() {
        let root = tempfile::tempdir().unwrap();
        let state = SafeModeState::new_safe();
        state.save(root.path(), "session-1").unwrap();
        let loaded = SafeModeState::load(root.path(), "session-1").unwrap();
        assert_eq!(loaded, state);
        assert!(loaded.mode.is_safe());
        assert_eq!(
            SafeModeState::load(root.path(), "other").unwrap().mode,
            Mode::Normal
        );
        assert!(
            root.path()
                .join(".rigel/sessions/session-1.safe-mode.json")
                .is_file()
        );

        assert!(matches!(
            SafeModeState::load(root.path(), "../escape"),
            Err(SafeModeError::UnsafeSession(_))
        ));
        assert!(matches!(
            state.save(root.path(), "a/b"),
            Err(SafeModeError::UnsafeSession(_))
        ));
    }

    #[test]
    fn entering_safe_mode_is_explicit() {
        let mut state = SafeModeState::default();
        state.enter();
        assert!(state.mode.is_safe());
    }
}
