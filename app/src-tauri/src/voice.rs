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
/// Notes stop at half a minute: at 150 bytes a second that is 4.5 kB, several minutes on the air.
pub const MAX_SECONDS: f32 = 30.0;

/// Below this peak a recording is treated as silence (a muted or blocked microphone).
const SILENT_PEAK: f32 = 0.002;

/// The loudest sample, 0 to 1.
fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
}

/// Brings speech to a steady level before it is squeezed: the loudest sample goes to 0.8, with at
/// most 20 times gain so a quiet room is not turned into hiss. Codec2 and a radio link both work
/// best with a strong, even signal; a mic that records at 0.05 of full scale sounds faint after it.
pub fn normalise(samples: &mut [f32]) {
    let p = peak(samples);
    if p < SILENT_PEAK {
        return;
    }
    let gain = (0.8 / p).min(20.0);
    if !(0.95..=1.05).contains(&gain) {
        for v in samples.iter_mut() {
            *v = (*v * gain).clamp(-1.0, 1.0);
        }
    }
}

/// What to tell the person when the microphone recorded nothing.
pub const SILENT_HELP: &str =
    "The microphone recorded no sound. On a Mac, open System Settings, Privacy & Security, \
Microphone and allow KAHF, then restart the app. Also check the microphone in Settings, under Radio, \
and that it is not muted.";

/// Sound devices that carry the radio's audio or are virtual cables, not a person's microphone or
/// speakers. A voice note recorded from or played to one of these is never heard.
fn is_virtual(name: &str) -> bool {
    let n = name.to_lowercase();
    [
        "blackhole",
        "loopback",
        "soundflower",
        "vb-cable",
        "vb-audio",
        "virtual",
        "aggregate",
        "multi-output",
    ]
    .iter()
    .any(|v| n.contains(v))
}

/// Which device a voice note uses. The person's choice in Settings wins. Otherwise the system
/// default, unless that is the radio's own device or a virtual cable (as when two copies of KAHF
/// are tested over BlackHole), in which case the first real microphone or speaker. `None` means
/// "use the system default". The second value says what was chosen, for the log.
pub fn choose_device(
    configured: Option<&str>,
    default: Option<&str>,
    all: &[String],
    radio: Option<&str>,
) -> (Option<String>, String) {
    if let Some(c) = configured.map(str::trim).filter(|c| !c.is_empty()) {
        return (Some(c.to_string()), format!("{c} (chosen in Settings)"));
    }
    let radio = radio.map(str::trim).filter(|r| !r.is_empty());
    let bad = |n: &str| is_virtual(n) || radio.is_some_and(|r| r == n);
    match default {
        Some(d) if !bad(d) => (None, format!("{d} (system default)")),
        _ => match all.iter().find(|n| !bad(n)) {
            Some(n) => (
                Some(n.clone()),
                format!("{n} (the system default is a radio or virtual device)"),
            ),
            None => (
                None,
                default.map_or("the system default".into(), |d| {
                    format!("{d} (system default, no other device found)")
                }),
            ),
        },
    }
}

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
    // Codec2 at this rate comes out faint; bring it to a comfortable level for the speakers.
    normalise(&mut out);
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
        *slot = Some(Recording {
            stop,
            samples,
            thread,
        });
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
        let Some(mut samples) = self.take() else {
            return Ok(None);
        };
        if samples.len() < 4000 {
            return Ok(None);
        }
        if peak(&samples) < SILENT_PEAK {
            return Err(SILENT_HELP.into());
        }
        normalise(&mut samples);
        let data = encode(&samples);
        let dir = voice_dir(data_dir);
        std::fs::create_dir_all(&dir).map_err(|e| format!("Cannot save the voice note: {e}"))?;
        let id = format!(
            "clip-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0)
        );
        std::fs::write(dir.join(format!("{id}.kvm")), &data)
            .map_err(|e| format!("Cannot save the voice note: {e}"))?;
        let path = dir.join(format!("{id}.kvm"));
        Ok(Some(
            json!({ "id": id, "seconds": (samples.len() as f32 / 800.0).round() / 10.0, "bytes": data.len(), "path": path.to_string_lossy() }),
        ))
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

    /// Stops the note that is playing, if any.
    pub fn stop_play(&self) {
        if let Ok(g) = self.play_stop.lock() {
            if let Some(f) = g.as_ref() {
                f.store(true, Ordering::SeqCst);
            }
        }
    }

    /// Plays a `.kvm` file on the speakers. Starting a second note stops the first.
    pub fn play(&self, path: &Path, device: Option<String>) -> Result<(), String> {
        let data = std::fs::read(path).map_err(|e| format!("Cannot open the voice note: {e}"))?;
        let samples = decode(&data)?;
        let mut out = AudioOut::open(device.as_deref())
            .map_err(|e| format!("The speakers could not be opened: {e}"))?;
        let stop = Arc::new(AtomicBool::new(false));
        if let Some(old) = self
            .play_stop
            .lock()
            .map_err(|e| e.to_string())?
            .replace(stop.clone())
        {
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

    /// A voiced sound (a 140 Hz pulse train through a few resonances, rising and falling in loudness
    /// like a spoken word) must come back with the same length and the same loudness shape.
    #[test]
    fn a_spoken_like_sound_keeps_its_length_and_its_loudness_shape() {
        let n = 8000 * 3;
        let mut speech = vec![0f32; n];
        for (i, s) in speech.iter_mut().enumerate() {
            let t = i as f32 / 8000.0;
            let env = ((t * std::f32::consts::PI / 1.5).sin()).abs();
            let mut v = 0.0;
            for h in 1..=20 {
                let f = 140.0 * h as f32;
                let res = 1.0 / (1.0 + ((f - 700.0) / 300.0).powi(2))
                    + 0.6 / (1.0 + ((f - 1800.0) / 400.0).powi(2));
                v += res * (2.0 * std::f32::consts::PI * f * t).sin() / h as f32;
            }
            *s = 0.5 * env * v;
        }
        let data = encode(&speech);
        // Three seconds are 75 frames of six bytes: 450 bytes, a few kilobytes on the air in a minute.
        assert_eq!(data.len(), 4 + 75 * 6);
        let back = decode(&data).unwrap();
        assert_eq!(back.len(), n);
        let frame = 320;
        let rms = |x: &[f32]| (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt();
        let a: Vec<f32> = speech.chunks(frame).map(rms).collect();
        let b: Vec<f32> = back.chunks(frame).map(rms).collect();
        let (ma, mb) = (
            a.iter().sum::<f32>() / a.len() as f32,
            b.iter().sum::<f32>() / b.len() as f32,
        );
        let cov: f32 = a.iter().zip(&b).map(|(x, y)| (x - ma) * (y - mb)).sum();
        let va: f32 = a.iter().map(|x| (x - ma).powi(2)).sum();
        let vb: f32 = b.iter().map(|y| (y - mb).powi(2)).sum();
        let corr = cov / (va * vb).sqrt();
        assert!(corr > 0.8, "the loudness shape is lost: correlation {corr}");
        assert!(mb > 0.02, "the decoded note is nearly silent: {mb}");
    }

    /// What a recording does on disk and what the receiver does with the bytes.
    #[test]
    fn a_note_saved_to_disk_reads_back_the_same_and_plays_after_the_receiver_decodes_it() {
        let dir = std::env::temp_dir().join(format!("kahf-voice-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let tone: Vec<f32> = (0..16000).map(|i| 0.3 * (i as f32 * 0.09).sin()).collect();
        let path = dir.join("clip-1.kvm");
        std::fs::write(&path, encode(&tone)).unwrap();
        let sent = std::fs::read(&path).unwrap();
        // Across the air a file arrives byte for byte; the receiver decodes the same bytes.
        let heard = decode(&sent).unwrap();
        assert_eq!(heard.len(), 16000);
        assert!(Voice::clip_path(&dir.join(".."), "x").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_quiet_recording_is_brought_up_and_silence_is_left_alone() {
        let mut quiet: Vec<f32> = (0..8000).map(|i| 0.05 * (i as f32 * 0.1).sin()).collect();
        normalise(&mut quiet);
        assert!((peak(&quiet) - 0.8).abs() < 0.02, "{}", peak(&quiet));
        let mut hiss = vec![0.0005f32; 8000];
        normalise(&mut hiss);
        assert!(peak(&hiss) < SILENT_PEAK, "silence must stay silent");
        let mut loud: Vec<f32> = (0..8000).map(|i| 0.8 * (i as f32 * 0.1).sin()).collect();
        let before = loud.clone();
        normalise(&mut loud);
        assert_eq!(loud, before, "a note at a good level is untouched");
        // Gain is capped so a nearly silent room is not blown up into noise.
        let mut faint = vec![0.004f32; 8000];
        normalise(&mut faint);
        assert!(peak(&faint) <= 0.004 * 20.0 + 1e-6);
    }

    #[test]
    fn a_note_decodes_at_a_audible_level() {
        let tone: Vec<f32> = (0..16000).map(|i| 0.05 * (i as f32 * 0.09).sin()).collect();
        let mut loud = tone.clone();
        normalise(&mut loud);
        let back = decode(&encode(&loud)).unwrap();
        assert!(peak(&back) > 0.3, "played back too faint: {}", peak(&back));
    }

    #[test]
    fn a_note_is_small_enough_for_hf() {
        let thirty_seconds = vec![0.3f32; (MAX_SECONDS * 8000.0) as usize];
        let bytes = encode(&thirty_seconds).len();
        assert_eq!(bytes, 4 + 750 * 6);
        assert!(bytes <= 5000, "{bytes}");
    }

    #[test]
    fn voice_notes_stay_off_the_radio_and_virtual_devices() {
        let all: Vec<String> = ["BlackHole 2ch", "MacBook Pro Microphone", "USB Audio CODEC"]
            .map(String::from)
            .to_vec();
        // The person's choice always wins.
        assert_eq!(
            choose_device(Some("USB Audio CODEC"), Some("BlackHole 2ch"), &all, None)
                .0
                .as_deref(),
            Some("USB Audio CODEC")
        );
        // A normal default is used as it is.
        assert_eq!(
            choose_device(
                None,
                Some("MacBook Pro Microphone"),
                &all,
                Some("USB Audio CODEC")
            )
            .0,
            None
        );
        // A virtual cable as default is skipped.
        assert_eq!(
            choose_device(None, Some("BlackHole 2ch"), &all, None)
                .0
                .as_deref(),
            Some("MacBook Pro Microphone")
        );
        // So is the radio's own device, even with a normal name.
        assert_eq!(
            choose_device(None, Some("USB Audio CODEC"), &all, Some("USB Audio CODEC"))
                .0
                .as_deref(),
            Some("MacBook Pro Microphone")
        );
        // Nothing better: fall back to the default rather than fail.
        let only: Vec<String> = vec!["BlackHole 2ch".into()];
        assert_eq!(choose_device(None, Some("BlackHole 2ch"), &only, None).0, None);
        assert_eq!(choose_device(Some("  "), None, &[], None).0, None);
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
