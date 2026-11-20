//! A small rotating log file that users can attach to bug reports.
//! `kahf.log` grows to `MAX_BYTES`, then rolls to `kahf.log.1`, `.2`, `.3`.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const MAX_BYTES: u64 = 512 * 1024;
const KEEP: usize = 3;
const NAME: &str = "kahf.log";

struct Sink {
    dir: PathBuf,
    file: File,
    size: u64,
}

static SINK: OnceLock<Mutex<Sink>> = OnceLock::new();

fn path_of(dir: &Path, n: usize) -> PathBuf {
    if n == 0 {
        dir.join(NAME)
    } else {
        dir.join(format!("{NAME}.{n}"))
    }
}

fn open(dir: &Path) -> std::io::Result<Sink> {
    fs::create_dir_all(dir)?;
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path_of(dir, 0))?;
    let size = file.metadata().map(|m| m.len()).unwrap_or(0);
    Ok(Sink {
        dir: dir.to_path_buf(),
        file,
        size,
    })
}

fn rotate(sink: &mut Sink) -> std::io::Result<()> {
    let _ = fs::remove_file(path_of(&sink.dir, KEEP));
    for n in (0..KEEP).rev() {
        let from = path_of(&sink.dir, n);
        if from.exists() {
            fs::rename(&from, path_of(&sink.dir, n + 1))?;
        }
    }
    sink.file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path_of(&sink.dir, 0))?;
    sink.size = 0;
    Ok(())
}

fn write_line(sink: &mut Sink, line: &str) {
    if sink.size + line.len() as u64 > MAX_BYTES && rotate(sink).is_err() {
        return;
    }
    if sink.file.write_all(line.as_bytes()).is_ok() {
        sink.size += line.len() as u64;
    }
}

fn stamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (h, m, s) = ((secs / 3600) % 24, (secs / 60) % 60, secs % 60);
    let days = secs / 86_400;
    // Civil date from days since 1970-01-01 (Hinnant).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(mo <= 2);
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{m:02}:{s:02}Z")
}

/// Starts logging into `dir` and records panics. Safe to call once; later calls do nothing.
pub fn init(dir: &Path) {
    let Ok(sink) = open(dir) else { return };
    if SINK.set(Mutex::new(sink)).is_err() {
        return;
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log("PANIC", &info.to_string());
        previous(info);
    }));
    log("INFO", &format!("KAHF {} started", env!("CARGO_PKG_VERSION")));
}

/// Appends one line. Does nothing before `init` or if the disk refuses.
pub fn log(level: &str, message: &str) {
    if let Some(m) = SINK.get() {
        // try_lock: a panic raised while the lock is held must not deadlock the hook.
        if let Ok(mut sink) = m.try_lock() {
            let line = format!("{} {level} {}\n", stamp(), message.replace('\n', " "));
            write_line(&mut sink, &line);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("kahf-applog-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn appends_lines() {
        let d = temp("append");
        let mut s = open(&d).unwrap();
        write_line(&mut s, "a\n");
        write_line(&mut s, "b\n");
        assert_eq!(fs::read_to_string(d.join(NAME)).unwrap(), "a\nb\n");
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn rotates_and_keeps_a_few_files() {
        let d = temp("rotate");
        let mut s = open(&d).unwrap();
        let big = "x".repeat(MAX_BYTES as usize - 10) + "\n";
        for _ in 0..(KEEP + 3) {
            write_line(&mut s, &big);
        }
        for n in 0..=KEEP {
            assert!(path_of(&d, n).exists(), "missing {n}");
        }
        assert!(!path_of(&d, KEEP + 1).exists());
        assert!(fs::metadata(d.join(NAME)).unwrap().len() <= MAX_BYTES);
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn stamp_is_plausible() {
        let s = stamp();
        assert_eq!(s.len(), 20);
        assert!(s.starts_with("20"));
    }
}
