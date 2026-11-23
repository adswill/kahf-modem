use kahf_app_core::backend::*;
use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn mk(name: &'static str, call: &str, t0: Instant) -> Arc<Shared> {
    let dir = std::env::temp_dir().join(format!("kahf-bench-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let emit: Emitter = Arc::new(move |v| {
        let s = v.to_string();
        if !s.contains("\"bins\"") && !s.contains("\"level\"") && !s.contains("\"signal\"") {
            println!(
                "[{:6.1}s {name}] {}",
                t0.elapsed().as_secs_f32(),
                &s[..s.len().min(220)]
            );
        }
    });
    let sh = Arc::new(Shared::new(emit, dir));
    let s = json!({"callsign":call,"audioIn":"BlackHole 2ch","audioOut":"BlackHole 2ch","freqKhz":18113.44,"listenOnly":false,"autoLink":true});
    engine_start(&sh, s).unwrap();
    start_poll_loop(sh.clone());
    sh
}

fn main() {
    let t0 = Instant::now();
    let a = mk("alpha", "K1ALF", t0);
    let b = mk("bravo", "K1BRV", t0);
    println!("gate a: {:?}", tx_gate_status(&a));
    std::thread::sleep(Duration::from_secs(8));
    println!(
        "send a->b: {:?}",
        engine_send(&a, "1".into(), "K1BRV".into(), "hello".into(), "chat".into())
    );
    std::thread::sleep(Duration::from_secs(70));
    println!(
        "send b->a: {:?}",
        engine_send(&b, "2".into(), "K1ALF".into(), "hi back".into(), "chat".into())
    );
    std::thread::sleep(Duration::from_secs(70));
    println!(
        "file b->a: {:?}",
        engine_send_file(&b, "3".into(), "K1ALF".into(), "/tmp/kahf-bench-clip.bin".into())
    );
    std::thread::sleep(Duration::from_secs(150));
    drop(b);
}
