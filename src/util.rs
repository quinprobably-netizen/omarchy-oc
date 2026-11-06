//! Small filesystem helpers. Every hardware path goes through [`Sys`], which
//! prefixes a root directory so the tests can run against a fake sysfs tree.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug)]
pub struct Sys {
    pub root: PathBuf,
}

impl Sys {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Sys { root: root.into() }
    }

    /// From $OMARCHY_OC_ROOT, default "/".
    pub fn from_env() -> Self {
        Sys::new(std::env::var("OMARCHY_OC_ROOT").unwrap_or_else(|_| "/".into()))
    }

    /// Map an absolute system path ("/sys/...") into the root.
    pub fn p(&self, path: impl AsRef<Path>) -> PathBuf {
        let path = path.as_ref();
        match path.strip_prefix("/") {
            Ok(rel) => self.root.join(rel),
            Err(_) => path.to_path_buf(),
        }
    }

    pub fn exists(&self, path: impl AsRef<Path>) -> bool {
        self.p(path).exists()
    }

    pub fn read(&self, path: impl AsRef<Path>) -> Option<String> {
        read_trim(&self.p(path))
    }

    pub fn read_u64(&self, path: impl AsRef<Path>) -> Option<u64> {
        self.read(path)?.parse().ok()
    }

    pub fn read_i64(&self, path: impl AsRef<Path>) -> Option<i64> {
        self.read(path)?.parse().ok()
    }

    pub fn write(&self, path: impl AsRef<Path>, value: &str) -> io::Result<()> {
        let p = self.p(path);
        fs::write(&p, value).map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", p.display())))
    }

    /// Entries of a directory (full real paths), sorted; empty if missing.
    pub fn list(&self, dir: impl AsRef<Path>) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = fs::read_dir(self.p(dir))
            .map(|rd| rd.flatten().map(|e| e.path()).collect())
            .unwrap_or_default();
        v.sort_by(|a, b| natural_cmp(&a.to_string_lossy(), &b.to_string_lossy()));
        v
    }
}

pub fn read_trim(p: &Path) -> Option<String> {
    fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

pub fn read_num<T: std::str::FromStr>(p: &Path) -> Option<T> {
    read_trim(p)?.parse().ok()
}

pub fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// Monotonic and boot-time clocks in seconds. Their difference grows by the
/// time spent suspended, which is how the daemon notices a resume.
pub fn clocks() -> (f64, f64) {
    fn get(id: libc::clockid_t) -> f64 {
        let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        unsafe { libc::clock_gettime(id, &mut ts) };
        ts.tv_sec as f64 + ts.tv_nsec as f64 / 1e9
    }
    (get(libc::CLOCK_MONOTONIC), get(libc::CLOCK_BOOTTIME))
}

/// "cpu10" sorts after "cpu9".
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let (mut ai, mut bi) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, _) => return std::cmp::Ordering::Less,
            (_, None) => return std::cmp::Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let mut na = String::new();
                while let Some(c) = ai.peek().copied().filter(char::is_ascii_digit) {
                    na.push(c);
                    ai.next();
                }
                let mut nb = String::new();
                while let Some(c) = bi.peek().copied().filter(char::is_ascii_digit) {
                    nb.push(c);
                    bi.next();
                }
                let o = na.parse::<u128>().unwrap_or(0).cmp(&nb.parse::<u128>().unwrap_or(0));
                if o != std::cmp::Ordering::Equal {
                    return o;
                }
            }
            (Some(x), Some(y)) => {
                if x != y {
                    return x.cmp(&y);
                }
                ai.next();
                bi.next();
            }
        }
    }
}

/// Write `data` to `path` via a temp file and rename, with the given mode.
/// The temp file is synced before the rename and the directory after it,
/// so a crash right after leaves the old file or the new one, never an
/// empty one.
pub fn write_atomic(path: &Path, data: &[u8], mode: u32) -> io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::PermissionsExt;
    let tmp = path.with_extension("tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(data)?;
        f.set_permissions(fs::Permissions::from_mode(mode))?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    if let Some(d) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::File::open(d)?.sync_all()?;
    }
    Ok(())
}

/// Open (creating it, mode 0600) a lock file for [`flock_exclusive`].
pub fn open_lock(path: &Path) -> io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(d) = path.parent() {
        fs::create_dir_all(d)?;
    }
    fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).mode(0o600).open(path)
}

/// An exclusive flock(2), released when dropped.
pub struct FlockGuard<'a>(&'a fs::File);

/// Wait for an exclusive flock(2) on `f`. It is held by the open file, so
/// it excludes other processes and other opens of the same file.
pub fn flock_exclusive(f: &fs::File) -> io::Result<FlockGuard<'_>> {
    use std::os::fd::AsRawFd;
    loop {
        if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) } == 0 {
            return Ok(FlockGuard(f));
        }
        let e = io::Error::last_os_error();
        if e.kind() != io::ErrorKind::Interrupted {
            return Err(e);
        }
    }
}

impl Drop for FlockGuard<'_> {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}

/// Parse a CPU list like "0-3,6,8-9".
pub fn parse_cpulist(s: &str) -> Vec<u32> {
    let mut v = Vec::new();
    for part in s.trim().split(',').filter(|p| !p.is_empty()) {
        if let Some((a, b)) = part.split_once('-') {
            if let (Ok(a), Ok(b)) = (a.trim().parse::<u32>(), b.trim().parse::<u32>()) {
                v.extend(a..=b);
            }
        } else if let Ok(n) = part.trim().parse() {
            v.push(n);
        }
    }
    v
}

pub fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

/// Look up a hwmon directory by its `name` file.
pub fn hwmon_by_name(sys: &Sys, name: &str) -> Option<PathBuf> {
    sys.list("/sys/class/hwmon")
        .into_iter()
        .find(|h| read_trim(&h.join("name")).as_deref() == Some(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpulist() {
        assert_eq!(parse_cpulist("0-3,6,8-9"), vec![0, 1, 2, 3, 6, 8, 9]);
        assert_eq!(parse_cpulist(""), Vec::<u32>::new());
    }

    fn try_lock(f: &fs::File) -> bool {
        use std::os::fd::AsRawFd;
        let ok = unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
        if ok {
            unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_UN) };
        }
        ok
    }

    #[test]
    fn flock_guard_excludes_until_dropped() {
        let dir = std::env::temp_dir().join(format!("oc-flock-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join("x.lock");
        let a = open_lock(&p).unwrap();
        let b = open_lock(&p).unwrap();
        {
            let _g = flock_exclusive(&a).unwrap();
            assert!(!try_lock(&b), "a second open of the lock file must wait");
        }
        assert!(try_lock(&b), "released on drop");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_atomic_replaces_whole_file() {
        let dir = std::env::temp_dir().join(format!("oc-atomic-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join("c.json");
        fs::write(&p, "old old old").unwrap();
        write_atomic(&p, b"new", 0o640).unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"new");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o640);
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1, "no temp file left");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn natural() {
        let mut v = vec!["cpu10", "cpu2", "cpu1"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, vec!["cpu1", "cpu2", "cpu10"]);
    }
}
