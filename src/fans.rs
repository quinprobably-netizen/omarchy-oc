//! Bridge to omarchy-fand (the fan-curve daemon from omarchy-fans): read
//! its status file, select its profile. LACT cannot drive Intel Arc fans;
//! omarchy-fand can, so profiles here carry a fan profile too.

use crate::util::Sys;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

pub struct Fans {
    sys: Sys,
    pub status: Value,
}

impl Fans {
    pub fn new(sys: &Sys) -> Fans {
        Fans { sys: sys.clone(), status: Value::Null }
    }

    fn run_dir(&self) -> String {
        std::env::var("OMARCHY_FANS_RUN").unwrap_or_else(|_| "/run/omarchy-fans".into())
    }

    pub fn sample(&mut self) {
        let p = self.sys.p(format!("{}/status.json", self.run_dir()));
        self.status = std::fs::read(&p).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null);
    }

    pub fn available(&self) -> bool {
        !self.status.is_null()
    }

    /// Compact summary for our own status file.
    pub fn summary(&self) -> Value {
        if self.status.is_null() {
            return json!({"available": false});
        }
        let st = &self.status;
        let fans: Vec<Value> = st["fans"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter(|f| f["connected"] != json!(false))
                    .map(|f| json!({"id": f["id"], "label": f["label"], "rpm": f["rpm"], "percent": f["percent"], "role": f["role"]}))
                    .collect()
            })
            .unwrap_or_default();
        let profiles: Vec<Value> = st["profiles"].as_array().map(|a| a.iter().map(|p| p["id"].clone()).collect()).unwrap_or_default();
        json!({"available": true, "profile": st["profile"], "mode": st["mode"], "fans": fans, "profiles": profiles, "time": st["time"]})
    }

    /// One request to omarchy-fand's control socket.
    pub fn call(&self, req: &Value) -> Result<Value, String> {
        let sock = self.sys.p(format!("{}/control.sock", self.run_dir()));
        let mut s = UnixStream::connect(&sock).map_err(|e| format!("omarchy-fand: {e}"))?;
        // Calibration replies only when it finishes (~2 min).
        let long = matches!(req["cmd"].as_str(), Some("calibrate" | "retest"));
        let _ = s.set_read_timeout(Some(Duration::from_secs(if long { 300 } else { 10 })));
        s.write_all(format!("{req}\n").as_bytes()).map_err(|e| e.to_string())?;
        let mut line = String::new();
        BufReader::new(s).read_line(&mut line).map_err(|e| e.to_string())?;
        let v: Value = serde_json::from_str(&line).map_err(|_| "omarchy-fand: garbled reply".to_string())?;
        if v["ok"] == json!(true) { Ok(v["result"].clone()) } else { Err(format!("omarchy-fand: {}", v["error"].as_str().unwrap_or("failed"))) }
    }

    pub fn set_profile(&self, name: &str) -> Result<(), String> {
        self.call(&json!({"cmd": "profile", "name": name})).map(|_| ())
    }
}
