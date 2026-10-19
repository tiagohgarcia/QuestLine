#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod game;
mod model;
mod storage;
mod view;

use chrono::{DateTime, NaiveDate, Utc};
use game::GameEvent;
use model::{EntryData, Goal, GoalKind, Store};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::State;

struct AppState {
    store: Mutex<Store>,
    path: PathBuf,
}

/// Every command returns the fresh UI snapshot plus what just happened (XP, badges, ...).
#[derive(Serialize)]
struct Response {
    view: view::AppView,
    events: Vec<GameEvent>,
}

#[derive(Deserialize)]
struct NewGoal {
    name: String,
    category: String,
    kind: String,
    target: Option<f64>,
    unit: Option<String>,
    distance_km: Option<f64>,
    time_min: Option<f64>,
    deadline: Option<String>,
    start_value: Option<f64>,
}

fn mutate(
    state: &AppState,
    f: impl FnOnce(&mut Store, DateTime<Utc>) -> Result<Vec<GameEvent>, String>,
) -> Result<Response, String> {
    let mut store = state.store.lock().map_err(|_| "State lock poisoned")?;
    let now = Utc::now();
    let events = f(&mut *store, now)?;
    game::ensure_quests(&mut store, now);
    storage::save(&state.path, &store)?;
    Ok(Response { view: view::build(&store, now), events })
}

#[tauri::command]
fn get_state(state: State<'_, AppState>) -> Result<Response, String> {
    mutate(&state, |_, _| Ok(Vec::new()))
}

#[tauri::command]
fn create_goal(state: State<'_, AppState>, input: NewGoal) -> Result<Response, String> {
    mutate(&state, move |store, now| {
        let name = input.name.trim().to_string();
        if name.is_empty() {
            return Err("Give your goal a name".into());
        }
        let category = match input.category.trim() {
            "" => "General".to_string(),
            c => c.to_string(),
        };
        let kind = match input.kind.as_str() {
            "counter" => GoalKind::Counter {
                target: input.target.filter(|t| *t > 0.0).ok_or("Target must be greater than 0")?,
                unit: input.unit.unwrap_or_default().trim().to_string(),
            },
            "performance" => GoalKind::Performance {
                distance_km: input.distance_km.filter(|d| *d > 0.0).ok_or("Distance must be greater than 0")?,
                time_min: input.time_min.filter(|t| *t > 0.0).ok_or("Time must be greater than 0")?,
            },
            other => return Err(format!("Unknown goal type: {other}")),
        };
        let deadline = match input.deadline.as_deref().map(str::trim) {
            None | Some("") => None,
            Some(s) => Some(NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|_| "Deadline must be YYYY-MM-DD")?),
        };
        let baseline = match &kind {
            GoalKind::Counter { target, .. } => {
                let start = input.start_value.unwrap_or(0.0).max(0.0);
                if start >= *target {
                    return Err("'Already done' must be below the target".into());
                }
                start
            }
            GoalKind::Performance { .. } => 0.0,
        };

        let mut goal = Goal {
            id: store.next_id,
            name,
            category,
            kind,
            created_at: now,
            deadline,
            baseline,
            entries: Vec::new(),
            completed_at: None,
            milestones_hit: Vec::new(),
        };
        // Progress you already had doesn't pay out milestone XP.
        let reached = goal.progress() * 100.0;
        goal.milestones_hit = game::MILESTONES.iter().copied().filter(|m| reached >= *m as f64).collect();
        store.next_id += 1;
        store.goals.push(goal);
        Ok(Vec::new())
    })
}

#[tauri::command]
fn add_progress(state: State<'_, AppState>, id: u64, delta: f64) -> Result<Response, String> {
    if !delta.is_finite() || delta == 0.0 {
        return Err("Enter a non-zero amount".into());
    }
    mutate(&state, |store, now| game::apply_entry(store, id, EntryData::Count { delta }, now))
}

#[tauri::command]
fn set_progress(state: State<'_, AppState>, id: u64, total: f64) -> Result<Response, String> {
    if !total.is_finite() || total < 0.0 {
        return Err("Enter a valid total".into());
    }
    mutate(&state, |store, now| {
        let current = store.goals.iter().find(|g| g.id == id).ok_or("Goal not found")?.current();
        let delta = total - current;
        if delta.abs() < 1e-9 {
            return Ok(Vec::new());
        }
        game::apply_entry(store, id, EntryData::Count { delta }, now)
    })
}

#[tauri::command]
fn log_run(state: State<'_, AppState>, id: u64, distance: f64, minutes: f64) -> Result<Response, String> {
    if !(distance > 0.0 && minutes > 0.0) {
        return Err("Distance and time must be greater than 0".into());
    }
    mutate(&state, |store, now| {
        game::apply_entry(store, id, EntryData::Run { distance_km: distance, time_min: minutes }, now)
    })
}

#[tauri::command]
fn delete_goal(state: State<'_, AppState>, id: u64) -> Result<Response, String> {
    mutate(&state, |store, _| {
        store.goals.retain(|g| g.id != id);
        Ok(Vec::new())
    })
}

fn main() {
    let path = storage::data_file();
    let store = storage::load(&path);
    tauri::Builder::default()
        .manage(AppState { store: Mutex::new(store), path })
        .invoke_handler(tauri::generate_handler![
            get_state,
            create_goal,
            add_progress,
            set_progress,
            log_run,
            delete_goal
        ])
        .run(tauri::generate_context!())
        .expect("error while running Questline");
}
