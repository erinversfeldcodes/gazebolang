use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "importer",
    about = "Reads photographed dictionary pages (locally, no cloud API) and creates flashcards from them on a deployed instance of the app."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    Segment {
        entries_out_file: PathBuf,
        #[arg(required = true, num_args = 1..)]
        images: Vec<PathBuf>,
    },
    Decompose {
        entries_in_file: PathBuf,
        cards_out_file: PathBuf,
    },
    Create {
        cards_in_file: PathBuf,
    },
    All {
        #[arg(required = true, num_args = 1..)]
        images: Vec<PathBuf>,
        #[arg(long)]
        run_dir: Option<PathBuf>,
    },
}
