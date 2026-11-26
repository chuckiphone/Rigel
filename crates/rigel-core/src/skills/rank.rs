//! Skill ranking (roadmap 150).
//!
//! A search result is ordered by four factors rather than by a single opaque number:
//!
//! - **fit**: how well the skill matches the query text.
//! - **freshness**: how recently it was seen, decaying over a 30-day half-life.
//! - **evidence**: whether its advertised numbers have been measured (P2.7).
//! - **install history**: a normalised, logarithmic count of installs.
//!
//! No factor is raw popularity: stars are deliberately excluded, and every score
//! carries the four factors plus the weights that combined them, so a caller can
//! show the user exactly why a skill ranked where it did.

use crate::skills::search::SearchHit;

#[derive(Debug, Clone, PartialEq)]
pub struct RankFactors {
    pub fit: f64,
    pub freshness: f64,
    pub evidence: f64,
    pub install_history: f64,
    pub fit_weight: f64,
    pub freshness_weight: f64,
    pub evidence_weight: f64,
    pub install_history_weight: f64,
    pub total: f64,
}

impl RankFactors {
    pub fn weighted(&self) -> f64 {
        self.fit * self.fit_weight
            + self.freshness * self.freshness_weight
            + self.evidence * self.evidence_weight
            + self.install_history * self.install_history_weight
    }
}

#[derive(Debug, Clone)]
pub struct RankedSkill {
    pub id: String,
    pub name: String,
    pub factors: RankFactors,
}

pub const FIT_WEIGHT: f64 = 0.40;
pub const FRESHNESS_WEIGHT: f64 = 0.20;
pub const EVIDENCE_WEIGHT: f64 = 0.20;
pub const INSTALL_HISTORY_WEIGHT: f64 = 0.20;

pub const FRESHNESS_HALF_LIFE_MS: i64 = 30 * 24 * 60 * 60 * 1000;

/// Score one skill against a query. `now_ms` is the reference instant for freshness.
pub fn score(hit: &SearchHit, query: &str, now_ms: i64) -> RankFactors {
    let fit = fit_score(hit, query);
    let freshness = freshness_score(hit.last_seen_at, now_ms);
    let evidence = evidence_score(hit.unmeasured);
    let install_history = install_history_score(hit.installs);
    let total = fit * FIT_WEIGHT
        + freshness * FRESHNESS_WEIGHT
        + evidence * EVIDENCE_WEIGHT
        + install_history * INSTALL_HISTORY_WEIGHT;
    RankFactors {
        fit,
        freshness,
        evidence,
        install_history,
        fit_weight: FIT_WEIGHT,
        freshness_weight: FRESHNESS_WEIGHT,
        evidence_weight: EVIDENCE_WEIGHT,
        install_history_weight: INSTALL_HISTORY_WEIGHT,
        total,
    }
}

pub fn rank(skills: &[SearchHit], query: &str, now_ms: i64) -> Vec<RankedSkill> {
    let mut ranked: Vec<RankedSkill> = skills
        .iter()
        .map(|hit| RankedSkill {
            id: hit.id.clone(),
            name: hit.name.clone(),
            factors: score(hit, query, now_ms),
        })
        .collect();
    ranked.sort_by(|a, b| {
        b.factors
            .total
            .partial_cmp(&a.factors.total)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.name.cmp(&b.name))
    });
    ranked
}

fn fit_score(hit: &SearchHit, query: &str) -> f64 {
    let terms: Vec<String> = query
        .split_whitespace()
        .map(|t| t.to_lowercase())
        .filter(|t| !t.is_empty())
        .collect();
    if terms.is_empty() {
        return 0.5;
    }
    let haystack = format!(
        "{} {}",
        hit.name.to_lowercase(),
        hit.description.to_lowercase()
    );
    let matched = terms
        .iter()
        .filter(|t| haystack.contains(t.as_str()))
        .count();
    matched as f64 / terms.len() as f64
}

fn freshness_score(last_seen_at: Option<i64>, now_ms: i64) -> f64 {
    match last_seen_at {
        Some(ts) => {
            let age = (now_ms - ts).max(0);
            // Exponential decay with a 30-day half-life.
            2f64.powf(-(age as f64) / (FRESHNESS_HALF_LIFE_MS as f64))
        }
        None => 0.5,
    }
}

fn evidence_score(unmeasured: bool) -> f64 {
    if unmeasured { 0.25 } else { 1.0 }
}

fn install_history_score(installs: Option<i64>) -> f64 {
    match installs {
        Some(n) if n > 0 => {
            let n = n as f64;
            // Logarithmic normalisation so a million installs is not 1000x a thousand.
            (n.ln_1p() / 10.0).min(1.0)
        }
        _ => 0.0,
    }
}

// Aliases for callers that expect different names.
pub type RankResult = RankedSkill;
pub type SkillScore = RankFactors;
pub type SkillRanking = RankedSkill;

pub fn rank_skills(skills: &[SearchHit], query: &str, now_ms: i64) -> Vec<RankedSkill> {
    rank(skills, query, now_ms)
}

pub fn score_skill(hit: &SearchHit, query: &str, now_ms: i64) -> RankFactors {
    score(hit, query, now_ms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::search::SearchHit;

    fn hit(
        id: &str,
        name: &str,
        desc: &str,
        unmeasured: bool,
        installs: Option<i64>,
        last_seen: Option<i64>,
    ) -> SearchHit {
        SearchHit {
            id: id.to_string(),
            name: name.to_string(),
            description: desc.to_string(),
            source: "test".to_string(),
            repo: None,
            path: None,
            url: None,
            grade: Some("A".to_string()),
            grade_scope: None,
            stars: None,
            installs,
            tokens: None,
            cluster_id: None,
            variants: 1,
            last_seen_at: last_seen,
            unmeasured,
            has_body: true,
        }
    }

    #[test]
    fn fit_ranking_prefers_matching_skills() {
        let now = 1_000_000;
        let skills = vec![
            hit(
                "a",
                "pdf parser",
                "parses pdf files",
                false,
                Some(10),
                Some(now),
            ),
            hit("b", "json tool", "handles json", false, Some(10), Some(now)),
        ];
        let ranked = rank(&skills, "pdf", now);
        assert_eq!(ranked[0].name, "pdf parser");
        assert!(ranked[0].factors.fit > ranked[1].factors.fit);
    }

    #[test]
    fn freshness_decays_over_time() {
        let now = 1_000_000;
        let recent = hit("a", "x", "y", false, None, Some(now));
        let old = hit(
            "b",
            "x",
            "y",
            false,
            None,
            Some(now - 60 * FRESHNESS_HALF_LIFE_MS),
        );
        let fr = freshness_score(recent.last_seen_at, now);
        let fo = freshness_score(old.last_seen_at, now);
        assert!(fr > fo);
        assert!(fo < 0.05);
    }

    #[test]
    fn evidence_accumulates_for_measured_skills() {
        let now = 1_000_000;
        let measured = hit("a", "x", "y", false, None, Some(now));
        let unmeasured = hit("b", "x", "y", true, None, Some(now));
        let rm = score(&measured, "x", now);
        let ru = score(&unmeasured, "x", now);
        assert!(rm.evidence > ru.evidence);
    }

    #[test]
    fn install_history_is_normalised_and_transparent() {
        let now = 1_000_000;
        let low = hit("a", "x", "y", false, Some(5), Some(now));
        let high = hit("b", "x", "y", false, Some(5000), Some(now));
        let rl = score(&low, "x", now);
        let rh = score(&high, "x", now);
        assert!(rh.install_history > rl.install_history);
        // Factor breakdown is present.
        assert_eq!(rl.install_history_weight, INSTALL_HISTORY_WEIGHT);
    }

    #[test]
    fn factor_breakdown_is_available() {
        let now = 1_000_000;
        let h = hit("a", "pdf", "parses pdf", false, Some(10), Some(now));
        let f = score(&h, "pdf", now);
        // total equals weighted sum.
        let recomputed = f.fit * f.fit_weight
            + f.freshness * f.freshness_weight
            + f.evidence * f.evidence_weight
            + f.install_history * f.install_history_weight;
        assert!((f.total - recomputed).abs() < 1e-9);
    }

    #[test]
    fn ranking_is_deterministic() {
        let now = 1_000_000;
        let skills = vec![
            hit(
                "a",
                "pdf parser",
                "parses pdf files",
                false,
                Some(10),
                Some(now),
            ),
            hit("b", "json tool", "handles json", false, Some(10), Some(now)),
        ];
        let r1 = rank(&skills, "pdf", now);
        let r2 = rank(&skills, "pdf", now);
        assert_eq!(r1[0].id, r2[0].id);
    }
}
