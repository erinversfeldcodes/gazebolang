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
    /// Vision pass: read photographed page(s) into raw per-headword entries.
    Segment {
        entries_out_file: PathBuf,
        #[arg(required = true, num_args = 1..)]
        images: Vec<PathBuf>,
    },
    /// Text pass: turn raw entries into flashcards.
    Decompose {
        entries_in_file: PathBuf,
        cards_out_file: PathBuf,
    },
    /// Looks up images and creates the cards on the deployed app.
    Create { cards_in_file: PathBuf },
    /// Runs all three stages in order against fresh photos (the default
    /// when the first argument isn't a known subcommand name).
    All {
        #[arg(required = true, num_args = 1..)]
        images: Vec<PathBuf>,
        /// Where this run's intermediate entries.json/cards.json go.
        /// Defaults to ./.import-runs/<first-image-name>-<timestamp>,
        /// resolved relative to the current directory (not the binary's
        /// own location).
        #[arg(long)]
        run_dir: Option<PathBuf>,
    },
}
