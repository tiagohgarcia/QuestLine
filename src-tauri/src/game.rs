use crate::model::{Entry, EntryData, EntryRef, Goal, GoalKind, Profile, Quest, QuestKind, Store, UnlockedBadge, WeeklyQuests, XpEvent};
use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, TimeZone, Utc};
use serde::Serialize;
use std::collections::BTreeSet;

const XP_UPDATE: u64 = 10;
const XP_RUN: u64 = 15;
/// XP for moving a goal from 0% to 100%, on top of the per-update XP.
const XP_FULL_PROGRESS: f64 = 200.0;
const XP_MILESTONE: u64 = 50;
const XP_COMPLETE: u64 = 300;
const XP_PERSONAL_BEST: u64 = 50;
const XP_GOAL_QUEST: u64 = 120;
const XP_ACTIVE_DAYS_QUEST: u64 = 150;
const LUCKY_CHANCE: f64 = 0.1;
const DONE: f64 = 0.9999;
pub const MILESTONES: [u8; 3] = [25, 50, 75];

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GameEvent {
    Xp { amount: u64, lucky: bool },
    LevelUp { level: u32, title: &'static str },
    Milestone { goal: String, percent: u8, xp: u64 },
    GoalCompleted { goal: String, xp: u64 },
    PersonalBest { goal: String, xp: u64 },
    QuestCompleted { quest: String, xp: u64 },
    BadgeUnlocked { name: &'static str, icon: &'static str },
    XpRevoked { reason: String, amount: u64 },
}

/// One-off things that happened during this update, used by badges.
#[derive(Default)]
struct Flags {
    lucky: bool,
    personal_best: bool,
}

// ---------- Levels ----------

/// Total XP needed to reach `level`: 0, 100, 300, 600, 1000, ...
pub fn level_threshold(level: u32) -> u64 {
    let level = level as u64;
    50 * level * level.saturating_sub(1)
}

pub fn level_for(xp: u64) -> u32 {
    let mut level = 1;
    while level_threshold(level + 1) <= xp {
        level += 1;
    }
    level
}

pub fn title_for(level: u32) -> &'static str {
    match level {
        0..=2 => "Rookie",
        3..=4 => "Apprentice",
        5..=7 => "Grinder",
        8..=11 => "Achiever",
        12..=15 => "Veteran",
        16..=19 => "Master",
        _ => "Legend",
    }
}

fn award_xp(
    profile: &mut Profile,
    amount: u64,
    reason: String,
    source: EntryRef,
    now: DateTime<Utc>,
    events: &mut Vec<GameEvent>,
) {
    if amount == 0 {
        return;
    }
    let before = level_for(profile.xp);
    profile.xp += amount;
    profile.xp_log.push(XpEvent { at: now, amount, reason, source: Some(source) });
    let after = level_for(profile.xp);
    if after > before {
        events.push(GameEvent::LevelUp { level: after, title: title_for(after) });
    }
}

pub fn xp_between(profile: &Profile, from: DateTime<Utc>, until: DateTime<Utc>) -> u64 {
    profile.xp_log.iter().filter(|e| e.at >= from && e.at < until).map(|e| e.amount).sum()
}

// ---------- Time ----------

pub fn local_date(at: DateTime<Utc>) -> NaiveDate {
    at.with_timezone(&Local).date_naive()
}

pub fn local_midnight(date: NaiveDate) -> DateTime<Utc> {
    let naive = date.and_hms_opt(0, 0, 0).unwrap();
    Local
        .from_local_datetime(&naive)
        .earliest()
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|| Utc.from_utc_datetime(&naive))
}

/// The current Monday-to-Sunday week in local time.
pub struct Week {
    pub key: String,
    pub first_day: NaiveDate,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

pub fn week_of(now: DateTime<Utc>) -> Week {
    let today = local_date(now);
    let first_day = today - Duration::days(today.weekday().num_days_from_monday() as i64);
    let iso = today.iso_week();
    Week {
        key: format!("{}-W{:02}", iso.year(), iso.week()),
        first_day,
        start: local_midnight(first_day),
        end: local_midnight(first_day + Duration::days(7)),
    }
}

/// Returns (current streak, best streak) in days.
/// The current streak survives until the end of today even if today has no update yet.
pub fn streaks(days: &BTreeSet<NaiveDate>, today: NaiveDate) -> (u32, u32) {
    let mut best = 0;
    let mut run = 0;
    let mut prev: Option<NaiveDate> = None;
    for &day in days {
        run = if prev.and_then(|p| p.succ_opt()) == Some(day) { run + 1 } else { 1 };
        best = best.max(run);
        prev = Some(day);
    }

    let mut current = 0;
    let mut day = if days.contains(&today) { today } else { today - Duration::days(1) };
    while days.contains(&day) {
        current += 1;
        day -= Duration::days(1);
    }
    (current, best)
}

// ---------- Progress ----------

pub fn apply_entry(store: &mut Store, goal_id: u64, data: EntryData, now: DateTime<Utc>) -> Result<Vec<GameEvent>, String> {
    let mut events = Vec::new();
    let mut flags = Flags::default();

    let goal = store.goals.iter_mut().find(|g| g.id == goal_id).ok_or("Goal not found")?;
    if goal.is_archived() {
        return Err("Restore this goal before updating it".into());
    }
    let is_run = match (&goal.kind, &data) {
        (GoalKind::Counter { .. }, EntryData::Count { .. }) => false,
        (GoalKind::Performance { .. }, EntryData::Run { .. }) => true,
        _ => return Err("This update does not match the goal type".into()),
    };

    let before = goal.progress();
    let peak = goal.peak_progress();
    let had_runs = !goal.entries.is_empty();
    goal.next_entry_id += 1;
    let source = EntryRef { goal_id, entry_id: goal.next_entry_id };
    goal.entries.push(Entry { id: source.entry_id, at: now, data });
    let after = goal.progress();
    let name = goal.name.clone();

    let mut rewards: Vec<(u64, String)> = Vec::new();

    // Every run counts, but counter updates only pay when they set a new high.
    let gain = (after - peak).max(0.0);
    if is_run || gain > 1e-9 {
        let base = if is_run { XP_RUN } else { XP_UPDATE };
        let mut xp = base + (gain * XP_FULL_PROGRESS).round() as u64;
        flags.lucky = fastrand::f64() < LUCKY_CHANCE;
        if flags.lucky {
            xp *= 2;
        }
        events.push(GameEvent::Xp { amount: xp, lucky: flags.lucky });
        rewards.push((xp, format!("Progress on {name}")));
        store.profile.active_days.insert(local_date(now));
    }

    let crossed: Vec<u8> = MILESTONES
        .iter()
        .copied()
        .filter(|m| after * 100.0 >= *m as f64 && !goal.milestones_hit.contains(m))
        .collect();
    goal.milestones_hit.extend(&crossed);
    let milestone_xp = XP_MILESTONE * crossed.len() as u64;

    if after >= DONE && goal.completed_at.is_none() {
        goal.completed_at = Some(now);
        let xp = XP_COMPLETE + milestone_xp;
        events.push(GameEvent::GoalCompleted { goal: name.clone(), xp });
        rewards.push((xp, format!("Completed {name}")));
    } else if let Some(&percent) = crossed.last() {
        events.push(GameEvent::Milestone { goal: name.clone(), percent, xp: milestone_xp });
        rewards.push((milestone_xp, format!("{percent}% on {name}")));
    }

    if is_run && had_runs && after > before + 1e-9 && after < DONE {
        flags.personal_best = true;
        events.push(GameEvent::PersonalBest { goal: name.clone(), xp: XP_PERSONAL_BEST });
        rewards.push((XP_PERSONAL_BEST, format!("Personal best on {name}")));
    }

    for (xp, reason) in rewards {
        award_xp(&mut store.profile, xp, reason, source, now, &mut events);
    }
    check_quests(store, source, now, &mut events);
    check_badges(store, &flags, now, &mut events);
    Ok(events)
}

/// Removes a goal's last entry and takes back everything it earned.
pub fn undo_last_entry(store: &mut Store, goal_id: u64) -> Result<Vec<GameEvent>, String> {
    let goal = store.goals.iter_mut().find(|g| g.id == goal_id).ok_or("Goal not found")?;
    if goal.is_archived() {
        return Err("Restore this goal before changing it".into());
    }
    let entry = goal.entries.pop().ok_or("Nothing to undo")?;
    let peak = goal.peak_progress();
    goal.milestones_hit.retain(|m| peak * 100.0 >= *m as f64);
    if peak < DONE {
        goal.completed_at = None;
    }
    let reason = format!("Undid last update on {}", goal.name);
    let source = EntryRef { goal_id, entry_id: entry.id };
    let amount = revoke_xp(&mut store.profile, |s| *s == source);
    Ok(vec![GameEvent::XpRevoked { reason, amount }])
}

/// Deletes a goal and takes back all XP it earned, as if it never existed.
pub fn delete_goal(store: &mut Store, goal_id: u64) -> Result<Vec<GameEvent>, String> {
    let index = store.goals.iter().position(|g| g.id == goal_id).ok_or("Goal not found")?;
    let goal = store.goals.remove(index);
    let amount = revoke_xp(&mut store.profile, |s| s.goal_id == goal_id);
    Ok(vec![GameEvent::XpRevoked { reason: format!("Deleted {}", goal.name), amount }])
}

/// Removes the XP awards whose source matches, reopens quests they completed,
/// and drops active days that no longer have any XP. Returns the XP removed.
fn revoke_xp(profile: &mut Profile, revoked: impl Fn(&EntryRef) -> bool) -> u64 {
    let mut amount = 0;
    let mut days = BTreeSet::new();
    profile.xp_log.retain(|e| {
        let hit = e.source.as_ref().map_or(false, &revoked);
        if hit {
            amount += e.amount;
            days.insert(local_date(e.at));
        }
        !hit
    });
    profile.xp = profile.xp.saturating_sub(amount);

    for day in days {
        if !profile.xp_log.iter().any(|e| local_date(e.at) == day) {
            profile.active_days.remove(&day);
        }
    }

    if let Some(weekly) = profile.quests.as_mut() {
        for quest in &mut weekly.quests {
            if quest.completed_by.as_ref().map_or(false, &revoked) {
                quest.completed = false;
                quest.completed_by = None;
                profile.quests_completed = profile.quests_completed.saturating_sub(1);
            }
        }
    }
    amount
}

// ---------- Weekly quests ----------

pub fn fmt_num(n: f64) -> String {
    if (n - n.round()).abs() < 1e-9 {
        format!("{}", n.round() as i64)
    } else {
        format!("{n:.1}")
    }
}

fn find_goal(store: &Store, id: u64) -> Option<&Goal> {
    store.goals.iter().find(|g| g.id == id)
}

/// Makes sure this week's quests exist. Returns true if anything changed.
pub fn ensure_quests(store: &mut Store, now: DateTime<Utc>) -> bool {
    let week = week_of(now);
    let is_new_week = store.profile.quests.as_ref().map_or(true, |q| q.week != week.key);
    if is_new_week {
        let mut quests = goal_quests(store, &week, 2);
        quests.push(Quest { kind: QuestKind::ActiveDays, target: 4.0, xp: XP_ACTIVE_DAYS_QUEST, completed: false, completed_by: None });
        store.profile.quests = Some(WeeklyQuests { week: week.key, quests });
        return true;
    }

    // Goals created mid-week on an empty board still get quests.
    let has_goal_quest = store
        .profile
        .quests
        .as_ref()
        .map_or(false, |w| w.quests.iter().any(|q| !matches!(q.kind, QuestKind::ActiveDays)));
    if !has_goal_quest {
        let extra = goal_quests(store, &week, 2);
        if let (false, Some(weekly)) = (extra.is_empty(), store.profile.quests.as_mut()) {
            weekly.quests.splice(0..0, extra);
            return true;
        }
    }
    false
}

/// Picks random active goals and sets a target that beats last week's ghost.
fn goal_quests(store: &Store, week: &Week, count: usize) -> Vec<Quest> {
    let mut active: Vec<&Goal> = store.goals.iter().filter(|g| !g.is_completed() && !g.is_archived()).collect();
    fastrand::shuffle(&mut active);
    active
        .into_iter()
        .take(count)
        .map(|g| match g.kind {
            GoalKind::Counter { target, .. } => {
                let last_week = g.amount_between(Some(week.start - Duration::days(7)), Some(week.start)).max(0.0);
                let remaining = (target - g.current()).max(1.0).ceil();
                let amount = (last_week * 1.2).ceil().max((target * 0.05).ceil()).max(1.0).min(remaining);
                Quest { kind: QuestKind::AddToGoal { goal_id: g.id }, target: amount, xp: XP_GOAL_QUEST, completed: false, completed_by: None }
            }
            GoalKind::Performance { .. } => {
                Quest { kind: QuestKind::LogRuns { goal_id: g.id }, target: 2.0, xp: XP_GOAL_QUEST, completed: false, completed_by: None }
            }
        })
        .collect()
}

pub fn quest_progress(store: &Store, quest: &Quest, week: &Week) -> f64 {
    match quest.kind {
        QuestKind::AddToGoal { goal_id } => find_goal(store, goal_id)
            .map_or(0.0, |g| g.amount_between(Some(week.start), Some(week.end)).max(0.0)),
        QuestKind::LogRuns { goal_id } => find_goal(store, goal_id)
            .map_or(0.0, |g| g.entries_between(Some(week.start), Some(week.end)).count() as f64),
        QuestKind::ActiveDays => store
            .profile
            .active_days
            .range(week.first_day..week.first_day + Duration::days(7))
            .count() as f64,
    }
}

pub fn quest_title(store: &Store, quest: &Quest) -> String {
    let n = fmt_num(quest.target);
    match quest.kind {
        QuestKind::AddToGoal { goal_id } => match find_goal(store, goal_id) {
            Some(g) => {
                let unit = match &g.kind {
                    GoalKind::Counter { unit, .. } => unit.as_str(),
                    GoalKind::Performance { .. } => "",
                };
                format!("Add {} to {}", format!("{n} {unit}").trim_end(), g.name)
            }
            None => "Goal was removed".into(),
        },
        QuestKind::LogRuns { goal_id } => match find_goal(store, goal_id) {
            Some(g) => format!("Log {n} runs for {}", g.name),
            None => "Goal was removed".into(),
        },
        QuestKind::ActiveDays => format!("Make progress on {n} different days"),
    }
}

fn check_quests(store: &mut Store, source: EntryRef, now: DateTime<Utc>, events: &mut Vec<GameEvent>) {
    ensure_quests(store, now);
    let week = week_of(now);
    let Some(weekly) = store.profile.quests.as_ref() else {
        return;
    };
    let done: Vec<(usize, String, u64)> = weekly
        .quests
        .iter()
        .enumerate()
        .filter(|(_, q)| !q.completed && quest_progress(store, q, &week) >= q.target - 1e-9)
        .map(|(i, q)| (i, quest_title(store, q), q.xp))
        .collect();

    for (i, title, xp) in done {
        if let Some(weekly) = store.profile.quests.as_mut() {
            weekly.quests[i].completed = true;
            weekly.quests[i].completed_by = Some(source);
        }
        store.profile.quests_completed += 1;
        events.push(GameEvent::QuestCompleted { quest: title.clone(), xp });
        award_xp(&mut store.profile, xp, format!("Quest: {title}"), source, now, events);
    }
}

// ---------- Badges ----------

pub struct BadgeDef {
    pub id: &'static str,
    pub name: &'static str,
    pub icon: &'static str,
    pub description: &'static str,
}

pub const BADGES: &[BadgeDef] = &[
    BadgeDef { id: "first_step", name: "First Step", icon: "👣", description: "Log your first progress" },
    BadgeDef { id: "goal_getter", name: "Goal Getter", icon: "🏆", description: "Complete a goal" },
    BadgeDef { id: "hat_trick", name: "Hat Trick", icon: "🎩", description: "Complete 3 goals" },
    BadgeDef { id: "on_fire", name: "On Fire", icon: "🔥", description: "Reach a 3-day streak" },
    BadgeDef { id: "unstoppable", name: "Unstoppable", icon: "⚡", description: "Reach a 7-day streak" },
    BadgeDef { id: "iron_will", name: "Iron Will", icon: "🛡️", description: "Reach a 30-day streak" },
    BadgeDef { id: "new_record", name: "New Record", icon: "🏃", description: "Beat a personal best" },
    BadgeDef { id: "adventurer", name: "Adventurer", icon: "🗺️", description: "Complete a weekly quest" },
    BadgeDef { id: "clean_sweep", name: "Clean Sweep", icon: "🧹", description: "Complete every quest in a week" },
    BadgeDef { id: "ghost_buster", name: "Ghost Buster", icon: "👻", description: "Earn more XP than all of last week" },
    BadgeDef { id: "lucky_strike", name: "Lucky Strike", icon: "🍀", description: "Get a lucky double-XP update" },
    BadgeDef { id: "centurion", name: "Centurion", icon: "💯", description: "Reach 100 on a counter goal" },
    BadgeDef { id: "rising_star", name: "Rising Star", icon: "⭐", description: "Reach level 5" },
    BadgeDef { id: "elite", name: "Elite", icon: "💎", description: "Reach level 10" },
];

fn check_badges(store: &mut Store, flags: &Flags, now: DateTime<Utc>, events: &mut Vec<GameEvent>) {
    let completed = store.goals.iter().filter(|g| g.is_completed()).count();
    let (_, best_streak) = streaks(&store.profile.active_days, local_date(now));
    let level = level_for(store.profile.xp);
    let week = week_of(now);
    let xp_this_week = xp_between(&store.profile, week.start, week.end);
    let xp_last_week = xp_between(&store.profile, week.start - Duration::days(7), week.start);

    for def in BADGES {
        if store.profile.badges.iter().any(|b| b.id == def.id) {
            continue;
        }
        let earned = match def.id {
            "first_step" => !store.profile.active_days.is_empty(),
            "goal_getter" => completed >= 1,
            "hat_trick" => completed >= 3,
            "on_fire" => best_streak >= 3,
            "unstoppable" => best_streak >= 7,
            "iron_will" => best_streak >= 30,
            "new_record" => flags.personal_best,
            "adventurer" => store.profile.quests_completed >= 1,
            "clean_sweep" => store
                .profile
                .quests
                .as_ref()
                .map_or(false, |w| w.quests.len() >= 2 && w.quests.iter().all(|q| q.completed)),
            "ghost_buster" => xp_last_week > 0 && xp_this_week > xp_last_week,
            "lucky_strike" => flags.lucky,
            "centurion" => store
                .goals
                .iter()
                .any(|g| matches!(g.kind, GoalKind::Counter { .. }) && g.current() >= 100.0),
            "rising_star" => level >= 5,
            "elite" => level >= 10,
            _ => false,
        };
        if earned {
            store.profile.badges.push(UnlockedBadge { id: def.id.to_string(), at: now });
            events.push(GameEvent::BadgeUnlocked { name: def.name, icon: def.icon });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Oct 12-18 2026 is ISO week 42 (Monday to Sunday). Tests run mid-week, on the 14th.
    const WEEK: &str = "2026-W42";

    fn local(y: i32, m: u32, d: u32, h: u32) -> DateTime<Utc> {
        Local.with_ymd_and_hms(y, m, d, h, 0, 0).single().unwrap().with_timezone(&Utc)
    }

    fn day(d: u32) -> DateTime<Utc> {
        local(2026, 10, d, 12)
    }

    fn date(d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, d).unwrap()
    }

    /// A store whose only quest never completes, so quest XP doesn't mix into the numbers.
    fn quiet_store() -> Store {
        let mut store = Store::default();
        store.profile.quests = Some(WeeklyQuests {
            week: WEEK.into(),
            quests: vec![quest(QuestKind::AddToGoal { goal_id: 999 }, 1.0)],
        });
        store
    }

    fn quest(kind: QuestKind, target: f64) -> Quest {
        Quest { kind, target, xp: XP_GOAL_QUEST, completed: false, completed_by: None }
    }

    fn new_goal(store: &mut Store, kind: GoalKind) -> u64 {
        let id = store.next_id;
        store.next_id += 1;
        store.goals.push(Goal {
            id,
            name: format!("Goal {id}"),
            category: "General".into(),
            kind,
            created_at: day(1),
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

    fn counter(store: &mut Store, target: f64) -> u64 {
        new_goal(store, GoalKind::Counter { target, unit: "problems".into() })
    }

    fn performance(store: &mut Store) -> u64 {
        new_goal(store, GoalKind::Performance { distance_km: 5.0, time_min: 30.0 })
    }

    fn goal(store: &Store, id: u64) -> &Goal {
        find_goal(store, id).unwrap()
    }

    fn goal_mut(store: &mut Store, id: u64) -> &mut Goal {
        store.goals.iter_mut().find(|g| g.id == id).unwrap()
    }

    fn add(store: &mut Store, id: u64, delta: f64, now: DateTime<Utc>) -> Vec<GameEvent> {
        apply_entry(store, id, EntryData::Count { delta }, now).unwrap()
    }

    fn run(store: &mut Store, id: u64, distance_km: f64, time_min: f64, now: DateTime<Utc>) -> Vec<GameEvent> {
        apply_entry(store, id, EntryData::Run { distance_km, time_min }, now).unwrap()
    }

    /// The update XP before any lucky doubling, or None if the update paid nothing.
    fn update_xp(events: &[GameEvent]) -> Option<u64> {
        events.iter().find_map(|e| match e {
            GameEvent::Xp { amount, lucky } => Some(if *lucky { amount / 2 } else { *amount }),
            _ => None,
        })
    }

    fn has_badge(store: &Store, id: &str) -> bool {
        store.profile.badges.iter().any(|b| b.id == id)
    }

    fn revoked(events: &[GameEvent]) -> u64 {
        match events {
            [GameEvent::XpRevoked { amount, .. }] => *amount,
            other => panic!("expected one XpRevoked event, got {other:?}"),
        }
    }

    /// Total XP must always equal the log, whatever was awarded or revoked.
    fn assert_xp_consistent(store: &Store) {
        let logged: u64 = store.profile.xp_log.iter().map(|e| e.amount).sum();
        assert_eq!(store.profile.xp, logged);
    }

    // ---------- Levels and time ----------

    #[test]
    fn level_thresholds() {
        assert_eq!([1, 2, 3, 4, 5].map(level_threshold), [0, 100, 300, 600, 1000]);
    }

    #[test]
    fn level_for_xp() {
        assert_eq!(level_for(0), 1);
        assert_eq!(level_for(99), 1);
        assert_eq!(level_for(100), 2);
        assert_eq!(level_for(299), 2);
        assert_eq!(level_for(300), 3);
        assert_eq!(level_for(1000), 5);
    }

    #[test]
    fn titles_by_level() {
        let titles = [1, 3, 5, 8, 12, 16, 20].map(title_for);
        assert_eq!(titles, ["Rookie", "Apprentice", "Grinder", "Achiever", "Veteran", "Master", "Legend"]);
    }

    #[test]
    fn xp_between_includes_start_and_excludes_end() {
        let mut profile = Profile::default();
        for d in [10, 12, 14] {
            profile.xp_log.push(XpEvent { at: day(d), amount: d as u64, reason: String::new(), source: None });
        }
        assert_eq!(xp_between(&profile, day(12), day(14)), 12);
    }

    #[test]
    fn local_midnight_is_start_of_local_day() {
        assert_eq!(local_date(local_midnight(date(14))), date(14));
        assert_eq!(local_date(local_midnight(date(14)) - Duration::seconds(1)), date(13));
    }

    #[test]
    fn week_runs_monday_to_sunday() {
        let week = week_of(day(14));
        assert_eq!(week.key, WEEK);
        assert_eq!(week.first_day, date(12));
        assert_eq!(week.start, local_midnight(date(12)));
        assert_eq!(week.end, local_midnight(date(19)));
        assert_eq!(week_of(local(2026, 10, 18, 23)).key, WEEK);
        assert_eq!(week_of(local(2026, 10, 19, 0)).key, "2026-W43");
    }

    #[test]
    fn week_key_uses_iso_year() {
        let week = week_of(local(2027, 1, 1, 12));
        assert_eq!(week.key, "2026-W53");
        assert_eq!(week.first_day, NaiveDate::from_ymd_opt(2026, 12, 28).unwrap());
    }

    #[test]
    fn streaks_without_days() {
        assert_eq!(streaks(&BTreeSet::new(), date(14)), (0, 0));
    }

    #[test]
    fn streak_survives_until_end_of_today() {
        let days: BTreeSet<_> = [10, 11, 12].map(date).into();
        assert_eq!(streaks(&days, date(12)), (3, 3));
        assert_eq!(streaks(&days, date(13)), (3, 3));
        assert_eq!(streaks(&days, date(14)), (0, 3));
    }

    #[test]
    fn best_streak_is_longest_run() {
        let days: BTreeSet<_> = [1, 2, 3, 4, 10, 11].map(date).into();
        assert_eq!(streaks(&days, date(11)), (2, 4));
    }

    #[test]
    fn fmt_num_drops_trailing_zeros() {
        assert_eq!(fmt_num(5.0), "5");
        assert_eq!(fmt_num(-3.0), "-3");
        assert_eq!(fmt_num(2.5), "2.5");
        assert_eq!(fmt_num(1.26), "1.3");
    }

    // ---------- Updates and XP ----------

    #[test]
    fn counter_update_pays_base_plus_progress() {
        let mut store = quiet_store();
        let id = counter(&mut store, 10.0);
        let events = add(&mut store, id, 1.0, day(14));
        assert_eq!(update_xp(&events), Some(XP_UPDATE + 20));
        let source = EntryRef { goal_id: id, entry_id: 1 };
        assert!(store.profile.xp_log.iter().all(|e| e.source == Some(source)));
        assert!(store.profile.active_days.contains(&date(14)));
        assert_xp_consistent(&store);
    }

    #[test]
    fn entry_ids_increment_per_goal() {
        let mut store = quiet_store();
        let a = counter(&mut store, 10.0);
        let b = counter(&mut store, 10.0);
        add(&mut store, a, 1.0, day(14));
        add(&mut store, a, 1.0, day(14));
        add(&mut store, b, 1.0, day(14));
        assert_eq!(goal(&store, a).entries.iter().map(|e| e.id).collect::<Vec<_>>(), [1, 2]);
        assert_eq!(goal(&store, a).next_entry_id, 2);
        assert_eq!(goal(&store, b).entries[0].id, 1);
    }

    #[test]
    fn update_must_match_goal_type() {
        let mut store = quiet_store();
        let c = counter(&mut store, 10.0);
        let p = performance(&mut store);
        assert!(apply_entry(&mut store, c, EntryData::Run { distance_km: 1.0, time_min: 5.0 }, day(14)).is_err());
        assert!(apply_entry(&mut store, p, EntryData::Count { delta: 1.0 }, day(14)).is_err());
        assert!(goal(&store, c).entries.is_empty());
        assert!(goal(&store, p).entries.is_empty());
    }

    #[test]
    fn update_of_missing_goal_fails() {
        let mut store = quiet_store();
        assert!(apply_entry(&mut store, 42, EntryData::Count { delta: 1.0 }, day(14)).is_err());
    }

    #[test]
    fn archived_goal_rejects_updates() {
        let mut store = quiet_store();
        let id = counter(&mut store, 10.0);
        goal_mut(&mut store, id).archived_at = Some(day(13));
        assert!(apply_entry(&mut store, id, EntryData::Count { delta: 1.0 }, day(14)).is_err());
        assert!(goal(&store, id).entries.is_empty());
    }

    #[test]
    fn going_down_and_back_up_earns_nothing() {
        let mut store = quiet_store();
        let id = counter(&mut store, 10.0);
        add(&mut store, id, 5.0, day(14));
        let xp = store.profile.xp;
        assert_eq!(update_xp(&add(&mut store, id, -1.0, day(15))), None);
        assert_eq!(update_xp(&add(&mut store, id, 1.0, day(15))), None);
        assert_eq!(store.profile.xp, xp);
        assert!(!store.profile.active_days.contains(&date(15)));
    }

    #[test]
    fn only_progress_above_previous_best_pays() {
        let mut store = quiet_store();
        let id = counter(&mut store, 10.0);
        add(&mut store, id, 5.0, day(14));
        add(&mut store, id, -3.0, day(14));
        // From 2 to 6: only the step from 5 to 6 is new.
        assert_eq!(update_xp(&add(&mut store, id, 4.0, day(14))), Some(XP_UPDATE + 20));
    }

    #[test]
    fn milestone_pays_once() {
        let mut store = quiet_store();
        let id = counter(&mut store, 4.0);
        let events = add(&mut store, id, 1.0, day(14));
        assert!(events.iter().any(|e| matches!(e, GameEvent::Milestone { percent: 25, xp: XP_MILESTONE, .. })));
        assert_eq!(goal(&store, id).milestones_hit, [25]);
        add(&mut store, id, -1.0, day(14));
        let events = add(&mut store, id, 1.0, day(14));
        assert!(!events.iter().any(|e| matches!(e, GameEvent::Milestone { .. })));
    }

    #[test]
    fn crossing_several_milestones_pays_each() {
        let mut store = quiet_store();
        let id = counter(&mut store, 4.0);
        let events = add(&mut store, id, 3.0, day(14));
        assert!(events.iter().any(|e| matches!(e, GameEvent::Milestone { percent: 75, xp: 150, .. })));
        assert_eq!(goal(&store, id).milestones_hit, [25, 50, 75]);
    }

    #[test]
    fn completion_includes_skipped_milestones() {
        let mut store = quiet_store();
        let id = counter(&mut store, 2.0);
        let events = add(&mut store, id, 2.0, day(14));
        let bonus = XP_COMPLETE + 3 * XP_MILESTONE;
        assert!(events.iter().any(|e| matches!(e, GameEvent::GoalCompleted { xp, .. } if *xp == bonus)));
        assert!(!events.iter().any(|e| matches!(e, GameEvent::Milestone { .. })));
        assert_eq!(goal(&store, id).completed_at, Some(day(14)));
        assert!(has_badge(&store, "goal_getter"));
    }

    #[test]
    fn completion_pays_once() {
        let mut store = quiet_store();
        let id = counter(&mut store, 2.0);
        add(&mut store, id, 2.0, day(14));
        add(&mut store, id, -1.0, day(14));
        let events = add(&mut store, id, 1.0, day(14));
        assert!(!events.iter().any(|e| matches!(e, GameEvent::GoalCompleted { .. })));
        assert_eq!(goal(&store, id).completed_at, Some(day(14)));
    }

    #[test]
    fn every_run_pays_base_xp() {
        let mut store = quiet_store();
        let id = performance(&mut store);
        // 3 km in 20 min scores 0.54.
        let events = run(&mut store, id, 3.0, 20.0, day(14));
        assert_eq!(update_xp(&events), Some(XP_RUN + 108));
        assert!(!events.iter().any(|e| matches!(e, GameEvent::PersonalBest { .. })));
        let events = run(&mut store, id, 2.0, 20.0, day(14));
        assert_eq!(update_xp(&events), Some(XP_RUN));
        assert!(!events.iter().any(|e| matches!(e, GameEvent::PersonalBest { .. })));
    }

    #[test]
    fn better_run_is_a_personal_best() {
        let mut store = quiet_store();
        let id = performance(&mut store);
        run(&mut store, id, 3.0, 20.0, day(14));
        // 4 km in 26 min scores 0.8 × 6/6.5 ≈ 0.738.
        let events = run(&mut store, id, 4.0, 26.0, day(14));
        assert_eq!(update_xp(&events), Some(XP_RUN + 40));
        assert!(events.iter().any(|e| matches!(e, GameEvent::PersonalBest { xp: XP_PERSONAL_BEST, .. })));
        assert!(has_badge(&store, "new_record"));
    }

    #[test]
    fn completing_run_is_not_also_a_personal_best() {
        let mut store = quiet_store();
        let id = performance(&mut store);
        run(&mut store, id, 3.0, 20.0, day(14));
        let events = run(&mut store, id, 5.0, 30.0, day(14));
        assert!(events.iter().any(|e| matches!(e, GameEvent::GoalCompleted { .. })));
        assert!(!events.iter().any(|e| matches!(e, GameEvent::PersonalBest { .. })));
    }

    #[test]
    fn level_up_is_announced() {
        let mut store = quiet_store();
        let id = counter(&mut store, 10.0);
        let events = add(&mut store, id, 10.0, day(14));
        let last_level = events.iter().rev().find_map(|e| match e {
            GameEvent::LevelUp { level, .. } => Some(*level),
            _ => None,
        });
        assert_eq!(last_level, Some(level_for(store.profile.xp)));
    }

    #[test]
    fn first_update_unlocks_first_step() {
        let mut store = quiet_store();
        let id = counter(&mut store, 10.0);
        let events = add(&mut store, id, 1.0, day(14));
        assert!(has_badge(&store, "first_step"));
        assert!(events.iter().any(|e| matches!(e, GameEvent::BadgeUnlocked { name: "First Step", .. })));
        let lucky = events.iter().any(|e| matches!(e, GameEvent::Xp { lucky: true, .. }));
        assert_eq!(has_badge(&store, "lucky_strike"), lucky);
    }

    #[test]
    fn badges_unlock_once() {
        let mut store = quiet_store();
        let id = counter(&mut store, 10.0);
        add(&mut store, id, 1.0, day(14));
        add(&mut store, id, 1.0, day(14));
        assert_eq!(store.profile.badges.iter().filter(|b| b.id == "first_step").count(), 1);
    }

    #[test]
    fn three_day_streak_unlocks_on_fire() {
        let mut store = quiet_store();
        store.profile.active_days.extend([date(12), date(13)]);
        let id = counter(&mut store, 10.0);
        add(&mut store, id, 1.0, day(14));
        assert!(has_badge(&store, "on_fire"));
        assert!(!has_badge(&store, "unstoppable"));
    }

    #[test]
    fn reaching_100_unlocks_centurion() {
        let mut store = quiet_store();
        let id = counter(&mut store, 200.0);
        add(&mut store, id, 99.0, day(14));
        assert!(!has_badge(&store, "centurion"));
        add(&mut store, id, 1.0, day(14));
        assert!(has_badge(&store, "centurion"));
    }

    #[test]
    fn beating_last_weeks_xp_unlocks_ghost_buster() {
        let mut store = quiet_store();
        store.profile.xp_log.push(XpEvent { at: day(7), amount: 10, reason: String::new(), source: None });
        store.profile.xp = 10;
        let id = counter(&mut store, 10.0);
        add(&mut store, id, 1.0, day(14));
        assert!(has_badge(&store, "ghost_buster"));
    }

    // ---------- Weekly quests ----------

    #[test]
    fn quests_pick_active_goals_only() {
        let mut store = Store::default();
        let open = [counter(&mut store, 10.0), performance(&mut store)];
        let done = counter(&mut store, 10.0);
        goal_mut(&mut store, done).completed_at = Some(day(2));
        let archived = counter(&mut store, 10.0);
        goal_mut(&mut store, archived).archived_at = Some(day(2));

        assert!(ensure_quests(&mut store, day(14)));
        let weekly = store.profile.quests.as_ref().unwrap();
        assert_eq!(weekly.week, WEEK);
        let mut goal_ids: Vec<u64> = weekly
            .quests
            .iter()
            .filter_map(|q| match q.kind {
                QuestKind::AddToGoal { goal_id } | QuestKind::LogRuns { goal_id } => Some(goal_id),
                QuestKind::ActiveDays => None,
            })
            .collect();
        goal_ids.sort();
        assert_eq!(goal_ids, open);
        assert!(matches!(weekly.quests.last().unwrap().kind, QuestKind::ActiveDays));
        assert!(!ensure_quests(&mut store, day(15)));
    }

    #[test]
    fn new_week_gets_new_quests() {
        let mut store = Store::default();
        counter(&mut store, 10.0);
        ensure_quests(&mut store, day(14));
        assert!(ensure_quests(&mut store, day(21)));
        assert_eq!(store.profile.quests.as_ref().unwrap().week, "2026-W43");
    }

    #[test]
    fn goal_created_mid_week_gets_quests() {
        let mut store = Store::default();
        ensure_quests(&mut store, day(13));
        assert_eq!(store.profile.quests.as_ref().unwrap().quests.len(), 1);
        let id = counter(&mut store, 10.0);
        assert!(ensure_quests(&mut store, day(14)));
        let quests = &store.profile.quests.as_ref().unwrap().quests;
        assert_eq!(quests.len(), 2);
        assert!(matches!(quests[0].kind, QuestKind::AddToGoal { goal_id } if goal_id == id));
    }

    fn only_goal_quest(store: &mut Store) -> Quest {
        ensure_quests(store, day(14));
        store.profile.quests.as_ref().unwrap().quests[0].clone()
    }

    #[test]
    fn counter_quest_beats_last_week() {
        let mut store = Store::default();
        let id = counter(&mut store, 100.0);
        let g = goal_mut(&mut store, id);
        g.entries.push(Entry { id: 1, at: day(7), data: EntryData::Count { delta: 10.0 } });
        assert_eq!(only_goal_quest(&mut store).target, 12.0);
    }

    #[test]
    fn counter_quest_asks_at_least_five_percent() {
        let mut store = Store::default();
        counter(&mut store, 100.0);
        assert_eq!(only_goal_quest(&mut store).target, 5.0);
    }

    #[test]
    fn counter_quest_never_exceeds_what_is_left() {
        let mut store = Store::default();
        let id = counter(&mut store, 100.0);
        let g = goal_mut(&mut store, id);
        g.baseline = 88.0;
        g.entries.push(Entry { id: 1, at: day(7), data: EntryData::Count { delta: 10.0 } });
        assert_eq!(only_goal_quest(&mut store).target, 2.0);
    }

    #[test]
    fn performance_quest_asks_for_two_runs() {
        let mut store = Store::default();
        performance(&mut store);
        let q = only_goal_quest(&mut store);
        assert!(matches!(q.kind, QuestKind::LogRuns { .. }));
        assert_eq!(q.target, 2.0);
    }

    #[test]
    fn quest_completion_is_rewarded_and_tagged() {
        let mut store = Store::default();
        let id = counter(&mut store, 10.0);
        ensure_quests(&mut store, day(14));
        let events = add(&mut store, id, 1.0, day(14));
        assert!(events.iter().any(|e| matches!(e, GameEvent::QuestCompleted { xp: XP_GOAL_QUEST, .. })));
        let source = Some(EntryRef { goal_id: id, entry_id: 1 });
        let q = &store.profile.quests.as_ref().unwrap().quests[0];
        assert!(q.completed);
        assert_eq!(q.completed_by, source);
        assert_eq!(store.profile.quests_completed, 1);
        assert!(store.profile.xp_log.iter().any(|e| e.reason.starts_with("Quest:") && e.source == source));
        assert!(has_badge(&store, "adventurer"));
    }

    #[test]
    fn completing_every_quest_unlocks_clean_sweep() {
        let mut store = Store::default();
        let id = counter(&mut store, 10.0);
        store.profile.quests = Some(WeeklyQuests {
            week: WEEK.into(),
            quests: vec![quest(QuestKind::AddToGoal { goal_id: id }, 1.0), quest(QuestKind::AddToGoal { goal_id: id }, 1.0)],
        });
        add(&mut store, id, 1.0, day(14));
        assert!(has_badge(&store, "clean_sweep"));
    }

    #[test]
    fn quest_progress_counts_this_week_only() {
        let mut store = Store::default();
        let c = counter(&mut store, 100.0);
        let p = performance(&mut store);
        let g = goal_mut(&mut store, c);
        g.entries.push(Entry { id: 1, at: day(7), data: EntryData::Count { delta: 10.0 } });
        g.entries.push(Entry { id: 2, at: day(13), data: EntryData::Count { delta: 2.0 } });
        g.entries.push(Entry { id: 3, at: day(14), data: EntryData::Count { delta: -1.0 } });
        goal_mut(&mut store, p).entries.push(Entry { id: 1, at: day(13), data: EntryData::Run { distance_km: 1.0, time_min: 6.0 } });
        store.profile.active_days.extend([date(7), date(12), date(13)]);
        let week = week_of(day(14));

        assert_eq!(quest_progress(&store, &quest(QuestKind::AddToGoal { goal_id: c }, 5.0), &week), 1.0);
        assert_eq!(quest_progress(&store, &quest(QuestKind::LogRuns { goal_id: p }, 2.0), &week), 1.0);
        assert_eq!(quest_progress(&store, &quest(QuestKind::ActiveDays, 4.0), &week), 2.0);
        assert_eq!(quest_progress(&store, &quest(QuestKind::AddToGoal { goal_id: 999 }, 5.0), &week), 0.0);
    }

    #[test]
    fn quest_titles() {
        let mut store = Store::default();
        let c = counter(&mut store, 100.0);
        let bare = new_goal(&mut store, GoalKind::Counter { target: 10.0, unit: String::new() });
        let p = performance(&mut store);
        let title = |kind, target| quest_title(&store, &quest(kind, target));
        assert_eq!(title(QuestKind::AddToGoal { goal_id: c }, 5.0), "Add 5 problems to Goal 1");
        assert_eq!(title(QuestKind::AddToGoal { goal_id: bare }, 1.5), "Add 1.5 to Goal 2");
        assert_eq!(title(QuestKind::LogRuns { goal_id: p }, 2.0), "Log 2 runs for Goal 3");
        assert_eq!(title(QuestKind::ActiveDays, 4.0), "Make progress on 4 different days");
        assert_eq!(title(QuestKind::AddToGoal { goal_id: 999 }, 5.0), "Goal was removed");
    }

    // ---------- Undo ----------

    #[test]
    fn undo_without_entries_fails() {
        let mut store = quiet_store();
        let id = counter(&mut store, 10.0);
        assert!(undo_last_entry(&mut store, id).is_err());
        assert!(undo_last_entry(&mut store, 42).is_err());
    }

    #[test]
    fn undo_on_archived_goal_fails() {
        let mut store = quiet_store();
        let id = counter(&mut store, 10.0);
        add(&mut store, id, 1.0, day(14));
        goal_mut(&mut store, id).archived_at = Some(day(14));
        assert!(undo_last_entry(&mut store, id).is_err());
        assert_eq!(goal(&store, id).entries.len(), 1);
    }

    #[test]
    fn undo_takes_back_only_the_last_entry() {
        let mut store = quiet_store();
        let id = counter(&mut store, 100.0);
        add(&mut store, id, 1.0, day(14));
        let xp = store.profile.xp;
        add(&mut store, id, 1.0, day(14));
        let gained = store.profile.xp - xp;

        let events = undo_last_entry(&mut store, id).unwrap();
        assert_eq!(revoked(&events), gained);
        assert_eq!(store.profile.xp, xp);
        assert_eq!(goal(&store, id).entries.len(), 1);
        assert_xp_consistent(&store);
    }

    #[test]
    fn undone_progress_can_be_earned_again() {
        let mut store = quiet_store();
        let id = counter(&mut store, 10.0);
        add(&mut store, id, 1.0, day(14));
        undo_last_entry(&mut store, id).unwrap();
        assert_eq!(update_xp(&add(&mut store, id, 1.0, day(14))), Some(XP_UPDATE + 20));
    }

    #[test]
    fn entry_ids_are_not_reused_after_undo() {
        let mut store = quiet_store();
        let id = counter(&mut store, 10.0);
        add(&mut store, id, 1.0, day(14));
        add(&mut store, id, 1.0, day(14));
        undo_last_entry(&mut store, id).unwrap();
        add(&mut store, id, 1.0, day(14));
        assert_eq!(goal(&store, id).entries.iter().map(|e| e.id).collect::<Vec<_>>(), [1, 3]);
    }

    #[test]
    fn undo_reverts_completion_and_milestones() {
        let mut store = quiet_store();
        let id = counter(&mut store, 2.0);
        add(&mut store, id, 1.0, day(14));
        let xp = store.profile.xp;
        add(&mut store, id, 1.0, day(14));
        assert!(goal(&store, id).is_completed());

        undo_last_entry(&mut store, id).unwrap();
        let g = goal(&store, id);
        assert!(!g.is_completed());
        assert_eq!(g.milestones_hit, [25, 50]);
        assert_eq!(store.profile.xp, xp);
        assert!(has_badge(&store, "goal_getter"), "badges are never revoked");
        assert_xp_consistent(&store);
    }

    #[test]
    fn undone_milestone_pays_again() {
        let mut store = quiet_store();
        let id = counter(&mut store, 4.0);
        add(&mut store, id, 1.0, day(14));
        undo_last_entry(&mut store, id).unwrap();
        assert!(goal(&store, id).milestones_hit.is_empty());
        let events = add(&mut store, id, 1.0, day(14));
        assert!(events.iter().any(|e| matches!(e, GameEvent::Milestone { percent: 25, .. })));
    }

    #[test]
    fn undo_keeps_milestones_from_starting_value() {
        let mut store = quiet_store();
        let id = counter(&mut store, 4.0);
        let g = goal_mut(&mut store, id);
        g.baseline = 1.0;
        g.milestones_hit = vec![25];
        add(&mut store, id, 1.0, day(14));
        undo_last_entry(&mut store, id).unwrap();
        assert_eq!(goal(&store, id).milestones_hit, [25]);
    }

    #[test]
    fn undo_reopens_quest_it_completed() {
        let mut store = Store::default();
        let id = counter(&mut store, 10.0);
        ensure_quests(&mut store, day(14));
        add(&mut store, id, 1.0, day(14));

        undo_last_entry(&mut store, id).unwrap();
        let q = &store.profile.quests.as_ref().unwrap().quests[0];
        assert!(!q.completed);
        assert!(q.completed_by.is_none());
        assert_eq!(store.profile.quests_completed, 0);
        assert_eq!(store.profile.xp, 0);
        assert_xp_consistent(&store);
    }

    #[test]
    fn undo_drops_day_without_other_xp() {
        let mut store = quiet_store();
        let id = counter(&mut store, 10.0);
        add(&mut store, id, 1.0, day(14));
        undo_last_entry(&mut store, id).unwrap();
        assert!(!store.profile.active_days.contains(&date(14)));
    }

    #[test]
    fn undo_keeps_day_with_other_xp() {
        let mut store = quiet_store();
        let a = counter(&mut store, 10.0);
        let b = counter(&mut store, 10.0);
        add(&mut store, a, 1.0, day(14));
        add(&mut store, b, 1.0, day(14));
        undo_last_entry(&mut store, b).unwrap();
        assert!(store.profile.active_days.contains(&date(14)));
        undo_last_entry(&mut store, a).unwrap();
        assert!(!store.profile.active_days.contains(&date(14)));
    }

    #[test]
    fn undo_of_old_untagged_entry_keeps_its_xp() {
        let mut store = quiet_store();
        let id = counter(&mut store, 10.0);
        goal_mut(&mut store, id).entries.push(Entry { id: 0, at: day(13), data: EntryData::Count { delta: 1.0 } });
        store.profile.xp_log.push(XpEvent { at: day(13), amount: 30, reason: String::new(), source: None });
        store.profile.xp = 30;
        store.profile.active_days.insert(date(13));

        let events = undo_last_entry(&mut store, id).unwrap();
        assert_eq!(revoked(&events), 0);
        assert_eq!(store.profile.xp, 30);
        assert!(goal(&store, id).entries.is_empty());
        assert!(store.profile.active_days.contains(&date(13)));
    }

    #[test]
    fn undo_run_restores_previous_best() {
        let mut store = quiet_store();
        let id = performance(&mut store);
        run(&mut store, id, 3.0, 20.0, day(14));
        run(&mut store, id, 4.0, 26.0, day(14));
        undo_last_entry(&mut store, id).unwrap();
        assert_eq!(goal(&store, id).best_run().map(|(d, t, _)| (d, t)), Some((3.0, 20.0)));
        assert_xp_consistent(&store);
    }

    // ---------- Delete ----------

    #[test]
    fn delete_takes_back_only_that_goals_xp() {
        let mut store = quiet_store();
        let a = counter(&mut store, 10.0);
        let b = counter(&mut store, 10.0);
        add(&mut store, a, 10.0, day(14));
        add(&mut store, b, 1.0, day(14));
        let a_xp: u64 = store.profile.xp_log.iter().filter(|e| e.source.unwrap().goal_id == a).map(|e| e.amount).sum();
        let total = store.profile.xp;

        let events = delete_goal(&mut store, a).unwrap();
        assert_eq!(revoked(&events), a_xp);
        assert_eq!(store.profile.xp, total - a_xp);
        assert!(find_goal(&store, a).is_none());
        assert!(store.profile.xp_log.iter().all(|e| e.source.unwrap().goal_id == b));
        assert!(has_badge(&store, "goal_getter"), "badges are never revoked");
        assert_xp_consistent(&store);
    }

    #[test]
    fn delete_of_missing_goal_fails() {
        let mut store = quiet_store();
        assert!(delete_goal(&mut store, 42).is_err());
    }

    #[test]
    fn delete_keeps_untagged_xp() {
        let mut store = quiet_store();
        let id = counter(&mut store, 10.0);
        store.profile.xp_log.push(XpEvent { at: day(13), amount: 30, reason: String::new(), source: None });
        store.profile.xp = 30;
        add(&mut store, id, 1.0, day(14));
        delete_goal(&mut store, id).unwrap();
        assert_eq!(store.profile.xp, 30);
        assert_xp_consistent(&store);
    }

    #[test]
    fn delete_drops_days_that_only_it_earned() {
        let mut store = quiet_store();
        let a = counter(&mut store, 10.0);
        let b = counter(&mut store, 10.0);
        add(&mut store, a, 1.0, day(13));
        add(&mut store, a, 1.0, day(14));
        add(&mut store, b, 1.0, day(14));
        delete_goal(&mut store, a).unwrap();
        assert!(!store.profile.active_days.contains(&date(13)));
        assert!(store.profile.active_days.contains(&date(14)));
    }

    #[test]
    fn delete_reopens_quests_it_completed() {
        let mut store = Store::default();
        let a = counter(&mut store, 10.0);
        store.profile.quests = Some(WeeklyQuests { week: WEEK.into(), quests: vec![quest(QuestKind::AddToGoal { goal_id: a }, 1.0)] });
        add(&mut store, a, 1.0, day(14));
        assert_eq!(store.profile.quests_completed, 1);
        delete_goal(&mut store, a).unwrap();
        assert!(!store.profile.quests.as_ref().unwrap().quests[0].completed);
        assert_eq!(store.profile.quests_completed, 0);
    }
}
