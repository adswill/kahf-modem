//! KAHF: radio messages without the internet. The desktop app, drawn natively with Alloy.

mod app;
mod model;
mod store;
mod types;
mod ui;
mod util;

use std::time::Duration;

use alloy_ui::prelude::*;

fn main() {
    let mut k = app::Kahf::new();
    k.init();
    sketch(k, |k: &app::Kahf| ui::view(k))
        .tick(Duration::from_millis(40), |k: &mut app::Kahf| k.pump())
        .run_sized("KAHF", 1360.0, 800.0)
}
