//! Socket protocol v2 against a fake sysfs tree: id-tagged replies on a
//! persistent connection, one-shot compatibility, status push, slow fan
//! requests off the critical path, request size cap.

mod common;

use common::Fake;
use omarchy_oc::daemon::Core;
use omarchy_oc::server::{self, Hub};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

struct Daemon {
    fake: Fake,
    core: Arc<Mutex<Core>>,
    hub: Arc<Hub>,
    sock: PathBuf,
}

impl Daemon {
    fn start(name: &str) -> Daemon {
        let fake = Fake::new(name);
        let core = Arc::new(Mutex::new(fake.core()));
        let hub = Hub::new();
        let sock = fake.dir.join("run/control.sock");
        server::serve(core.clone(), hub.clone(), &sock, "nonexistent-group").unwrap();
        Daemon { fake, core, hub, sock }
    }

    fn connect(&self) -> Conn {
        let s = UnixStream::connect(&self.sock).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        Conn { r: BufReader::new(s.try_clone().unwrap()), w: s }
    }
}

struct Conn {
    r: BufReader<UnixStream>,
    w: UnixStream,
}

impl Conn {
    fn send(&mut self, v: Value) {
        self.w.write_all(format!("{v}\n").as_bytes()).unwrap();
    }

    /// Next line as JSON; None at EOF.
    fn recv(&mut self) -> Option<Value> {
        let mut line = String::new();
        let n = self.r.read_line(&mut line).unwrap();
        if n == 0 { None } else { Some(serde_json::from_str(&line).unwrap()) }
    }

    fn recv_ok(&mut self) -> Value {
        let v = self.recv().expect("reply before EOF");
        assert_eq!(v["ok"], json!(true), "{v}");
        v
    }
}

#[test]
fn ids_round_trip() {
    let d = Daemon::start("ids");
    let mut c = d.connect();
    c.send(json!({"id": 1, "cmd": "status"}));
    c.send(json!({"id": 2, "cmd": "apply", "profile": "performance"}));
    let a = c.recv_ok();
    let b = c.recv_ok();
    assert_eq!(a["id"], json!(1));
    assert_eq!(a["result"]["active"], json!("stock"));
    assert_eq!(b["id"], json!(2));
    assert_eq!(b["result"]["profile"], json!("performance"));
}

#[test]
fn oneshot_without_id_closes() {
    let d = Daemon::start("oneshot");
    let mut c = d.connect();
    c.send(json!({"cmd": "status"}));
    let v = c.recv_ok();
    assert!(v.get("id").is_none(), "{v}");
    assert_eq!(v["result"]["active"], json!("stock"));
    assert!(c.recv().is_none(), "connection closes after a one-shot request");
}

#[test]
fn subscribe_pushes_status() {
    let d = Daemon::start("subscribe");
    let mut c = d.connect();
    c.send(json!({"id": 7, "cmd": "subscribe"}));
    let v = c.recv_ok();
    assert_eq!(v["id"], json!(7));
    let first = c.recv().unwrap();
    assert_eq!(first["event"], json!("status"), "immediate status after subscribe: {first}");
    server::tick_and_publish(&d.core, &d.hub);
    server::tick_and_publish(&d.core, &d.hub);
    for _ in 0..2 {
        let ev = c.recv().unwrap();
        assert_eq!(ev["event"], json!("status"), "{ev}");
        assert_eq!(ev["active"], json!("stock"));
        assert!(ev["history"]["t"].as_array().unwrap().len() <= 120);
    }
}

#[test]
fn pending_events_are_pushed() {
    let d = Daemon::start("events");
    let mut c = d.connect();
    c.send(json!({"id": 1, "cmd": "subscribe"}));
    c.recv_ok();
    c.recv().unwrap(); // immediate status
    c.send(json!({"id": 2, "cmd": "apply", "profile": "max"}));
    assert_eq!(c.recv_ok()["result"]["pending"], json!(true));
    server::tick_and_publish(&d.core, &d.hub);
    let ev = c.recv().unwrap();
    assert_eq!(ev["event"], json!("pending"), "{ev}");
    assert_eq!(ev["pending"]["profile"], json!("max"));
    assert_eq!(c.recv().unwrap()["event"], json!("status"));
    c.send(json!({"id": 3, "cmd": "revert"}));
    c.recv_ok();
    server::tick_and_publish(&d.core, &d.hub);
    let ev = c.recv().unwrap();
    assert_eq!(ev["event"], json!("pending"), "{ev}");
    assert_eq!(ev["pending"], Value::Null);
    assert_eq!(c.recv().unwrap()["event"], json!("status"));
}

#[test]
fn stalled_subscriber_is_bounded_and_dropped() {
    // Hub level: a receiver that is never drained holds at most SUB_QUEUE
    // lines and is then dropped.
    let hub = Hub::new();
    let rx = hub.subscribe();
    for i in 0..1000 {
        hub.publish(format!("{{\"n\":{i}}}"));
    }
    let queued = rx.try_iter().count();
    assert!(queued <= server::SUB_QUEUE, "queued {queued} lines");
    assert!(rx.recv().is_err(), "the overflowing subscriber is unsubscribed");

    // Socket level: a client that stops reading is cut off instead of
    // growing a queue; it gets EOF once it reads what was buffered.
    let d = Daemon::start("stalled");
    let mut c = d.connect();
    c.send(json!({"id": 1, "cmd": "subscribe"}));
    c.recv_ok();
    for _ in 0..200 {
        server::tick_and_publish(&d.core, &d.hub);
    }
    let mut got = 0;
    while c.recv().is_some() {
        got += 1;
    }
    assert!(got < 200, "received {got} of 200 pushed statuses");
}

#[test]
fn resubscribe_after_reconnect() {
    let d = Daemon::start("resub");
    {
        let mut c = d.connect();
        c.send(json!({"id": 1, "cmd": "subscribe"}));
        c.recv_ok();
        assert_eq!(c.recv().unwrap()["event"], json!("status"));
    }
    server::tick_and_publish(&d.core, &d.hub);
    let mut c = d.connect();
    c.send(json!({"id": 1, "cmd": "subscribe"}));
    c.recv_ok();
    assert_eq!(c.recv().unwrap()["event"], json!("status"));
    server::tick_and_publish(&d.core, &d.hub);
    assert_eq!(c.recv().unwrap()["event"], json!("status"));
}

#[test]
fn slow_fan_request_does_not_block() {
    // Fans::call maps $OMARCHY_FANS_RUN through the fake root.
    unsafe { std::env::set_var("OMARCHY_FANS_RUN", "/fans") };
    let d = Daemon::start("slowfan");
    let fan_dir = d.fake.root.join("fans");
    std::fs::create_dir_all(&fan_dir).unwrap();
    let fan_sock = UnixListener::bind(fan_dir.join("control.sock")).unwrap();
    std::thread::spawn(move || {
        for s in fan_sock.incoming().flatten() {
            std::thread::spawn(move || {
                let mut line = String::new();
                let mut w = s.try_clone().unwrap();
                BufReader::new(s).read_line(&mut line).unwrap();
                std::thread::sleep(Duration::from_secs(3));
                let _ = w.write_all(b"{\"ok\":true,\"result\":{\"calibrated\":true}}\n");
            });
        }
    });

    let mut c = d.connect();
    let t0 = Instant::now();
    c.send(json!({"id": 1, "cmd": "fans", "req": {"cmd": "calibrate"}}));
    c.send(json!({"id": 2, "cmd": "status"}));
    let first = c.recv_ok();
    assert_eq!(first["id"], json!(2), "status must not wait for the fan request: {first}");
    assert!(t0.elapsed() < Duration::from_secs(1), "status took {:?}", t0.elapsed());
    let second = c.recv_ok();
    assert_eq!(second["id"], json!(1));
    assert_eq!(second["result"]["calibrated"], json!(true));
}

#[test]
fn oversize_line_rejected() {
    let d = Daemon::start("oversize");
    let mut c = d.connect();
    let pad = "x".repeat(300 * 1024);
    c.send(json!({"id": 1, "cmd": "status", "pad": pad}));
    let v = c.recv().expect("a reply to the oversize request");
    assert_eq!(v["ok"], json!(false), "{v}");
    // The daemon keeps serving others.
    let mut c2 = d.connect();
    c2.send(json!({"id": 3, "cmd": "status"}));
    let v = c2.recv_ok();
    assert_eq!(v["id"], json!(3));
}

// ---- stability tests run by the daemon -------------------------------------

/// A tester that prints `lines` (one JSON object each) and exits with `code`.
/// Its arguments and `$OMARCHY_OC_CHILD` land in files the test can read.
fn script_lines(lines: &[&str], code: i32) -> String {
    let mut s = String::from("echo \"$@\" > \"$OUT/args\"\necho \"${OMARCHY_OC_CHILD:-unset}\" > \"$OUT/child\"\n");
    for l in lines {
        s.push_str(&format!("echo '{l}'\n"));
    }
    s.push_str(&format!("exit {code}\n"));
    s
}

/// A tester that waits for SIGINT, then prints its done line and exits 0.
const SCRIPT_WAITS: &str = "trap 'kill $SLP 2>/dev/null; echo \"{\\\"t\\\":1,\\\"done\\\":true,\\\"passed\\\":true,\\\"errors\\\":0,\\\"interrupted\\\":true,\\\"message\\\":\\\"stopped before the end\\\"}\"; exit 0' INT TERM
echo '{\"t\":0,\"phase\":\"all cores\",\"blocks\":1,\"errors\":0}'
sleep 30 >/dev/null 2>&1 & SLP=$!
wait $SLP
";

impl Daemon {
    /// Tick until an event named `name` is pushed to `c`, collecting every
    /// non-status event seen on the way (in order, `name` last).
    fn events_until(&self, c: &mut Conn, name: &str, timeout: Duration) -> Vec<Value> {
        let deadline = Instant::now() + timeout;
        let mut seen = Vec::new();
        while Instant::now() < deadline {
            // Each tick pushes its events, then exactly one status: read
            // through the status so no push is left for the next reply.
            server::tick_and_publish(&self.core, &self.hub);
            let mut hit = false;
            loop {
                let ev = c.recv().expect("push before EOF");
                if ev["event"] == json!("status") {
                    break;
                }
                hit |= ev["event"] == json!(name);
                seen.push(ev);
            }
            if hit {
                return seen;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("no {name} event within {timeout:?}; saw {seen:?}");
    }

    fn profile(&self, name: &str) -> Value {
        let st = self.core.lock().unwrap().status(1);
        st["profiles"].as_array().unwrap().iter().find(|p| p["name"] == name).unwrap().clone()
    }

    fn dirty(&self) -> bool {
        self.fake.dir.join("state/dirty").exists()
    }
}

fn subscribe(c: &mut Conn) {
    c.send(json!({"id": 1, "cmd": "subscribe"}));
    c.recv_ok();
    assert_eq!(c.recv().unwrap()["event"], json!("status"));
}

#[test]
fn test_runs_and_records() {
    let d = Daemon::start("test-run");
    d.fake.tester(
        "omarchy-oc",
        &script_lines(
            &[
                r#"{"t":1,"phase":"all cores","blocks":10,"errors":0,"done":false}"#,
                r#"{"t":2,"phase":"all cores","blocks":20,"errors":0,"done":false}"#,
                r#"{"t":2.5,"phase":"all cores","blocks":25,"errors":0,"done":true,"passed":true,"isa":"Avx2"}"#,
            ],
            0,
        ),
    );
    let mut c = d.connect();
    subscribe(&mut c);
    c.send(json!({"id": 2, "cmd": "test-start", "kind": "cpu", "seconds": 5}));
    let r = c.recv_ok();
    assert_eq!(r["result"]["profile"], json!("stock"), "{r}");
    assert!(d.dirty(), "the crash-guard marker is set while a test runs");
    let st = d.core.lock().unwrap().status(1);
    assert_eq!(st["stress"]["kind"], json!("cpu"), "{}", st["stress"]);
    assert_eq!(st["stress"]["profile"], json!("stock"));

    let evs = d.events_until(&mut c, "test-done", Duration::from_secs(10));
    assert_eq!(evs.len(), 3, "{evs:?}");
    assert_eq!(evs[0]["event"], json!("test-progress"));
    assert_eq!(evs[0]["kind"], json!("cpu"));
    assert_eq!(evs[0]["blocks"], json!(10));
    assert_eq!(evs[1]["event"], json!("test-progress"));
    assert_eq!(evs[1]["blocks"], json!(20));
    let done = &evs[2];
    assert_eq!(done["kind"], json!("cpu"));
    assert_eq!(done["passed"], json!(true));
    assert_eq!(done["errors"], json!(0));
    assert_eq!(done["profile"], json!("stock"));

    let p = d.profile("stock");
    assert_eq!(p["cpu_test"]["passed"], json!(true), "{p}");
    assert_eq!(p["cpu_test"]["errors"], json!(0));
    let st = d.core.lock().unwrap().status(1);
    assert_eq!(st["stress"], Value::Null);
    assert!(!d.dirty(), "the marker is gone after test-done");
    // The child command and environment.
    assert_eq!(std::fs::read_to_string(d.fake.dir.join("args")).unwrap().trim(), "stress cpu --json --seconds 5");
    assert_eq!(std::fs::read_to_string(d.fake.dir.join("child")).unwrap().trim(), "1");
}

#[test]
fn test_failure_recorded() {
    let d = Daemon::start("test-fail");
    d.fake.tester(
        "omarchy-oc",
        &script_lines(&[r#"{"t":3,"phase":"all cores","blocks":9,"errors":3,"done":true,"passed":false,"message":"computation errors on core 1"}"#], 1),
    );
    let mut c = d.connect();
    subscribe(&mut c);
    c.send(json!({"id": 2, "cmd": "test-start", "kind": "cpu", "seconds": 5}));
    c.recv_ok();
    let evs = d.events_until(&mut c, "test-done", Duration::from_secs(10));
    let done = evs.last().unwrap();
    assert_eq!(done["passed"], json!(false), "{done}");
    assert_eq!(done["errors"], json!(3));
    let p = d.profile("stock");
    assert_eq!(p["cpu_test"]["passed"], json!(false), "{p}");
    assert_eq!(p["cpu_test"]["errors"], json!(3));
    assert!(p["cpu_test"]["detail"].as_str().unwrap().contains("core 1"), "{p}");
    assert!(!d.dirty());
}

#[test]
fn test_exit_without_done_is_failure() {
    let d = Daemon::start("test-nodone");
    d.fake.tester("omarchy-oc", &script_lines(&[r#"{"t":1,"phase":"reference","blocks":0,"errors":0,"done":false}"#], 2));
    let mut c = d.connect();
    subscribe(&mut c);
    c.send(json!({"id": 2, "cmd": "test-start", "kind": "cpu", "seconds": 5}));
    c.recv_ok();
    let evs = d.events_until(&mut c, "test-done", Duration::from_secs(10));
    let done = evs.last().unwrap();
    assert_eq!(done["passed"], json!(false), "{done}");
    let p = d.profile("stock");
    assert_eq!(p["cpu_test"]["passed"], json!(false), "{p}");
    assert_eq!(p["cpu_test"]["detail"], json!("tester exited with code 2"), "{p}");
    assert!(!d.dirty());
}

#[test]
fn test_start_refused_when_pending_or_running() {
    let d = Daemon::start("test-refuse");
    d.fake.tester("omarchy-oc", SCRIPT_WAITS);
    let mut c = d.connect();
    subscribe(&mut c);
    c.send(json!({"id": 2, "cmd": "test-start", "kind": "cpu", "seconds": 5}));
    c.recv_ok();
    c.send(json!({"id": 3, "cmd": "test-start", "kind": "cpu", "seconds": 5}));
    let r = c.recv().unwrap();
    assert_eq!(r["ok"], json!(false), "{r}");
    assert!(r["error"].as_str().unwrap().contains("already running"), "{r}");
    c.send(json!({"id": 4, "cmd": "test-stop"}));
    c.recv_ok();
    d.events_until(&mut c, "test-done", Duration::from_secs(10));
    c.send(json!({"id": 5, "cmd": "test-stop"}));
    let r = c.recv().unwrap();
    assert_eq!(r["ok"], json!(false), "{r}");
    assert_eq!(r["error"], json!("no test running"));

    c.send(json!({"id": 6, "cmd": "apply", "profile": "max"}));
    assert_eq!(c.recv_ok()["result"]["pending"], json!(true));
    c.send(json!({"id": 7, "cmd": "test-start", "kind": "cpu", "seconds": 5}));
    let r = c.recv().unwrap();
    assert_eq!(r["ok"], json!(false), "{r}");
    assert!(r["error"].as_str().unwrap().contains("confirm or revert"), "{r}");
    c.send(json!({"id": 8, "cmd": "revert"}));
    c.recv_ok();
}

#[test]
fn test_stop_interrupts() {
    let d = Daemon::start("test-stop");
    d.fake.tester("omarchy-oc", SCRIPT_WAITS);
    let mut c = d.connect();
    subscribe(&mut c);
    c.send(json!({"id": 2, "cmd": "test-start", "kind": "cpu", "seconds": 60}));
    c.recv_ok();
    assert!(d.dirty());
    // Let the child reach its wait before interrupting it.
    let evs = d.events_until(&mut c, "test-progress", Duration::from_secs(10));
    assert_eq!(evs.len(), 1, "{evs:?}");
    c.send(json!({"id": 3, "cmd": "test-stop"}));
    c.recv_ok();
    let t0 = Instant::now();
    let evs = d.events_until(&mut c, "test-done", Duration::from_secs(10));
    assert!(t0.elapsed() < Duration::from_secs(2), "test-done took {:?}", t0.elapsed());
    let done = evs.last().unwrap();
    assert_eq!(done["kind"], json!("cpu"), "{done}");
    assert_eq!(done["interrupted"], json!(true), "{done}");
    // An interrupted run is not a verdict: nothing recorded, marker gone.
    let p = d.profile("stock");
    assert_eq!(p["cpu_test"], Value::Null, "{p}");
    assert!(!d.dirty());
    assert_eq!(d.core.lock().unwrap().status(1)["stress"], Value::Null);
}

#[test]
fn test_gpu_and_per_core_commands() {
    let d = Daemon::start("test-cmds");
    d.fake.tester(
        "omarchy-oc-gpustress",
        &script_lines(&[r#"{"t":7,"mode":"mixed","iterations":100,"errors":0,"gflops":1000,"vram_gbps":300,"adapter":"fake","done":true,"passed":true,"interrupted":false}"#], 0),
    );
    d.fake.tester("omarchy-oc", &script_lines(&[r#"{"t":30,"phase":"core 1","blocks":5,"errors":0,"done":true,"passed":true}"#], 0));
    let mut c = d.connect();
    subscribe(&mut c);
    c.send(json!({"id": 2, "cmd": "test-start", "kind": "gpu", "seconds": 7}));
    c.recv_ok();
    let done = d.events_until(&mut c, "test-done", Duration::from_secs(10)).pop().unwrap();
    assert_eq!(done["kind"], json!("gpu"), "{done}");
    assert_eq!(done["passed"], json!(true));
    assert_eq!(std::fs::read_to_string(d.fake.dir.join("args")).unwrap().trim(), "--json --seconds 7");
    let p = d.profile("stock");
    assert_eq!(p["gpu_test"]["passed"], json!(true), "{p}");
    assert_eq!(p["cpu_test"], Value::Null);

    // Two cores in the fake tree: 30 s / 2 = 15 s per core; never below 10.
    c.send(json!({"id": 3, "cmd": "test-start", "kind": "cpu-per-core", "seconds": 30}));
    c.recv_ok();
    let done = d.events_until(&mut c, "test-done", Duration::from_secs(10)).pop().unwrap();
    assert_eq!(done["kind"], json!("cpu-per-core"), "{done}");
    assert_eq!(std::fs::read_to_string(d.fake.dir.join("args")).unwrap().trim(), "stress cpu --json --seconds 30 --per-core --core-seconds 15");
    let p = d.profile("stock");
    assert_eq!(p["cpu_test"]["passed"], json!(true), "{p}");

    c.send(json!({"id": 4, "cmd": "test-start", "kind": "disk", "seconds": 30}));
    let r = c.recv().unwrap();
    assert_eq!(r["ok"], json!(false), "{r}");
}

#[test]
fn test_blocks_apply_and_applying_edits() {
    let d = Daemon::start("test-blocks");
    d.fake.tester("omarchy-oc", SCRIPT_WAITS);
    let mut c = d.connect();
    // Test on the user profile "custom"; "other" stays inactive.
    c.send(json!({"id": 1, "cmd": "copy", "from": "performance", "to": "other"}));
    c.recv_ok();
    c.send(json!({"id": 1, "cmd": "apply", "profile": "custom"}));
    c.recv_ok();
    subscribe(&mut c);
    c.send(json!({"id": 2, "cmd": "test-start", "kind": "cpu", "seconds": 60}));
    c.recv_ok();
    let refused = |c: &mut Conn, id: u64, mut req: Value| {
        req["id"] = json!(id);
        c.send(req);
        let r = c.recv().unwrap();
        assert_eq!(r["ok"], json!(false), "{r}");
        assert!(r["error"].as_str().unwrap().contains("stop the test first"), "{r}");
    };
    // The verdict is recorded against the profile the test started on, so
    // nothing may change the hardware under it.
    refused(&mut c, 3, json!({"cmd": "apply", "profile": "performance"}));
    refused(&mut c, 4, json!({"cmd": "edit", "profile": "other", "changes": {"cpu.epp": "power"}, "apply": true}));
    refused(&mut c, 5, json!({"cmd": "edit", "profile": "custom", "changes": {"cpu.epp": "power"}})); // active
    refused(&mut c, 6, json!({"cmd": "edit", "profile": "stock", "changes": {"cpu.epp": "power"}})); // forks into the active custom
    refused(&mut c, 7, json!({"cmd": "set", "profile": "custom", "key": "cpu.epp", "value": "power"}));
    assert_eq!(d.core.lock().unwrap().cfg.profiles["custom"].cpu.epp, None, "nothing saved either");
    // A safe edit of an inactive profile changes nothing on the hardware.
    c.send(json!({"id": 8, "cmd": "edit", "profile": "other", "changes": {"gpu.max_mhz": 2000}}));
    let r = c.recv_ok();
    assert_eq!(r["result"]["applied"], json!(false), "{r}");
    assert_eq!(d.core.lock().unwrap().active, "custom");
    c.send(json!({"id": 9, "cmd": "test-stop"}));
    c.recv_ok();
    d.events_until(&mut c, "test-done", Duration::from_secs(10));
    c.send(json!({"id": 10, "cmd": "apply", "profile": "performance"}));
    assert_eq!(c.recv_ok()["result"]["profile"], json!("performance"));
}

#[test]
fn watchdog_fallback_during_test_fails_it() {
    let d = Daemon::start("test-watchdog");
    d.fake.tester("omarchy-oc", SCRIPT_WAITS);
    let mut c = d.connect();
    c.send(json!({"id": 1, "cmd": "apply", "profile": "performance"}));
    c.recv_ok();
    c.send(json!({"id": 2, "cmd": "watchdog", "gpu_max_c": 60, "seconds": 1}));
    c.recv_ok();
    subscribe(&mut c);
    c.send(json!({"id": 3, "cmd": "test-start", "kind": "cpu", "seconds": 60}));
    c.recv_ok();
    d.events_until(&mut c, "test-progress", Duration::from_secs(10));
    // The GPU overheats under test: safety beats the test bookkeeping.
    common::w(&d.fake.root, "/sys/devices/pci0000:00/0000:00:01.1/0000:03:00.0/hwmon/hwmon2/temp2_input", "70000");
    // The same tick also pushes the watchdog alert.
    let evs = d.events_until(&mut c, "test-done", Duration::from_secs(10));
    assert!(evs.iter().any(|e| e["event"] == json!("alert")), "{evs:?}");
    let done = evs.iter().find(|e| e["event"] == json!("test-done")).unwrap();
    assert_eq!(done["passed"], json!(false), "{done}");
    assert_eq!(done["recorded"], json!(true), "{done}");
    assert_eq!(done["profile"], json!("performance"), "{done}");
    assert!(done["detail"].as_str().unwrap().to_lowercase().contains("watchdog"), "{done}");
    let core = d.core.lock().unwrap();
    assert_eq!(core.active, "stock", "the watchdog still switched");
    drop(core);
    let p = d.profile("performance");
    assert_eq!(p["cpu_test"]["passed"], json!(false), "{p}");
    assert!(p["cpu_test"]["detail"].as_str().unwrap().to_lowercase().contains("watchdog"), "{p}");
    assert_eq!(d.core.lock().unwrap().status(1)["stress"], Value::Null);
    assert!(!d.dirty());
}
