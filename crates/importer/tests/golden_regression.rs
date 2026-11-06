//! Golden-set regression test against real photographed dictionary
//! pages — `#[ignore]`d by default and opt-in, since it needs the real
//! segment model on disk plus a local checkout of a private fixtures
//! repo (ask about `IMPORTER_GOLDEN_FIXTURES_DIR` if you don't already
//! know where that lives) and takes a few minutes to run real model
//! inference. Run explicitly with:
//!
//! ```sh
//! cargo test -p importer --test golden_regression -- --ignored --nocapture
//! ```
//!
//! This is a black-box test of the real `importer` binary (this crate
//! has no library target), run against the three real pages that
//! surfaced every segmentation bug fixed this session. It checks
//! structural invariants confirmed from those real failures, not exact
//! text: a real run showed this model's output isn't perfectly
//! reproducible across different builds or model versions over time
//! (two separate development sessions' runs against the identical page
//! disagreed on a mislabeled headword — "kanga" vs. "accelerate" — for
//! the same underlying bug), so an exact-match golden file would be
//! too fragile to be useful here, even though repeated runs *within*
//! one build are confirmed bit-for-bit deterministic.

use std::path::PathBuf;
use std::process::Command;

struct Entry {
    headword: String,
    raw_entry: String,
}

/// Returns `None` (meaning: skip this test) unless every environment
/// variable a real run needs is set. Missing any of them isn't a
/// failure — it just means this opt-in test wasn't configured to run
/// here, same as being `#[ignore]`d in the first place.
fn required_env() -> Option<(PathBuf, PathBuf, PathBuf)> {
    let fixtures_dir = std::env::var_os("IMPORTER_GOLDEN_FIXTURES_DIR").map(PathBuf::from)?;
    let model_path = std::env::var_os("IMPORTER_SEGMENT_MODEL_PATH").map(PathBuf::from)?;
    let mmproj_path = std::env::var_os("IMPORTER_SEGMENT_MMPROJ_PATH").map(PathBuf::from)?;
    Some((fixtures_dir, model_path, mmproj_path))
}

fn load_entries(path: &std::path::Path) -> Vec<Entry> {
    let raw = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
    let values: Vec<serde_json::Value> = serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("failed to parse {}: {e}", path.display()));
    values
        .into_iter()
        .map(|v| Entry {
            headword: v["headword"].as_str().unwrap_or_default().to_string(),
            raw_entry: v["raw_entry"].as_str().unwrap_or_default().to_string(),
        })
        .collect()
}

fn count_headword(entries: &[Entry], headword: &str) -> usize {
    entries
        .iter()
        .filter(|e| e.headword.eq_ignore_ascii_case(headword))
        .count()
}

#[test]
#[ignore = "needs IMPORTER_GOLDEN_FIXTURES_DIR, IMPORTER_SEGMENT_MODEL_PATH, IMPORTER_SEGMENT_MMPROJ_PATH and takes a few minutes of real model inference — run with `cargo test -p importer --test golden_regression -- --ignored`"]
fn real_three_page_segmentation_matches_known_structural_invariants() {
    let Some((fixtures_dir, model_path, mmproj_path)) = required_env() else {
        eprintln!(
            "skipping golden_regression: set IMPORTER_GOLDEN_FIXTURES_DIR, \
             IMPORTER_SEGMENT_MODEL_PATH and IMPORTER_SEGMENT_MMPROJ_PATH to run it"
        );
        return;
    };

    let page1 = fixtures_dir.join("page-1-abet-aboard.jpg");
    let page2 = fixtures_dir.join("page-2-abode-abrupt.jpg");
    let page3 = fixtures_dir.join("page-3-abscess-accelerate.jpg");
    for page in [&page1, &page2, &page3] {
        assert!(
            page.is_file(),
            "expected fixture photo at {} — is IMPORTER_GOLDEN_FIXTURES_DIR pointing \
             at the fixtures repo's dictionary-pages/ directory?",
            page.display()
        );
    }

    let out_file = std::env::temp_dir().join(format!(
        "importer-golden-regression-{}.json",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&out_file);

    let status = Command::new(env!("CARGO_BIN_EXE_importer"))
        .env("IMPORTER_SEGMENT_MODEL_PATH", &model_path)
        .env("IMPORTER_SEGMENT_MMPROJ_PATH", &mmproj_path)
        .arg("segment")
        .arg(&out_file)
        .arg(&page1)
        .arg(&page2)
        .arg(&page3)
        .status()
        .expect("failed to run the importer binary");
    assert!(status.success(), "importer segment exited with {status}");

    let entries = load_entries(&out_file);
    let _ = std::fs::remove_file(&out_file);

    // A catastrophic failure (model crash, near-empty output) shows up
    // as a wildly low count; this is a loose sanity bound, not an
    // exact expectation — legitimate entry counts have varied in the
    // 66-70 range across every real run so far this session.
    assert!(
        entries.len() >= 50,
        "only {} entries found — expected at least 50 across these 3 pages",
        entries.len()
    );

    // Real bug: a dangling sense continuation of "abet" got split off
    // into its own wrongly-headworded entry under "aboard" instead of
    // being recognized as the rest of "abet"'s own second sense.
    assert_eq!(
        count_headword(&entries, "abet"),
        1,
        "\"abet\" should appear exactly once, merged — found {} (entries: {:?})",
        count_headword(&entries, "abet"),
        entries.iter().map(|e| &e.headword).collect::<Vec<_>>()
    );

    // Real bug: the column-boundary crop starting mid-sentence with no
    // bold heading to anchor on invented a pseudo-headword from body
    // text it saw first.
    for bogus in ["apha", "ezininzi"] {
        assert_eq!(
            count_headword(&entries, bogus),
            0,
            "\"{bogus}\" shouldn't appear as a headword at all — it's a hallucinated \
             pseudo-headword from body text, not a real entry"
        );
    }

    // Real bug: "about" (which legitimately repeats 3 times on this
    // page for distinct senses) and "above" (twice) got swallowed by
    // an earlier, naive version of the boundary fix.
    assert_eq!(
        count_headword(&entries, "about"),
        3,
        "\"about\" should appear 3 times"
    );
    assert_eq!(
        count_headword(&entries, "above"),
        2,
        "\"above\" should appear twice"
    );

    // Real bug: the page's own read cut "absurd" off mid-word with no
    // terminal punctuation, then invented a new headword for the
    // leftover text — observed as "kanga" in one run, "accelerate" (a
    // real headword that also exists correctly, later, on this same
    // page) in another. Either shape shows up as a duplicate.
    assert_eq!(
        count_headword(&entries, "accelerate"),
        1,
        "\"accelerate\" should appear exactly once — a second occurrence is \
         \"absurd\"'s own truncated continuation, mislabeled"
    );

    // Nothing should come through as a totally empty husk.
    for e in &entries {
        assert!(
            !e.raw_entry.trim().is_empty(),
            "entry {:?} has an empty raw_entry",
            e.headword
        );
    }
}
