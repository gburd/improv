//! `improv-cosmic`'s command line. Declarations only: `improv_cli`'s man
//! page generator includes this file to build `improv-cosmic(1)` from it.

use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "improv-cosmic",
    // Not argv[0]: "improv-cosmic.exe" on Windows would change every help text.
    bin_name = "improv-cosmic",
    version,
    about = "COSMIC desktop workspace for Improv models",
    long_about = "Open a model in a COSMIC (libcosmic) window that follows the desktop's theme."
)]
pub struct Cli {
    /// The model: a store file, or postgres://USER@HOST/DB?model=NAME.
    /// Without it the model is in memory only
    #[arg(value_name = "model")]
    pub model: Option<String>,
}
