//! In-memory history of the key metrics (1 h at 1 Hz) for the charts and
//! CSV export.

use serde_json::{Value, json};
use std::collections::VecDeque;

pub const SERIES: [&str; 14] = [
    "cpu_mhz", "cpu_mhz_max", "cpu_util", "cpu_temp", "cpu_w", "ppt_w", "gpu_mhz", "gpu_temp", "gpu_vram_temp", "gpu_w", "gpu_busy",
    "gpu_fan", "mem_used_pct", "fan_cpu",
];

pub struct History {
    cap: usize,
    rows: VecDeque<(f64, [Option<f32>; SERIES.len()])>,
}

impl History {
    pub fn new(cap: usize) -> History {
        History { cap, rows: VecDeque::with_capacity(cap) }
    }

    pub fn push(&mut self, t: f64, vals: [Option<f64>; SERIES.len()]) {
        if self.rows.len() == self.cap {
            self.rows.pop_front();
        }
        self.rows.push_back((t, vals.map(|v| v.map(|x| x as f32))));
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The last `n` samples as {"t": [...], "series": {name: [...]}}.
    pub fn tail(&self, n: usize) -> Value {
        let start = self.rows.len().saturating_sub(n);
        let rows: Vec<_> = self.rows.iter().skip(start).collect();
        let t: Vec<f64> = rows.iter().map(|r| (r.0 * 10.0).round() / 10.0).collect();
        let mut series = serde_json::Map::new();
        for (i, name) in SERIES.iter().enumerate() {
            let v: Vec<Value> = rows.iter().map(|r| r.1[i].map(|x| json!((x * 10.0).round() / 10.0)).unwrap_or(Value::Null)).collect();
            series.insert(name.to_string(), Value::Array(v));
        }
        json!({"t": t, "series": series})
    }

    pub fn csv(&self, seconds: Option<f64>) -> String {
        let cutoff = seconds.and_then(|s| self.rows.back().map(|r| r.0 - s)).unwrap_or(f64::MIN);
        let mut out = String::from("time");
        for s in SERIES {
            out.push(',');
            out.push_str(s);
        }
        out.push('\n');
        for (t, vals) in self.rows.iter().filter(|r| r.0 >= cutoff) {
            out.push_str(&format!("{t:.1}"));
            for v in vals {
                out.push(',');
                if let Some(x) = v {
                    out.push_str(&format!("{x}"));
                }
            }
            out.push('\n');
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring() {
        let mut h = History::new(3);
        for i in 0..5 {
            let mut v = [None; SERIES.len()];
            v[0] = Some(i as f64);
            h.push(i as f64, v);
        }
        assert_eq!(h.len(), 3);
        let t = h.tail(2);
        assert_eq!(t["t"], json!([3.0, 4.0]));
        assert_eq!(t["series"]["cpu_mhz"], json!([3.0, 4.0]));
        let csv = h.csv(None);
        assert!(csv.starts_with("time,cpu_mhz,"));
        assert_eq!(csv.lines().count(), 4);
    }
}
