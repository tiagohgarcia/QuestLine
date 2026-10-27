//! Read-only snapshots of the store, shaped for the UI.

use crate::game::{self, Week};
use crate::model::{Entry, EntryRef, Goal, GoalKind, Profile, Store};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use serde::Serialize;

#[derive(Serialize)]
pub struct AppView {
    profile: ProfileView,
    goals: Vec<GoalView>,
    trophies: Vec<GoalView>,
    archived: Vec<GoalView>,
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
    archived_at: Option<DateTime<Utc>>,
    /// XP that deleting this goal would take back.
    xp_earned: u64,
    /// XP that undoing the last entry would take back; None when there is nothing to undo.
    undo_xp: Option<u64>,
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
    let mut archived: Vec<GoalView> = Vec::new();
    for g in &store.goals {
        let view = goal_view(g, &store.profile, now, &week);
        if g.is_archived() {
            archived.push(view)
        } else if g.is_completed() {
            trophies.push(view)
        } else {
            goals.push(view)
        }
    }
    trophies.sort_by(|a, b| b.completed_at.cmp(&a.completed_at));
    archived.sort_by(|a, b| b.archived_at.cmp(&a.archived_at));

    let mut categories: Vec<String> = store.goals.iter().map(|g| g.category.clone()).collect();
    categories.sort();
    categories.dedup();

    AppView {
        profile: profile_view(store, now, &week),
        goals,
        trophies,
        archived,
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

fn xp_from(profile: &Profile, matches: impl Fn(&EntryRef) -> bool) -> u64 {
    profile.xp_log.iter().filter(|e| e.source.as_ref().map_or(false, &matches)).map(|e| e.amount).sum()
}

fn goal_view(g: &Goal, profile: &Profile, now: DateTime<Utc>, week: &Week) -> GoalView {
    let last_week_start = week.start - Duration::days(7);
    let (kind, unit, target, target_distance_km, target_time_min) = match &g.kind {
        GoalKind::Counter { target, unit } => ("counter", unit.clone(), *target, None, None),
        GoalKind::Performance { distance_km, time_min } => {
            ("performance", "km".to_string(), 1.0, Some(*distance_km), Some(*time_min))
        }
    };
    let progress = g.progress();
    let current = g.current();
    let rival = match (g.deadline, g.completed_at, g.archived_at) {
        (Some(deadline), None, None) => Some(rival_view(g, deadline, now, progress, current, target)),
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
        archived_at: g.archived_at,
        xp_earned: xp_from(profile, |s| s.goal_id == g.id),
        undo_xp: g.entries.last().map(|e| xp_from(profile, |s| *s == EntryRef { goal_id: g.id, entry_id: e.id })),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{EntryData, Profile, Quest, QuestKind, UnlockedBadge, WeeklyQuests, XpEvent};
    use chrono::{Local, TimeZone};

    fn local(d: u32, h: u32) -> DateTime<Utc> {
        Local.with_ymd_and_hms(2026, 10, d, h, 0, 0).single().unwrap().with_timezone(&Utc)
    }

    fn date(d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, d).unwrap()
    }

    fn goal(id: u64, category: &str, target: f64) -> Goal {
        Goal {
            id,
            name: format!("Goal {id}"),
            category: category.into(),
            kind: GoalKind::Counter { target, unit: "pages".into() },
            created_at: local(1, 12),
            deadline: None,
            baseline: 0.0,
            entries: Vec::new(),
            completed_at: None,
            milestones_hit: Vec::new(),
            next_entry_id: 0,
            archived_at: None,
        }
    }

    fn entry(id: u64, at: DateTime<Utc>, delta: f64) -> Entry {
        Entry { id, at, data: EntryData::Count { delta } }
    }

    fn xp(at: DateTime<Utc>, amount: u64, source: Option<(u64, u64)>) -> XpEvent {
        XpEvent { at, amount, reason: String::new(), source: source.map(|(goal_id, entry_id)| EntryRef { goal_id, entry_id }) }
    }

    fn ids(views: &[GoalView]) -> Vec<u64> {
        views.iter().map(|g| g.id).collect()
    }

    #[test]
    fn goals_are_split_into_board_trophies_and_archive() {
        let mut store = Store::default();
        let active = goal(1, "A", 10.0);
        let mut done_early = goal(2, "A", 10.0);
        done_early.completed_at = Some(local(3, 12));
        let mut done_late = goal(3, "A", 10.0);
        done_late.completed_at = Some(local(5, 12));
        let mut archived_done = goal(4, "A", 10.0);
        archived_done.completed_at = Some(local(4, 12));
        archived_done.archived_at = Some(local(6, 12));
        let mut archived_late = goal(5, "A", 10.0);
        archived_late.archived_at = Some(local(7, 12));
        store.goals = vec![active, done_early, done_late, archived_done, archived_late];

        let view = build(&store, local(14, 12));
        assert_eq!(ids(&view.goals), [1]);
        assert_eq!(ids(&view.trophies), [3, 2]);
        assert_eq!(ids(&view.archived), [5, 4]);
    }

    #[test]
    fn categories_are_sorted_and_unique() {
        let mut store = Store::default();
        store.goals = vec![goal(1, "Run", 1.0), goal(2, "Code", 1.0), goal(3, "Run", 1.0)];
        assert_eq!(build(&store, local(14, 12)).categories, ["Code", "Run"]);
    }

    #[test]
    fn goal_shows_xp_at_stake() {
        let mut store = Store::default();
        let mut g = goal(1, "A", 10.0);
        g.entries = vec![entry(1, local(13, 12), 1.0), entry(2, local(14, 12), 1.0)];
        store.goals = vec![g, goal(2, "A", 10.0)];
        store.profile.xp_log = vec![
            xp(local(13, 12), 30, Some((1, 1))),
            xp(local(14, 12), 12, Some((1, 2))),
            xp(local(14, 12), 120, Some((1, 2))),
            xp(local(14, 12), 5, Some((2, 1))),
            xp(local(12, 12), 100, None),
        ];

        let view = build(&store, local(14, 12));
        assert_eq!(view.goals[0].xp_earned, 162);
        assert_eq!(view.goals[0].undo_xp, Some(132));
        assert_eq!(view.goals[1].xp_earned, 5);
        assert_eq!(view.goals[1].undo_xp, None, "nothing to undo");
    }

    #[test]
    fn untagged_last_entry_can_be_undone_for_free() {
        let mut store = Store::default();
        let mut g = goal(1, "A", 10.0);
        g.entries = vec![entry(0, local(13, 12), 1.0)];
        store.goals = vec![g];
        store.profile.xp_log = vec![xp(local(13, 12), 30, None)];
        assert_eq!(build(&store, local(14, 12)).goals[0].undo_xp, Some(0));
    }

    #[test]
    fn rival_tracks_linear_pace_to_deadline() {
        let mut store = Store::default();
        let mut g = goal(1, "A", 10.0);
        // Ten days from the 10th to the end of the 19th; the 15th is halfway.
        g.created_at = local(10, 0);
        g.deadline = Some(date(19));
        g.entries = vec![entry(1, local(11, 12), 2.0)];
        store.goals = vec![g];

        let view = build(&store, local(15, 0));
        let rival = view.goals[0].rival.as_ref().unwrap();
        assert!((rival.expected - 0.5).abs() < 1e-9);
        assert!((rival.ahead + 3.0).abs() < 1e-9);
        assert_eq!(rival.days_left, 4);
        assert!((rival.per_day_needed.unwrap() - 1.6).abs() < 1e-9);
    }

    #[test]
    fn rival_needs_nothing_per_day_once_overdue() {
        let mut store = Store::default();
        let mut g = goal(1, "A", 10.0);
        g.deadline = Some(date(10));
        store.goals = vec![g];
        let view = build(&store, local(14, 12));
        let rival = view.goals[0].rival.as_ref().unwrap();
        assert_eq!(rival.days_left, -4);
        assert!(rival.per_day_needed.is_none());
        assert_eq!(rival.expected, 1.0);
    }

    #[test]
    fn no_rival_without_deadline_or_when_finished_or_archived() {
        let mut store = Store::default();
        let plain = goal(1, "A", 10.0);
        let mut done = goal(2, "A", 10.0);
        done.deadline = Some(date(20));
        done.completed_at = Some(local(5, 12));
        let mut archived = goal(3, "A", 10.0);
        archived.deadline = Some(date(20));
        archived.archived_at = Some(local(5, 12));
        store.goals = vec![plain, done, archived];

        let view = build(&store, local(14, 12));
        assert!(view.goals[0].rival.is_none());
        assert!(view.trophies[0].rival.is_none());
        assert!(view.archived[0].rival.is_none());
    }

    #[test]
    fn goal_ghost_compares_with_last_week_so_far() {
        let mut store = Store::default();
        let mut g = goal(1, "A", 100.0);
        // Last week: Tuesday 6th and Friday 9th. This week: Monday 12th. Now: Wednesday 14th.
        g.entries = vec![entry(1, local(6, 12), 3.0), entry(2, local(9, 12), 4.0), entry(3, local(12, 12), 2.0)];
        store.goals = vec![g];

        let ghost = &build(&store, local(14, 12)).goals[0].ghost;
        assert_eq!((ghost.this_week, ghost.by_now, ghost.last_week), (2.0, 3.0, 7.0));
    }

    #[test]
    fn profile_shows_level_streak_and_ghost_xp() {
        let mut store = Store::default();
        store.profile = Profile {
            xp: 150,
            xp_log: vec![xp(local(6, 12), 40, None), xp(local(9, 12), 60, None), xp(local(13, 12), 50, None)],
            active_days: [date(12), date(13)].into(),
            ..Profile::default()
        };

        let p = build(&store, local(14, 12)).profile;
        assert_eq!((p.level, p.title), (2, "Rookie"));
        assert_eq!((p.level_start_xp, p.next_level_xp), (100, 300));
        assert_eq!((p.streak, p.best_streak), (2, 2));
        assert_eq!(p.xp_this_week, 50);
        assert_eq!(p.ghost_xp_by_now, 40);
        assert_eq!(p.ghost_xp_last_week, 100);
    }

    #[test]
    fn quests_show_titles_and_progress() {
        let mut store = Store::default();
        let mut g = goal(1, "A", 100.0);
        g.entries = vec![entry(1, local(13, 12), 3.0)];
        store.goals = vec![g];
        store.profile.active_days = [date(13)].into();
        let quest = |kind, target| Quest { kind, target, xp: 120, completed: false, completed_by: None };
        store.profile.quests = Some(WeeklyQuests {
            week: "2026-W42".into(),
            quests: vec![quest(QuestKind::AddToGoal { goal_id: 1 }, 5.0), quest(QuestKind::ActiveDays, 4.0)],
        });

        let view = build(&store, local(14, 12));
        assert_eq!(view.quests.week, "2026-W42");
        let q = &view.quests.quests;
        assert_eq!((q[0].kind, q[0].title.as_str(), q[0].progress), ("add_to_goal", "Add 5 pages to Goal 1", 3.0));
        assert_eq!((q[1].kind, q[1].progress), ("active_days", 1.0));
    }

    #[test]
    fn badges_list_every_badge_with_unlock_time() {
        let mut store = Store::default();
        store.profile.badges = vec![UnlockedBadge { id: "first_step".into(), at: local(3, 12) }];
        let badges = build(&store, local(14, 12)).badges;
        assert_eq!(badges.len(), game::BADGES.len());
        assert_eq!(badges.iter().find(|b| b.id == "first_step").unwrap().unlocked_at, Some(local(3, 12)));
        assert!(badges.iter().filter(|b| b.id != "first_step").all(|b| b.unlocked_at.is_none()));
    }
}
