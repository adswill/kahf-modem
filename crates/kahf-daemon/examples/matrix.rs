//! A scripted test matrix over real audio (BlackHole 2ch), every station an engine on the same
//! device. Prints PASS or FAIL for each step and a summary. Cleanly exits non-zero on a failure.
//! Usage: cargo run --release -p kahf-daemon --example matrix -- [direct|relays|all] ["BlackHole 2ch"]
use std::path::PathBuf;
use std::time::{Duration, Instant};

use kahf_core::link::LinkState;
use kahf_core::relay::{Direction, Radio, Role, Slot};
use kahf_daemon::{Config, Engine, Event, RelayAck};

struct St {
    name: &'static str,
    call: &'static str,
    e: Engine,
    dir: PathBuf,
    msgs: Vec<String>,
    files: Vec<String>,
    linked: Option<String>,
}

fn station(name: &'static str, call: &'static str, dev: &str) -> St {
    let dir = std::env::temp_dir().join(format!("kahf-matrix-{call}"));
    let _ = std::fs::remove_dir_all(&dir);
    let mut e = Engine::new(Config {
        callsign: call.into(),
        audio_in: Some(dev.into()),
        audio_out: Some(dev.into()),
        data_dir: dir.clone(),
        ..Config::default()
    })
    .expect("engine");
    e.set_listen_only(false);
    e.listen().expect("listen");
    St {
        name,
        call,
        e,
        dir,
        msgs: Vec::new(),
        files: Vec::new(),
        linked: None,
    }
}

fn make_relay(s: &St) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    s.e.configure_relay(Role::Relay, Radio::HalfDuplex, &[18_113.0])
        .unwrap();
    s.e.accept_relay_schedule(vec![Slot {
        channel_khz: 18_113.0,
        start_unix: now - 10,
        duration_s: 6 * 3600,
        direction: Direction::Transmit,
    }])
    .unwrap();
    s.e.start_relay(RelayAck {
        licence_held: true,
        terms_accepted: true,
    })
    .unwrap();
}

struct Run {
    t0: Instant,
    pass: u32,
    fail: u32,
}

impl Run {
    fn say(&self, what: &str) {
        println!("{:7.1}s {what}", self.t0.elapsed().as_secs_f32());
    }
    fn step(&mut self, name: &str, ok: bool, took: Duration) {
        if ok {
            self.pass += 1;
        } else {
            self.fail += 1;
        }
        println!(
            "{:7.1}s {} {name} ({:.1} s)",
            self.t0.elapsed().as_secs_f32(),
            if ok { "PASS" } else { "FAIL" },
            took.as_secs_f32()
        );
    }
}

/// Reads every station's events once.
fn pump(all: &mut [&mut St]) {
    for s in all.iter_mut() {
        for ev in s.e.poll_events() {
            if std::env::var("KAHF_MATRIX_TRACE").is_ok() && !matches!(ev, Event::Spectrum(_)) {
                println!("          {}: {ev:?}", s.name);
            }
            match ev {
                Event::Message { text, .. } => s.msgs.push(text),
                Event::FileProgress {
                    name, done, total, ..
                } if done >= total => s.files.push(name),
                Event::Link(LinkState::Connected { peer }) => s.linked = Some(peer),
                Event::Link(LinkState::Idle) => s.linked = None,
                _ => {}
            }
        }
    }
}

fn wait(all: &mut [&mut St], secs: u64, cond: &dyn Fn(&[&mut St]) -> bool) -> Option<Duration> {
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(secs) {
        pump(all);
        if cond(all) {
            return Some(t.elapsed());
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    None
}

fn direct(run: &mut Run, dev: &str, cycles: u32) {
    run.say("== direct link: two stations");
    let mut a = station("A", "ZZ1ALPHA", dev);
    let mut b = station("B", "ZZ2BRAVO", dev);
    std::thread::sleep(Duration::from_secs(1));
    for cycle in 1..=3 {
        a.e.connect("ZZ2BRAVO").unwrap();
        let r = wait(&mut [&mut a, &mut b], 60, &|s| {
            s[0].linked.is_some() && s[1].linked.is_some()
        });
        run.step(
            &format!("cycle {cycle}: connect"),
            r.is_some(),
            r.unwrap_or_default(),
        );
        let text = format!("A to B, cycle {cycle}");
        a.e.send_message("ZZ2BRAVO", &text).unwrap();
        let r = wait(&mut [&mut a, &mut b], 90, &|s| s[1].msgs.contains(&text));
        run.step(
            &format!("cycle {cycle}: A to B message"),
            r.is_some(),
            r.unwrap_or_default(),
        );
        let text = format!("B to A, cycle {cycle}");
        b.e.send_message("ZZ1ALPHA", &text).unwrap();
        let r = wait(&mut [&mut a, &mut b], 90, &|s| s[0].msgs.contains(&text));
        run.step(
            &format!("cycle {cycle}: B to A message"),
            r.is_some(),
            r.unwrap_or_default(),
        );
        if cycle <= 2 {
            let name = format!("a-to-b-{cycle}.bin");
            let path = a.dir.join(&name);
            let data: Vec<u8> = (0..1500u32).map(|i| (i * 7 + cycle) as u8).collect();
            std::fs::write(&path, &data).unwrap();
            a.e.send_file("ZZ2BRAVO", &path).unwrap();
            let r = wait(&mut [&mut a, &mut b], 150, &|s| s[1].files.contains(&name));
            let same = std::fs::read(b.dir.join("inbox").join(&name))
                .map(|d| d == data)
                .unwrap_or(false);
            run.step(
                &format!("cycle {cycle}: 1.5 kB file A to B, content identical"),
                r.is_some() && same,
                r.unwrap_or_default(),
            );
            let name = format!("b-to-a-{cycle}.bin");
            let path = b.dir.join(&name);
            let data: Vec<u8> = (0..900u32).map(|i| (i * 13 + cycle) as u8).collect();
            std::fs::write(&path, &data).unwrap();
            b.e.send_file("ZZ1ALPHA", &path).unwrap();
            let r = wait(&mut [&mut a, &mut b], 240, &|s| s[0].files.contains(&name));
            let same = std::fs::read(a.dir.join("inbox").join(&name))
                .map(|d| d == data)
                .unwrap_or(false);
            run.step(
                &format!("cycle {cycle}: 0.9 kB file B to A, content identical"),
                r.is_some() && same,
                r.unwrap_or_default(),
            );
        }
        a.e.disconnect().unwrap();
        let r = wait(&mut [&mut a, &mut b], 40, &|s| {
            s[0].linked.is_none() && s[1].linked.is_none()
        });
        run.step(
            &format!("cycle {cycle}: disconnect seen by both"),
            r.is_some(),
            r.unwrap_or_default(),
        );
        std::thread::sleep(Duration::from_secs(3));
    }
}

/// One session, files both ways again and again: the second file of a session is the one that
/// used to stall.
fn files(run: &mut Run, dev: &str, pairs: u32) {
    run.say(&format!(
        "== files: {pairs} pairs (A to B, then B to A) in one session"
    ));
    let mut a = station("A", "ZZ1ALPHA", dev);
    let mut b = station("B", "ZZ2BRAVO", dev);
    std::thread::sleep(Duration::from_secs(1));
    a.e.connect("ZZ2BRAVO").unwrap();
    let r = wait(&mut [&mut a, &mut b], 60, &|s| {
        s[0].linked.is_some() && s[1].linked.is_some()
    });
    run.step("connect", r.is_some(), r.unwrap_or_default());
    for i in 1..=pairs {
        let name = format!("pair-{i}-a-to-b.bin");
        let data: Vec<u8> = (0..1200u32).map(|k| (k * 7 + i) as u8).collect();
        std::fs::write(a.dir.join(&name), &data).unwrap();
        a.e.send_file("ZZ2BRAVO", &a.dir.join(&name)).unwrap();
        let r = wait(&mut [&mut a, &mut b], 240, &|s| s[1].files.contains(&name));
        let same = std::fs::read(b.dir.join("inbox").join(&name))
            .map(|d| d == data)
            .unwrap_or(false);
        run.step(
            &format!("pair {i}: 1.2 kB file A to B"),
            r.is_some() && same,
            r.unwrap_or_default(),
        );
        let name = format!("pair-{i}-b-to-a.bin");
        let data: Vec<u8> = (0..900u32).map(|k| (k * 13 + i) as u8).collect();
        std::fs::write(b.dir.join(&name), &data).unwrap();
        b.e.send_file("ZZ1ALPHA", &b.dir.join(&name)).unwrap();
        let r = wait(&mut [&mut a, &mut b], 240, &|s| s[0].files.contains(&name));
        let same = std::fs::read(a.dir.join("inbox").join(&name))
            .map(|d| d == data)
            .unwrap_or(false);
        run.step(
            &format!("pair {i}: 0.9 kB file B to A"),
            r.is_some() && same,
            r.unwrap_or_default(),
        );
    }
}

fn relays(run: &mut Run, dev: &str, hops: usize) {
    run.say(&format!(
        "== relays: {hops} relay(s) in a row, messages A to D, plus a bystander"
    ));
    let mut a = station("A", "ZZ1ALPHA", dev);
    let mut r1 = station("R1", "ZZ2RELAY", dev);
    let mut r2 = station("R2", "ZZ3RELAY", dev);
    let mut d = station("D", "ZZ4DELTA", dev);
    let mut e = station("E", "ZZ5ECHO", dev);
    make_relay(&r1);
    a.e.set_relay_paths(&["ZZ2RELAY"]).unwrap();
    a.e.set_relay_timeout_ms(8_000);
    if hops == 2 {
        make_relay(&r2);
        r1.e.set_relay_paths(&["ZZ3RELAY"]).unwrap();
        r1.e.set_relay_timeout_ms(8_000);
    }
    std::thread::sleep(Duration::from_secs(1));
    for i in 1..=3 {
        let text = format!("{hops}-relay message {i}");
        let t0 = Instant::now();
        a.e.send_message("ZZ4DELTA", &text).unwrap();
        a.e.connect("ZZ2RELAY").ok();
        // The relay holds the message; A hangs up once it does.
        let mut hung = false;
        let r = wait(&mut [&mut a, &mut r1, &mut r2, &mut d, &mut e], 600, &|s| {
            let _ = &hung;
            s[3].msgs.contains(&text)
        });
        let _ = &mut hung;
        run.step(
            &format!("message {i} reached D through {hops} relay(s)"),
            r.is_some(),
            t0.elapsed(),
        );
        a.e.disconnect().ok();
        let bystander = e.msgs.iter().any(|m| *m == text);
        run.step(
            &format!("message {i} not shown to the bystander"),
            !bystander,
            Duration::ZERO,
        );
        std::thread::sleep(Duration::from_secs(5));
    }
    let _ = (&a.name, &r1.call, &r2.name);
}

fn main() {
    let what = std::env::args().nth(1).unwrap_or_else(|| "all".into());
    let dev = std::env::args().nth(2).unwrap_or_else(|| "BlackHole 2ch".into());
    let mut run = Run {
        t0: Instant::now(),
        pass: 0,
        fail: 0,
    };
    if what == "direct" || what == "all" {
        direct(
            &mut run,
            &dev,
            std::env::var("KAHF_MATRIX_CYCLES")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(3),
        );
    }
    if what == "files" {
        files(
            &mut run,
            &dev,
            std::env::var("KAHF_MATRIX_PAIRS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(10),
        );
    }
    if what == "relays" || what == "all" {
        relays(&mut run, &dev, 1);
        relays(&mut run, &dev, 2);
    }
    println!("\n{} passed, {} failed", run.pass, run.fail);
    if run.fail > 0 {
        std::process::exit(1);
    }
}
