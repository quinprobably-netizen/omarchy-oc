//! A stability test run as a child of the daemon: `omarchy-oc stress cpu
//! --json` or `omarchy-oc-gpustress --json`. The child's stdout is read on
//! a thread and drained by the daemon tick with [`TestRun::poll`], so the
//! test belongs to the daemon: a panel that started it may close, the
//! daemon still streams its progress and records the verdict.
//!
//! The child runs with `OMARCHY_OC_CHILD=1`, which tells `omarchy-oc stress`
//! not to report to the daemon itself (the daemon already tracks the run).

use serde_json::{Value, json};
use std::io::{self, BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

/// Shortest per-core phase of a `cpu-per-core` test, seconds.
pub const MIN_CORE_SECONDS: u32 = 10;
/// How long after the child's exit to wait for the end of its output
/// before giving up on it (something else may hold the pipe open).
const EOF_GRACE: Duration = Duration::from_secs(2);

pub struct TestRun {
    /// `cpu`, `cpu-per-core` or `gpu`, as requested.
    pub kind: String,
    child: Child,
    lines: Receiver<String>,
    /// The reader thread saw EOF: every line is in `lines`.
    eof: bool,
    /// The `done:true` line, once seen.
    done: Option<Value>,
    /// When the child was first seen to have exited.
    exited: Option<Instant>,
    stopped: bool,
}

/// The child command for `kind`: `cores` is the number of physical cores,
/// which sets the per-core phase length of `cpu-per-core`.
pub fn command(kind: &str, seconds: u32, cores: usize, bin_dir: &Path) -> io::Result<Command> {
    let mut cmd = match kind {
        "cpu" | "cpu-per-core" => {
            let mut c = Command::new(bin_dir.join("omarchy-oc"));
            c.args(["stress", "cpu", "--json", "--seconds"]).arg(seconds.to_string());
            if kind == "cpu-per-core" {
                let per_core = (seconds / cores.max(1) as u32).max(MIN_CORE_SECONDS);
                c.arg("--per-core").arg("--core-seconds").arg(per_core.to_string());
            }
            c
        }
        "gpu" => {
            let mut c = Command::new(bin_dir.join("omarchy-oc-gpustress"));
            c.args(["--json", "--seconds"]).arg(seconds.to_string());
            c
        }
        other => return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("test kind must be cpu, cpu-per-core or gpu, not \"{other}\""))),
    };
    cmd.env("OMARCHY_OC_CHILD", "1").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::inherit());
    Ok(cmd)
}

impl TestRun {
    /// Spawn the tester for `kind` from `bin_dir`.
    pub fn start(kind: &str, seconds: u32, cores: usize, bin_dir: &Path) -> io::Result<TestRun> {
        let mut child = command(kind, seconds, cores, bin_dir)?.spawn()?;
        let out = child.stdout.take().ok_or_else(|| io::Error::other("no stdout pipe"))?;
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for l in BufReader::new(out).lines().map_while(Result::ok) {
                if tx.send(l).is_err() {
                    break;
                }
            }
        });
        Ok(TestRun { kind: kind.to_string(), child, lines, eof: false, done: None, exited: None, stopped: false })
    }

    /// Every progress line received since the last call, each tagged with
    /// `kind`. The `done:true` line is kept back for [`TestRun::finished`].
    /// Never blocks.
    pub fn poll(&mut self) -> Vec<Value> {
        let mut out = Vec::new();
        loop {
            match self.lines.try_recv() {
                Ok(l) => {
                    let Ok(mut v) = serde_json::from_str::<Value>(&l) else { continue };
                    if !v.is_object() {
                        continue;
                    }
                    v["kind"] = json!(self.kind);
                    if v["done"] == json!(true) {
                        self.done = Some(v);
                    } else {
                        out.push(v);
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.eof = true;
                    break;
                }
            }
        }
        out
    }

    /// Ask the tester to stop (SIGINT): it finishes the block in hand and
    /// prints its done line.
    pub fn stop(&mut self) {
        self.stopped = true;
        unsafe { libc::kill(self.child.id() as libc::pid_t, libc::SIGINT) };
    }

    /// True once [`TestRun::stop`] was called.
    pub fn stopped(&self) -> bool {
        self.stopped
    }

    /// Once the child has exited and its output is fully read: the final
    /// `done:true` line with `"exit"` added (the exit code, or 128 + signal).
    /// A child that exits without a done line yields a failed verdict with
    /// the message `tester exited with code N`.
    pub fn finished(&mut self) -> Option<Value> {
        let status = self.child.try_wait().ok().flatten()?;
        let exited = *self.exited.get_or_insert_with(Instant::now);
        if !self.eof && exited.elapsed() < EOF_GRACE {
            return None; // the last lines may still be on their way
        }
        let code = status.code().unwrap_or_else(|| {
            use std::os::unix::process::ExitStatusExt;
            128 + status.signal().unwrap_or(0)
        });
        let mut v = self.done.take().unwrap_or_else(|| {
            json!({
                "kind": self.kind, "done": true, "passed": false, "errors": 0,
                "interrupted": self.stopped, "message": format!("tester exited with code {code}"),
            })
        });
        v["exit"] = json!(code);
        Some(v)
    }
}

impl Drop for TestRun {
    /// The daemon is going away (or dropped the run): stop the tester rather
    /// than leave it loading the machine with nobody to record the result.
    fn drop(&mut self) {
        if !self.stopped && matches!(self.child.try_wait(), Ok(None)) {
            self.stop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(c: &Command) -> Vec<String> {
        std::iter::once(c.get_program().to_string_lossy().into_owned())
            .chain(c.get_args().map(|a| a.to_string_lossy().into_owned()))
            .collect()
    }

    #[test]
    fn child_commands() {
        let bin = Path::new("/opt/bin");
        assert_eq!(argv(&command("cpu", 120, 8, bin).unwrap()), ["/opt/bin/omarchy-oc", "stress", "cpu", "--json", "--seconds", "120"]);
        assert_eq!(
            argv(&command("cpu-per-core", 240, 8, bin).unwrap()),
            ["/opt/bin/omarchy-oc", "stress", "cpu", "--json", "--seconds", "240", "--per-core", "--core-seconds", "30"]
        );
        // Never under MIN_CORE_SECONDS per core.
        assert_eq!(argv(&command("cpu-per-core", 30, 8, bin).unwrap())[8], "10");
        assert_eq!(argv(&command("gpu", 60, 8, bin).unwrap()), ["/opt/bin/omarchy-oc-gpustress", "--json", "--seconds", "60"]);
        assert!(command("disk", 60, 8, bin).is_err());
        let c = command("cpu", 1, 1, bin).unwrap();
        assert!(c.get_envs().any(|(k, v)| k == "OMARCHY_OC_CHILD" && v.is_some_and(|v| v == "1")));
    }
}
