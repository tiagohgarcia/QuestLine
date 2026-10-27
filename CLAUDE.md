# Questline: agent notes

Gamified goal tracker for Linux and Windows.
Tauri v2 app: Rust backend in `src-tauri/`, plain HTML/CSS/JS frontend in `ui/` (no bundler, no framework).
See `README.md` for setup, build commands and the game rules as the user sees them.

## Status

v1 is built and runs: counter and performance goals, categories, XP/levels, milestones, pace rival, ghost, weekly quests, streaks, badges and the Trophy Room.
Undo, archive and XP revocation on delete were added afterwards; they compile but have not been clicked through yet.
It compiles cleanly, and the release binary is used daily through a desktop launcher.
Unit tests cover `model.rs`, `game.rs`, `view.rs` and `storage.rs` (`cargo test` in `src-tauri/`); `main.rs` commands and the frontend have none.
Manual testing so far covered launching and creating a goal; the XP, celebration, quest and badge flows have not been clicked through end to end.

## Architecture

- `model.rs`: persisted types (`Store`, `Goal`, `Entry`, `Profile`, `Quest`) and progress math (`run_score`, `Goal::progress`).
- `game.rs`: all game rules. `apply_entry` is the single path for progress updates: it records the entry, then awards XP, milestones, completion, personal bests, quests and badges, and returns `GameEvent`s.
  Every award is tagged with the `EntryRef` that caused it; `undo_last_entry` and `delete_goal` take tagged XP back through `revoke_xp`.
- `view.rs`: builds the read-only `AppView` snapshot sent to the UI. Derived values (ghost, rival, quest progress) are computed here, not stored.
- `storage.rs`: JSON load/save with atomic writes, and the save file's location.
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
- The save file is `questline.json` in Tauri's app data dir (`~/.local/share/dev.questline.app/` on Linux, `%APPDATA%\dev.questline.app\` on Windows), so dev and release builds share it.
  It holds the user's real progress: never run dev builds against it.
  Use `QUESTLINE_DATA_DIR=<scratch dir>` instead. Unit tests never touch it.
- The old save location `data/questline.json` (found via `CARGO_MANIFEST_DIR`) is copied to the new one on first start; the old file is kept as a backup.
- Windows builds run on GitHub Actions (`.github/workflows/windows.yml`, started by hand) and produce an NSIS installer artifact.
  Windows can't be built from this Linux machine. `tauri.conf.json` lists only `deb`; the workflow passes `--bundles nsis`.
- The first build takes a few minutes; ask the user before running `npm run dev` or a release build.
- The desktop launchers in `~/.local/share/applications/` and `~/Desktop/` point at `src-tauri/target/release/questline`.

## XP rules (decided with the user)

- Counter updates only earn XP above the goal's peak progress (`Goal::peak_progress`), so −1/+1 earns nothing; every run earns its base XP.
- Undo removes the last entry and revokes its XP, milestones, completion and quest completions.
- Deleting a goal revokes all its tagged XP. Archiving keeps the XP and hides the goal; archived goals can be restored or deleted.
- A day stays in `active_days` only while it has XP in the log, so revoking can shorten streaks.
- Badges are never revoked. Levels follow total XP, so they can go down.
- XP earned before tagging existed has no `source` and is never revoked.
- Not addressed on purpose: farming with many tiny completed goals that are kept.

## Future work

### Other ideas discussed but not built

- Seasons: monthly XP compared against your own past seasons.
- Unlockable rewards: color themes, titles and profile icons by level.
- Streak freezes earned through consistency.
- Editing a goal (name, target, deadline); currently it can only be archived or deleted.
- A history view or chart of entries per goal.
- Tests for the Tauri commands in `main.rs` (goal creation validation, `set_progress`).
