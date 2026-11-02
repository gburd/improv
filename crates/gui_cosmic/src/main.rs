//! `improv-cosmic`: Improv's COSMIC desktop GUI, on libcosmic and iced.
//! See `docs/src/cosmic-gui.md`.

mod app;
mod args;
mod pivot;
mod service;
mod sheet;
mod shot;

use clap::Parser;

fn main() -> cosmic::iced::Result {
    let cli = args::Cli::parse();
    let settings = cosmic::app::Settings::default().size(cosmic::iced::Size::new(1200.0, 800.0));
    cosmic::app::run::<app::App>(
        settings,
        app::Flags {
            target: cli.model.unwrap_or_default(),
        },
    )
}
