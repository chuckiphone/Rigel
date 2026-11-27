//! Desktop notification routing: the D90 gate and the Q283 quiet window.
//!
//! Rigel raises one desktop notification per interesting event — an approval waiting on the user,
//! a turn that finished, a turn that failed. The transport is the WebView's Notification API
//! (Tauri 2 exposes it on Windows); this module owns the decision those events pass through, so
//! the window, the headless surfaces and any future emitter route identically.
//!
//! The decision is the same for every kind and rests on three guards, in order:
//!
//! 1. **Watching** — a turn the user is already watching never notifies. The transcript is on
//!    screen, so the event is already visible and repeating it would be noise.
//! 2. **Quiet hours** (Q283) — a local-time window in which every desktop notification stays
//!    silent. The window is `HH:MM` on both ends, the start inclusive and the end exclusive, a
//!    window that crosses midnight wraps, and a start equal to an end means the window is off.
//! 3. **The event gate** — the per-event switch (`notify_approval`, `notify_turn_done`,
//!    `notify_turn_failed`) decides whether the event may notify at all.
//!
//! An event that is muted still records: the transcript and the audit trail carry it, only the
//! desktop notification is suppressed.

use serde::{Deserialize, Serialize};

/// The events a desktop notification can carry, one per kind of turn lifecycle signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NotifyKind {
    Approval,
    Finished,
    Failed,
}

impl NotifyKind {
    /// The stable wire and transcript name of the kind.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Approval => "approval",
            Self::Finished => "finished",
            Self::Failed => "failed",
        }
    }
}

impl std::fmt::Display for NotifyKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for NotifyKind {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "approval" => Ok(Self::Approval),
            "finished" => Ok(Self::Finished),
            "failed" => Ok(Self::Failed),
            other => Err(format!("unknown notification kind: {other}")),
        }
    }
}

/// Where an event is surfaced. Approval questions follow the user into the window; turn lifecycle
/// events also badge the tray so a hidden window still tells the truth.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotifyTarget {
    /// A desktop notification and the tray's attention badge.
    Tray,
    /// The in-app notification row, which the transcript also records.
    InApp,
}

impl NotifyTarget {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tray => "tray",
            Self::InApp => "in_app",
        }
    }
}

impl std::fmt::Display for NotifyTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One row of the routing table, as the settings page shows it. `enabled` and `silent` describe
/// the *current* settings; the target itself never changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotifyRoute {
    pub kind: NotifyKind,
    pub target: NotifyTarget,
    pub enabled: bool,
    /// True when the Q283 window currently silences this row.
    pub silent: bool,
}

/// The explicit table: every event kind has exactly one destination. Nothing is inferred from an
/// event body, so the settings view and the emitter cannot drift apart.
pub const ROUTING_TABLE: [(NotifyKind, NotifyTarget); 3] = [
    (NotifyKind::Approval, NotifyTarget::InApp),
    (NotifyKind::Finished, NotifyTarget::Tray),
    (NotifyKind::Failed, NotifyTarget::Tray),
];

/// Render the table for the current settings and local clock. A row stays visible when muted;
/// `silent` says why it will not fire.
pub fn routing_table(settings: &NotificationSettings, now_minutes: i32) -> Vec<NotifyRoute> {
    let quiet = in_quiet_hours(
        now_minutes,
        &settings.quiet_hours_start,
        &settings.quiet_hours_end,
    );
    ROUTING_TABLE
        .iter()
        .map(|(kind, target)| NotifyRoute {
            kind: *kind,
            target: *target,
            enabled: settings.notify_for(*kind),
            silent: quiet,
        })
        .collect()
}

/// The per-event notification switches plus the Q283 quiet window. Old settings files that carry
/// only the three switches deserialize with the quiet hours defaulting to off.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationSettings {
    /// D90: notify when an approval is waiting on the user.
    pub notify_approval: bool,
    /// D90: notify when a turn finished.
    pub notify_turn_done: bool,
    /// D90: notify when a turn failed.
    pub notify_turn_failed: bool,
    /// Q283: the local-time window (`HH:MM`) in which every desktop notification stays silent.
    /// Empty means the window is off.
    pub quiet_hours_start: String,
    pub quiet_hours_end: String,
}

impl Default for NotificationSettings {
    fn default() -> Self {
        Self {
            notify_approval: true,
            notify_turn_done: true,
            notify_turn_failed: true,
            quiet_hours_start: String::new(),
            quiet_hours_end: String::new(),
        }
    }
}

impl NotificationSettings {
    /// The event switch that governs one kind of notification.
    pub fn notify_for(&self, kind: NotifyKind) -> bool {
        match kind {
            NotifyKind::Approval => self.notify_approval,
            NotifyKind::Finished => self.notify_turn_done,
            NotifyKind::Failed => self.notify_turn_failed,
        }
    }
}

/// Parse a Q283 quiet-hours bound, `HH:MM` local time. Anything that is not a real minute of the
/// day — empty, short, or past 23:59 — is not a bound, so the caller treats the window as off.
pub fn quiet_hours_minutes(value: &str) -> Option<i32> {
    let text = value.trim();
    let mut parts = text.split(':');
    let hour = parts.next()?.parse::<i32>().ok()?;
    let minute = parts.next()?.parse::<i32>().ok()?;
    if parts.next().is_some() || !(0..=23).contains(&hour) || !(0..=59).contains(&minute) {
        return None;
    }
    Some(hour * 60 + minute)
}

/// Q283: is `now_minutes` (minutes since local midnight) inside the quiet window? The start is
/// inclusive, the end exclusive, a window that crosses midnight wraps, and a start equal to an end
/// is the off state rather than "silent all day".
pub fn in_quiet_hours(now_minutes: i32, start: &str, end: &str) -> bool {
    let Some(from) = quiet_hours_minutes(start) else {
        return false;
    };
    let Some(to) = quiet_hours_minutes(end) else {
        return false;
    };
    if from == to {
        return false;
    }
    if from < to {
        now_minutes >= from && now_minutes < to
    } else {
        // The window crosses midnight, so it wraps.
        now_minutes >= from || now_minutes < to
    }
}

/// The one routing decision for a notification event. `watching` is the window's own focus state,
/// so a turn on screen never notifies; `now_minutes` is local minutes since midnight, for the
/// Q283 quiet window.
pub fn route(
    kind: NotifyKind,
    settings: &NotificationSettings,
    watching: bool,
    now_minutes: i32,
) -> Option<NotifyTarget> {
    if watching {
        return None;
    }
    let row = routing_table(settings, now_minutes)
        .into_iter()
        .find(|row| row.kind == kind)?;
    if row.silent || !row.enabled {
        return None;
    }
    Some(row.target)
}

/// Whether one event may fire a notification at all. The routing decision is the single source;
/// both callers and the settings view read the same table.
pub fn should_notify(
    kind: NotifyKind,
    settings: &NotificationSettings,
    watching: bool,
    now_minutes: i32,
) -> bool {
    route(kind, settings, watching, now_minutes).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn settings() -> NotificationSettings {
        NotificationSettings::default()
    }

    fn minutes(value: &str) -> Option<i32> {
        quiet_hours_minutes(value)
    }

    #[test]
    fn the_routing_table_gives_every_kind_one_destination() {
        let rows = routing_table(&settings(), 60 * 10);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].kind, NotifyKind::Approval);
        assert_eq!(rows[0].target, NotifyTarget::InApp);
        assert_eq!(rows[1].target, NotifyTarget::Tray);
        assert_eq!(rows[2].target, NotifyTarget::Tray);
        assert!(rows.iter().all(|row| row.enabled && !row.silent));
        assert_eq!(
            route(NotifyKind::Approval, &settings(), false, 60 * 10),
            Some(NotifyTarget::InApp)
        );
        assert_eq!(
            route(NotifyKind::Failed, &settings(), false, 60 * 10),
            Some(NotifyTarget::Tray)
        );
    }

    #[test]
    fn nothing_routes_during_quiet_hours() {
        let quiet = NotificationSettings {
            quiet_hours_start: "22:00".to_string(),
            quiet_hours_end: "06:00".to_string(),
            ..settings()
        };
        for kind in [
            NotifyKind::Approval,
            NotifyKind::Finished,
            NotifyKind::Failed,
        ] {
            assert_eq!(route(kind, &quiet, false, 23 * 60), None, "{kind}");
            assert!(!should_notify(kind, &quiet, false, 2 * 60), "{kind}");
        }
        let rows = routing_table(&quiet, 23 * 60);
        assert!(rows.iter().all(|row| row.silent));
        assert_eq!(
            route(NotifyKind::Finished, &quiet, false, 12 * 60),
            Some(NotifyTarget::Tray)
        );
    }

    #[test]
    fn every_kind_has_its_own_event_switch() {
        let settings = settings();
        assert!(settings.notify_for(NotifyKind::Approval));
        assert!(settings.notify_for(NotifyKind::Finished));
        assert!(settings.notify_for(NotifyKind::Failed));

        let off = NotificationSettings {
            notify_approval: false,
            ..NotificationSettings::default()
        };
        assert!(!off.notify_for(NotifyKind::Approval));
        assert!(off.notify_for(NotifyKind::Finished));
        assert!(off.notify_for(NotifyKind::Failed));

        let failed = NotificationSettings {
            notify_turn_failed: false,
            ..NotificationSettings::default()
        };
        assert!(failed.notify_for(NotifyKind::Approval));
        assert!(failed.notify_for(NotifyKind::Finished));
        assert!(!failed.notify_for(NotifyKind::Failed));
    }

    #[test]
    fn a_watched_turn_never_notifies_even_when_enabled() {
        let settings = settings();
        assert!(!should_notify(
            NotifyKind::Approval,
            &settings,
            true,
            60 * 10,
        ));
        assert!(!should_notify(
            NotifyKind::Finished,
            &settings,
            true,
            60 * 10
        ));
        assert!(!should_notify(NotifyKind::Failed, &settings, true, 60 * 10));
    }

    #[test]
    fn a_muted_event_still_records_but_does_not_notify() {
        let settings = NotificationSettings {
            notify_turn_done: false,
            ..NotificationSettings::default()
        };
        assert!(!should_notify(
            NotifyKind::Finished,
            &settings,
            false,
            60 * 10
        ));
        assert!(should_notify(
            NotifyKind::Approval,
            &settings,
            false,
            60 * 10
        ));
        assert!(should_notify(NotifyKind::Failed, &settings, false, 60 * 10));
    }

    #[test]
    fn the_default_settings_notify_all_three_events() {
        let settings = settings();
        assert!(settings.notify_for(NotifyKind::Approval));
        assert!(settings.notify_for(NotifyKind::Finished));
        assert!(settings.notify_for(NotifyKind::Failed));
    }

    #[test]
    fn quiet_hours_accept_a_real_bound_and_reject_a_malformed_one() {
        assert_eq!(minutes("09:30"), Some(570));
        assert_eq!(minutes(""), None);
        assert_eq!(minutes("9:30"), Some(570));
        assert_eq!(minutes("24:00"), None);
        assert_eq!(minutes("09:60"), None);
        assert_eq!(minutes("09:30:00"), None);
    }

    #[test]
    fn quiet_hours_are_half_open_and_off_when_equal() {
        // A window that is off: start == end.
        assert!(!in_quiet_hours(60 * 10, "09:00", "09:00"));
        // 09:00 inclusive, 17:00 exclusive.
        assert!(in_quiet_hours(60 * 9, "09:00", "17:00"));
        assert!(!in_quiet_hours(60 * 17, "09:00", "17:00"));
        assert!(in_quiet_hours(60 * 16 + 59, "09:00", "17:00"));
        // An invalid window is off.
        assert!(!in_quiet_hours(60 * 12, "", "17:00"));
        assert!(!in_quiet_hours(60 * 12, "17:00", "09:00"));
    }

    #[test]
    fn quiet_hours_wrap_when_the_window_crosses_midnight() {
        // 22:00-07:00 wraps: notify before 07:00 and from 22:00 onward are quiet.
        assert!(in_quiet_hours(60 * 22, "22:00", "07:00"));
        assert!(in_quiet_hours(60 * 3, "22:00", "07:00"));
        assert!(in_quiet_hours(60 * 6, "22:00", "07:00"));
        assert!(!in_quiet_hours(60 * 21 + 59, "22:00", "07:00"));
    }

    #[test]
    fn quiet_hours_do_not_fire_outside_the_window() {
        assert!(!in_quiet_hours(60 * 8, "09:00", "17:00"));
        assert!(!in_quiet_hours(60 * 17, "09:00", "17:00"));
        assert!(!in_quiet_hours(60 * 23, "09:00", "17:00"));
    }

    #[test]
    fn an_enabled_event_notifies_outside_the_quiet_window_and_not_during_it() {
        let settings = settings();
        assert!(should_notify(
            NotifyKind::Approval,
            &settings,
            false,
            60 * 10
        ));
        assert!(should_notify(
            NotifyKind::Approval,
            &settings,
            false,
            60 * 12
        ));
        assert!(should_notify(
            NotifyKind::Approval,
            &settings,
            false,
            60 * 23
        ));

        let quiet = NotificationSettings {
            quiet_hours_start: "09:00".to_string(),
            quiet_hours_end: "17:00".to_string(),
            ..NotificationSettings::default()
        };
        assert!(!should_notify(NotifyKind::Finished, &quiet, false, 60 * 12));
        assert!(should_notify(NotifyKind::Finished, &quiet, false, 60 * 8));
    }

    #[test]
    fn the_kind_names_round_trip_through_the_string_form() {
        assert_eq!(NotifyKind::Approval.as_str(), "approval");
        assert_eq!(NotifyKind::from_str("failed").unwrap(), NotifyKind::Failed);
        assert!(NotifyKind::from_str("nope").is_err());
    }
}
