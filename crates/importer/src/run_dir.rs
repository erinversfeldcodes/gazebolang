//! Names the per-run directory a pipeline run's intermediate `entries.json`/
//! `cards.json` live in, from the first input image's own basename plus a
//! timestamp — resolved relative to the current working directory by the
//! caller, not the binary's own path (unlike the original script, which
//! resolved it relative to the *script file's* directory — that stopped
//! making sense once "the script" is a binary that could be sitting
//! anywhere).
use std::path::Path;

pub fn run_dir_name(first_image_path: &str) -> String {
    let file_name = Path::new(first_image_path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let base = strip_extension(&file_name);
    let sanitized: String = base
        .chars()
        .map(|c| if is_safe_char(c) { c } else { '-' })
        .collect();
    format!("{sanitized}-{}", timestamp())
}

fn is_safe_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

fn strip_extension(name: &str) -> &str {
    match name.rfind('.') {
        Some(i) if i + 1 < name.len() => &name[..i],
        _ => name,
    }
}

fn timestamp() -> String {
    let iso = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();
    iso.chars()
        .map(|c| if c == ':' || c == '.' { '-' } else { c })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_the_image_basename_and_appends_a_timestamp() {
        let name = run_dir_name("/some/path/521e49a4-image.JPG");
        assert!(name.starts_with("521e49a4-image-"), "{name}");
        let stamp = &name["521e49a4-image-".len()..];
        let bytes = stamp.as_bytes();
        assert!(bytes.len() >= 11, "{stamp}");
        assert!(bytes[..4].iter().all(u8::is_ascii_digit), "{stamp}");
        assert_eq!(bytes[4], b'-');
        assert!(bytes[5..7].iter().all(u8::is_ascii_digit), "{stamp}");
        assert_eq!(bytes[7], b'-');
        assert!(bytes[8..10].iter().all(u8::is_ascii_digit), "{stamp}");
        assert_eq!(bytes[10], b'T');
    }

    #[test]
    fn strips_characters_a_directory_name_cant_safely_contain() {
        let name = run_dir_name("my photo (1)!.png");
        assert!(!name.chars().any(|c| "()! ".contains(c)), "{name}");
    }
}
