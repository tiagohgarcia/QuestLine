# Questline: agent notes

Gamified goal tracker for Linux.
Tauri v2 app: Rust backend in `src-tauri/`, plain HTML/CSS/JS frontend in `ui/` (no bundler, no framework).
See `README.md` for setup, build commands and the game rules as the user sees them.

## Status

v1 is built and runs: counter and performance goals, categories, XP/levels, milestones, pace rival, ghost, weekly quests, streaks, badges and the Trophy Room.
It compiles cleanly, and the release binary is used daily through a desktop launcher.
There are no automated tests yet.
Manual testing so far covered launching and creating a goal; the XP, celebration, quest and badge flows have not been clicked through end to end.

## Architecture

- `model.rs`: persisted types (`Store`, `Goal`, `Entry`, `Profile`, `Quest`) and progress math (`run_score`, `Goal::progress`).
- `game.rs`: all game rules. `apply_entry` is the single path for progress updates: it records the entry, then awards XP, milestones, completion, personal bests, quests and badges, and returns `GameEvent`s.
- `view.rs`: builds the read-only `AppView` snapshot sent to the UI. Derived values (ghost, rival, quest progress) are computed here, not stored.
- `storage.rs`: JSON load/save with atomic writes.
- `main.rs`: Tauri commands. Every command goes through `mutate()`, which locks the store, applies the change, refreshes quests, saves, and returns `{ view, events }`.
- `ui/app.js`: renders the whole page from `view` on every response and plays `events` as toasts, floating XP and confetti.

## Conventions

- Game logic lives in Rust; the frontend only renders and calls commands.
- JSON fields are snake_case on both sides.
- Tauri command arguments are single words (`id`, `delta`, `total`, `input`) to avoid Tauri's camelCase conversion of top-level argument names.
- New persisted fields must have `#[serde(default)]` so existing save files keep loading.
- Times are stored in UTC; days, weeks and streaks use local time via the helpers in `game.rs`.

## Gotchas

- `cargo` is in `~/.cargo/bin` and may not be on PATH in non-interactive shells: prefix commands with `export PATH="$HOME/.cargo/bin:$PATH"`.
- The data file is `data/questline.json`, found via `CARGO_MANIFEST_DIR` at compile time, so dev and release builds share it.
  It holds the user's real progress: never run dev builds or tests against it.
  Use `QUESTLINE_DATA_DIR=<scratch dir>` instead.
- The first build takes a few minutes; ask the user before running `npm run dev` or a release build.
- The desktop launchers in `~/.local/share/applications/` and `~/Desktop/` point at `src-tauri/target/release/questline`.

## Future work

### XP exploits and mistakes (reported by the user)

XP is an append-only log (`Profile.xp_log`) that is never revoked, which causes two problems:

1. **Create-and-delete farming.**
   Create a goal with a target of 1, click +1, delete it, and repeat.
   Each cycle earns the update XP plus the 300 XP completion bonus, and deleting the goal keeps the XP.
   A smaller version: alternating −1 and +1 earns the 10 XP base reward on every +1.
2. **Mistakes can't be undone.**
   If the user adds progress by accident, −1 or "Set total" corrects the number, but the XP, milestones, completion and quest rewards stay.

Possible directions:
- Link each XP award to the entry that caused it, so undoing an entry (or deleting a goal) revokes its XP.
- Add a real "undo last update" action instead of relying on negative deltas.
- Only reward progress above a goal's previous best, so going down and back up earns nothing.
- Scale or delay the completion bonus, e.g. by target size or minimum goal age, so tiny goals aren't worth farming.

Revoking XP can also undo level-ups and badges, so decide how to handle those.

### Other ideas discussed but not built

- Seasons: monthly XP compared against your own past seasons.
- Unlockable rewards: color themes, titles and profile icons by level.
- Streak freezes earned through consistency.
- Editing a goal (name, target, deadline); currently it can only be deleted.
- A history view or chart of entries per goal.
- Unit tests for `game.rs` and `model.rs` (progress math, levels, streaks, quests).
