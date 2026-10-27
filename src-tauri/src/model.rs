use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GoalKind {
    /// Reach a total, e.g. 150 LeetCode problems.
    Counter { target: f64, unit: String },
    /// Cover a distance within a time, e.g. 5 km in 30 min.
    Performance { distance_km: f64, time_min: f64 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EntryData {
    Count { delta: f64 },
    Run { distance_km: f64, time_min: f64 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    /// Unique within its goal, starting at 1. Entries saved before ids existed have 0.
    #[serde(default)]
    pub id: u64,
    pub at: DateTime<Utc>,
    pub data: EntryData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Goal {
    pub id: u64,
    pub name: String,
    pub category: String,
    pub kind: GoalKind,
    pub created_at: DateTime<Utc>,
    pub deadline: Option<NaiveDate>,
    /// Progress made before the goal was tracked here (counter goals only).
    #[serde(default)]
    pub baseline: f64,
    #[serde(default)]
    pub entries: Vec<Entry>,
    pub completed_at: Option<DateTime<Utc>>,
    /// Milestone percentages already rewarded, so they pay out only once.
    #[serde(default)]
    pub milestones_hit: Vec<u8>,
    /// Id of the last entry added; the next one gets this plus one.
    #[serde(default)]
    pub next_entry_id: u64,
    #[serde(default)]
    pub archived_at: Option<DateTime<Utc>>,
}

impl Goal {
    pub fn entries_between(
        &self,
        from: Option<DateTime<Utc>>,
        until: Option<DateTime<Utc>>,
    ) -> impl Iterator<Item = &Entry> + '_ {
        self.entries
            .iter()
            .filter(move |e| from.map_or(true, |f| e.at >= f) && until.map_or(true, |u| e.at < u))
    }

    /// Counter: net units added in the window. Performance: best run score in the window.
    pub fn amount_between(&self, from: Option<DateTime<Utc>>, until: Option<DateTime<Utc>>) -> f64 {
        match self.kind {
            GoalKind::Counter { .. } => self
                .entries_between(from, until)
                .map(|e| match e.data {
                    EntryData::Count { delta } => delta,
                    EntryData::Run { .. } => 0.0,
                })
                .sum(),
            GoalKind::Performance { distance_km, time_min } => self
                .entries_between(from, until)
                .map(|e| match e.data {
                    EntryData::Run { distance_km: d, time_min: t } => run_score(d, t, distance_km, time_min),
                    EntryData::Count { .. } => 0.0,
                })
                .fold(0.0, f64::max),
        }
    }

    pub fn current(&self) -> f64 {
        match self.kind {
            GoalKind::Counter { .. } => self.baseline + self.amount_between(None, None),
            GoalKind::Performance { .. } => self.amount_between(None, None),
        }
    }

    /// Progress in 0..=1.
    pub fn progress(&self) -> f64 {
        match self.kind {
            GoalKind::Counter { target, .. } => (self.current() / target).clamp(0.0, 1.0),
            GoalKind::Performance { .. } => self.current().clamp(0.0, 1.0),
        }
    }

    /// Best run as (distance_km, time_min, score).
    pub fn best_run(&self) -> Option<(f64, f64, f64)> {
        let GoalKind::Performance { distance_km, time_min } = self.kind else {
            return None;
        };
        self.entries
            .iter()
            .filter_map(|e| match e.data {
                EntryData::Run { distance_km: d, time_min: t } => Some((d, t, run_score(d, t, distance_km, time_min))),
                EntryData::Count { .. } => None,
            })
            .max_by(|a, b| a.2.total_cmp(&b.2))
    }

    /// Highest progress this goal has ever reached. Progress XP is only paid above it,
    /// so going down and back up earns nothing.
    pub fn peak_progress(&self) -> f64 {
        match self.kind {
            GoalKind::Counter { target, .. } => {
                let mut total = self.baseline;
                let mut peak = total;
                for e in &self.entries {
                    if let EntryData::Count { delta } = e.data {
                        total += delta;
                        peak = peak.max(total);
                    }
                }
                (peak / target).clamp(0.0, 1.0)
            }
            // The best run never goes down.
            GoalKind::Performance { .. } => self.progress(),
        }
    }

    pub fn is_completed(&self) -> bool {
        self.completed_at.is_some()
    }

    pub fn is_archived(&self) -> bool {
        self.archived_at.is_some()
    }
}

/// How close a run is to the target: distance covered × how close the pace is.
/// Example: 3 km in 20 min vs 5 km in 30 min → (3/5) × (6:00/6:40) = 0.54.
pub fn run_score(distance: f64, time: f64, target_distance: f64, target_time: f64) -> f64 {
    if distance <= 0.0 || time <= 0.0 {
        return 0.0;
    }
    let distance_ratio = (distance / target_distance).min(1.0);
    let pace_ratio = ((target_time / target_distance) / (time / distance)).min(1.0);
    distance_ratio * pace_ratio
}

/// The entry that caused an XP award, so undoing it (or deleting its goal) takes the XP back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntryRef {
    pub goal_id: u64,
    pub entry_id: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XpEvent {
    pub at: DateTime<Utc>,
    pub amount: u64,
    pub reason: String,
    /// Missing for XP earned before awards were linked to entries; that XP is never revoked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<EntryRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnlockedBadge {
    pub id: String,
    pub at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum QuestKind {
    AddToGoal { goal_id: u64 },
    LogRuns { goal_id: u64 },
    ActiveDays,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Quest {
    pub kind: QuestKind,
    pub target: f64,
    pub xp: u64,
    pub completed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_by: Option<EntryRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeeklyQuests {
    /// ISO week key, e.g. "2026-W43".
    pub week: String,
    pub quests: Vec<Quest>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    pub xp: u64,
    pub xp_log: Vec<XpEvent>,
    /// Local days with at least one rewarded update. A day is dropped again when its XP is revoked.
    pub active_days: BTreeSet<NaiveDate>,
    pub badges: Vec<UnlockedBadge>,
    pub quests: Option<WeeklyQuests>,
    pub quests_completed: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Store {
    pub next_id: u64,
    pub goals: Vec<Goal>,
    pub profile: Profile,
}

impl Default for Store {
    fn default() -> Self {
        Self { next_id: 1, goals: Vec::new(), profile: Profile::default() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone};

    fn at(day: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, day, 12, 0, 0).unwrap()
    }

    fn goal(kind: GoalKind, baseline: f64, entries: Vec<(u32, EntryData)>) -> Goal {
        Goal {
            id: 1,
            name: "Test".into(),
            category: "General".into(),
            kind,
            created_at: at(1),
            deadline: None,
            baseline,
            entries: entries
                .into_iter()
                .enumerate()
                .map(|(i, (day, data))| Entry { id: i as u64 + 1, at: at(day), data })
                .collect(),
            completed_at: None,
            milestones_hit: Vec::new(),
            next_entry_id: 0,
            archived_at: None,
        }
    }

    fn counter(target: f64, baseline: f64, deltas: &[(u32, f64)]) -> Goal {
        let entries = deltas.iter().map(|&(day, delta)| (day, EntryData::Count { delta })).collect();
        goal(GoalKind::Counter { target, unit: "problems".into() }, baseline, entries)
    }

    fn runs(runs: &[(u32, f64, f64)]) -> Goal {
        let entries = runs.iter().map(|&(day, d, t)| (day, EntryData::Run { distance_km: d, time_min: t })).collect();
        goal(GoalKind::Performance { distance_km: 5.0, time_min: 30.0 }, 0.0, entries)
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn run_score_matches_documented_example() {
        assert!(close(run_score(3.0, 20.0, 5.0, 30.0), 0.54));
    }

    #[test]
    fn run_score_caps_distance_and_pace_at_one() {
        assert!(close(run_score(10.0, 40.0, 5.0, 30.0), 1.0));
        assert!(close(run_score(5.0, 30.0, 5.0, 30.0), 1.0));
    }

    #[test]
    fn run_score_is_zero_for_empty_runs() {
        assert_eq!(run_score(0.0, 20.0, 5.0, 30.0), 0.0);
        assert_eq!(run_score(3.0, 0.0, 5.0, 30.0), 0.0);
    }

    #[test]
    fn counter_current_includes_baseline_and_net_deltas() {
        let g = counter(10.0, 2.0, &[(2, 3.0), (3, -1.0)]);
        assert!(close(g.current(), 4.0));
        assert!(close(g.progress(), 0.4));
    }

    #[test]
    fn counter_progress_is_clamped() {
        assert_eq!(counter(10.0, 0.0, &[(2, 15.0)]).progress(), 1.0);
        assert_eq!(counter(10.0, 0.0, &[(2, -3.0)]).progress(), 0.0);
    }

    #[test]
    fn amount_between_includes_start_and_excludes_end() {
        let g = counter(100.0, 0.0, &[(1, 1.0), (2, 2.0), (3, 4.0)]);
        assert!(close(g.amount_between(Some(at(2)), Some(at(3))), 2.0));
        assert!(close(g.amount_between(Some(at(2)), None), 6.0));
        assert!(close(g.amount_between(None, Some(at(2))), 1.0));
    }

    #[test]
    fn performance_progress_is_best_run() {
        let g = runs(&[(1, 3.0, 20.0), (2, 2.0, 20.0), (3, 1.0, 5.0)]);
        assert!(close(g.progress(), 0.54));
        let (d, t, score) = g.best_run().unwrap();
        assert_eq!((d, t), (3.0, 20.0));
        assert!(close(score, 0.54));
    }

    #[test]
    fn best_run_is_none_without_runs_or_for_counters() {
        assert!(runs(&[]).best_run().is_none());
        assert!(counter(10.0, 0.0, &[(1, 1.0)]).best_run().is_none());
    }

    #[test]
    fn performance_amount_between_is_best_run_in_window() {
        let g = runs(&[(1, 5.0, 30.0), (3, 3.0, 20.0)]);
        assert!(close(g.amount_between(Some(at(2)), None), 0.54));
    }

    #[test]
    fn peak_progress_remembers_highest_point() {
        let g = counter(10.0, 0.0, &[(1, 5.0), (2, -3.0), (3, 1.0)]);
        assert!(close(g.progress(), 0.3));
        assert!(close(g.peak_progress(), 0.5));
    }

    #[test]
    fn peak_progress_starts_at_baseline_and_is_clamped() {
        assert!(close(counter(10.0, 4.0, &[(1, -2.0)]).peak_progress(), 0.4));
        assert_eq!(counter(10.0, 0.0, &[(1, 20.0), (2, -15.0)]).peak_progress(), 1.0);
    }

    #[test]
    fn peak_progress_of_performance_goal_is_best_run() {
        let g = runs(&[(1, 3.0, 20.0), (2, 1.0, 10.0)]);
        assert!(close(g.peak_progress(), g.progress()));
    }

    #[test]
    fn entries_between_filters_by_time() {
        let g = counter(10.0, 0.0, &[(1, 1.0), (2, 1.0), (3, 1.0)]);
        assert_eq!(g.entries_between(Some(at(2)), None).count(), 2);
        assert_eq!(g.entries_between(Some(at(1) + Duration::hours(1)), Some(at(3))).count(), 1);
    }

    #[test]
    fn save_files_without_new_fields_still_load() {
        let json = r#"{
            "next_id": 2,
            "goals": [{
                "id": 1, "name": "LeetCode", "category": "Coding",
                "kind": {"type": "counter", "target": 150, "unit": "problems"},
                "created_at": "2026-10-01T12:00:00Z", "deadline": null, "completed_at": null,
                "entries": [{"at": "2026-10-02T12:00:00Z", "data": {"type": "count", "delta": 1}}]
            }],
            "profile": {
                "xp": 30,
                "xp_log": [{"at": "2026-10-02T12:00:00Z", "amount": 30, "reason": "Progress on LeetCode"}],
                "quests": {"week": "2026-W40", "quests": [{"kind": {"type": "active_days"}, "target": 4, "xp": 150, "completed": true}]}
            }
        }"#;
        let store: Store = serde_json::from_str(json).unwrap();
        let g = &store.goals[0];
        assert_eq!(g.entries[0].id, 0);
        assert_eq!(g.next_entry_id, 0);
        assert!(g.archived_at.is_none());
        assert!(g.milestones_hit.is_empty());
        assert!(store.profile.xp_log[0].source.is_none());
        assert!(store.profile.quests.unwrap().quests[0].completed_by.is_none());
    }

    #[test]
    fn untagged_fields_are_not_written() {
        let event = XpEvent { at: at(1), amount: 5, reason: "x".into(), source: None };
        assert!(!serde_json::to_string(&event).unwrap().contains("source"));
    }
}
