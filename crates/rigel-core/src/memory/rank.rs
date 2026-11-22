//! Memory fact ranking and decay (roadmap 55).
//!
//! A committed memory file is prose; the unit this module ranks is one **fact** — a non-empty,
//! non-heading line, with its bullet marker and repeated whitespace removed. Each fact is scored
//! from three factors, and the score carries every factor's value, weight and reason so a rank
//! is never a bare number:
//!
//! | factor | weight | what it measures |
//! |---|---|---|
//! | recency | 0.50 | how long since the topic file was written, with a 30-day half-life |
//! | use | 0.30 | how often a recall returned the fact; every four recalls halves the gap to one |
//! | pin | 0.20 | whether the user pinned the fact |
//!
//! **A pinned fact never decays.** Pinning holds the recency value at 1.0 rather than merely
//! adding the 0.20 pin weight, so the same pinned fact scores identically today and a year from
//! now. An unpinned fact loses half its recency value every 30 days.

use std::cmp::Ordering;

use serde::Serialize;
use sha2::{Digest, Sha256};

/// A fact stops losing recency value at half this rate: 30 days.
pub const RECENCY_HALF_LIFE_MS: i64 = 30 * 24 * 60 * 60 * 1_000;

/// Every four recalls halve the distance between the use factor and 1.0.
pub const USE_HALF_LIFE: f64 = 4.0;

pub const WEIGHT_RECENCY: f64 = 0.50;
pub const WEIGHT_USE: f64 = 0.30;
pub const WEIGHT_PIN: f64 = 0.20;

pub const METHOD: &str = "weighted recency/use/pin; 30-day recency half-life";

const DAY_MS: f64 = 86_400_000.0;

/// One committed-memory line, with the state the ranker reads.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Fact {
    /// Stable across a reindex: `agent:topic:hash(normalised text)`.
    pub id: String,
    pub agent_id: String,
    pub topic: String,
    pub text: String,
    pub pinned: bool,
    /// How many times a recall returned this fact.
    pub uses: u64,
    pub last_used_at: Option<i64>,
    /// The topic file's write time.
    pub updated_at: i64,
}

/// One scored factor, with the arithmetic and the reason a human can check.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Factor {
    pub name: &'static str,
    /// 0.0..=1.0.
    pub value: f64,
    pub weight: f64,
    /// `value × weight`.
    pub contribution: f64,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Score {
    pub total: f64,
    pub method: String,
    /// The instant the score was computed, so a decay claim is reproducible.
    pub now: i64,
    pub factors: Vec<Factor>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RankedFact {
    #[serde(flatten)]
    pub fact: Fact,
    pub score: Score,
}

/// Score one fact at one instant. Every factor is present, even when it is zero.
pub fn score(fact: &Fact, now: i64) -> Score {
    let age_ms = (now - fact.updated_at).max(0) as f64;
    let age_days = age_ms / DAY_MS;
    let (recency_value, recency_reason) = if fact.pinned {
        (
            1.0,
            "pinned: recency is held at full value and does not decay".to_string(),
        )
    } else {
        (
            0.5f64.powf(age_ms / RECENCY_HALF_LIFE_MS as f64),
            format!("updated {age_days:.1} days ago; 30-day half-life"),
        )
    };

    let use_value = 1.0 - 0.5f64.powf(fact.uses as f64 / USE_HALF_LIFE);
    let use_reason = if fact.uses == 0 {
        "never recalled".to_string()
    } else {
        format!(
            "recalled {} time(s); every 4 recalls halves the remaining gap",
            fact.uses
        )
    };

    let pin_value = if fact.pinned { 1.0 } else { 0.0 };
    let pin_reason = if fact.pinned {
        "pinned by the user".to_string()
    } else {
        "not pinned".to_string()
    };

    let factors = vec![
        factor("recency", recency_value, WEIGHT_RECENCY, recency_reason),
        factor("use", use_value, WEIGHT_USE, use_reason),
        factor("pin", pin_value, WEIGHT_PIN, pin_reason),
    ];
    let total = factors.iter().map(|factor| factor.contribution).sum();
    Score {
        total,
        method: METHOD.to_string(),
        now,
        factors,
    }
}

fn factor(name: &'static str, value: f64, weight: f64, reason: String) -> Factor {
    Factor {
        name,
        value,
        weight,
        contribution: value * weight,
        reason,
    }
}

/// Score and explain one fact.
pub fn ranked(fact: Fact, now: i64) -> RankedFact {
    let score = score(&fact, now);
    RankedFact { fact, score }
}

/// Score every fact and sort highest first. Ties break by id so a rank is deterministic.
pub fn rank(facts: Vec<Fact>, now: i64) -> Vec<RankedFact> {
    let mut rows: Vec<RankedFact> = facts.into_iter().map(|fact| ranked(fact, now)).collect();
    rows.sort_by(|a, b| {
        b.score
            .total
            .partial_cmp(&a.score.total)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.fact.id.cmp(&b.fact.id))
    });
    rows
}

/// One fact per non-empty, non-heading line, with bullets and repeated whitespace removed.
/// A repeated line is one fact: the same sentence twice is not two pieces of evidence.
pub fn facts_in(body: &str) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    let mut facts = Vec::new();
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.trim_start_matches(['-', '*', '+']).trim();
        let normalised = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if normalised.is_empty() {
            continue;
        }
        if seen.insert(normalised.clone()) {
            facts.push(normalised);
        }
    }
    facts
}

/// The stable identity of a fact's text. Sixteen hex characters are enough to separate the
/// lines one memory file can hold, and short enough to read in a CLI row.
pub fn fact_hash(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    hasher.finalize()[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fact(text: &str, updated_at: i64, pinned: bool, uses: u64) -> Fact {
        Fact {
            id: format!("agent:notes:{}", fact_hash(text)),
            agent_id: "agent".to_string(),
            topic: "notes".to_string(),
            text: text.to_string(),
            pinned,
            uses,
            last_used_at: None,
            updated_at,
        }
    }

    #[test]
    fn every_factor_is_named_weighted_and_explained() {
        let scored = ranked(fact("Uses sqlx.", 1_000, false, 4), 1_000);
        assert_eq!(scored.score.method, METHOD);
        assert_eq!(scored.score.now, 1_000);
        assert_eq!(
            scored
                .score
                .factors
                .iter()
                .map(|factor| factor.name)
                .collect::<Vec<_>>(),
            vec!["recency", "use", "pin"]
        );
        let recency = &scored.score.factors[0];
        let used = &scored.score.factors[1];
        let pin = &scored.score.factors[2];
        assert!((recency.value - 1.0).abs() < 1e-9);
        assert!((recency.contribution - 0.5).abs() < 1e-9);
        // Four recalls half the remaining gap: value 0.5 × weight 0.3 = 0.15.
        assert!((used.value - 0.5).abs() < 1e-9);
        assert!((used.contribution - 0.15).abs() < 1e-9);
        assert_eq!(pin.value, 0.0);
        assert!((scored.score.total - 0.65).abs() < 1e-9);
        let sum: f64 = scored
            .score
            .factors
            .iter()
            .map(|factor| factor.contribution)
            .sum();
        assert!((sum - scored.score.total).abs() < 1e-9);
        assert!(recency.reason.contains("30-day"));
        assert!(used.reason.contains("4 recalls"));
        assert!(pin.reason.contains("not pinned"));
    }

    #[test]
    fn a_pinned_fact_never_decays_while_an_unpinned_one_does() {
        let now = 1_700_000_000_000;
        let year = 365 * 24 * 60 * 60 * 1_000;
        let pinned = fact("Never force-push.", now - 3 * year, true, 0);
        let later = fact("Never force-push.", now - 3 * year, true, 0);
        let today = score(&pinned, now);
        let future = score(&later, now + 5 * year);
        assert_eq!(
            today.total, future.total,
            "pinning is a promise that the score does not move"
        );
        assert_eq!(today.factors, future.factors);

        let unpinned = fact("Never force-push.", now, false, 0);
        let decayed = score(&unpinned, now + year);
        assert!(
            decayed.total < score(&unpinned, now).total,
            "an unpinned fact must lose value with age"
        );
        assert!(decayed.factors[0].value < 0.001, "{decayed:?}");
        assert!(decayed.factors[0].reason.contains("365.0 days"));
    }

    #[test]
    fn use_raises_the_score_but_cannot_exceed_its_weight() {
        let cold = score(&fact("x", 0, false, 0), 0);
        let warm = score(&fact("x", 0, false, 4), 0);
        let hot = score(&fact("x", 0, false, 10_000), 0);
        assert!(cold.total < warm.total);
        assert!(warm.total < hot.total);
        assert!(hot.factors[1].value <= 1.0);
        assert!((hot.factors[1].contribution - WEIGHT_USE).abs() < 1e-6);
    }

    #[test]
    fn facts_are_lines_with_bullets_dropped_and_duplicates_collapsed() {
        let body = "# Notes\n\n- Uses sqlx.\n* Uses sqlx.\n+  Build   with   cargo.\n\n## More\n";
        assert_eq!(
            facts_in(body),
            vec!["Uses sqlx.".to_string(), "Build with cargo.".to_string()]
        );
    }

    #[test]
    fn a_rank_puts_the_pinned_recent_used_fact_first() {
        let now = 1_700_000_000_000;
        let rows = rank(
            vec![
                fact("old", now - 10 * RECENCY_HALF_LIFE_MS, false, 0),
                fact("pinned", now - 10 * RECENCY_HALF_LIFE_MS, true, 0),
                fact("used", now, false, 4),
            ],
            now,
        );
        assert_eq!(rows[0].fact.text, "pinned");
        assert_eq!(rows[1].fact.text, "used");
        assert_eq!(rows[2].fact.text, "old");
    }
}
