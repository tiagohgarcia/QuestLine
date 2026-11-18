//! Focus timer rules: sessions earn XP in 25-minute blocks, and focus goals pay out once per week or month.
//! Like `game::apply_entry`, every change here goes through the shared XP log, so revoking works the same way.

use crate::game::{self, Flags, GameEvent};
use crate::model::{FocusSession, GoalKind, Period, Store, XpSource};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use std::collections::BTreeSet;

const BLOCK_SECS: i64 = 25 * 60;
const XP_PER_BLOCK: u64 = 10;
const XP_WEEK_TARGET: u64 = 150;
const XP_MONTH_TARGET: u64 = 400;
/// Shorter sessions are treated as an accidental start and dropped.
const MIN_SESSION_SECS: i64 = 5;

/// "2h 05m", "25m", "40s".
pub fn fmt_duration(secs: i64) -> String {
    let (h, m) = (secs / 3600, secs % 3600 / 60);
    match (h, m) {
        (0, 0) => format!("{secs}s"),
        (0, m) => format!("{m}m"),
        (h, m) => format!("{h}h {m:02}m"),
    }
}

pub fn session_xp(secs: i64) -> u64 {
    (secs / BLOCK_SECS).max(0) as u64 * XP_PER_BLOCK
}

pub fn target_xp(period: Period) -> u64 {
    match period {
        Period::Week => XP_WEEK_TARGET,
        Period::Month => XP_MONTH_TARGET,
    }
}

pub fn start(store: &mut Store, now: DateTime<Utc>) -> Result<Vec<GameEvent>, String> {
    if store.focus.running_since.is_some() {
        return Err("The timer is already running".into());
    }
    store.focus.running_since = Some(now);
    Ok(Vec::new())
}

/// Stops the timer and turns the time since start into a session.
pub fn pause(store: &mut Store, now: DateTime<Utc>) -> Result<Vec<GameEvent>, String> {
    let start = store.focus.running_since.take().ok_or("The timer is not running")?;
    if (now - start).num_seconds() < MIN_SESSION_SECS {
        return Ok(Vec::new());
    }
    store.focus.next_id += 1;
    let id = store.focus.next_id;
    store.focus.sessions.push(FocusSession { id, start, end: now });

    let mut events = Vec::new();
    let earned = award_session(store, id, &mut events);
    if earned > 0 {
        events.insert(0, GameEvent::Xp { amount: earned, lucky: false });
    }
    after_change(store, id, now, &mut events);
    Ok(events)
}

/// Changes a finished session's times, e.g. when the timer was left running by mistake.
pub fn edit_session(
    store: &mut Store,
    id: u64,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<Vec<GameEvent>, String> {
    if start >= end {
        return Err("The session must end after it starts".into());
    }
    if end > now {
        return Err("The session can't end in the future".into());
    }
    if store.focus.running_since.map_or(false, |r| end > r) {
        return Err("The session overlaps the running timer".into());
    }
    let focus = &mut store.focus;
    if focus.sessions.iter().any(|s| s.id != id && s.start < end && start < s.end) {
        return Err("The session overlaps another session".into());
    }
    let session = focus.sessions.iter_mut().find(|s| s.id == id).ok_or("Session not found")?;
    let before = session_xp(session.seconds());
    session.start = start;
    session.end = end;
    focus.sessions.sort_by_key(|s| s.start);

    let mut events = Vec::new();
    // Revoking also reopens a quest this session completed; `after_change` checks it again.
    game::revoke_xp(&mut store.profile, |s| *s == XpSource::Focus { focus_session: id });
    let after = award_session(store, id, &mut events);
    if after > before {
        events.insert(0, GameEvent::Xp { amount: after - before, lucky: false });
    } else if before > after {
        events.insert(0, GameEvent::XpRevoked { reason: "Edited a focus session".into(), amount: before - after });
    }
    after_change(store, id, now, &mut events);
    Ok(events)
}

pub fn delete_session(store: &mut Store, id: u64, now: DateTime<Utc>) -> Result<Vec<GameEvent>, String> {
    let index = store.focus.sessions.iter().position(|s| s.id == id).ok_or("Session not found")?;
    let session = store.focus.sessions.remove(index);
    let amount = game::revoke_xp(&mut store.profile, |s| *s == XpSource::Focus { focus_session: id });
    let reason = format!("Deleted {} of focus", fmt_duration(session.seconds()));
    let mut events = vec![GameEvent::XpRevoked { reason, amount }];
    sync_goals(store, now, &mut events);
    Ok(events)
}

/// Pays the session's block XP, dated at its end so the streak lands on the day it happened.
/// Returns the XP awarded.
fn award_session(store: &mut Store, id: u64, events: &mut Vec<GameEvent>) -> u64 {
    let Some(session) = store.focus.sessions.iter().find(|s| s.id == id) else {
        return 0;
    };
    let (secs, end) = (session.seconds(), session.end);
    let xp = session_xp(secs);
    if xp > 0 {
        let reason = format!("Focused {}", fmt_duration(secs));
        game::award_xp(&mut store.profile, xp, reason, XpSource::Focus { focus_session: id }, end, events);
        store.profile.active_days.insert(game::local_date(end));
    }
    xp
}

fn after_change(store: &mut Store, id: u64, now: DateTime<Utc>, events: &mut Vec<GameEvent>) {
    game::check_quests(store, XpSource::Focus { focus_session: id }, now, events);
    sync_goals(store, now, events);
    game::check_badges(store, &Flags::default(), now, events);
}

/// Makes focus goal rewards match the finished sessions: every week or month (since the goal's
/// creation) that reached the target has its reward, and no other does. The running session
/// doesn't count until it is paused.
pub fn sync_goals(store: &mut Store, now: DateTime<Utc>, events: &mut Vec<GameEvent>) {
    let today = game::local_date(now);
    let goals: Vec<(u64, String, f64, Period, NaiveDate)> = store
        .goals
        .iter()
        .filter(|g| !g.is_archived())
        .filter_map(|g| match g.kind {
            GoalKind::Focus { hours, period } => {
                Some((g.id, g.name.clone(), hours, period, game::local_date(g.created_at)))
            }
            _ => None,
        })
        .collect();

    for (goal_id, name, hours, period, created) in goals {
        let target_secs = (hours * 3600.0).round() as i64;
        let mut reached = BTreeSet::new();
        let mut span = game::span_of(period, created);
        while span.first_day <= today {
            if store.focus.seconds_between(span.start, span.end, None) >= target_secs {
                reached.insert(span.first_day);
            }
            span = game::span_of(period, game::local_date(span.end));
        }

        let rewarded: BTreeSet<NaiveDate> = store
            .profile
            .xp_log
            .iter()
            .filter_map(|e| match e.source {
                Some(XpSource::FocusGoal { goal_id: g, period_start }) if g == goal_id => Some(period_start),
                _ => None,
            })
            .collect();

        for &period_start in reached.difference(&rewarded) {
            let xp = target_xp(period);
            let source = XpSource::FocusGoal { goal_id, period_start };
            events.push(GameEvent::FocusTargetReached { goal: name.clone(), period, xp });
            game::award_xp(&mut store.profile, xp, format!("Reached {name}"), source, now, events);
        }
        for &period_start in rewarded.difference(&reached) {
            let source = XpSource::FocusGoal { goal_id, period_start };
            let amount = game::revoke_xp(&mut store.profile, |s| *s == source);
            events.push(GameEvent::XpRevoked { reason: format!("{name} no longer reached"), amount });
        }
    }
}

/// Focused seconds per local day, from finished sessions. Sessions are split at midnight.
pub fn seconds_per_day(store: &Store) -> Vec<(NaiveDate, i64)> {
    let mut days: std::collections::BTreeMap<NaiveDate, i64> = Default::default();
    for s in &store.focus.sessions {
        let mut day = game::local_date(s.start);
        while game::local_midnight(day) < s.end {
            let next = day + Duration::days(1);
            let secs = (s.end.min(game::local_midnight(next)) - s.start.max(game::local_midnight(day))).num_seconds();
            if secs > 0 {
                *days.entry(day).or_default() += secs;
            }
            day = next;
        }
    }
    days.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Goal, Quest, QuestKind, WeeklyQuests};
    use chrono::{Local, TimeZone};

    // Oct 12-18 2026 is ISO week 42 (Monday to Sunday).
    fn local(d: u32, h: u32, m: u32) -> DateTime<Utc> {
        Local.with_ymd_and_hms(2026, 10, d, h, m, 0).single().unwrap().with_timezone(&Utc)
    }

    fn date(d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, d).unwrap()
    }

    /// A store whose only quest never completes, so quest XP doesn't mix into the numbers.
    fn quiet_store() -> Store {
        let mut store = Store::default();
        store.profile.quests = Some(WeeklyQuests {
            week: "2026-W42".into(),
            quests: vec![Quest {
                kind: QuestKind::AddToGoal { goal_id: 999 },
                target: 1.0,
                xp: 120,
                completed: false,
                completed_by: None,
            }],
        });
        store
    }

    fn focus_goal(store: &mut Store, hours: f64, period: Period, created: DateTime<Utc>) -> u64 {
        let id = store.next_id;
        store.next_id += 1;
        store.goals.push(Goal {
            id,
            name: format!("Focus {hours}h"),
            category: "Work".into(),
            kind: GoalKind::Focus { hours, period },
            created_at: created,
            deadline: None,
            baseline: 0.0,
            entries: Vec::new(),
            completed_at: None,
            milestones_hit: Vec::new(),
            next_entry_id: 0,
            archived_at: None,
        });
        id
    }

    /// Runs the timer from `from` to `to` and returns the pause events.
    fn session(store: &mut Store, from: DateTime<Utc>, to: DateTime<Utc>) -> Vec<GameEvent> {
        start(store, from).unwrap();
        pause(store, to).unwrap()
    }

    fn assert_xp_consistent(store: &Store) {
        let logged: u64 = store.profile.xp_log.iter().map(|e| e.amount).sum();
        assert_eq!(store.profile.xp, logged);
    }

    fn reached(events: &[GameEvent]) -> usize {
        events.iter().filter(|e| matches!(e, GameEvent::FocusTargetReached { .. })).count()
    }

    #[test]
    fn durations_are_formatted() {
        assert_eq!(fmt_duration(40), "40s");
        assert_eq!(fmt_duration(25 * 60), "25m");
        assert_eq!(fmt_duration(2 * 3600 + 5 * 60 + 30), "2h 05m");
    }

    #[test]
    fn xp_is_paid_per_full_block() {
        assert_eq!(session_xp(24 * 60 + 59), 0);
        assert_eq!(session_xp(25 * 60), 10);
        assert_eq!(session_xp(74 * 60), 20);
    }

    #[test]
    fn start_and_pause_record_a_session() {
        let mut store = quiet_store();
        start(&mut store, local(14, 9, 0)).unwrap();
        assert!(start(&mut store, local(14, 9, 1)).is_err());
        let events = pause(&mut store, local(14, 10, 0)).unwrap();

        assert!(store.focus.running_since.is_none());
        assert_eq!(store.focus.sessions.len(), 1);
        assert_eq!(store.focus.sessions[0].seconds(), 3600);
        assert!(matches!(events[0], GameEvent::Xp { amount: 20, lucky: false }));
        assert_eq!(store.profile.xp, 20);
        assert!(store.profile.active_days.contains(&date(14)));
        assert!(pause(&mut store, local(14, 11, 0)).is_err());
    }

    #[test]
    fn short_sessions_earn_nothing_and_keep_the_streak_unchanged() {
        let mut store = quiet_store();
        let events = session(&mut store, local(14, 9, 0), local(14, 9, 20));
        assert_eq!(store.focus.sessions.len(), 1);
        assert_eq!(store.profile.xp, 0);
        assert!(store.profile.active_days.is_empty());
        assert!(!events.iter().any(|e| matches!(e, GameEvent::Xp { .. })));
    }

    #[test]
    fn accidental_clicks_are_dropped() {
        let mut store = quiet_store();
        session(&mut store, local(14, 9, 0), local(14, 9, 0) + Duration::seconds(2));
        assert!(store.focus.sessions.is_empty());
        assert!(store.focus.running_since.is_none());
    }

    #[test]
    fn xp_is_dated_at_session_end() {
        let mut store = quiet_store();
        session(&mut store, local(13, 23, 0), local(14, 0, 30));
        assert_eq!(store.profile.xp_log[0].at, local(14, 0, 30));
        assert!(store.profile.active_days.contains(&date(14)));
        assert!(!store.profile.active_days.contains(&date(13)));
    }

    #[test]
    fn sessions_are_split_at_midnight() {
        let mut store = quiet_store();
        session(&mut store, local(13, 23, 0), local(14, 0, 30));
        session(&mut store, local(14, 9, 0), local(14, 10, 0));
        assert_eq!(seconds_per_day(&store), vec![(date(13), 3600), (date(14), 30 * 60 + 3600)]);
    }

    #[test]
    fn seconds_between_clips_and_counts_running_session_only_with_now() {
        let mut store = quiet_store();
        session(&mut store, local(14, 9, 0), local(14, 11, 0));
        start(&mut store, local(14, 12, 0)).unwrap();
        let focus = &store.focus;
        assert_eq!(focus.seconds_between(local(14, 10, 0), local(14, 23, 0), None), 3600);
        assert_eq!(focus.seconds_between(local(14, 10, 0), local(14, 23, 0), Some(local(14, 12, 30))), 3600 + 1800);
    }

    #[test]
    fn editing_a_session_recalculates_its_xp() {
        let mut store = quiet_store();
        session(&mut store, local(14, 9, 0), local(14, 12, 0));
        assert_eq!(store.profile.xp, 70);

        // Forgot to pause: it really ended at 10:00.
        let events = edit_session(&mut store, 1, local(14, 9, 0), local(14, 10, 0), local(14, 13, 0)).unwrap();
        assert!(matches!(events[0], GameEvent::XpRevoked { amount: 50, .. }));
        assert_eq!(store.profile.xp, 20);
        assert_xp_consistent(&store);

        let events = edit_session(&mut store, 1, local(14, 8, 0), local(14, 10, 0), local(14, 13, 0)).unwrap();
        assert!(matches!(events[0], GameEvent::Xp { amount: 20, .. }));
        assert_eq!(store.profile.xp, 40);
    }

    #[test]
    fn editing_moves_the_active_day() {
        let mut store = quiet_store();
        session(&mut store, local(14, 9, 0), local(14, 10, 0));
        edit_session(&mut store, 1, local(13, 9, 0), local(13, 10, 0), local(14, 11, 0)).unwrap();
        assert!(store.profile.active_days.contains(&date(13)));
        assert!(!store.profile.active_days.contains(&date(14)));
    }

    #[test]
    fn invalid_edits_are_rejected() {
        let mut store = quiet_store();
        session(&mut store, local(14, 9, 0), local(14, 10, 0));
        session(&mut store, local(14, 11, 0), local(14, 12, 0));
        start(&mut store, local(14, 13, 0)).unwrap();
        let now = local(14, 14, 0);
        assert!(edit_session(&mut store, 1, local(14, 10, 0), local(14, 9, 0), now).is_err());
        assert!(edit_session(&mut store, 1, local(14, 9, 0), local(14, 15, 0), now).is_err());
        assert!(edit_session(&mut store, 1, local(14, 9, 0), local(14, 11, 30), now).is_err());
        assert!(edit_session(&mut store, 2, local(14, 11, 0), local(14, 13, 30), now).is_err());
        assert!(edit_session(&mut store, 9, local(14, 9, 0), local(14, 9, 30), now).is_err());
        assert_eq!(store.focus.sessions[0].end, local(14, 10, 0));
    }

    #[test]
    fn deleting_a_session_revokes_its_xp_and_streak_day() {
        let mut store = quiet_store();
        session(&mut store, local(14, 9, 0), local(14, 10, 0));
        let events = delete_session(&mut store, 1, local(14, 11, 0)).unwrap();
        assert!(matches!(events[0], GameEvent::XpRevoked { amount: 20, .. }));
        assert!(store.focus.sessions.is_empty());
        assert_eq!(store.profile.xp, 0);
        assert!(store.profile.active_days.is_empty());
    }

    #[test]
    fn weekly_goal_pays_once_per_week() {
        let mut store = quiet_store();
        let goal = focus_goal(&mut store, 2.0, Period::Week, local(12, 8, 0));
        assert_eq!(reached(&session(&mut store, local(12, 9, 0), local(12, 10, 0))), 0);
        let events = session(&mut store, local(13, 9, 0), local(13, 10, 0));
        assert_eq!(reached(&events), 1);
        assert_eq!(reached(&session(&mut store, local(14, 9, 0), local(14, 10, 0))), 0);

        let source = XpSource::FocusGoal { goal_id: goal, period_start: date(12) };
        let paid: Vec<_> = store.profile.xp_log.iter().filter(|e| e.source == Some(source)).collect();
        assert_eq!(paid.len(), 1);
        assert_eq!(paid[0].amount, XP_WEEK_TARGET);

        // A new week starts over.
        session(&mut store, local(19, 9, 0), local(19, 10, 0));
        assert_eq!(reached(&session(&mut store, local(20, 9, 0), local(20, 10, 0))), 1);
        assert_xp_consistent(&store);
    }

    #[test]
    fn monthly_goal_counts_the_calendar_month() {
        let mut store = quiet_store();
        focus_goal(&mut store, 3.0, Period::Month, local(1, 8, 0));
        session(&mut store, local(2, 9, 0), local(2, 11, 0));
        assert_eq!(reached(&session(&mut store, local(30, 9, 0), local(30, 10, 0))), 1);
    }

    #[test]
    fn deleting_below_the_target_takes_the_reward_back() {
        let mut store = quiet_store();
        let goal = focus_goal(&mut store, 2.0, Period::Week, local(12, 8, 0));
        session(&mut store, local(13, 9, 0), local(13, 11, 0));
        assert_eq!(store.profile.xp, 40 + XP_WEEK_TARGET);

        let events = delete_session(&mut store, 1, local(14, 9, 0)).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(store.profile.xp, 0);
        assert!(!store.profile.xp_log.iter().any(|e| e.source.and_then(|s| s.goal_id()) == Some(goal)));
        assert_xp_consistent(&store);
    }

    #[test]
    fn running_time_does_not_reach_the_target_until_paused() {
        let mut store = quiet_store();
        focus_goal(&mut store, 1.0, Period::Week, local(12, 8, 0));
        start(&mut store, local(13, 9, 0)).unwrap();
        let mut events = Vec::new();
        sync_goals(&mut store, local(13, 11, 0), &mut events);
        assert!(events.is_empty());
        assert_eq!(reached(&pause(&mut store, local(13, 11, 0)).unwrap()), 1);
    }

    #[test]
    fn focus_before_the_goal_was_created_counts_in_its_first_period_only() {
        let mut store = quiet_store();
        session(&mut store, local(5, 9, 0), local(5, 12, 0)); // the week before
        session(&mut store, local(12, 9, 0), local(12, 11, 0)); // Monday of the creation week
        focus_goal(&mut store, 2.0, Period::Week, local(14, 8, 0));
        let mut events = Vec::new();
        sync_goals(&mut store, local(14, 9, 0), &mut events);
        assert_eq!(reached(&events), 1);
    }

    #[test]
    fn deleting_or_archiving_a_focus_goal() {
        let mut store = quiet_store();
        let goal = focus_goal(&mut store, 1.0, Period::Week, local(12, 8, 0));
        session(&mut store, local(13, 9, 0), local(13, 10, 0));

        // Archived goals keep their XP, even if sessions change.
        store.goals[0].archived_at = Some(local(13, 11, 0));
        delete_session(&mut store, 1, local(13, 12, 0)).unwrap();
        assert_eq!(store.profile.xp, XP_WEEK_TARGET);

        let events = game::delete_goal(&mut store, goal).unwrap();
        assert!(matches!(events[0], GameEvent::XpRevoked { amount: XP_WEEK_TARGET, .. }));
        assert_eq!(store.profile.xp, 0);
    }

    #[test]
    fn focus_days_complete_the_active_days_quest_and_undo_reopens_it() {
        let mut store = Store::default();
        store.profile.quests = Some(WeeklyQuests {
            week: "2026-W42".into(),
            quests: vec![Quest { kind: QuestKind::ActiveDays, target: 2.0, xp: 150, completed: false, completed_by: None }],
        });
        session(&mut store, local(12, 9, 0), local(12, 10, 0));
        let events = session(&mut store, local(13, 9, 0), local(13, 10, 0));
        assert!(events.iter().any(|e| matches!(e, GameEvent::QuestCompleted { .. })));

        delete_session(&mut store, 2, local(13, 11, 0)).unwrap();
        assert!(!store.profile.quests.as_ref().unwrap().quests[0].completed);
        assert_xp_consistent(&store);
    }

    #[test]
    fn focus_counts_for_streak_badges() {
        let mut store = quiet_store();
        for d in 12..=14 {
            session(&mut store, local(d, 9, 0), local(d, 10, 0));
        }
        assert!(store.profile.badges.iter().any(|b| b.id == "on_fire"));
    }
}
