//! Watches the kernel log for the signs of an unstable overclock: machine
//! check exceptions (the Linux face of Windows' WHEA errors), and GPU hangs
//! or engine resets from the xe driver.

use std::io::{BufRead, BufReader};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub struct Health {
    pub mce: AtomicU64,
    pub gpu_hangs: AtomicU64,
    pub last: Mutex<Vec<String>>,
}

pub fn classify(line: &str) -> Option<&'static str> {
    let l = line.to_ascii_lowercase();
    if l.contains("[hardware error]") || l.contains("machine check") || l.contains("mce: ") && l.contains("error") {
        return Some("mce");
    }
    if (l.contains("xe ") || l.contains("xe:") || l.contains("i915")) && (l.contains("gpu hang") || l.contains("timedout job") || l.contains("engine reset") || l.contains("gt reset")) {
        return Some("gpu");
    }
    None
}

impl Health {
    /// Follow /dev/kmsg from now on (old messages are skipped).
    pub fn start(path: std::path::PathBuf) -> Arc<Health> {
        let h = Arc::new(Health::default());
        let hh = h.clone();
        std::thread::spawn(move || {
            let Ok(mut f) = std::fs::File::open(&path) else { return };
            use std::io::{Seek, SeekFrom};
            let _ = f.seek(SeekFrom::End(0));
            let r = BufReader::new(f);
            for line in r.lines() {
                let Ok(line) = line else { continue };
                let msg = line.split_once(';').map(|(_, m)| m).unwrap_or(&line);
                if let Some(kind) = classify(msg) {
                    match kind {
                        "mce" => hh.mce.fetch_add(1, Ordering::Relaxed),
                        _ => hh.gpu_hangs.fetch_add(1, Ordering::Relaxed),
                    };
                    if let Ok(mut v) = hh.last.lock() {
                        v.push(msg.chars().take(200).collect());
                        let n = v.len();
                        if n > 10 {
                            v.drain(..n - 10);
                        }
                    }
                }
            }
        });
        h
    }

    pub fn counts(&self) -> (u64, u64) {
        (self.mce.load(Ordering::Relaxed), self.gpu_hangs.load(Ordering::Relaxed))
    }
}

#[cfg(test)]
mod tests {
    use super::classify;

    #[test]
    fn kinds() {
        assert_eq!(classify("mce: [Hardware Error]: Machine check events logged"), Some("mce"));
        assert_eq!(classify("xe 0000:03:00.0: [drm] GT0: Timedout job: seqno=1"), Some("gpu"));
        assert_eq!(classify("xe 0000:03:00.0: [drm] GT0: Engine reset: engine_class=rcs"), Some("gpu"));
        assert_eq!(classify("usb 1-1: new device"), None);
    }
}
