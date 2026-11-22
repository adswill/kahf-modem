//! One source of time for the engine: real in the app, virtual in tests.
//!
//! A virtual clock only moves when every thread that joined it is asleep on it (or parked
//! waiting for another). Then it jumps to the earliest wake-up. So a test that needs 40 s of
//! radio time takes only as long as the computing needs, and runs the same way every time.

use std::cell::RefCell;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Clone)]
pub struct Clock(Option<Arc<Virt>>);

struct Virt {
    base: Instant,
    st: Mutex<St>,
    cv: Condvar,
}

#[derive(Default)]
struct St {
    now: Duration,
    parts: usize,
    asleep: usize,
    wakes: Vec<Duration>,
}

/// Keeps a thread counted as a participant of a virtual clock. Dropping it leaves.
pub struct Joined(Option<Arc<Virt>>);

impl Clock {
    pub fn real() -> Self {
        Clock(None)
    }

    pub fn virtual_time() -> Self {
        Clock(Some(Arc::new(Virt {
            base: Instant::now(),
            st: Mutex::new(St::default()),
            cv: Condvar::new(),
        })))
    }

    pub fn is_virtual(&self) -> bool {
        self.0.is_some()
    }

    pub fn now(&self) -> Instant {
        match &self.0 {
            None => Instant::now(),
            Some(v) => v.base + v.st.lock().unwrap().now,
        }
    }

    /// Time since `t`, never negative.
    pub fn since(&self, t: Instant) -> Duration {
        self.now().saturating_duration_since(t)
    }

    /// Count the calling thread as a participant (real clock: nothing to do).
    pub fn join(&self) -> Joined {
        if let Some(v) = &self.0 {
            v.st.lock().unwrap().parts += 1;
        }
        Joined(self.0.clone())
    }

    pub fn sleep(&self, d: Duration) {
        let Some(v) = &self.0 else {
            thread::sleep(d);
            return;
        };
        let mut st = v.st.lock().unwrap();
        let wake = st.now + d;
        st.asleep += 1;
        st.wakes.push(wake);
        loop {
            if st.now >= wake {
                break;
            }
            // Everyone asleep and nobody due: jump to the earliest wake-up.
            if st.asleep == st.parts && st.wakes.iter().all(|w| *w > st.now) {
                if let Some(next) = st.wakes.iter().min().copied() {
                    st.now = next;
                    v.cv.notify_all();
                    continue;
                }
            }
            st = v.cv.wait(st).unwrap();
        }
        let i = st.wakes.iter().position(|w| *w == wake).unwrap();
        st.wakes.swap_remove(i);
        st.asleep -= 1;
    }

    /// Run `f`, which blocks on something outside the clock (a thread join), without stalling
    /// the clock for the others.
    pub fn park<R>(&self, f: impl FnOnce() -> R) -> R {
        let Some(v) = &self.0 else {
            return f();
        };
        {
            let mut st = v.st.lock().unwrap();
            st.asleep += 1;
            v.cv.notify_all();
        }
        let r = f();
        v.st.lock().unwrap().asleep -= 1;
        r
    }
}

impl Drop for Joined {
    fn drop(&mut self) {
        if let Some(v) = &self.0 {
            v.st.lock().unwrap().parts -= 1;
            v.cv.notify_all();
        }
    }
}

thread_local! {
    static HELD: RefCell<Vec<Joined>> = const { RefCell::new(Vec::new()) };
}

impl Clock {
    /// Join for the rest of this thread's life (tests).
    pub fn join_thread(&self) {
        let j = self.join();
        HELD.with(|h| h.borrow_mut().push(j));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_sleepers_finish_in_virtual_time_only() {
        let c = Clock::virtual_time();
        let t0 = c.now();
        let c2 = c.clone();
        let _me = c.join();
        let j2 = c2.join();
        let h = thread::spawn(move || {
            let _j = j2;
            c2.sleep(Duration::from_secs(50));
            c2.now()
        });
        let real = Instant::now();
        c.sleep(Duration::from_secs(100));
        let t2 = c.park(|| h.join().unwrap());
        assert!(c.since(t0) >= Duration::from_secs(100));
        assert!(t2 - t0 >= Duration::from_secs(50));
        assert!(real.elapsed() < Duration::from_secs(1));
    }
}
