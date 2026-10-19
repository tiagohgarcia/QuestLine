use crate::model::{Entry, EntryData, Goal, GoalKind, Profile, Quest, QuestKind, Store, UnlockedBadge, WeeklyQuests, XpEvent};
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

fn award_xp(profile: &mut Profile, amount: u64, reason: String, now: DateTime<Utc>, events: &mut Vec<GameEvent>) {
    if amount == 0 {
        return;
    }
    let before = level_for(profile.xp);
    profile.xp += amount;
    profile.xp_log.push(XpEvent { at: now, amount, reason });
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
    let is_run = match (&goal.kind, &data) {
        (GoalKind::Counter { .. }, EntryData::Count { .. }) => false,
        (GoalKind::Performance { .. }, EntryData::Run { .. }) => true,
        _ => return Err("This update does not match the goal type".into()),
    };
    let positive = match data {
        EntryData::Count { delta } => delta > 0.0,
        EntryData::Run { .. } => true,
    };

    let before = goal.progress();
    let had_runs = !goal.entries.is_empty();
    goal.entries.push(Entry { at: now, data });
    let after = goal.progress();
    let name = goal.name.clone();

    let mut rewards: Vec<(u64, String)> = Vec::new();

    if positive {
        let base = if is_run { XP_RUN } else { XP_UPDATE };
        let mut xp = base + ((after - before).max(0.0) * XP_FULL_PROGRESS).round() as u64;
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
        award_xp(&mut store.profile, xp, reason, now, &mut events);
    }
    check_quests(store, now, &mut events);
    check_badges(store, &flags, now, &mut events);
    Ok(events)
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
        quests.push(Quest { kind: QuestKind::ActiveDays, target: 4.0, xp: XP_ACTIVE_DAYS_QUEST, completed: false });
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
    let mut active: Vec<&Goal> = store.goals.iter().filter(|g| !g.is_completed()).collect();
    fastrand::shuffle(&mut active);
    active
        .into_iter()
        .take(count)
        .map(|g| match g.kind {
            GoalKind::Counter { target, .. } => {
                let last_week = g.amount_between(Some(week.start - Duration::days(7)), Some(week.start)).max(0.0);
                let remaining = (target - g.current()).max(1.0).ceil();
                let amount = (last_week * 1.2).ceil().max((target * 0.05).ceil()).max(1.0).min(remaining);
                Quest { kind: QuestKind::AddToGoal { goal_id: g.id }, target: amount, xp: XP_GOAL_QUEST, completed: false }
            }
            GoalKind::Performance { .. } => {
                Quest { kind: QuestKind::LogRuns { goal_id: g.id }, target: 2.0, xp: XP_GOAL_QUEST, completed: false }
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

fn check_quests(store: &mut Store, now: DateTime<Utc>, events: &mut Vec<GameEvent>) {
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
        }
        store.profile.quests_completed += 1;
        events.push(GameEvent::QuestCompleted { quest: title.clone(), xp });
        award_xp(&mut store.profile, xp, format!("Quest: {title}"), now, events);
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
