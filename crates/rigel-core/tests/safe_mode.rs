//! Safe mode (roadmap 82) integration tests.

use rigel_core::SafeModeState;

#[test]
fn safe_mode_round_trip() {
    let mut state = SafeModeState::new_safe();
    assert!(state.mode.is_safe());
    assert!(!state.mode.tools_enabled());
    assert!(state.leave("leave safe mode").is_ok());
    assert!(!state.mode.is_safe());
    assert!(state.mode.tools_enabled());
}