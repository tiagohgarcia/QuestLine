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

    pub fn is_completed(&self) -> bool {
        self.completed_at.is_some()
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XpEvent {
    pub at: DateTime<Utc>,
    pub amount: u64,
    pub reason: String,
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
    /// Local days with at least one positive update; kept even if goals are deleted.
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
