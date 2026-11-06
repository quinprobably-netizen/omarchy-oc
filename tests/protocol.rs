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
