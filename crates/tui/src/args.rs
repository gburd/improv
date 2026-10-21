//! `improv-tui`'s command line. Declarations only: `improv_cli`'s man page
//! generator includes this file to build `improv-tui(1)` from it.

use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "improv-tui",
    version,
    about = "terminal pivot viewer and editor for Improv models",
    long_about = "Show one measure of a model as a pivot grid in the terminal."
)]
pub struct Cli {
    /// The model: a store file, or postgres://USER@HOST/DB?model=NAME
    #[arg(value_name = "model")]
    pub model: String,
}
