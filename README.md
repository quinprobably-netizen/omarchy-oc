# omarchy-oc

Overclocking and tuning for an [Omarchy](https://omarchy.org) desktop, from the bar. Built for this machine: Ryzen 5 8500G (Phoenix 2: 2 Zen 4 + 4 Zen 4c cores), Intel Arc A580 on the `xe` driver, DDR5, Gigabyte B650M D3HP, fans driven by [omarchy-fand](https://github.com/quinprobably-netizen/omarchy-fans). It does what LACT does for this hardware and what LACT cannot: CPU and firmware (SMU) limits, Curve Optimizer, Arc fans, a stability test that catches silent computation errors, and a safety net that puts the machine back to stock when an overclock goes wrong.

**Hardware-verified: pending (see [Verified on this machine](#verified-on-this-machine)).**

- **`omarchy-ocd`**: Rust daemon (root, systemd). Samples everything once a second, applies profiles, serves a Unix control socket, and keeps the machine safe (Keep/Revert, crash guard, watchdog).
- **`omarchy-oc`**: CLI for scripting, and the CPU stability tester. The panel never uses it.
- **`omarchy-oc-gpustress`**: GPU stability tester (Vulkan via `wgpu`).
- **`qqq.oc`**: Omarchy shell plugin. Bar widget with live temperatures or clocks, and a drop-down panel: an icon rail on the left, one page on the right. It replaces the `qqq.fans` widget; `omarchy-fand` stays as the fan backend.

Pages:

- **Overview**: profile chips, the automation line, CPU/GPU/Memory/Fans tiles with sparklines, alert and Keep/Revert banners.
- **CPU**: amd-pstate mode, governor, EPP, boost, SMT, C-state limit, SMU limit sliders (stock tick, live use), Curve Optimizer core grid.
- **GPU**: live chart, PL1 and clock sliders, power profile, the processes using the GPU.
- **Memory**: usage, DIMM table from SMBIOS and SPD, EXPO/XMP profiles, and the note that timings are BIOS-only.
- **Fans**: what the `qqq.fans` widget did: fan profiles, RPM list, drag-to-edit curves, calibrate, re-test, hand to BIOS.
- **Auto**: automation on/off, rules (add from running programs), fallback profile, apply at boot, watchdog, Keep/Revert window length.
- **Test**: CPU all-core, CPU per-core, GPU; run length, live per-core grid, suggestions for a failing core, history.

Bar widget: left click opens the panel, or keeps pending settings while they count down; right click cycles the favourite profiles (`eco,balanced,max` by default). It turns urgent when the daemon is offline or has an alert. Settings in `~/.config/omarchy/shell.json` (`qqq.oc` entry): `barMode` is `Temps`, `Clocks` or `Icon`; `favourites`; `socketPath`. Shell IPC: `omarchy-shell qqq.oc toggle`, `open`, `page cpu`, `keep`.

## Install

```sh
./install.sh            # cargo build --release (daemon + gpustress), install daemon (sudo once), add bar widget
omarchy-oc probe        # SMU and PM table diagnostics
```

`./install.sh --plugin` reinstalls only the widget. `./install.sh --uninstall` stops the daemon (every setting goes back to stock), removes the system files and the widget, and leaves `/var/lib/omarchy-oc` (profiles, rules, test history) for you to delete by hand.

The root half (`system/install-system.sh`):

- installs `omarchy-ocd`, `omarchy-oc` and `omarchy-oc-gpustress` into `/usr/local/bin`
- installs `omarchy-ocd.service` (hardened: `ProtectSystem=strict`, `ProtectHome`, loopback only, capabilities limited to sysfs, PCI config, `/dev/mem`, `/dev/kmsg` and other users' `/proc/*/fdinfo`)
- loads `spd5118` (DDR5 SPD hub; gives DIMM temperatures on boards that expose the SMBus, harmless where they don't)
- enables and (re)starts the service

The full install then disables the `qqq.fans` widget and enables `qqq.oc` in the bar's right section. A plugin-only reinstall keeps `qqq.fans`. New QML needs `omarchy restart shell`; the plugin hot-reload serves stale files.

The control socket is `/run/omarchy-oc/control.sock`, mode 0660 `root:wheel`, so the CLI and the panel need a user in `wheel`.

## What you can control on this machine

Every setting lives in a profile. Built-ins (read-only): **stock** (empty: everything as the hardware booted), **eco**, **balanced**, **performance**, **max**. The first edit while a built-in is active copies it into **custom** and switches to it. `omarchy-oc keys` lists every key with its range; `omarchy-oc set <profile> <key> <value|stock>` sets one. Values outside the bounds are rejected, never clamped.

### CPU (`cpu.*`, kernel side, safe to edit live)

- `pstate_mode`: amd-pstate `active`, `passive` or `guided`
- `governor`; `epp` (`performance`, `balance_performance`, `balance_power`, `power`; active mode only)
- `boost` on/off
- `min_mhz`, `max_mhz`, and `max_mhz_dense`: a separate ceiling for the compact Zen 4c cores, which boost lower than the two full Zen 4 cores. The daemon tells the tiers apart from the CPPC preferred-core ranking.
- `smt` on/off
- `idle_max`: the deepest C-state allowed (`POLL`, `C1`, `C2`, `C3`); deeper ones are disabled on every thread

Readings: per-thread MHz and utilisation, Tctl, RAPL package and core watts.

### SMU firmware limits (`smu.*`, risky: staged behind Apply)

The CPU's power-management firmware, not the kernel. The daemon talks to the SMU mailbox through the SMN index/data pair in the host bridge's PCI config space (`0000:00:00.0`, offsets 0xB8/0xBC), so it needs no kernel module: just root and a kernel that is not locked down. The live limits and their current values come from the PM table, read through `/dev/mem`. Supported: Phoenix, Phoenix 2 and Hawk Point (family 19h, models 0x74, 0x75, 0x78: Ryzen 7040/8040 mobile and 8000G desktop). The protocol is the one [FlyGoat/RyzenAdj](https://github.com/FlyGoat/RyzenAdj) documents, written in Rust from the documentation (no code copied).

- `stapm_w`, `fast_w`, `slow_w`: sustained (STAPM), PPT fast and PPT slow power limits, 10–200 W. Stock PPT on this 65 W part is 88 W.
- `tctl_c`: temperature target, 60–100 °C
- `tdc_a`, `edc_a`, `soc_tdc_a`, `soc_edc_a`: core and SoC VRM thermal and peak currents, 10–250 A
- `co_all`: Curve Optimizer for every core, −50…30 (negative is an undervolt)
- `co_cores.<n>`: Curve Optimizer per physical core, applied after `co_all`

Everything set here is volatile: the SMU forgets it on reboot and on suspend (the daemon re-applies after resume). If the SMU is unavailable (no root, lockdown, unsupported CPU) the panel disables these controls and shows why. A limit whose stock value the PM table could not supply can only be restored by a reboot; the daemon says so (`needs_reboot`).

### GPU (`gpu.*`)

Intel Arc on `xe`. Safe to edit live: `min_mhz` and `max_mhz` within the card's RPn–RP0 range, `power_window_ms` (PL1 averaging window), `power_profile` (`base` or `power_saving`). `power_w` is PL1, 30–300 W; it is safe at or below stock and risky above it.

Readings: actual and requested MHz, package and VRAM temperature, watts from the energy counter, millivolts, busy % from GT idle residency, fan RPM, throttle status and reasons. Identity: name, board vendor, GSC firmware, Resizable BAR, the real PCIe slot link. Per-process VRAM and engine use from DRM fdinfo (root sees every process), rescanned every 2 s.

The card's fans are driven by omarchy-fand (through the DG2 PCODE fan table), which is why profiles here carry a fan profile too.

### Memory (read-only)

Live usage from `/proc/meminfo`; SMBIOS type 17 (slots, part numbers, configured speed); the DDR5 SPD EEPROM through `spd5118` nvmem, with the XMP 3.0 and AMD EXPO profile blocks decoded; DIMM temperatures from `spd5118` or `jc42` hwmon, mapped to slots. On this board the BIOS reserves the SMBus, so there are no DIMM sensors. Timings and frequency are BIOS-only (EXPO/XMP); the OS can only watch them.

### Platform (`platform.*`)

`aspm`: PCIe ASPM policy (`default`, `performance`, `powersave`, `powersupersave`), safe to edit live. Readings: every hwmon temperature and voltage the kernel exposes outside the CPU and GPU (motherboard, NVMe, chipset, USB, Ethernet, Wi-Fi), board and BIOS identity, kernel lockdown state.

### Fans (`fans.profile`)

Through omarchy-fand: a profile picks a fan profile (`silent`, `balanced`, `performance`, `full`, `custom`). The Fans page proxies omarchy-fand's own commands (status, profile, curve, tuning, fan, calibrate, retest, mode, rescan); a 2-minute calibration runs on a worker thread and never blocks the panel. If omarchy-fand is not running the fan part of a profile is skipped with a note.

## Safety model

Everything the daemon does to the hardware is reversible, and the risky part is time-boxed.

- **Edit model.** Safe keys apply live, about 250 ms after the last change: `cpu.*`, GPU clocks, window and profile, `gpu.power_w` at or below stock, `platform.aspm`, `fans.*`. Risky keys go to a stage bar ("N changes · Discard · Apply"): every `smu.*` limit, a non-zero Curve Optimizer value, `gpu.power_w` above stock. The daemon enforces the split: a live-edit batch with a risky key is refused unless it says `apply`.
- **Keep/Revert.** Applying a risky profile that has never been kept opens a window (`confirm_seconds`, default 20 s; `omarchy-oc confirm-time`). It counts down on the page and in the bar (`Keep "max"? 14 s`); a bar click, Enter in the panel or `omarchy-oc confirm` keeps it. Otherwise the daemon reverts to the previous profile. Keeping marks the profile *confirmed* for its current risky settings; editing a safe key keeps that record, editing an SMU limit or the GPU power limit clears it.
- **Only kept overclocks apply by themselves.** A risky profile that was never kept is refused at boot and by automation rules ("apply it by hand and keep it once").
- **Crash guard.** While a risky profile is unconfirmed or under test the daemon holds a `dirty` marker (profile name + boot id) in `/var/lib/omarchy-oc`; a `running` marker always names the active profile. On start, a `dirty` marker from an earlier boot means the machine died in that state: the profile's crash count goes up, its confirmation is withdrawn, and the daemon starts at stock with an alert. A `running` marker from an earlier boot naming a risky profile counts an unclean shutdown; at two in a row apply-at-boot is switched off. A risky profile that runs 30 minutes without a crash clears its count. A daemon restart within the same boot just carries on.
- **Watchdog** (on by default). CPU at or above 98 °C or GPU at or above 100 °C for 5 s, or, while a risky profile is active, a machine-check error or a GPU hang/reset in the kernel log (`/dev/kmsg`), switches to the fallback profile (`stock` by default) and raises an alert. A test running at the time is cut short and recorded as failed. `omarchy-oc watchdog [--on|--off] [--cpu-max C] [--gpu-max C] [--seconds N] [--fallback P] [--errors on|off]`.
- **Stopping the daemon restores stock.** The first write to any sysfs knob snapshots its boot value in `/run/omarchy-oc/original.json`; the SMU's stock limits are saved from the PM table. `ExecStopPost=omarchy-ocd restore` puts everything back however the daemon exits, SIGKILL included.
- **Suspend.** The daemon notices a resume (boot-time clock jumping ahead of monotonic) and re-applies the active profile, since the SMU forgets its limits.
- **No changes under test.** While a stability test runs, applying, editing, saving or re-applying the active profile is refused ("stop the test first"), and a test cannot start while settings are pending.
- **Every failed write** is in the apply report and shown next to its control.

## Stability testing

Overclocks that crash are the easy case. The dangerous ones compute wrong answers for hours without crashing, which is what too much Curve Optimizer undervolt does. Both testers check results, not just survival.

**CPU** (`omarchy-oc stress cpu [--seconds N] [--per-core] [--core-seconds N] [--json]`, default 120 s). Each worker runs fixed blocks of AVX-512 (or AVX2) FMA chains, integer hash chains and a cache-busting 8 MiB memory walk, and compares every result bit-for-bit with a golden value computed at start. Errors are attributed to the core that made them. `--per-core` loads one physical core at a time (30 s each by default; the daemon's per-core run divides the total by the core count, never under 10 s per core), so it doubles as a Curve Optimizer tuner in the manner of CoreCycler.

**GPU** (`omarchy-oc stress gpu [--seconds N] [--mode compute|vram|mixed] [--json]`, runs `omarchy-oc-gpustress`). *compute*: a 1M-invocation kernel of integer hash chains and 32 lanes of a chaotic FMA map, run twice with the same inputs and compared bit-for-bit on the GPU, plus 1024 sampled integer chains per seed checked against the CPU. *vram*: fills about 70 % of VRAM with address-dependent patterns (walking ones and zeros, address ^ seed and its inverse, hashes) and verifies every word on the GPU. *mixed* (default) alternates 5 s phases. Dispatches stay about 20 ms so the `xe` job timeout is never approached and the desktop stays usable.

The panel starts tests through the daemon (`test-start` with `cpu`, `cpu-per-core` or `gpu`), which runs the tester as its child and streams progress: a panel that started a test may close and the result is still recorded. Results are stored per profile, keyed to a fingerprint of its risky settings (SMU limits and GPU power limit), so a safe edit keeps the record and a risky one starts over. A machine-check error during any test, or a GPU hang during a GPU test, counts as a failure. A run stopped early with no errors is not recorded. The profile list shows `cpu test passed` / `FAILED`, kept or untested, and crash counts.

When a core fails, the Test page says which core, how many errors, and at what Curve Optimizer value, and offers **Set −N**: the core's value moved 5 closer to zero, staged as `smu.co_cores.<n>` behind Apply.

## Automation

```
omarchy-oc auto on
omarchy-oc rule add process:cyberpunk* max
omarchy-oc rule add steam-game performance
omarchy-oc rule add gamemode performance
omarchy-oc rules | rule del <n>
omarchy-oc boot on|off
```

Matchers: `process:<glob>` (process name or executable basename, case-insensitive, `*` and `?`), `steam-game` (any running Steam game), `gamemode` (any process run under gamemoderun / libgamemode). Rules are checked every 2 s; the first match wins; when nothing matches the daemon goes back to the profile you chose. Rules never fire while settings are pending or a test runs. A rule pointing at an unkept overclock is blocked until you keep it by hand, and after the watchdog steps in the rule's profile stays blocked until the match changes. Apply at boot (on by default) applies your chosen profile when the daemon starts, subject to the crash guard above.

## Monitoring and metrics

The daemon samples at 1 Hz and keeps one hour of fourteen series (CPU MHz average and max, utilisation, Tctl, package W, PPT W, GPU MHz, temperature, VRAM temperature, W, busy, GPU fan, memory %, CPU fan). The panel's charts draw from it; `omarchy-oc csv [file] [--seconds N]` exports it.

`/run/omarchy-oc/metrics.prom` is rewritten every second in Prometheus text format (`omarchy_oc_*` gauges: CPU, SMU limits and values, GPU, DIMM and board temperatures, voltages, fans, machine checks, GPU hangs, active profile), ready for node_exporter's textfile collector. `omarchy-oc metrics --http on [--listen 127.0.0.1:9877]` also serves it over HTTP; the listener refuses anything but a loopback address. `omarchy-oc metrics print` shows the file.

`omarchy-oc status`, `info`, `watch` and `probe` read the same data; `/run/omarchy-oc/status.json` and `info.json` are world-readable.

## vs LACT

| | LACT (Intel) | omarchy-oc |
|---|---|---|
| GPU clocks within the card's limits | yes | yes (RPn–RP0) |
| GPU power limit | yes | yes (PL1, window, power profile) |
| GPU monitoring, per-process use | yes | yes |
| Profiles, auto-switch by process / GameMode | yes | yes, plus any Steam game |
| History charts, CSV export | yes | yes (1 h) |
| Hardware info page | yes | yes (CPU, GPU, DIMMs/SPD/EXPO, board, firmware) |
| Metrics exporter | OpenTelemetry | Prometheus textfile + loopback HTTP |
| CLI | yes | yes |
| Re-apply on resume | yes | yes |
| Arc fan control | no | yes (omarchy-fand) |
| CPU: pstate, governor, EPP, boost, clocks, SMT, C-states | no | yes |
| SMU: PPT, Tctl, TDC/EDC, Curve Optimizer | no | yes |
| Platform: ASPM, board sensors | no | yes |
| Stability test with error detection | no | yes (CPU and GPU) |
| Keep/Revert, crash guard, watchdog | no | yes |
| Remote TCP control | yes (no authentication) | no (loopback metrics only) |
| UI | separate GTK app | Omarchy bar panel |

## Not possible, out of scope

- **Memory timings and frequency**: BIOS only (EXPO/XMP). The Memory page is information only.
- **Arc clocks above RP0** (2400 MHz on the A580) **and a voltage offset**: `xe` has no interface for either. The PCODE overclocking mailbox is undocumented, so it is not attempted.
- **The Radeon 740M iGPU**: disabled in the BIOS.
- **Remote TCP control**: LACT's has no authentication. Only the Prometheus listener goes on the network, and only on loopback.

## Verified on this machine

**Hardware-verified: pending (see Task 10).** The controller fills this in after the real install.

- TODO: `omarchy-oc probe` result: SMU family, `smu: true/false`, PM table (version and limits object) or `pm_error`
- TODO: apply/revert round trip: `omarchy-oc apply performance` → `scaling_governor` = `performance`; `omarchy-oc apply stock` → stock value
- TODO: overclock round trip from the panel: CO all −5 staged and applied, Keep/Revert shown in panel and bar, timed-out revert, then Apply + Keep
- TODO: CPU test result (2 min, all-core): pass/fail, errors, ISA used
- TODO: GPU test result (1 min, mixed): pass/fail, compute errors, VRAM tested

## Files

| Path | What |
|---|---|
| `/var/lib/omarchy-oc/config.json` | chosen profile, user profiles, rules, watchdog, metrics, stability records |
| `/var/lib/omarchy-oc/dirty`, `running` | crash-guard markers (profile + boot id) |
| `/run/omarchy-oc/control.sock` | JSON-line control socket, 0660 root:wheel, 16 connections |
| `/run/omarchy-oc/status.json`, `info.json` | live status (every second) and hardware details |
| `/run/omarchy-oc/metrics.prom` | Prometheus text metrics |
| `/run/omarchy-oc/original.json`, `smu-stock.json`, `smu-applied.json` | boot-time stock values and what the SMU was told, for `restore` |

## Development

```sh
cargo test                                          # unit tests + protocol/safety integration tests against a fake sysfs
cargo build --release --manifest-path gpustress/Cargo.toml
node --test plugin/tests/*.test.js                  # panel model tests
omarchy-ocd detect                                  # one read-only status snapshot, any user
```

Socket protocol: one JSON object per line. `{"id":7,"cmd":"status"}` → `{"id":7,"ok":true,"result":{...}}`; a request without `id` is answered once and the connection closes (the CLI). `{"cmd":"subscribe"}` pushes a `status` event every tick (with the last 120 history samples) and `pending`, `alert`, `test-progress` and `test-done` events as they happen. Fan commands (`{"cmd":"fans","req":{...}}`) run on their own thread.

## License

MIT
