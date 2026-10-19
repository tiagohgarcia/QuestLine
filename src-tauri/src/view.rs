//! Read-only snapshots of the store, shaped for the UI.

use crate::game::{self, Week};
use crate::model::{Entry, Goal, GoalKind, Store};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use serde::Serialize;

#[derive(Serialize)]
pub struct AppView {
    profile: ProfileView,
    goals: Vec<GoalView>,
    trophies: Vec<GoalView>,
    categories: Vec<String>,
    quests: QuestsView,
    badges: Vec<BadgeView>,
}

#[derive(Serialize)]
struct ProfileView {
    xp: u64,
    level: u32,
    title: &'static str,
    level_start_xp: u64,
    next_level_xp: u64,
    streak: u32,
    best_streak: u32,
    xp_this_week: u64,
    /// XP earned last week up to this same moment.
    ghost_xp_by_now: u64,
    ghost_xp_last_week: u64,
    quests_completed: u32,
}

#[derive(Serialize)]
struct GoalView {
    id: u64,
    name: String,
    category: String,
    kind: &'static str,
    unit: String,
    target: f64,
    current: f64,
    progress: f64,
    target_distance_km: Option<f64>,
    target_time_min: Option<f64>,
    best_run: Option<RunView>,
    deadline: Option<NaiveDate>,
    created_at: DateTime<Utc>,
    completed_at: Option<DateTime<Utc>>,
    days_taken: Option<i64>,
    rival: Option<RivalView>,
    ghost: GhostView,
    recent: Vec<Entry>,
}

#[derive(Serialize)]
struct RunView {
    distance_km: f64,
    time_min: f64,
    score: f64,
}

/// The pace rival moves linearly from creation to the deadline.
#[derive(Serialize)]
struct RivalView {
    expected: f64,
    /// Counter: units ahead (negative = behind). Performance: percentage points.
    ahead: f64,
    days_left: i64,
    per_day_needed: Option<f64>,
}

/// Counter: units added. Performance: best run score.
#[derive(Serialize)]
struct GhostView {
    this_week: f64,
    by_now: f64,
    last_week: f64,
}

#[derive(Serialize)]
struct QuestsView {
    week: String,
    ends_at: DateTime<Utc>,
    quests: Vec<QuestView>,
}

#[derive(Serialize)]
struct QuestView {
    kind: &'static str,
    title: String,
    progress: f64,
    target: f64,
    xp: u64,
    completed: bool,
}

#[derive(Serialize)]
struct BadgeView {
    id: &'static str,
    name: &'static str,
    icon: &'static str,
    description: &'static str,
    unlocked_at: Option<DateTime<Utc>>,
}

pub fn build(store: &Store, now: DateTime<Utc>) -> AppView {
    let week = game::week_of(now);

    let mut goals: Vec<GoalView> = Vec::new();
    let mut trophies: Vec<GoalView> = Vec::new();
    for g in &store.goals {
        let view = goal_view(g, now, &week);
        if g.is_completed() { trophies.push(view) } else { goals.push(view) }
    }
    trophies.sort_by(|a, b| b.completed_at.cmp(&a.completed_at));

    let mut categories: Vec<String> = store.goals.iter().map(|g| g.category.clone()).collect();
    categories.sort();
    categories.dedup();

    AppView {
        profile: profile_view(store, now, &week),
        goals,
        trophies,
        categories,
        quests: quests_view(store, &week),
        badges: game::BADGES
            .iter()
            .map(|b| BadgeView {
                id: b.id,
                name: b.name,
                icon: b.icon,
                description: b.description,
                unlocked_at: store.profile.badges.iter().find(|u| u.id == b.id).map(|u| u.at),
            })
            .collect(),
    }
}

fn profile_view(store: &Store, now: DateTime<Utc>, week: &Week) -> ProfileView {
    let p = &store.profile;
    let last_week_start = week.start - Duration::days(7);
    let (streak, best_streak) = game::streaks(&p.active_days, game::local_date(now));
    let level = game::level_for(p.xp);
    ProfileView {
        xp: p.xp,
        level,
        title: game::title_for(level),
        level_start_xp: game::level_threshold(level),
        next_level_xp: game::level_threshold(level + 1),
        streak,
        best_streak,
        xp_this_week: game::xp_between(p, week.start, week.end),
        ghost_xp_by_now: game::xp_between(p, last_week_start, now - Duration::days(7)),
        ghost_xp_last_week: game::xp_between(p, last_week_start, week.start),
        quests_completed: p.quests_completed,
    }
}

fn goal_view(g: &Goal, now: DateTime<Utc>, week: &Week) -> GoalView {
    let last_week_start = week.start - Duration::days(7);
    let (kind, unit, target, target_distance_km, target_time_min) = match &g.kind {
        GoalKind::Counter { target, unit } => ("counter", unit.clone(), *target, None, None),
        GoalKind::Performance { distance_km, time_min } => {
            ("performance", "km".to_string(), 1.0, Some(*distance_km), Some(*time_min))
        }
    };
    let progress = g.progress();
    let current = g.current();
    let rival = match (g.deadline, g.completed_at) {
        (Some(deadline), None) => Some(rival_view(g, deadline, now, progress, current, target)),
        _ => None,
    };

    GoalView {
        id: g.id,
        name: g.name.clone(),
        category: g.category.clone(),
        kind,
        unit,
        target,
        current,
        progress,
        target_distance_km,
        target_time_min,
        best_run: g.best_run().map(|(distance_km, time_min, score)| RunView { distance_km, time_min, score }),
        deadline: g.deadline,
        created_at: g.created_at,
        completed_at: g.completed_at,
        days_taken: g
            .completed_at
            .map(|c| (game::local_date(c) - game::local_date(g.created_at)).num_days()),
        rival,
        ghost: GhostView {
            this_week: g.amount_between(Some(week.start), Some(week.end)),
            by_now: g.amount_between(Some(last_week_start), Some(now - Duration::days(7))),
            last_week: g.amount_between(Some(last_week_start), Some(week.start)),
        },
        recent: g.entries.iter().rev().take(5).cloned().collect(),
    }
}

fn rival_view(g: &Goal, deadline: NaiveDate, now: DateTime<Utc>, progress: f64, current: f64, target: f64) -> RivalView {
    let end = game::local_midnight(deadline + Duration::days(1));
    let total = (end - g.created_at).num_seconds().max(1) as f64;
    let expected = ((now - g.created_at).num_seconds() as f64 / total).clamp(0.0, 1.0);
    // 0 means the deadline is today.
    let days_left = (deadline - game::local_date(now)).num_days();
    let (ahead, per_day_needed) = match g.kind {
        GoalKind::Counter { .. } => (
            current - expected * target,
            (days_left >= 0).then(|| (target - current).max(0.0) / (days_left + 1) as f64),
        ),
        GoalKind::Performance { .. } => ((progress - expected) * 100.0, None),
    };
    RivalView { expected, ahead, days_left, per_day_needed }
}

fn quests_view(store: &Store, week: &Week) -> QuestsView {
    let quests = store
        .profile
        .quests
        .iter()
        .flat_map(|w| w.quests.iter())
        .map(|q| QuestView {
            kind: match q.kind {
                crate::model::QuestKind::AddToGoal { .. } => "add_to_goal",
                crate::model::QuestKind::LogRuns { .. } => "log_runs",
                crate::model::QuestKind::ActiveDays => "active_days",
            },
            title: game::quest_title(store, q),
            progress: game::quest_progress(store, q, week),
            target: q.target,
            xp: q.xp,
            completed: q.completed,
        })
        .collect();
    QuestsView { week: week.key.clone(), ends_at: week.end, quests }
}
