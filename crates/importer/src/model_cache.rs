//! Downloads and caches the GGUF model files in `model_catalog.rs` —
//! this is what replaces `ollama pull` as the "get the weights onto this
//! machine" step. Weights still download once into a local cache; only
//! the inference *runtime* is embedded in the binary (see `engine.rs`'s
//! module doc comment).
use crate::model_catalog::ModelSpec;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

fn cache_dir() -> Result<PathBuf, String> {
    let dirs = directories::ProjectDirs::from("dev", "gazebo", "importer")
        .ok_or("couldn't determine a cache directory on this platform")?;
    let dir = dirs.cache_dir().join("models");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

pub fn resolve(spec: &ModelSpec) -> Result<PathBuf, String> {
    let path = cache_dir()?.join(spec.file);
    if path.exists() {
        return Ok(path);
    }
    download(spec, &path)?;
    Ok(path)
}

fn download(spec: &ModelSpec, dest: &Path) -> Result<(), String> {
    let url = format!(
        "https://huggingface.co/{}/resolve/main/{}",
        spec.repo, spec.file
    );
    println!(
        "downloading {} from {} (this only happens once; cached at {})...",
        spec.file,
        spec.repo,
        dest.display()
    );
    let response = reqwest::blocking::get(&url).map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!(
            "download of {} failed: HTTP {}",
            spec.file,
            response.status()
        ));
    }
    let total = response.content_length();

    let part_path = dest.with_extension("part");
    let mut file = std::fs::File::create(&part_path).map_err(|e| e.to_string())?;
    let progress = total.map(|t| {
        let bar = indicatif::ProgressBar::new(t);
        if let Ok(style) = indicatif::ProgressStyle::with_template(
            "  {bar:40} {bytes}/{total_bytes} ({eta} remaining)",
        ) {
            bar.set_style(style);
        }
        bar
    });

    let mut hasher = Sha256::new();
    let mut reader = response;
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        hasher.update(&buf[..n]);
        if let Some(bar) = &progress {
            bar.inc(u64::try_from(n).unwrap_or(0));
        }
    }
    if let Some(bar) = progress {
        bar.finish_and_clear();
    }
    drop(file);

    let digest = to_hex(&hasher.finalize());
    if digest != spec.sha256 {
        let _ = std::fs::remove_file(&part_path);
        return Err(format!(
            "checksum mismatch for {} (expected {}, got {digest}) — download was corrupted, try again",
            spec.file, spec.sha256
        ));
    }
    std::fs::rename(&part_path, dest).map_err(|e| e.to_string())?;
    println!("  done.");
    Ok(())
}

fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}
