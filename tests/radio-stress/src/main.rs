//! Radio driver stress test. Opens every driver with no hardware attached and
//! checks that each failure is a clean error: no panic, no hang.
//! Run: cargo run -q --manifest-path tests/radio-stress/Cargo.toml
//! It never streams unless KAHF_RADIO_STRESS_STREAM=1 and a radio really opened.
use kahf_radio::{drivers, open, parse_spec, probe_all, RadioSpec};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc;
use std::time::Duration;

const LIMIT: Duration = Duration::from_secs(20);

fn guarded<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let r = catch_unwind(AssertUnwindSafe(f));
        let _ = tx.send(r.map_err(|p| {
            p.downcast_ref::<&str>().map(|s| s.to_string())
                .or_else(|| p.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "panic".into())
        }));
    });
    match rx.recv_timeout(LIMIT) {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(m)) => Err(format!("PANIC: {m}")),
        Err(_) => Err(format!("HANG: no return within {} s", LIMIT.as_secs())),
    }
}

fn main() {
    std::panic::set_hook(Box::new(|_| {})); // panics are reported by the harness
    let stream = std::env::var("KAHF_RADIO_STRESS_STREAM").ok().as_deref() == Some("1");
    let mut bad = 0;
    let mut total = 0;
    let mut check = |name: String, r: Result<String, String>| {
        total += 1;
        match r {
            Ok(note) => println!("PASS {name}: {note}"),
            Err(e) => { bad += 1; println!("FAIL {name}: {e}") }
        }
    };

    check("probe_all".into(), guarded(|| probe_all().len()).map(|n| format!("{n} found")));

    // Ids chosen so no real host is contacted: empty, junk, and a closed local port.
    let ids: [Option<&str>; 5] = [None, Some(""), Some("zzz-no-such-serial"), Some("127.0.0.1:1"), Some("\u{0}:\u{ffff}")];
    for d in drivers::NAMES {
        for id in ids {
            let name = format!("open {d} id={id:?}");
            let (d2, id2) = (d.to_string(), id.map(String::from));
            let r = guarded(move || {
                let spec = RadioSpec { driver: d2, id: id2 };
                match open(&spec) {
                    Err(e) if e.trim().is_empty() => Err("empty error message".to_string()),
                    Err(e) => Ok(format!("clean error: {e}")),
                    Ok(mut radio) => {
                        // A radio really opened. Exercise queries; stream only if asked.
                        let label = radio.label();
                        let _ = radio.freq_range_hz();
                        let _ = radio.sample_rates();
                        let _ = radio.gain_stages();
                        if stream {
                            let _ = radio.start();
                            let mut buf = vec![(0f32, 0f32); 4096];
                            let _ = radio.read_iq(&mut buf);
                            radio.stop();
                        }
                        Ok(format!("opened {label} (hardware present)"))
                    }
                }
            });
            check(name, r.and_then(|x| x));
        }
    }

    // Spec parsing must never panic on odd text.
    for t in ["", " ", ":", "::", "rtlsdr:", ":rtlsdr", "RTLSDR", "rtltcp:[::1]:99999", "\u{0}", "a:b:c:d"] {
        let t = t.to_string();
        check(format!("parse_spec {t:?}"), guarded(move || match parse_spec(&t) {
            Ok(_) => "accepted".to_string(), Err(e) => format!("rejected: {e}") }));
    }

    println!("radio-stress: {} checks, {} failed", total, bad);
    std::process::exit(if bad == 0 { 0 } else { 1 });
}
