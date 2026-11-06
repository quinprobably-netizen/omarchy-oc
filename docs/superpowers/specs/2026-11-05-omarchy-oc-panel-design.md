# omarchy-oc: overclocking panel design

Date: 2026-11-05. Status: design approved in brainstorming; spec awaiting review.

## Goal

One Omarchy bar widget that monitors and tunes everything on this machine
that Linux can reach (Ryzen 5 8500G, Arc A580 on `xe`, DDR5, Gigabyte
B650M D3HP, omarchy-fand fans), better than LACT on every function LACT
offers for this hardware, with no command line needed.

Priorities, from the user, in order: automate it (rules, boot apply,
watchdog), monitor live, switch profiles, actively tune.

## Decisions

| Topic | Decision |
|---|---|
| UI form | Omarchy shell panel only (bar widget + drop-down). No separate app window, no web UI. |
| Layout | Icon rail on the left, page on the right, ~540 px wide (mockup C). |
| Pages | Overview · CPU · GPU · Memory · Fans · Auto · Test |
| Fans | Absorbed: the OC panel takes over fan profiles, RPM, curve editing and calibration. The `qqq.fans` bar widget is retired; `omarchy-fand` stays as the fan backend. |
| Edit model | C: safe keys apply live (debounced); risky keys are staged behind Apply, then Keep/Revert. |
| Panel ↔ daemon | One persistent Unix-socket connection (Quickshell `Socket`), live status stream pushed by the daemon. No CLI subprocesses. |
| Built-in profiles | Read-only. The first edit while a built-in is active copies it into `custom` and switches to it. |
| CLI | `omarchy-oc` stays as a scripting tool; the panel never uses it. |

## Architecture

### omarchy-ocd (Rust, root, systemd)

Already built and tested (27 unit + 5 integration tests):

- `cpu.rs`: topology (Zen 4 vs Zen 4c tiers from CPPC ranking), readings
  (per-thread MHz/util, Tctl, RAPL package/core W), knobs (amd-pstate mode,
  governor, EPP, boost, min/max MHz per tier, SMT, deepest C-state).
- `smu.rs`: SMU mailbox via the host bridge's SMN window (PCI config
  0xB8/0xBC), Phoenix-family commands (STAPM/fast/slow PPT, Tctl, TDC/EDC,
  SoC TDC/EDC, Curve Optimizer all-core and per-core), PM table read via
  /dev/mem for live limits and values.
- `gpu.rs`: xe identity (name, board vendor, GSC firmware, ReBAR, real
  PCIe slot link), readings (act/cur MHz, pkg/VRAM °C, W from energy,
  mV, busy % from GT idle residency, fans, throttle reasons), per-process
  VRAM and engine use from DRM fdinfo, knobs (min/max MHz, PL1 W and
  window, power profile).
- `mem.rs`: /proc/meminfo, SMBIOS type 17, DDR5 SPD + XMP/EXPO decode,
  DIMM temps (none on this board: the BIOS reserves the SMBus).
- `platform.rs`: ASPM policy, every hwmon temperature and voltage.
- `fans.rs`: omarchy-fand status and control proxy.
- `apply.rs`: once-per-boot stock snapshot in /run, ordered writes with
  min/max pair retry, restore-all.
- `daemon.rs`: profiles, Keep/Revert, crash guard (dirty/running markers
  with boot id), boot apply only for kept overclocks, auto-switch rules
  (`process:<glob>`, `steam-game`, `gamemode`), watchdog (temperature,
  machine checks, GPU hangs → fallback profile), resume re-apply, history
  (1 h), Prometheus file + optional loopback HTTP.
- `stress.rs`: CPU error-checking test (AVX-512/AVX2 FMA, integer, memory
  walk; bit-exact against a golden reference; per-core attribution).
- `gpustress/`: `omarchy-oc-gpustress` (wgpu compute A/B + CPU-reference
  checks, VRAM pattern test).

### To build

1. **Socket protocol v2** (`daemon.rs`): persistent connections,
   newline-delimited JSON.
   - Request `{"id": 7, "cmd": "...", ...}` → reply `{"id": 7, "ok": true, "result": ...}` or `{"id": 7, "ok": false, "error": "..."}`.
   - `{"cmd": "subscribe"}` → daemon pushes `{"event": "status", ...}` every tick (history tail limited to 120 samples; the full history on request) and immediate events: `pending`, `alert`, `test-progress`, `test-done`.
   - Requests without `id` keep working one-shot (CLI compatibility).
   - Fan commands (`{"cmd": "fans", "req": {...}}`) run on a worker thread so a 2-minute calibration never blocks other requests on the connection.
   - Limits: 16 connections, 256 KiB per request line, socket 0660 root:wheel.
2. **Tests run by the daemon**: `test-start {kind: cpu|cpu-per-core|gpu, seconds}`, `test-stop`. The daemon spawns `omarchy-oc stress … --json` or `omarchy-oc-gpustress --json` as a child, relays progress as events, records results against the active profile (existing `stress-begin/end` logic), and keeps the crash-guard marker set while a test runs. One test at a time.
3. **Live-edit endpoint**: `edit {profile, changes: {key: value}}` applies a batch of keys to a user profile (copying a built-in into `custom` first and switching to it) and re-applies. The daemon classifies keys; if any change is risky, the batch is refused unless `"apply": true`, in which case it opens the Keep/Revert window. The panel only sends safe keys live and sends the staged risky set with `"apply": true`.
4. **Fan profile in tunes stays** (`fans.profile`), so OC profiles can also pick a fan profile.

### qqq.oc plugin (QML, thin)

```
plugin/
  manifest.json        bar-widget, id qqq.oc
  Panel.qml            bar widget + KeyboardPanel, rail, page loader
  Client.qml           Socket, reconnect (2 s), request ids/callbacks, latest status, events
  pages/Overview.qml   profile chips, automation line, CPU/GPU/Memory/Fans tiles with sparklines, alert/pending banners
  pages/Cpu.qml        mode/EPP/boost/SMT/C-state controls, SMU limit sliders (stock tick + live use), Curve Optimizer core grid
  pages/Gpu.qml        live chart, PL1/min/max sliders, power profile, GPU clients list
  pages/Memory.qml     usage, DIMM table (SMBIOS/SPD), EXPO/XMP info, note that timings are BIOS-only
  pages/Fans.qml       fan profiles, RPM list, curve editor (ported from qqq.fans CurveEditor.qml), calibrate/re-test
  pages/Auto.qml       auto on/off, rules (add from running programs), fallback, boot apply, watchdog, Keep/Revert window
  pages/Test.qml       CPU all-core / per-core / GPU, length, live per-core grid, suggestions, history
  components/          Slider (stock tick, use bar), Segmented, Toggle, Sparkline, Chart, StageBar
  Model.js             pure helpers: key risk classification, staged diff, debounce bookkeeping, formatting, markup neutralising
  i18n/en.json
  tests/model.test.js  node tests
```

## Behaviour

- **Bar widget**:
  - Shows the profile icon and two readings, configurable (`Temps` or `Clocks`).
  - Left click opens the panel; right click cycles three favourite profiles (setting).
  - While settings are pending it shows `KEEP 14s`, and a click keeps them.
  - Urgent colour on alert or offline.
- **Edit model C**:
  - Safe keys apply ~250 ms after the last change: `cpu.*`, `gpu.min_mhz`, `gpu.max_mhz`, `gpu.power_profile`, `gpu.power_window_ms`, `gpu.power_w` at or below stock, `platform.aspm`, `fans.*` and the fan page.
  - Risky keys go to the stage bar: `smu.*` and `gpu.power_w` above stock. The bar shows "N changes · Discard · Apply".
  - Apply → Keep/Revert countdown, which shows on the page and in the bar.
- **Keyboard**: arrows move through the rail; Tab moves inside a page; Enter keeps pending settings; Esc closes.
- **Test page**:
  - A failing core offers "Set −N", which stages a CO value 5 closer to zero.
  - Results show in the profile list as tested ✓ or ✗.
- **Offline**: the panel shows how to start the daemon. Staged edits survive a reconnect.

## Errors and safety

- Every failed write or SMU response is in the reply's `report` and is shown next to its control.
- **SMU unavailable** (no root, lockdown, unsupported CPU): its controls are disabled with the reason shown.
- **Untested overclocks**: never applied at boot or by rules (implemented).
- **Watchdog**: falls back to the chosen profile on overheating, machine checks or GPU hangs (implemented).
- **Stopping the daemon** restores everything to stock (ExecStopPost `restore`).
- All text from the daemon (process names, messages) is shown with `Text.PlainText`.

## Testing

- **Rust**: existing tests plus protocol v2 tests:
  - ids;
  - subscribe pushes;
  - a slow fan command not blocking others;
  - `edit` risk refusal;
  - test start, progress and done with a fake test binary.
- **Plugin**: `node --test plugin/tests/model.test.js` for risk classification, staged diff, formatting and markup neutralising.
- **Install and real hardware** (needs the user's sudo once):
  1. `./install.sh`, then `omarchy-oc probe` (SMU, PM table);
  2. apply performance, then stock, and check sysfs;
  3. apply custom with CO −5 and Keep/Revert;
  4. run a 2-minute CPU test and a 1-minute GPU test;
  5. screenshot the panel on both monitors.
- **After install**: retire `qqq.fans` (`omarchy plugin disable qqq.fans`), add `qqq.oc` to the bar (right section), run `omarchy restart shell` (plugin hot-reload serves stale QML).

## Out of scope

- **Memory timings and frequency**: BIOS only.
- **Arc clocks above RP0 (2400 MHz) and voltage offset**: no Linux interface. The PCODE OC mailbox is undocumented, so it is not attempted.
- **iGPU (Radeon 740M)**: disabled in BIOS.
- **Remote TCP control**: LACT's has no authentication; we keep loopback metrics only.
