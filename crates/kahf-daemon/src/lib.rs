//! KAHF engine: the one API that the GUI, the CLI and the JSON server all use.
//! Owner: daemon agent (implements); GUI agent consumes this signature list as is.
use std::path::PathBuf;

#[derive(Clone, Debug, Default)]
pub struct Config {
    pub callsign: String,
    pub audio_in: Option<String>,
    pub audio_out: Option<String>,
    pub rigctld: Option<String>,
    pub sdr: Option<String>,
    pub data_dir: PathBuf,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Link(kahf_core::link::LinkState),
    Message { from: String, text: String },
    Mail { id: u64, from: String, subject: String },
    FileProgress { id: u64, name: String, done: u64, total: u64 },
    Spectrum(Vec<f32>),
    Log(String),
}

pub struct Engine { pub config: Config }
impl Engine {
    pub fn new(config: Config) -> Result<Self, String> { Ok(Engine { config }) }
    pub fn connect(&mut self, _peer: &str) -> Result<(), String> { unimplemented!() }
    pub fn disconnect(&mut self) -> Result<(), String> { unimplemented!() }
    pub fn send_message(&mut self, _to: &str, _text: &str) -> Result<(), String> { unimplemented!() }
    pub fn send_mail(&mut self, _to: &str, _subject: &str, _body: &str, _attachments: &[PathBuf]) -> Result<u64, String> { unimplemented!() }
    pub fn send_file(&mut self, _to: &str, _path: &std::path::Path) -> Result<u64, String> { unimplemented!() }
    pub fn poll_events(&mut self) -> Vec<Event> { unimplemented!() }
}
