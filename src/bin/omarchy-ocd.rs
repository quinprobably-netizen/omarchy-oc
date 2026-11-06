//! omarchy-ocd: the overclocking daemon (runs as root under systemd).
//!
//!   omarchy-ocd run       sample, apply profiles, serve the control socket
//!   omarchy-ocd restore   put every changed setting back to stock
//!   omarchy-ocd detect    print one status snapshot and exit (any user)

use omarchy_oc::daemon::{self, Paths, SIGNAL_STOP};
use omarchy_oc::util::Sys;
use std::path::PathBuf;
use std::sync::atomic::Ordering;

extern "C" fn on_signal(_: libc::c_int) {
    SIGNAL_STOP.store(true, Ordering::Relaxed);
}

fn arg(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1).cloned())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let paths = Paths {
        run_dir: PathBuf::from(arg(&args, "--run-dir").unwrap_or_else(|| "/run/omarchy-oc".into())),
        state_dir: PathBuf::from(arg(&args, "--state-dir").unwrap_or_else(|| "/var/lib/omarchy-oc".into())),
        group: arg(&args, "--group").unwrap_or_else(|| "wheel".into()),
        kmsg: PathBuf::from(arg(&args, "--kmsg").unwrap_or_else(|| "/dev/kmsg".into())),
    };
    let sys = Sys::from_env();
    match args.first().map(String::as_str) {
        Some("run") => {
            unsafe {
                libc::signal(libc::SIGTERM, on_signal as *const () as libc::sighandler_t);
                libc::signal(libc::SIGINT, on_signal as *const () as libc::sighandler_t);
            }
            if let Err(e) = daemon::run(sys, paths) {
                eprintln!("omarchy-ocd: {e}");
                std::process::exit(1);
            }
        }
        Some("restore") => {
            for l in daemon::restore(sys, paths) {
                println!("{l}");
            }
        }
        Some("detect") => {
            let mut core = daemon::Core::passive(sys, paths);
            core.tick();
            std::thread::sleep(std::time::Duration::from_millis(1100));
            core.tick();
            println!("{}", serde_json::to_string_pretty(&core.status(5)).unwrap_or_default());
        }
        _ => {
            eprintln!("usage: omarchy-ocd run|restore|detect [--run-dir D] [--state-dir D] [--group G]");
            std::process::exit(2);
        }
    }
}
