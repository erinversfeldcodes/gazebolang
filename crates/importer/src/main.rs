//! Reads photographed dictionary page(s) — entirely locally, no cloud API
//! — and creates real, fully-detailed flashcards from them on a deployed
//! instance of the app. A standalone downloadable binary: unlike the
//! Node.js/Ollama prototype this replaces, there is nothing else to
//! install or run — the inference engine (`llama-cpp-2`, a Rust binding
//! to llama.cpp) is linked straight into this executable. The only thing
//! that still happens on first run is downloading the GGUF model weight
//! files themselves (multi-GB — that's unavoidable for local inference)
//! into a local cache; see `model_cache.rs`/`model_catalog.rs`.
//!
//! # Building
//!
//! Needs a C/C++ toolchain and `CMake` (`llama-cpp-sys-2` vendors and
//! builds llama.cpp from source via `build.rs` — both are listed in this
//! repo's `Brewfile`/`flake.nix`). On Apple Silicon, Metal GPU
//! acceleration is linked in automatically (no extra feature flag needed
//! — see `llama-cpp-2`'s own `Cargo.toml`). First build is slow (tens of
//! minutes, compiling llama.cpp itself); subsequent builds are fast.
//!
//! ```text
//! cargo build --release -p importer
//! ```
//!
//! # Usage
//!
//! ```text
//! AUTH_EMAIL=you@example.com AUTH_PASSWORD=... \
//!   target/release/importer photo1.jpg photo2.jpg
//! ```
//!
//! Three stages, run as three separate OS processes (`segment`,
//! `decompose`, `create` — `all`, the implicit default when the first
//! argument isn't one of those names, just runs all three as child
//! processes in order):
//!
//! 1. **segment**   Vision pass. Reads a photo into one
//!    `{headword, raw_entry}` block per bold headword, verbatim, no
//!    restructuring.
//! 2. **decompose** Per-entry, per-item text pass (no image). Two
//!    sub-calls per entry: first *enumerate* the entry's distinct
//!    learnable units (senses, sub-senses, worked examples, derived
//!    words) as short labels, then *fill in* each one's full card fields
//!    with its own small call. Splitting it this way — rather than
//!    asking for a whole JSON array of multi-field cards in one
//!    generation — is deliberate: that one-big-array shape is what
//!    produced inconsistent output (hallucinated derived words,
//!    phrase-shattering, self-referential cards) when first tried.
//! 3. **create**    Image lookup (Pixabay) + POST to the app. No model
//!    inference at all, so it never competes with either model for
//!    memory.
//!
//! Running each stage as its own OS process (rather than switching
//! models within one long-lived process) is deliberate: on a host with
//! limited RAM, letting one model stay resident while a second,
//! differently-sized model loads for the next stage risks a low-memory
//! kill, even though neither model alone is the problem.
//!
//! Every model call also passes a JSON Schema to the engine's
//! grammar-constrained sampling, which guarantees valid JSON *shape*; it
//! does nothing for hallucinated *content*, which is why the mechanical
//! card filter (see `decompose.rs`'s `card_rejection_reason`) still
//! matters. See `engine.rs`'s own doc comment for a real, confirmed crash
//! risk in that sampling path and how it's worked around.
//!
//! Or run/resume one stage at a time (useful if a long run fails partway
//! — `all` prints the run directory it's using, which holds
//! `entries.json` and `cards.json` so you can inspect or hand-edit them
//! between stages):
//!
//! ```text
//! target/release/importer segment    <entries.json> <photo.jpg...>
//! target/release/importer decompose  <entries.json> <cards.json>
//! target/release/importer create     <cards.json>
//! ```
//!
//! # Env vars
//!
//! - `BASE_URL` — default `http://127.0.0.1:3000`. Your local
//!   `cargo leptos watch` server, or a real deployed instance of the app.
//! - `AUTH_EMAIL` / `AUTH_PASSWORD` — required (only `create` actually
//!   uses them). Logs in for real, the same way the app's own login form
//!   does, every run — logging in fresh rather than caching the resulting
//!   cookie is deliberate: a stale cached session cookie is exactly what
//!   broke a long run partway through once already.
//! - `PIXABAY_API_KEY` — same key the app's own `PIXABAY_API_KEY` uses;
//!   omit it and cards are still created, just with no image.
//! - `IMPORTER_SEGMENT_MODEL_PATH` / `IMPORTER_SEGMENT_MMPROJ_PATH` /
//!   `IMPORTER_DECOMPOSE_MODEL_PATH` — optional. Point straight at a
//!   local GGUF file to skip the default catalog/download entirely.
//!   Unset means the pinned defaults in `model_catalog.rs` are downloaded
//!   into a local cache on first use (see `model_cache.rs`).
//! - `IMPORTER_NUM_CTX` — default `16384`. The model's context window;
//!   raise this if a dense page or long entry overflows it.
//!
//! This is slow — a real page runs one vision call plus two text calls
//! per learnable unit (dozens of sequential model calls), easily 10+
//! minutes for a dense page. That's fine for "let it run and review
//! after"; it's not meant to be interactive.
mod app_client;
mod cli;
mod create;
mod decompose;
mod engine;
mod json_extract;
mod model_cache;
mod model_catalog;
mod pixabay;
mod run_dir;
mod schema;
mod segment;

use app_client::AppClient;
use clap::Parser;
use cli::{Cli, Commands};
use std::path::{Path, PathBuf};

fn main() {
    if let Err(e) = run() {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let cli = Cli::parse_from(normalize_args(std::env::args().collect()));
    match cli.command {
        Commands::Segment {
            entries_out_file,
            images,
        } => run_segment_cmd(&entries_out_file, &images),
        Commands::Decompose {
            entries_in_file,
            cards_out_file,
        } => run_decompose_cmd(&entries_in_file, &cards_out_file),
        Commands::Create { cards_in_file } => run_create_cmd(&cards_in_file),
        Commands::All { images, run_dir } => run_all(&images, run_dir.as_deref()),
    }
}

fn normalize_args(argv: Vec<String>) -> Vec<String> {
    const KNOWN: &[&str] = &[
        "segment",
        "decompose",
        "create",
        "all",
        "-h",
        "--help",
        "-V",
        "--version",
    ];
    match argv.get(1) {
        Some(first) if KNOWN.contains(&first.as_str()) => argv,
        _ => {
            let mut out = Vec::with_capacity(argv.len() + 1);
            out.push(argv[0].clone());
            out.push("all".to_string());
            out.extend(argv.into_iter().skip(1));
            out
        }
    }
}

fn require_env(name: &str) -> Result<String, String> {
    std::env::var(name).map_err(|_| format!("{name} env var is required"))
}

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

fn resolve_model_path(env_var: &str, spec: &model_catalog::ModelSpec) -> Result<PathBuf, String> {
    match std::env::var(env_var) {
        Ok(path) => Ok(PathBuf::from(path)),
        Err(_) => model_cache::resolve(spec),
    }
}

fn run_segment_cmd(entries_out_file: &Path, images: &[PathBuf]) -> Result<(), String> {
    let model_path =
        resolve_model_path("IMPORTER_SEGMENT_MODEL_PATH", &model_catalog::SEGMENT_MODEL)?;
    let mmproj_path = resolve_model_path(
        "IMPORTER_SEGMENT_MMPROJ_PATH",
        &model_catalog::SEGMENT_MMPROJ,
    )?;
    segment::run_segment(&model_path, &mmproj_path, entries_out_file, images)
}

fn run_decompose_cmd(entries_in_file: &Path, cards_out_file: &Path) -> Result<(), String> {
    let model_path = resolve_model_path(
        "IMPORTER_DECOMPOSE_MODEL_PATH",
        &model_catalog::DECOMPOSE_MODEL,
    )?;
    decompose::run_decompose(&model_path, entries_in_file, cards_out_file)
}

fn run_create_cmd(cards_in_file: &Path) -> Result<(), String> {
    let base_url = env_or("BASE_URL", "http://127.0.0.1:3000");
    let auth_email = require_env("AUTH_EMAIL")?;
    let auth_password = require_env("AUTH_PASSWORD")?;
    let pixabay_api_key = std::env::var("PIXABAY_API_KEY").ok();
    let config = create::CreateConfig {
        base_url,
        auth_email: &auth_email,
        auth_password: &auth_password,
        pixabay_api_key: pixabay_api_key.as_deref(),
    };
    create::run_create(&config, cards_in_file)
}

fn run_all(images: &[PathBuf], run_dir_override: Option<&Path>) -> Result<(), String> {
    if images.is_empty() {
        return Err("usage: importer [all] <photo1.jpg> [photo2.jpg ...]".to_string());
    }

    let base_url = env_or("BASE_URL", "http://127.0.0.1:3000");
    let auth_email = require_env("AUTH_EMAIL")?;
    let auth_password = require_env("AUTH_PASSWORD")?;
    AppClient::new(base_url)?.log_in(&auth_email, &auth_password)?;

    let run_dir = run_dir_override.map_or_else(
        || PathBuf::from(".import-runs").join(run_dir::run_dir_name(&images[0].to_string_lossy())),
        Path::to_path_buf,
    );
    let entries_file = run_dir.join("entries.json");
    let cards_file = run_dir.join("cards.json");
    println!("run directory: {}", run_dir.display());

    let mut segment_args = vec!["segment".to_string(), path_arg(&entries_file)];
    segment_args.extend(images.iter().map(|p| path_arg(p)));
    run_child_stage(&segment_args)?;
    run_child_stage(&[
        "decompose".to_string(),
        path_arg(&entries_file),
        path_arg(&cards_file),
    ])?;
    run_child_stage(&["create".to_string(), path_arg(&cards_file)])?;
    Ok(())
}

fn path_arg(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn run_child_stage(args: &[String]) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let status = std::process::Command::new(&exe)
        .args(args)
        .status()
        .map_err(|e| e.to_string())?;
    if !status.success() {
        return Err(format!(
            "stage \"{}\" failed ({status}) — fix the issue, then resume with:\n  {} {}",
            args[0],
            exe.display(),
            args.join(" ")
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn known_subcommands_pass_through_unchanged() {
        for cmd in ["segment", "decompose", "create", "all"] {
            assert_eq!(
                normalize_args(args(&["importer", cmd, "x"])),
                args(&["importer", cmd, "x"])
            );
        }
    }

    #[test]
    fn help_and_version_flags_pass_through_unchanged() {
        for flag in ["-h", "--help", "-V", "--version"] {
            assert_eq!(
                normalize_args(args(&["importer", flag])),
                args(&["importer", flag])
            );
        }
    }

    #[test]
    fn an_unknown_first_argument_gets_all_spliced_in() {
        assert_eq!(
            normalize_args(args(&["importer", "photo1.jpg", "photo2.jpg"])),
            args(&["importer", "all", "photo1.jpg", "photo2.jpg"])
        );
    }

    #[test]
    fn no_arguments_at_all_gets_all_spliced_in() {
        assert_eq!(
            normalize_args(args(&["importer"])),
            args(&["importer", "all"])
        );
    }
}
