//! Control socket: one JSON object per line, persistent connections.
//!
//! * Request `{"id":N,"cmd":...}` → reply `{"id":N,"ok":true,"result":...}`
//!   or `{"id":N,"ok":false,"error":"..."}`, on the same connection, in
//!   any order (fan requests run on their own thread, everything else is
//!   answered in turn).
//! * A request without `id` is answered without `id` and the connection
//!   then closes: the CLI's one-shot protocol.
//! * `{"cmd":"subscribe"}` gets an immediate `{"event":"status",...}` and
//!   then a status event every tick, plus queued `pending`/`alert` events.
//!
//! Each connection has a reader thread and a writer thread; every line the
//! connection emits goes through one channel, so replies and pushed events
//! never interleave mid-line.

use crate::daemon::{Core, Paths, SIGNAL_STOP, fans_proxy};
use crate::fans::Fans;
use crate::util::Sys;
use serde_json::{Value, json};
use std::io::{self, BufRead, BufReader, Read as _, Write as _};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

/// Longest accepted request line.
pub const MAX_LINE: usize = 256 * 1024;
/// Open connections at once; later ones are refused (closed at once).
pub const MAX_CONNECTIONS: usize = 16;
/// Samples of history carried by every pushed status.
pub const PUSH_HISTORY: usize = 120;
/// Pushed lines a subscriber may leave unread before it is dropped.
pub const SUB_QUEUE: usize = 8;
/// Lines queued for one connection's writer before senders block.
const CONN_QUEUE: usize = 16;

/// Fan-out of pushed lines to every subscribed connection. Each subscriber
/// has a bounded queue ([`SUB_QUEUE`] lines); one that falls behind is
/// dropped rather than buffered without limit, so no line is ever skipped
/// for a live subscriber.
pub struct Hub {
    subs: Mutex<Vec<SyncSender<String>>>,
}

impl Hub {
    pub fn new() -> Arc<Hub> {
        Arc::new(Hub { subs: Mutex::new(Vec::new()) })
    }

    pub fn subscribe(&self) -> Receiver<String> {
        let (tx, rx) = mpsc::sync_channel(SUB_QUEUE);
        unpoison(self.subs.lock()).push(tx);
        rx
    }

    /// Send `line` to every subscriber, dropping those that hung up or
    /// whose queue is full.
    pub fn publish(&self, line: String) {
        unpoison(self.subs.lock()).retain(|s| s.try_send(line.clone()).is_ok());
    }
}

fn unpoison<'a, T>(r: Result<MutexGuard<'a, T>, std::sync::PoisonError<MutexGuard<'a, T>>>) -> MutexGuard<'a, T> {
    r.unwrap_or_else(|p| p.into_inner())
}

/// Bind the control socket, set its mode (0660 root:`group`, or 0600 when the
/// group does not exist) and start accepting connections on a thread.
pub fn serve(core: Arc<Mutex<Core>>, hub: Arc<Hub>, sock: &Path, group: &str) -> io::Result<()> {
    let _ = std::fs::remove_file(sock);
    let listener = UnixListener::bind(sock)?;
    {
        use std::os::unix::fs::PermissionsExt;
        let gid = group_id(group);
        if let (Some(gid), Ok(c)) = (gid, std::ffi::CString::new(sock.to_string_lossy().as_bytes())) {
            unsafe { libc::chown(c.as_ptr(), 0, gid) };
        }
        std::fs::set_permissions(sock, std::fs::Permissions::from_mode(if gid.is_some() { 0o660 } else { 0o600 }))?;
    }
    std::thread::spawn(move || {
        let slots = Arc::new(AtomicUsize::new(0));
        for s in listener.incoming().flatten() {
            if slots.fetch_add(1, Ordering::Relaxed) >= MAX_CONNECTIONS {
                slots.fetch_sub(1, Ordering::Relaxed);
                continue;
            }
            let (core, hub, slots) = (core.clone(), hub.clone(), slots.clone());
            std::thread::spawn(move || {
                Conn::run(core, hub, s);
                slots.fetch_sub(1, Ordering::Relaxed);
            });
        }
    });
    Ok(())
}

fn group_id(name: &str) -> Option<libc::gid_t> {
    let c = std::ffi::CString::new(name).ok()?;
    let g = unsafe { libc::getgrnam(c.as_ptr()) };
    if g.is_null() { None } else { Some(unsafe { (*g).gr_gid }) }
}

/// What the writer thread drains.
enum Out {
    Line(String),
    Close,
}

struct Conn {
    core: Arc<Mutex<Core>>,
    hub: Arc<Hub>,
    /// Bounded: a client that stops reading blocks its own senders (reader,
    /// fan and forwarder threads) instead of growing a queue.
    tx: SyncSender<Out>,
}

impl Conn {
    fn run(core: Arc<Mutex<Core>>, hub: Arc<Hub>, stream: UnixStream) {
        let Ok(mut w) = stream.try_clone() else { return };
        let (tx, rx) = mpsc::sync_channel::<Out>(CONN_QUEUE);
        std::thread::spawn(move || {
            while let Ok(Out::Line(l)) = rx.recv() {
                if w.write_all(l.as_bytes()).and_then(|_| w.write_all(b"\n")).is_err() {
                    break;
                }
            }
            // Wakes the reader with EOF whichever side ended first.
            let _ = w.shutdown(std::net::Shutdown::Both);
        });
        let conn = Conn { core, hub, tx };
        let mut r = BufReader::new(stream);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match read_request(&mut r, &mut buf) {
                Ok(Some(line)) => match conn.request(&line) {
                    Flow::Keep => {}
                    Flow::Close => break,
                    Flow::Detached => return, // a worker thread closes it
                },
                Ok(None) => break, // EOF
                Err(e) => {
                    conn.send(json!({"ok": false, "error": e}));
                    break;
                }
            }
        }
        let _ = conn.tx.send(Out::Close);
    }

    fn send(&self, v: Value) {
        let _ = self.tx.send(Out::Line(v.to_string()));
    }

    /// Handle one line.
    fn request(&self, line: &str) -> Flow {
        let Ok(req) = serde_json::from_str::<Value>(line) else {
            self.send(json!({"ok": false, "error": "request is not JSON"}));
            return Flow::Close;
        };
        let id = req.get("id").cloned();
        let keep = id.is_some();
        match req.get("cmd").and_then(Value::as_str) {
            Some("fans") => {
                // omarchy-fand can take minutes (calibration): off this thread,
                // without the core lock.
                let sys: Sys = lock(&self.core).sys.clone();
                let tx = self.tx.clone();
                std::thread::spawn(move || {
                    let r = fans_proxy(&Fans::new(&sys), &req);
                    let _ = tx.send(Out::Line(reply(id, r).to_string()));
                    if !keep {
                        let _ = tx.send(Out::Close);
                    }
                });
                if !keep {
                    return Flow::Detached;
                }
            }
            Some("subscribe") => {
                let rx = self.hub.subscribe();
                let status = status_event(&lock(&self.core));
                self.send(reply(id, Ok(Value::Null)));
                self.send(status);
                let tx = self.tx.clone();
                std::thread::spawn(move || {
                    while let Ok(l) = rx.recv() {
                        if tx.send(Out::Line(l)).is_err() {
                            return; // connection gone
                        }
                    }
                    // The hub dropped us (queue overflowed): close, so the
                    // client reconnects and gets a fresh status.
                    let _ = tx.send(Out::Close);
                });
            }
            _ => {
                let r = lock(&self.core).handle(&req);
                self.send(reply(id, r));
            }
        }
        if keep { Flow::Keep } else { Flow::Close }
    }
}

/// What the reader does after a request.
enum Flow {
    /// Persistent connection: read the next request.
    Keep,
    /// One-shot request answered: the reader closes the connection.
    Close,
    /// One-shot fan request: the fan thread closes the connection when its
    /// reply is out; the reader just stops.
    Detached,
}

fn reply(id: Option<Value>, r: Result<Value, String>) -> Value {
    let mut v = match r {
        Ok(result) => json!({"ok": true, "result": result}),
        Err(error) => json!({"ok": false, "error": error}),
    };
    if let Some(id) = id {
        v["id"] = id;
    }
    v
}

/// One request line, capped at [`MAX_LINE`]. `Ok(None)` at EOF. An oversize
/// line is drained to its newline so the client's write completes, then
/// rejected.
fn read_request<R: BufRead>(r: &mut R, buf: &mut Vec<u8>) -> Result<Option<String>, String> {
    let n = r.take(MAX_LINE as u64 + 1).read_until(b'\n', buf).map_err(|e| e.to_string())?;
    if n == 0 {
        return Ok(None);
    }
    if buf.last() != Some(&b'\n') && buf.len() > MAX_LINE {
        let mut sink = Vec::new();
        loop {
            sink.clear();
            match r.take(MAX_LINE as u64).read_until(b'\n', &mut sink) {
                Ok(0) | Err(_) => break,
                Ok(_) if sink.last() == Some(&b'\n') => break,
                Ok(_) => {}
            }
        }
        return Err(format!("request too long (max {} KiB)", MAX_LINE / 1024));
    }
    Ok(Some(String::from_utf8_lossy(buf).trim_end().to_string()))
}

fn lock(core: &Arc<Mutex<Core>>) -> MutexGuard<'_, Core> {
    unpoison(core.lock())
}

fn status_event(core: &Core) -> Value {
    let mut st = core.status(PUSH_HISTORY);
    st["event"] = json!("status");
    st
}

/// One daemon tick: sample, then push queued events and the status.
pub fn tick_and_publish(core: &Arc<Mutex<Core>>, hub: &Hub) {
    let (events, status) = {
        let mut c = lock(core);
        c.tick();
        (c.take_events(), status_event(&c))
    };
    for ev in events {
        hub.publish(ev.to_string());
    }
    hub.publish(status.to_string());
}

/// The daemon main loop: serve the socket, tick once a second until a
/// signal, then hand the hardware back.
pub fn run(sys: Sys, paths: Paths) -> io::Result<()> {
    let core = Arc::new(Mutex::new(Core::new(sys, paths.clone())));
    {
        let mut c = lock(&core);
        c.start_metrics();
        c.tick();
    }
    let hub = Hub::new();
    let sock = paths.run("control.sock");
    serve(core.clone(), hub.clone(), &sock, &paths.group)?;
    let mut next = std::time::Instant::now();
    while !SIGNAL_STOP.load(Ordering::Relaxed) {
        next += Duration::from_secs(1);
        tick_and_publish(&core, &hub);
        let now_i = std::time::Instant::now();
        if next > now_i {
            std::thread::sleep(next - now_i);
        } else {
            next = now_i;
        }
    }
    lock(&core).shutdown();
    let _ = std::fs::remove_file(&sock);
    Ok(())
}
