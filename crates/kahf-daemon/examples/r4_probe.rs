//! One ladder rung on two stations over a real sound device (for example BlackHole 2ch): does a
//! link come up, and how many of N messages each way arrive, and how fast?
//! Usage: cargo run --release -p kahf-daemon --example r4_probe -- "BlackHole 2ch" [rungA] [rungB] [messages]
//! Defaults: rungs R4 and R4, 5 messages each way. Run it once per rung (R4, R8, Q12, ...) and also
//! with mixed rungs (R4 Q12). Exit status 1 when any message is lost.
use std::time::{Duration, Instant};

use kahf_daemon::{Config, Engine, Event};

fn station(call: &str, dev: &str, rung: &str) -> Engine {
    let dir = std::env::temp_dir().join(format!("kahf-r4-probe-{call}"));
    let _ = std::fs::remove_dir_all(&dir);
    let mut e = Engine::new(Config {
        callsign: call.into(),
        audio_in: Some(dev.into()),
        audio_out: Some(dev.into()),
        data_dir: dir,
        ..Config::default()
    })
    .expect("engine");
    e.set_listen_only(false);
    e.pin_hf_rung(Some(rung));
    e.listen().expect("listen");
    e
}

/// Polls both stations until `want` shows up at `at` (0 = A, 1 = B) or `secs` pass.
fn wait(st: &mut [Engine; 2], at: usize, secs: u64, want: impl Fn(&Event) -> bool) -> Option<f32> {
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(secs) {
        for (i, e) in st.iter_mut().enumerate() {
            for ev in e.poll_events() {
                if i == at && want(&ev) {
                    return Some(t0.elapsed().as_secs_f32());
                }
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    None
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let dev = a.first().cloned().unwrap_or_else(|| "BlackHole 2ch".into());
    let ra = a.get(1).cloned().unwrap_or_else(|| "R4".into());
    let rb = a.get(2).cloned().unwrap_or_else(|| ra.clone());
    let n: usize = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(5);
    let mut st = [station("ZZ3CHARLIE", &dev, &ra), station("ZZ4DELTA", &dev, &rb)];
    std::thread::sleep(Duration::from_secs(1));
    println!("A on {ra}, B on {rb}: A connects to ZZ4DELTA");
    st[0].connect("ZZ4DELTA").expect("connect");
    let up = wait(&mut st, 0, 60, |e| {
        matches!(e, Event::Link(kahf_core::link::LinkState::Connected { .. }))
    });
    match up {
        Some(t) => println!("link up in {t:.1} s"),
        None => {
            println!("link did NOT come up in 60 s");
            std::process::exit(1);
        }
    }
    let mut lost = 0;
    for k in 0..n {
        for (from, to) in [(0usize, 1usize), (1, 0)] {
            let text = format!("probe {k} from {from}");
            let call = if to == 1 { "ZZ4DELTA" } else { "ZZ3CHARLIE" };
            st[from].send_message(call, &text).expect("send");
            let want = text.clone();
            match wait(
                &mut st,
                to,
                90,
                move |e| matches!(e, Event::Message { text, .. } if *text == want),
            ) {
                Some(t) => println!("{text:>16}: delivered in {t:.1} s"),
                None => {
                    println!("{text:>16}: LOST (90 s)");
                    lost += 1;
                }
            }
        }
    }
    println!("{} of {} delivered", 2 * n - lost, 2 * n);
    let _ = st[0].disconnect();
    if lost > 0 {
        std::process::exit(1);
    }
}
