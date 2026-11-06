# omarchy-oc panel Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Finish omarchy-oc: a live socket protocol, daemon-run stability tests and a live-edit endpoint in the Rust daemon, plus the `qqq.oc` Omarchy shell panel (icon rail, seven pages, edit model C). The panel replaces the `qqq.fans` widget.

**Architecture:**
- **Daemon:** `omarchy-ocd` (Rust, root) already owns all hardware access and the safety logic. This plan adds a persistent, id-tagged JSON-lines protocol with pushed status and events.
- **Panel:** `qqq.oc` is a thin QML panel. One `Client.qml` holds the socket, and `Model.js` holds every rule that can be unit-tested under node.

**Tech Stack:** Rust 2024 (serde, serde_json, libc; no new crates), Quickshell QML (`Quickshell.Io` `Socket` + `SplitParser`), Omarchy shell kit (`qs.Commons`, `qs.Ui`: `Panel`, `KeyboardPanel`, `WidgetButton`), node 26 `node:test`.

**Spec:** `docs/superpowers/specs/2026-11-05-omarchy-oc-panel-design.md`

## Global Constraints

- **The panel never spawns `omarchy-oc` or any other process.** All control goes over `/run/omarchy-oc/control.sock`, overridable with the plugin setting `socketPath`.
- **Socket limits:** 16 connections, 256 KiB per request line, mode 0660 root:wheel.
- **Status history:** pushed status carries a 120-sample history tail; the full history is sent on request.
- **Timings:**

  | What | Value |
  |---|---|
  | Debounce for live (safe) edits | 250 ms after the last change |
  | Reconnect retry | every 2 s |
  | Default Keep/Revert window | 20 s |

- **Safe keys:** `cpu.*`, `gpu.min_mhz`, `gpu.max_mhz`, `gpu.power_profile`, `gpu.power_window_ms`, `gpu.power_w` ≤ stock, `platform.aspm`, `fans.*`.
- **Risky keys:** `smu.*`, `gpu.power_w` > stock.
- **Built-in profiles** (`stock eco balanced performance max`) are read-only. The first edit copies the active built-in into `custom` and switches to it.
- **Text:** all daemon-provided text goes into `Text.PlainText`. Motion uses `Style.duration(ms)`, ≤ 300 ms.
- **Panel shape:** icon rail on the left, about 540 px wide. Pages, in rail order: Overview, CPU, GPU, Memory, Fans, Auto, Test.
- **`qqq.fans`** is disabled at install; `omarchy-fand` keeps running.
- **Git identity is repo-local** (already set). Commits end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

1. **Daemon restarts while the panel is open.** The panel shows "offline", reconnects within 2 s, re-subscribes, and keeps staged edits. Tested in Task 1 (`resubscribe_after_reconnect`) and Task 4 (`Model.reconnect`).
2. **An auto rule switches the active profile while risky edits are staged.** Staged edits stay bound to the profile they were made on, and Apply targets that profile, not the new active one. Tested in Task 2 (`edit_targets_named_profile`) and Task 4 (`stageKeepsProfile`).
3. **A slow fan command (2-minute calibrate) on the shared connection.** Other requests on the same connection still answer promptly. Tested in Task 1 (`slow_fan_request_does_not_block`).
4. **Starting a test while settings are pending, or while a test already runs.** The daemon refuses with a clear message, and the panel shows it next to the button. Tested in Task 3 (`test_start_refused_when_pending_or_running`).
5. **Markup-shaped text from processes** (a GPU client or rule program named `<img src=x>`). It renders literally. Tested in Task 4 (`plainNeutralisesMarkup`); PlainText is used everywhere.

---

### Task 1: Socket protocol v2

**Files:**
- Modify: `src/daemon.rs` (split the socket server out)
- Create: `src/server.rs`: the connection loop, the `Hub`, and the fan worker
- Modify: `src/lib.rs` (`pub mod server;`), `src/bin/omarchy-ocd.rs` (use `server::run`)
- Test: `tests/protocol.rs` (reuse the fake-tree builder from `tests/safety.rs`; move it to `tests/common/mod.rs`)

**Interfaces:**
- Produces:
  - `pub struct Hub` with:
    - `pub fn new() -> Arc<Hub>`;
    - `pub fn subscribe(&self) -> std::sync::mpsc::Receiver<String>`;
    - `pub fn publish(&self, line: String)`, which drops any subscriber whose receiver has hung up.
  - `pub fn serve(core: Arc<Mutex<Core>>, hub: Arc<Hub>, sock: &Path, group: &str) -> io::Result<()>`: binds, sets permissions, and spawns the accept thread.
  - `pub fn run(sys: Sys, paths: Paths) -> io::Result<()>`: replaces `daemon::run`. It ticks once a second and publishes `{"event":"status", ...status(120)}` plus queued events.
  - `Core::push_event(&mut self, ev: Value)` and `Core::take_events(&mut self) -> Vec<Value>` in `daemon.rs`. Events: `{"event":"pending",...}` when pending is set or cleared, and `{"event":"alert","text":...}`.
- Wire format, one JSON object per line:
  - Request: `{"id":N,"cmd":...}`. Reply: `{"id":N,"ok":true,"result":...}` or `{"id":N,"ok":false,"error":"..."}`.
  - A request without `id` gets a reply without `id`, and then the connection closes (CLI compatibility).
  - `{"cmd":"subscribe"}` replies `{"id":N,"ok":true}`, sends an immediate status event, then pushes every tick.

- [ ] **Step 1: Write failing tests in `tests/protocol.rs`**
  - `ids_round_trip`: send `{"id":1,"cmd":"status"}` then `{"id":2,"cmd":"apply","profile":"performance"}` on one connection. Both replies arrive with matching ids, and `result.profile == "performance"`.
  - `oneshot_without_id_closes`: send `{"cmd":"status"}`. Read one reply line, then EOF.
  - `subscribe_pushes_status`: subscribe, call `tick_and_publish` (Step 3) twice, read two lines with `event == "status"` and `history.t.len() <= 120`.
  - `resubscribe_after_reconnect`: drop the connection, open a new one, subscribe. A status arrives.
  - `slow_fan_request_does_not_block`:
    - Point `OMARCHY_FANS_RUN` at a temp dir with a fake fan socket that sleeps 3 s before replying.
    - Send `{"id":1,"cmd":"fans","req":{"cmd":"calibrate"}}` then `{"id":2,"cmd":"status"}`.
    - The reply to id 2 arrives before the reply to id 1, and within 1 s.
  - `oversize_line_rejected`: a 300 KiB line gets `ok:false`, and the daemon keeps serving others.
- [ ] **Step 2: Run them.** `cargo test --offline --test protocol`. Expected: compile errors or FAIL (no `server` module).
- [ ] **Step 3: Implement `src/server.rs`.**
  - One reader thread per connection. Replies go through a per-connection `mpsc::Sender<String>` that a writer thread drains, so pushes and replies never interleave mid-line.
  - `fans` requests run on a spawned thread that sends their reply when done.
  - Expose `pub fn tick_and_publish(core: &Arc<Mutex<Core>>, hub: &Hub)` for tests and the main loop.
  - Remove the old `serve_client`, `TakeLine` and `run` from `daemon.rs`; `restore` stays.
- [ ] **Step 4: Run all tests.** `cargo test --offline`. Expected: every test passes (27 unit + 5 safety + 6 protocol).
- [ ] **Step 5: Check the CLI still works one-shot.** `cargo build --offline`, run the daemon as yourself (`OMARCHY_OC_RUN=$d/run ./target/debug/omarchy-ocd run --run-dir $d/run --state-dir $d/state --group qqq --kmsg /dev/null &`), then `OMARCHY_OC_RUN=$d/run ./target/debug/omarchy-oc profiles`. Expected: the profile list prints.
- [ ] **Step 6: Commit.** `git add -A src tests && git commit -m "Daemon: persistent id-tagged socket protocol with status/event push"`

### Task 2: Live-edit endpoint

**Files:**
- Modify: `src/daemon.rs` (new `edit` command, `Core::risky_keys`)
- Modify: `src/tune.rs` (`pub fn key_is_risky(key: &str, value: &Value, stock_gpu_w: Option<f64>) -> bool`)
- Test: `tests/safety.rs`

**Interfaces:**
- Consumes: `Tune::set_key`, `Config::profile`, and `Core::apply_profile(name, Origin::User)` (existing).
- Produces the request `{"cmd":"edit","profile":P,"changes":{"cpu.epp":"power",...},"apply":bool}`, which returns `{"profile": <name actually edited>, "pending": bool, "report": {...}}`:
  - If `P` is built in, it is copied to `custom` (overwriting it) and `custom` is edited and becomes active.
  - Any risky change without `"apply":true` → error `"risky changes need Apply"`.
  - If `P` is not active and no risky key is present, the change is saved but not applied.
  - The Keep/Revert previous is the profile active before the edit.

- [ ] **Step 1: Write failing tests in `tests/safety.rs`.**
  - `edit_builtin_forks_custom`: active `balanced`. `edit {profile:"balanced", changes:{"cpu.epp":"power"}}` → `result.profile == "custom"`, active `custom`, EPP file `power`, `custom.cpu.governor == "powersave"` (copied from balanced).
  - `edit_risky_needs_apply`: `edit {profile:"custom", changes:{"smu.co_all":"-10"}}` → `Err` containing `"risky changes need Apply"`. With `"apply":true` → `pending == true`.
  - `edit_targets_named_profile`: active `performance`. Edit `custom` with `gpu.max_mhz 2000` → active stays `performance`, and `custom.gpu.max_mhz == 2000`.
  - `gpu_power_risk_is_relative_to_stock`: `key_is_risky("gpu.power_w", json!(140.0), Some(150.0)) == false` and `json!(160.0)` → `true`.
- [ ] **Step 2: Run them.** `cargo test --offline --test safety`. Expected: FAIL.
- [ ] **Step 3: Implement `key_is_risky` and the `edit` arm.** Validate the whole batch with `set_key` on a clone before saving anything, so it's all or nothing.
- [ ] **Step 4: Run them.** `cargo test --offline`. Expected: PASS.
- [ ] **Step 5: Commit.** `git commit -am "Daemon: live edit endpoint with risk gating and built-in fork"`

### Task 3: Stability tests run by the daemon

**Files:**
- Create: `src/testrun.rs`: spawns the child and parses its JSON lines.
- Modify: `src/daemon.rs` (`test-start`, `test-stop`; reuse the existing stress-begin and stress-end recording via `Core::record_test(kind, &Value)`), `src/server.rs` (publish test events), `src/lib.rs`
- Modify: `src/bin/omarchy-oc.rs` (`stress` keeps working standalone, but it must not call `stress-begin`/`stress-end` when `OMARCHY_OC_CHILD=1`)
- Test: `tests/protocol.rs` (a fake test binary written by the test as a shell script)

**Interfaces:**
- Consumes: `Hub::publish`; the existing stability recording (`stress-end` logic, moved into `Core::record_test`).
- Produces:
  - `pub struct TestRun` with:
    - `pub fn start(kind: &str, seconds: u32, bin_dir: &Path) -> io::Result<TestRun>`;
    - `pub fn poll(&mut self) -> Vec<Value>`, non-blocking: progress lines tagged `"kind"`;
    - `pub fn stop(&mut self)`, which sends SIGINT;
    - `pub fn finished(&mut self) -> Option<Value>`: the final `done:true` line plus its exit code.
  - Requests `{"cmd":"test-start","kind":"cpu"|"cpu-per-core"|"gpu","seconds":N}` and `{"cmd":"test-stop"}`.
  - Events `{"event":"test-progress",...line}` and `{"event":"test-done","passed":bool,"errors":N,"kind":...}`.
  - `Paths.bin_dir: PathBuf` (default `/usr/local/bin`, flag `--bin-dir`).
  - Child commands:
    - `cpu`: `omarchy-oc stress cpu --json --seconds N`
    - `cpu-per-core`: `... --per-core --core-seconds max(10, N/cores)`
    - `gpu`: `omarchy-oc-gpustress --json --seconds N`
  - All children run with env `OMARCHY_OC_CHILD=1`.

- [ ] **Step 1: Write failing tests.**
  - `test_runs_and_records`: the fake binary prints two progress lines and `{"done":true,"passed":true,"errors":0}`, exit 0. Events arrive in order. After done, `status.profiles[active].cpu_test.passed == true` and `status.stress` is null.
  - `test_failure_recorded`: the fake binary prints `{"done":true,"passed":false,"errors":3}`, exit 1 → `cpu_test.passed == false`, `errors == 3`.
  - `test_start_refused_when_pending_or_running`: with a test running, a second `test-start` → `Err` containing `"already running"`. With Keep/Revert pending → `Err` containing `"confirm or revert"`.
  - `test_stop_interrupts`: the fake binary sleeps until SIGINT, then prints done. `test-stop` produces a `test-done` event within 2 s.
  - The dirty marker exists while a test runs and is gone after `test-done`.
- [ ] **Step 2: Run them.** `cargo test --offline --test protocol`. Expected: FAIL.
- [ ] **Step 3: Implement `testrun.rs`, the daemon arms and the tick polling.** The tick calls `poll` and pushes events.
- [ ] **Step 4: Run them.** `cargo test --offline`. Expected: PASS.
- [ ] **Step 5: Commit.** `git commit -am "Daemon: run CPU/GPU stability tests as children, stream progress"`

### Task 4: Plugin skeleton: client, model, bar widget, rail, Overview

**Files:**
- Create: `plugin/manifest.json`
  - Copy the shape of `~/Work/omarchy-fans/plugin/manifest.json`.
  - id `qqq.oc`, name "Overclock", kinds `["bar-widget"]`, entry `Panel.qml`, `defaultSection` `right`.
  - Settings schema: `barMode` enum `["Temps","Clocks","Icon"]` (default `Temps`), `socketPath` string (default `/run/omarchy-oc/control.sock`), `favourites` string (default `eco,balanced,max`).
- Create: `plugin/Client.qml`, `plugin/Model.js`, `plugin/Panel.qml`, `plugin/pages/Overview.qml`, `plugin/components/{Rail.qml,Tile.qml,Sparkline.qml,Chip.qml}`, `plugin/i18n/en.json`
- Create: `plugin/tests/model.test.js`
- Create: `install.sh` (pattern of `~/Work/omarchy-fans/install.sh`): `--plugin` installs only the plugin; a plain run does `cargo build --release` for both crates, then `system/install-system.sh` via `as_root`, then the plugin; `--uninstall`. The plugin step also runs `omarchy plugin disable qqq.fans || true`.

**Interfaces:**
- **`Model.js`**: plain functions, same export pattern as the fans `Model.js`, all pure.
  - `RISKY_PREFIXES = ["smu."]`
  - `isRisky(key, value, stockGpuW) -> bool`
  - `stage(staged, profile, key, value) -> staged'`, where `staged = {profile, changes:{}}`; staging for a different profile resets it.
  - `stageKeepsProfile`: covered by the test of that name.
  - `stagedCount(staged) -> int`
  - `splitEdit(changes, stockGpuW) -> {safe:{}, risky:{}}`
  - `plain(s) -> string`: replaces `<` and `>` with `‹` and `›`, for any sink whose text format the plugin can't set.
  - `fmt(v, digits, unit) -> string`: returns "—" for null.
  - `suggestCo(v) -> min(0, v + 5)`
  - `profileIcon(name)`: stock 󰓦, eco 󰌪, balanced 󰾅, performance 󰓅, max 󱐋, other 󰒓.
  - `barLabel(status, alive, mode)`, `tooltip(status, alive)`
  - `nextFavourite(csv, active) -> name`
  - `reconnect(state, nowMs) -> {retryAt}`
- **`Client.qml`**: `QtObject` holding a `Socket`.
  - Properties: `connected`, `status` (the latest status event), `lastEvent`.
  - Signals: `event(var ev)`.
  - Methods: `request(cmd, args, cb)`, which assigns an id, queues if disconnected, and calls `cb(ok, resultOrError)`.
  - It subscribes automatically on connect and retries every 2 s.
- **`Panel.qml`**:
  - Exposes `client`, `page` (string), and `stageBar` state (`staged`) to the pages through properties: `required property var root` on each page.
  - IPC target `qqq.oc` with `open`, `close`, `toggle`, `page(name)`, `keep`, `revert`.

- [ ] **Step 1: Write `plugin/tests/model.test.js`** with these tests:
  - `isRisky`: `smu.co_all` → true. `cpu.epp` → false. `gpu.power_w` 160 with stock 150 → true, 140 → false.
  - `stageKeepsProfile`: stage on `custom`, then stage on `custom` again → 2 changes. Stage on `other` → 1 change, profile `other`.
  - `splitEdit` splits mixed changes correctly.
  - `plainNeutralisesMarkup`: `plain("<img src=x>")` contains no `<`.
  - `fmt(null) === "—"`. `fmt(71.125, 0, "°") === "71°"`.
  - `suggestCo(-20) === -15`. `suggestCo(-3) === 0`.
  - `reconnect` gives `retryAt` = now + 2000.
  - `barLabel` with `pending.remaining` 14 → `"KEEP 14s"`. With `alive=false` → `"OC offline"`. With mode `Temps` → `"󰓅 71° · 52°"`.
  - `nextFavourite("eco,balanced,max", "balanced") === "max"`, and the list wraps around.
- [ ] **Step 2: Run it.** `node --test plugin/tests/model.test.js`. Expected: FAIL (no `Model.js`).
- [ ] **Step 3: Implement `Model.js`.** Run the test → PASS.
- [ ] **Step 4: Implement the rest.**
  - `Client.qml`.
  - `Panel.qml`: bar `WidgetButton` with `barLabel`/`tooltip`. Left click toggles the panel, or keeps pending settings while a countdown shows. Right click applies `nextFavourite(favourites, active)`. `KeyboardPanel` with `Rail` plus a `Loader` for `pages/<Page>.qml`; arrow keys move the rail; Enter keeps pending settings.
  - `Overview.qml`: profile chips, the automation line (`auto_profile` / `base` / `rules`), CPU/GPU/Memory/Fans tiles with sparklines from `status.history`, and pending and alert banners with Keep/Revert buttons.
  - Pages not built yet show a placeholder `Text` "Coming in a later task".
- [ ] **Step 5: Validate and lint.** `omarchy plugin validate plugin && /usr/lib/qt6/bin/qmllint plugin/*.qml plugin/pages/*.qml plugin/components/*.qml`. Expected: validate OK; no qmllint errors (warnings about the `qs.*` imports are acceptable).
- [ ] **Step 6: Live check against a user-mode daemon.**
  - Start `target/release/omarchy-ocd run --run-dir $d/run --state-dir $d/state --group qqq --kmsg /dev/null`, which has real readings and fails writes.
  - Run `./install.sh --plugin`, then set `socketPath` to `$d/run/control.sock` with `omarchy plugin` settings or in shell.json.
  - Run `omarchy restart shell`, then `omarchy-shell qqq.oc open`, then `grim -o HDMI-A-1 /tmp/oc-overview.png`.
  - Expected: the screenshot shows the rail and live Overview tiles with numbers.
  - Kill the daemon: the panel shows offline. Restart it: back within 2 s.
- [ ] **Step 7: Commit.** `git add plugin install.sh && git commit -m "qqq.oc: socket client, model, bar widget, rail and Overview"`

### Task 5: CPU page and shared controls

**Files:**
- Create: `plugin/pages/Cpu.qml`, `plugin/components/{LimitSlider.qml,Segmented.qml,Toggle.qml,StageBar.qml,CoreGrid.qml}`
- Modify: `plugin/Panel.qml` (StageBar sits under every page; Apply sends `edit` with `apply:true`)
- Modify: `plugin/Model.js`, `plugin/tests/model.test.js` (`limitRow(statusSmu, key) -> {value, stock, use, max}`, `coreTiles(status, profileTune) -> [{core, tier, co, test}]`)

**Interfaces:**
- **`LimitSlider`** props: `label`, `value`, `stock` (gold tick), `use` (green bar, may be null), `from`, `to`, `unit`, `disabledReason`. Signal: `moved(real v)`.
- **`StageBar`** reads `root.staged`. Signals: `apply()`, `discard()`.
- **Live (safe) edits:** `root.liveEdit(key, value)` debounces 250 ms, then sends `edit` without `apply`.
- **Risky edits:** `root.stageEdit(key, value)` adds to the staged set.

- [ ] **Step 1: Add tests.**
  - `limitRow` maps `smu.limits.fast_limit` 88 / `fast_value` 38 / `stock.fast_limit` 88 to `{value:88, stock:88, use:38}`, and returns nulls when `limits` is null.
  - `coreTiles` gives 6 tiles for the 6-core status fixture, with tiers `fast` on cores 0 and 4, and `co` from `co_cores` falling back to `co_all`.
- [ ] **Step 2:** `node --test plugin/tests/model.test.js` → FAIL. Implement both → PASS.
- [ ] **Step 3: Build `Cpu.qml`.**
  - Segmented controls for mode and EPP, and toggles for boost and SMT. These are live.
  - A C-state segmented control (`all`, `≤C2`, `≤C1`) mapping to `cpu.idle_max` (null, `C2`, `C1`). Live.
  - Four `LimitSlider`s: PPT fast, PPT slow, Tctl, and TDC/EDC (staged). When `caps.smu` is false they are disabled with `caps.smu_error`.
  - A `CoreGrid` (staged): tapping a core opens a −50…30 stepper; an all-core stepper sits above the grid.
- [ ] **Step 4: Check it.** Validate, lint, `omarchy restart shell`, `omarchy-shell qqq.oc page cpu`, and a screenshot. Expected: it renders.
  - Dragging EPP changes the daemon's `custom` profile (`omarchy-oc show custom` shows it).
  - A CO change shows "1 change" in StageBar.
- [ ] **Step 5: Commit.** `git commit -am "qqq.oc: CPU page, limit sliders, core grid, stage bar"`

### Task 6: GPU and Memory pages

**Files:**
- Create: `plugin/pages/Gpu.qml`, `plugin/pages/Memory.qml`, `plugin/components/Chart.qml`
- Modify: the `Model.js` tests (`gpuClients(status) -> top 6 [{name, vram, busy}]`, sorted by busy, then VRAM; `dimmRows(info) -> rows`)
- No daemon change: the Memory page sends the existing `{"cmd":"info"}` request once on open and reads `mem`.

**Interfaces:**
- **`Chart`** props: `series` (array of `{values, color, label}`), `minY`, `maxY`.
- **GPU page:**
  - The chart shows `gpu_mhz`, `gpu_temp` and `gpu_w` from `status.history.series`.
  - `PL1` is a `LimitSlider`, staged only when above stock (`Model.isRisky`).
  - Min and max clock sliders (range rpn–rp0) are live; the power profile segmented control is live.
  - The client list uses `Text.PlainText`.

- [ ] **Step 1: Add tests.**
  - `gpuClients` drops entries with zero VRAM and zero busy, and caps the list at 6.
  - `dimmRows` handles `dimms: []` with the "Run as root" note and a populated SMBIOS fixture.
- [ ] **Step 2: Run the tests (FAIL), implement, run them (PASS).**
- [ ] **Step 3: Build both pages.** Then validate, lint, restart the shell and take screenshots of `page gpu` and `page memory`. Expected: the chart moves while llama-server or any GPU load runs, and the client list shows `llama-server`.
- [ ] **Step 4: Commit.** `git commit -am "qqq.oc: GPU page with chart and clients, Memory page"`

### Task 7: Fans page (absorbs qqq.fans)

**Files:**
- Create: `plugin/pages/Fans.qml`. Create `plugin/components/CurveEditor.qml`, ported from `~/Work/omarchy-fans/plugin/CurveEditor.qml`: same drag behaviour, with its `saveCurve` routed through `client.request("fans", {req:{cmd:"curve",...}})`.
- Modify: `plugin/Model.js` and its tests. Port `curveTabs`, `findTab` and `pointsArg` from the fans `Model.js` along with their tests.

**Interfaces:**
- Consumes: the `fans` proxy (`{"cmd":"fans","req":{...}}`) for `status`, `profile`, `curve`, `calibrate`, `retest` and `mode`.
- The page requests the full omarchy-fand status every 2 s while it is open. Live RPMs come from the pushed `status.fans`.

- [ ] **Step 1: Port the tests for `curveTabs`, `findTab` and `pointsArg`** from `~/Work/omarchy-fans/plugin/tests/model.test.js`. Run them → FAIL. Port the functions → PASS.
- [ ] **Step 2: Build `Fans.qml`.**
  - A fan-profile segmented control (live) and the RPM list.
  - Curve tabs per role and sensor, with the CurveEditor.
  - Calibrate needs a second click within 4 s to confirm, as in the fans widget. Re-test buttons for fans that ignore PWM. A BIOS/auto mode toggle.
- [ ] **Step 3: Check it.** Validate, lint, restart the shell, then `page fans`. Expected: dragging a point changes `omarchy-fans status` for that curve, and choosing "silent" switches the omarchy-fand profile.
- [ ] **Step 4: Commit.** `git commit -am "qqq.oc: Fans page with curve editor via omarchy-fand"`

### Task 8: Auto page

**Files:**
- Create: `plugin/pages/Auto.qml`, `plugin/components/RuleRow.qml`
- Modify: `src/daemon.rs`, adding the request `{"cmd":"processes"}` → `[{name}]`, unique names from `processes()` minus kernel threads, sorted, max 200. Plus a test in `tests/safety.rs`: `processes_lists_unique_names`, using the fake `/proc` with two `bash` and one `blender` → `["bash","blender"]`.
- Modify: `plugin/Model.js` and its tests: `ruleLabel(matcher)` maps `steam-game` → "Any Steam game", `gamemode` → "GameMode games", and `process:x` → "x".

- [ ] **Step 1: Write the Rust and node tests.** Run them → FAIL. Implement → PASS (`cargo test --offline`, `node --test ...`).
- [ ] **Step 2: Build `Auto.qml`.**
  - The auto toggle and the rules list (delete, plus profile pick per rule).
  - "Add rule": a picker with Any Steam game, GameMode, and the running programs from `processes`.
  - An "Otherwise" fallback profile row, which sets `base` via `apply`.
  - Toggles for apply-at-boot and the watchdog, with steppers for CPU and GPU limits and a watchdog fallback-profile picker (the `watchdog` request with `fallback`).
  - Keep/Revert window choices of 10, 20 and 60 s, set with `confirm-seconds`.
- [ ] **Step 3: Check it.** Validate, lint, restart the shell. Add a rule for a running program and set its profile to `performance`. Expected: within 2 s the Overview line reads "‹program› → performance".
- [ ] **Step 4: Commit.** `git commit -am "qqq.oc: Auto page (rules, boot apply, watchdog)"`

### Task 9: Test page

**Files:**
- Create: `plugin/pages/Test.qml`
- Modify: `plugin/Model.js` and its tests, adding `testView(events) -> {phase, pct, cores:[{core, state:"ok"|"bad"|"run"|"idle", errors}], suggestion:{core, co}|null}`.

- [ ] **Step 1: Write the tests.** `testView` over a recorded event sequence gives:
  - 46 % progress;
  - core 2 `bad` with 3 errors;
  - a suggestion `{core:2, co:-15}` when the profile has core 2 at −20.

  Run them → FAIL, implement → PASS.
- [ ] **Step 2: Build `Test.qml`.**
  - Three start buttons (CPU all cores, CPU per core, GPU) and length choices of 2, 10, 30 and 60 min.
  - A progress bar, a live `CoreGrid` in result mode, and Stop.
  - The suggestion's "Set −N" button calls `root.stageEdit("smu.co_cores.N", v)`.
  - Results history from `status.profiles[*].cpu_test` / `gpu_test`.
  - Errors from `test-start` (pending, already running) show next to the buttons.
- [ ] **Step 3: Check it.** Validate, lint, restart the shell. Start a 2-minute CPU test from the panel, close and reopen it; expected: progress is still running. Stop it; expected: the history shows the stopped run.
- [ ] **Step 4: Commit.** `git commit -am "qqq.oc: Test page with live per-core results and CO suggestions"`

### Task 10: Install as root and verify on real hardware

**Files:**
- Modify: `README.md` (create it). Cover install, what is controllable, the safety model, LACT comparison, uninstall, and the out-of-scope list from the spec.

- [ ] **Step 1: The user runs** `! cd ~/Work/omarchy-oc && ./install.sh`, which asks for the sudo password. Expected: `systemctl status omarchy-ocd` shows active, `qqq.fans` is disabled, and `qqq.oc` is in the bar.
- [ ] **Step 2: Probe.** `omarchy-oc probe`. Expected: `smu: true`. `pm_table` is either a limits object or a clear `pm_error`. Record which in the README.
- [ ] **Step 3: Round trip.** `omarchy-oc apply performance`, then `cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor` → `performance`. Then `omarchy-oc apply stock` → back to the stock value.
- [ ] **Step 4: Overclock round trip from the panel.**
  1. Stage CO all −5 and Apply. Expected: Keep/Revert shows in the panel and the bar.
  2. Let it time out. Expected: revert to the previous profile.
  3. Apply again and Keep.
  4. Run a 2-minute CPU test and a 1-minute GPU test (ask the user first to stop GPU jobs such as llama-server). Expected: both pass, and the profile shows tested ✓.
- [ ] **Step 5: Screenshots** of every page on both monitors with `grim -o HDMI-A-1` / `grim -o HDMI-A-2`. Check each against the mockups.
- [ ] **Step 6: Commit.** `git add -A && git commit -m "README; verified on Ryzen 5 8500G + Arc A580"`
