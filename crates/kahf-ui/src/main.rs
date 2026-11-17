mod app;
mod icons;
mod panels;
mod sample;
mod screens;
mod spectrum;
mod strings;
mod tech;
mod theme;
mod view;

use iced::{Size, window};

fn main() -> iced::Result {
    iced::application(app::App::new, app::App::update, app::App::view)
        .title(strings::APP_TITLE)
        .default_font(theme::UI_FONT)
        .theme(app::App::theme)
        .subscription(app::App::subscription)
        .window(window::Settings {
            size: Size::new(1512.0, 940.0),
            min_size: Some(Size::new(1100.0, 700.0)),
            ..Default::default()
        })
        .run()
}
