//! `improv-gui`'s command line. Declarations only: `improv_cli`'s man page
//! generator includes this file to build `improv-gui(1)` from it.

use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "improv-gui",
    version,
    about = "desktop workspace for Improv models",
    long_about = "Open a model in a NeXTSTEP-style workspace."
)]
pub struct Cli {
    /// The model: a store file, or postgres://USER@HOST/DB?model=NAME.
    /// Without it the model is in memory only
    #[arg(value_name = "model")]
    pub model: Option<String>,
}
