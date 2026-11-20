//! HF channel benchmark: frame error rate and goodput per modulation over AWGN and the
//! ITU-R F.1487 Watterson channels. SNR is quoted in a 2500 Hz noise bandwidth, the
//! reference FT8/JS8/VARA/ARDOP figures use (the sim SNR is per 4 kHz of real-sample noise).
//! Run: cargo run --release -p kahf-sim --example hf_bench -- [frames] [snr_from snr_to snr_step]
//! Output is CSV: channel,mod,snr_2500,fer,goodput_bps
use kahf_core::phy::{self, Modulation};
use kahf_core::{frame, ldpc};
use kahf_sim::{apply, Profile};
use rayon::prelude::*;

/// Sim SNR (full 4 kHz Nyquist band) to 2500 Hz-reference SNR.
const PAD: usize = 2000;
const REF_OFFSET_DB: f32 = 2.04;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let frames: usize = a.first().and_then(|s| s.parse().ok()).unwrap_or(100);
    let from: f32 = a.get(1).and_then(|s| s.parse().ok()).unwrap_or(-6.0);
    let to: f32 = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(30.0);
    let step: f32 = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(3.0);
    let chans = [
        ("awgn", Profile::Awgn),
        ("good", Profile::WattersonGood),
        ("moderate", Profile::WattersonModerate),
        ("poor", Profile::WattersonPoor),
        ("flutter", Profile::WattersonFlutter),
    ];
    let mods = [
        ("qpsk", Modulation::Qpsk),
        ("16qam", Modulation::Qam16),
        ("64qam", Modulation::Qam64),
    ];
    println!("channel,mod,snr_2500,fer,goodput_bps");
    for (cn, prof) in chans {
        for (mn, m) in mods {
            let mut snr = from;
            while snr <= to + 1e-3 {
                let sim_snr = snr - REF_OFFSET_DB;
                let mut airtime = 0usize;
                let bad = (0..frames)
                    .into_par_iter()
                    .map(|f| {
                        let data: Vec<u8> = (0..36).map(|i| ((f * 37 + i * 11) % 251) as u8).collect();
                        let coded = ldpc::encode(&frame::build_payload(&data));
                        let tx = phy::modulate_frame(&coded, m);
                        // Real streams have silence around a frame. Pad, and raise the nominal SNR so
                        // the in-frame SNR is the stated one (apply() measures over the whole buffer).
                        let mut padded = vec![0f32; PAD];
                        padded.extend_from_slice(&tx);
                        padded.extend(std::iter::repeat_n(0f32, PAD));
                        let comp = 10.0 * (padded.len() as f32 / tx.len() as f32).log10();
                        let rx = apply(&padded, prof, sim_snr + comp, 9000 + f as u64);
                        let ok = phy::demodulate_frame(&rx, m, ldpc::N)
                            .and_then(|l| ldpc::decode(&l, 50))
                            .and_then(|b| frame::parse_payload(&b))
                            .map(|o| o == data)
                            .unwrap_or(false);
                        (tx.len(), !ok)
                    })
                    .collect::<Vec<_>>();
                airtime = bad.first().map(|x| x.0).unwrap_or(airtime);
                let fer = bad.iter().filter(|x| x.1).count() as f64 / frames as f64;
                let gp = (1.0 - fer) * 36.0 * 8.0 / (airtime as f64 / 8000.0);
                println!("{cn},{mn},{snr:.1},{fer:.3},{gp:.0}");
                snr += step;
            }
        }
    }
}
