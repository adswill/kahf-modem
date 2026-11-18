//! Voice notes: record from the microphone, squeeze with Codec2 (1200 bit/s), play back.
//!
//! A note is a `.kvm` file: the four bytes `KVM1`, then Codec2 frames of six bytes, each 40 ms
//! of speech. That is 150 bytes a second, so a ten-second note is 1.5 kB and goes over the
//! air as an ordinary file. Codec2 is an open codec without patents (LGPL).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use codec2::{Codec2, Codec2Mode};
use kahf_audio::{level_db, AudioIn, AudioOut};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

const MAGIC: &[u8; 4] = b"KVM1";
const MODE: Codec2Mode = Codec2Mode::MODE_1200;
/// Notes stop at two minutes: a ten-second note already takes about half a minute on the air.
pub const MAX_SECONDS: f32 = 120.0;

fn to_i16(x: f32) -> i16 {
    (x.clamp(-1.0, 1.0) * 32767.0) as i16
}

/// Speech (8 kHz mono, -1 to 1) to the bytes of a `.kvm` file.
pub fn encode(samples: &[f32]) -> Vec<u8> {
    let mut c2 = Codec2::new(MODE);
    let frame = c2.samples_per_frame();
    let bytes = c2.bits_per_frame().div_ceil(8);
    let mut out = MAGIC.to_vec();
    let mut pcm: Vec<i16> = samples.iter().map(|&s| to_i16(s)).collect();
    pcm.resize(pcm.len().div_ceil(frame) * frame, 0);
    let mut buf = vec![0u8; bytes];
    for block in pcm.chunks(frame) {
        c2.encode(&mut buf, block);
        out.extend_from_slice(&buf);
    }
    out
}

/// The bytes of a `.kvm` file back to speech.
pub fn decode(data: &[u8]) -> Result<Vec<f32>, String> {
    if data.len() < 4 || &data[..4] != MAGIC {
        return Err("This is not a KAHF voice note.".into());
    }
    let mut c2 = Codec2::new(MODE);
    let frame = c2.samples_per_frame();
    let bytes = c2.bits_per_frame().div_ceil(8);
    let mut out = Vec::new();
    let mut pcm = vec![0i16; frame];
    for chunk in data[4..].chunks_exact(bytes) {
        c2.decode(&mut pcm, chunk);
        out.extend(pcm.iter().map(|&s| s as f32 / 32768.0));
    }
    Ok(out)
}

/// Length in seconds of a note of `bytes` bytes.
#[cfg(test)]
fn seconds_of(bytes: usize) -> f32 {
    bytes.saturating_sub(4) as f32 / 6.0 * 0.04
}

/// A recording in progress. The thread keeps listening until `stop` is called.
struct Recording {
    stop: Arc<AtomicBool>,
    samples: Arc<Mutex<Vec<f32>>>,
    thread: std::thread::JoinHandle<()>,
}

#[derive(Default)]
pub struct Voice {
    rec: Mutex<Option<Recording>>,
    /// Stops the note that is playing, if any.
    play_stop: Mutex<Option<Arc<AtomicBool>>>,
}

fn voice_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("voice")
}

impl Voice {
    pub fn record(&self, app: AppHandle, device: Option<String>) -> Result<(), String> {
        let mut slot = self.rec.lock().map_err(|e| e.to_string())?;
        if slot.is_some() {
            return Ok(());
        }
        let mut input = AudioIn::open(device.as_deref())
            .map_err(|e| format!("The microphone could not be opened: {e}"))?;
        let stop = Arc::new(AtomicBool::new(false));
        let samples = Arc::new(Mutex::new(Vec::<f32>::new()));
        let (st, sm) = (stop.clone(), samples.clone());
        let thread = std::thread::spawn(move || {
            let mut buf = vec![0f32; 1024];
            let mut last_level = Instant::now();
            let mut window: Vec<f32> = Vec::new();
            while !st.load(Ordering::SeqCst) {
                let n = input.read(&mut buf);
                if n == 0 {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                }
                window.extend_from_slice(&buf[..n]);
                if let Ok(mut s) = sm.lock() {
                    if s.len() as f32 / 8000.0 < MAX_SECONDS {
                        s.extend_from_slice(&buf[..n]);
                    }
                }
                if last_level.elapsed() >= Duration::from_millis(90) {
                    // About -50 dB (quiet room) to -10 dB (loud talker) across the meter.
                    let level = ((level_db(&window) + 50.0) / 40.0).clamp(0.0, 1.0);
                    let _ = app.emit("engine", json!({ "type": "level", "level": level }));
                    window.clear();
                    last_level = Instant::now();
                }
            }
        });
        *slot = Some(Recording { stop, samples, thread });
        Ok(())
    }

    fn take(&self) -> Option<Vec<f32>> {
        let rec = self.rec.lock().ok()?.take()?;
        rec.stop.store(true, Ordering::SeqCst);
        let _ = rec.thread.join();
        let s = rec.samples.lock().ok()?.clone();
        Some(s)
    }

    /// Stops recording and saves the note. `None` when it was shorter than half a second.
    pub fn stop(&self, data_dir: &Path) -> Result<Option<Value>, String> {
        let Some(samples) = self.take() else { return Ok(None) };
        if samples.len() < 4000 {
            return Ok(None);
        }
        let data = encode(&samples);
        let dir = voice_dir(data_dir);
        std::fs::create_dir_all(&dir).map_err(|e| format!("Cannot save the voice note: {e}"))?;
        let id = format!("clip-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0));
        std::fs::write(dir.join(format!("{id}.kvm")), &data).map_err(|e| format!("Cannot save the voice note: {e}"))?;
        Ok(Some(json!({ "id": id, "seconds": (samples.len() as f32 / 800.0).round() / 10.0, "bytes": data.len() })))
    }

    pub fn cancel(&self) {
        let _ = self.take();
    }

    /// The file of a saved clip.
    pub fn clip_path(data_dir: &Path, clip_id: &str) -> Result<PathBuf, String> {
        if !clip_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err("Unknown voice note.".into());
        }
        let p = voice_dir(data_dir).join(format!("{clip_id}.kvm"));
        if p.is_file() {
            Ok(p)
        } else {
            Err("That voice note is gone.".into())
        }
    }

    /// Plays a `.kvm` file on the speakers. Starting a second note stops the first.
    pub fn play(&self, path: &Path, device: Option<String>) -> Result<(), String> {
        let data = std::fs::read(path).map_err(|e| format!("Cannot open the voice note: {e}"))?;
        let samples = decode(&data)?;
        let mut out = AudioOut::open(device.as_deref()).map_err(|e| format!("The speakers could not be opened: {e}"))?;
        let stop = Arc::new(AtomicBool::new(false));
        if let Some(old) = self.play_stop.lock().map_err(|e| e.to_string())?.replace(stop.clone()) {
            old.store(true, Ordering::SeqCst);
        }
        std::thread::spawn(move || {
            for chunk in samples.chunks(800) {
                if stop.load(Ordering::SeqCst) || out.write(chunk).is_err() {
                    return;
                }
            }
            // The buffer holds up to two seconds that are still to be heard.
            std::thread::sleep(Duration::from_millis(2200));
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_note_keeps_its_length_and_loudness() {
        let tone: Vec<f32> = (0..16000).map(|i| 0.4 * (i as f32 * 0.07).sin()).collect();
        let data = encode(&tone);
        assert_eq!(&data[..4], b"KVM1");
        // Two seconds are 50 frames of six bytes.
        assert_eq!(data.len(), 4 + 50 * 6);
        assert!((seconds_of(data.len()) - 2.0).abs() < 0.05);
        let back = decode(&data).unwrap();
        assert_eq!(back.len(), 16000);
        assert!(back.iter().any(|v| v.abs() > 0.05), "the decoded note is silent");
    }

    #[test]
    fn something_else_is_refused() {
        assert!(decode(b"nope").is_err());
        assert!(decode(&[]).is_err());
    }

    #[test]
    fn clip_names_cannot_leave_the_voice_folder() {
        assert!(Voice::clip_path(Path::new("/tmp"), "../etc/passwd").is_err());
    }
}
