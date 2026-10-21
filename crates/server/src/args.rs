//! `improv-server`'s command line. Declarations only: `improv_cli`'s man page
//! generator includes this file to build `improv-server(1)` from it.

use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "improv-server",
    version,
    about = "JSON HTTP API over an Improv model",
    long_about = "Serve one model over HTTP/1.1 as JSON."
)]
pub struct Cli {
    /// The model: a store file, or postgres://USER@HOST/DB?model=NAME
    #[arg(value_name = "model", default_value = "improv.db")]
    pub model: String,
    /// The address to listen on
    #[arg(
        value_name = "addr",
        env = "IMPROV_ADDR",
        default_value = "127.0.0.1:3000"
    )]
    pub addr: String,
}
