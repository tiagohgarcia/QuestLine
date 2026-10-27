# Questline

A gamified goal tracker for Linux and Windows, built with Tauri (Rust backend, HTML/CSS/JS UI).

## Prerequisites (Linux)

- Rust (via [rustup](https://rustup.rs)), with `cargo` on your PATH.
  On zsh, add `. "$HOME/.cargo/env"` to `~/.zshrc`; zsh does not read `~/.profile`.
- Node.js 18+ and npm.
- WebKitGTK and build tools: `libwebkit2gtk-4.1-dev`, `build-essential`, `libssl-dev`, `libayatana-appindicator3-dev`, `librsvg2-dev`.

Then install the Tauri CLI into `node_modules/`:

```sh
npm install
```

## Development

```sh
npm run dev
```

This runs a debug build and reloads as you edit.
The first run compiles all Rust dependencies and takes a few minutes; later runs are fast.

Dev mode uses the same save file as the release app.
To experiment without touching your real progress, point it at a separate data folder:

```sh
QUESTLINE_DATA_DIR=./data-dev npm run dev
```

## Release build

```sh
npm run tauri build -- --no-bundle
```

This produces an optimized binary at `src-tauri/target/release/questline`.
`--no-bundle` skips the `.deb` package; drop it to also get one in `src-tauri/target/release/bundle/`.
After changing the code, run the same command again and restart the app.

## Desktop launcher

Create `~/.local/share/applications/questline.desktop` to add Questline to the app menu:

```ini
[Desktop Entry]
Type=Application
Name=Questline
Comment=Gamified goal tracker
Exec=/path/to/progress_bar/src-tauri/target/release/questline
Icon=/path/to/progress_bar/src-tauri/icons/icon.png
Terminal=false
Categories=Utility;
StartupWMClass=questline
```

For a desktop icon, copy the same file to `~/Desktop/`, then make it executable and trusted:

```sh
chmod +x ~/Desktop/questline.desktop
gio set ~/Desktop/questline.desktop metadata::trusted true
```

Both launchers point at the release binary, so a new release build updates them automatically.

## Windows

The Windows installer is built on GitHub, so no Windows build tools are needed:

1. Push your changes to GitHub.
2. Open the repository's **Actions** tab, pick **Windows build** and click **Run workflow**.
3. When the run finishes (about 10–15 minutes), download `questline-windows-installer` from the run's page, unzip it and run the `.exe`.

The installer adds Questline to the Start menu, and running a newer one updates it in place.
The app isn't code-signed, so Windows SmartScreen may warn about it: click **More info**, then **Run anyway**.
It needs the WebView2 runtime, which is built into Windows 10 and 11.

## Data

Progress is saved to `questline.json` in the per-user app data folder:

- Linux: `~/.local/share/dev.questline.app/`
- Windows: `%APPDATA%\dev.questline.app\`

Dev and release builds share it.
Set `QUESTLINE_DATA_DIR` to store it elsewhere.
Linux and Windows each keep their own progress.

Earlier builds saved to `data/questline.json` inside this folder.
On first start, the app copies that file to the new location if there is no save there yet; the old file stays as a backup.

- Saves are atomic: the app writes a temp file and renames it.
- If the file can't be parsed, it is moved to `questline.corrupt-<timestamp>.json` instead of being overwritten.
- When adding fields to the data model, mark them `#[serde(default)]` so old save files still load.
- Back up the save file separately; it is not in git.

## How it plays

- **Counter goals** track a number, e.g. 42 / 150 LeetCode problems.
  "Already done" sets a starting value that earns no XP.
- **Performance goals** track a distance in a time, e.g. 5 km in 30 min.
  Progress = (distance / target distance) × min(1, target pace / your pace).
  For example, 3 km in 20 min counts as 54%.
  The goal is complete when a run covers the distance at or below the target pace.
- **XP and levels**: every run earns XP, and a counter update earns XP when it takes the goal above its previous best.
  The amount scales with how much new progress it made.
  Each update has a 10% chance of double XP.
- **Undo, archive and delete**: ↶ undoes a goal's last update and takes back everything it earned, including milestones, completion and quests.
  🗄 archives a goal: it leaves the board but keeps its XP, and can be restored from the Archive section.
  ✕ deletes a goal and takes back all the XP it earned.
  Badges stay once unlocked.
- **Milestones** at 25 / 50 / 75% give bonus XP, and reaching 100% triggers a celebration.
- **Pace rival** (goals with a deadline): a 🏁 marker on the bar shows where you should be today.
- **Ghost**: this week vs. last week at the same moment, per goal and for total XP.
- **Weekly quests**: three quests every Monday, with targets set to beat your ghost.
- **Streaks, badges and the Trophy Room** for completed goals.

## Code map

- `src-tauri/src/model.rs`: data types and progress math
- `src-tauri/src/game.rs`: XP, levels, streaks, quests, badges
- `src-tauri/src/view.rs`: snapshots sent to the UI
- `src-tauri/src/storage.rs`: JSON load/save
- `src-tauri/src/main.rs`: Tauri commands
- `ui/`: the interface (plain HTML/CSS/JS, no bundler)
