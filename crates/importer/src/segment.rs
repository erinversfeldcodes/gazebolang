//! Pass 1 — reads one photographed dictionary page into one
//! `{headword, raw_entry}` block per bold headword, verbatim, no
//! restructuring.
use crate::engine::VisionEngine;
use crate::json_extract::extract_json_array;
use crate::schema::segment_schema;
use std::path::{Path, PathBuf};

const SUPPORTED_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp"];

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct Entry {
    pub headword: String,
    pub raw_entry: String,
}

const SEGMENT_PROMPT: &str = "This is a photo of a page from a dictionary. Each entry starts with a bold \
headword. Transcribe the page into one object per bold headword: \
{\"headword\": \"<the headword exactly as printed>\", \"raw_entry\": \"<the \
complete text of that entry, from the headword up to (not including) the \
next bold headword, transcribed exactly as printed, verbatim — do not \
summarize, reorder, or drop anything>\"}. Transcribe every headword on the \
page, even ones you're only partly confident about — getting the \
segmentation boundaries right matters more than getting every character \
perfect.";

pub fn segment_page(engine: &mut VisionEngine, image_path: &Path) -> Result<Vec<Entry>, String> {
    let ext = image_path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase)
        .unwrap_or_default();
    if !SUPPORTED_EXTENSIONS.contains(&ext.as_str()) {
        return Err(format!(
            "unrecognized image type for {} (expected .jpg/.jpeg/.png/.webp)",
            image_path.display()
        ));
    }

    let schema = segment_schema();
    let text = engine.generate(SEGMENT_PROMPT, image_path, Some(&schema))?;
    let context = image_path.display().to_string();
    let items = extract_json_array(&text, &context)?;
    Ok(items
        .into_iter()
        .filter_map(|v| serde_json::from_value::<Entry>(v).ok())
        .filter(|e| !e.headword.is_empty() && !e.raw_entry.is_empty())
        .collect())
}

pub fn run_segment(
    model_path: &Path,
    mmproj_path: &Path,
    entries_out_file: &Path,
    image_paths: &[PathBuf],
) -> Result<(), String> {
    println!("loading segment model from {} ...", model_path.display());
    let mut engine = VisionEngine::load(model_path, mmproj_path)?;

    let mut entries = Vec::new();
    for image_path in image_paths {
        println!("reading {} ...", image_path.display());
        let found = segment_page(&mut engine, image_path)?;
        println!("  found {} headword(s)", found.len());
        entries.extend(found);
    }
    if let Some(parent) = entries_out_file.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(&entries).map_err(|e| e.to_string())?;
    std::fs::write(entries_out_file, json).map_err(|e| e.to_string())?;
    println!(
        "wrote {} entries to {}",
        entries.len(),
        entries_out_file.display()
    );
    Ok(())
}
