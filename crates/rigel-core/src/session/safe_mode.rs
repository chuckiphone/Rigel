//! Safe mode (roadmap 82).
//!
//! An untrusted project can be opened with tools, network and scripts turned off.
//! Every turn row records the mode it ran under, and leaving safe mode requires an
//! explicit action rather than a settings flip that could be triggered by accident.

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
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SafeModeError {
    #[error("not in safe mode")]
    NotInSafeMode,
    #[error("confirmation phrase does not match")]
    BadConfirmation,
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
    fn entering_safe_mode_is_explicit() {
        let mut state = SafeModeState::default();
        state.enter();
        assert!(state.mode.is_safe());
    }
}
