//! Pass 2 — turns one already-correctly-read dictionary entry into one or
//! more flashcards. Two sub-calls per entry, not one: first *enumerate*
//! the entry's distinct learnable units (senses, sub-senses, worked
//! examples, derived words) as short labels, then *fill in* each one's
//! full card fields with its own small call. Splitting it this way —
//! rather than asking for a whole JSON array of multi-field cards in one
//! generation — is deliberate: that one-big-array shape is what produced
//! inconsistent output (hallucinated derived words, phrase-shattering,
//! self-referential cards) when first tried.
use crate::engine::TextEngine;
use crate::json_extract::{extract_json_array, extract_json_object};
use crate::schema::{enumerate_schema, fill_schema};
use serde_json::Value;
use std::collections::HashSet;
use std::path::Path;

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Card {
    pub english: String,
    pub target_text: String,
    #[serde(default)]
    pub part_of_speech: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub example_target: String,
    #[serde(default)]
    pub example_gloss: String,
    #[serde(default)]
    pub image_query: String,
    /// Filled in by the `create` stage after a Pixabay lookup — absent
    /// from the model's own output.
    #[serde(default)]
    pub image_url: Option<String>,
}

/// Returns a short, human-readable reason to reject a drafted card, or
/// `None` if it should be kept. Not fully pure — a kept card's key is
/// recorded into `seen_card_keys` so a later duplicate of it is caught too
/// — but synchronous and needs no model/network call, so it's unit-tested
/// on its own rather than only indirectly through a full decompose run.
/// Strips cosmetic differences that don't change a card's real identity
/// — a leading/trailing "-" marking a Xhosa bound stem, and parentheses
/// around a qualifying word — before two cards are compared. Confirmed
/// necessary in a real run against the production model:
/// `"ablaze with"`/`"khazimla"` slipped past the duplicate check against
/// an already-created `"ablaze (with)"`/`"-khazimla"` card, and
/// `"the nqa"`/`"-the nqa"` slipped past the self-referential check,
/// both differing only by this kind of formatting, not by content.
fn normalize_for_comparison(s: &str) -> String {
    s.chars()
        .filter(|c| *c != '(' && *c != ')')
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_matches('-')
        .to_lowercase()
}

pub fn card_rejection_reason(
    card: &Card,
    seen_card_keys: &mut HashSet<String>,
) -> Option<&'static str> {
    let english = card.english.trim();
    let target_text = card.target_text.trim();
    if english.is_empty() || target_text.is_empty() {
        return Some("empty or self-referential");
    }
    if english.contains('~') || target_text.contains('~') {
        return Some("unreconstructed ~ left in output");
    }
    let normalized_english = normalize_for_comparison(english);
    let normalized_target = normalize_for_comparison(target_text);
    if normalized_english == normalized_target {
        return Some("empty or self-referential");
    }
    // Keyed on the pair actually shown to the learner (english +
    // target_text), not the enumerated item's label — two different
    // enumerate labels (e.g. "ablaze" and "ablaze (excited)") can still
    // land on the identical card when the entry text overlaps, and that's
    // the duplicate worth catching.
    let key = format!("{normalized_english}|{normalized_target}");
    if seen_card_keys.contains(&key) {
        return Some("duplicate of an already-created card");
    }
    seen_card_keys.insert(key);
    None
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct Item {
    pub label: String,
    pub source_excerpt: String,
}

/// Pass 2a — enumerate one already-correctly-read entry's distinct
/// learnable units (not their fields yet, just what they are).
pub fn enumerate_items(
    engine: &TextEngine,
    headword: &str,
    raw_entry: &str,
) -> Result<Vec<Item>, String> {
    let prompt = format!(
        "Below is one raw, correctly-transcribed dictionary entry (it may cover \
several related words). List every distinct learnable unit in it as a \
short item — you are NOT filling in translations yet, just identifying \
what the separate flashcards should be. That means:\n\
- The headword's own general sense(s), each as one item.\n\
- Each distinct sense given in parentheses (e.g. \"(skilful)\", \"(of \
conditions)\"), each as one item.\n\
- Each worked example (\"eg. ...\") that is itself a useful standalone \
phrase, as one item.\n\
- Each derived word mentioned (spelled out in full, or abbreviated with a \
~ standing in for the headword — reconstruct its full spelling, never \
leave a ~ in your output), each as one item.\n\n\
Do NOT invent an item for a derived word that isn't actually written in \
the entry below.\n\n\
For EACH item, output an object with \"label\" (a short description of \
what this card teaches, e.g. \"abandon (verb)\", \"abandoned (adj, given up \
to bad ways)\", \"ability (noun)\") and \"source_excerpt\" (the exact \
substring of the entry below that this item comes from, copied \
verbatim).\n\n\
Entry (headword: \"{headword}\"):\n{raw_entry}"
    );
    let schema = enumerate_schema();
    let text = engine.generate(&prompt, Some(&schema))?;
    let context = format!("{headword} (enumerate)");
    let items = extract_json_array(&text, &context)?;
    Ok(items
        .into_iter()
        .filter_map(|v| serde_json::from_value::<Item>(v).ok())
        .filter(|i| !i.label.is_empty() && !i.source_excerpt.is_empty())
        .collect())
}

/// Pass 2b — fill in one previously-enumerated item's full card fields.
pub fn fill_item(
    engine: &TextEngine,
    headword: &str,
    raw_entry: &str,
    item: &Item,
) -> Result<Card, String> {
    let prompt = format!(
        "Below is one raw, correctly-transcribed dictionary entry, and ONE \
specific item within it that you previously identified as its own \
flashcard. Fill in that one card's fields — do not produce cards for \
any other part of the entry.\n\n\
Output an object with: \"english\" (a short, clean English label for \
exactly what this card teaches — just the word or phrase, no part-of-\
speech abbreviation baked in), \"target_text\" (the exact target-language \
text for just this card — never paste in content for a different sense/\
word), \"part_of_speech\" (a short abbreviation ONLY, e.g. \"n.\", \"v.\", \
\"adj.\", \"adv.\" — never a definition, an example, or a full sentence; if \
truly unknown use \"\"), \"notes\" (brief extra context if useful, else \
\"\"), \"example_target\" and \"example_gloss\" (if this card itself is \
built from a worked example, else both \"\"), and \"image_query\" — this \
field is REQUIRED and must never be empty: a short, concrete, \
literally-photographable English phrase illustrating this specific \
card's meaning — for an abstract word, describe a scene that conveys it, \
e.g. \"aback\" -> \"person looking shocked and surprised\", not the word \
itself. Always write a real image_query, even for a derived word or a \
grammatical form — picture what the word itself describes.\n\n\
Keep every field to just its own content — do not restate the entry, \
explain your reasoning, or put one field's content (an example, a note, \
an image idea) inside a different field.\n\n\
target_text must never be identical to english — if you can't find an \
actual translation, leave target_text as \"\" rather than echoing English \
or an unrelated fragment back as both fields.\n\n\
Full entry (headword: \"{headword}\"):\n{raw_entry}\n\n\
The one item to fill in:\nLabel: {}\nExcerpt: {}",
        item.label, item.source_excerpt
    );
    let schema = fill_schema();
    let text = engine.generate(&prompt, Some(&schema))?;
    let context = format!("{headword} / {}", item.label);
    let obj = extract_json_object(&text, &context)?;
    serde_json::from_value(Value::Object(obj)).map_err(|e| e.to_string())
}

pub fn run_decompose(
    model_path: &Path,
    entries_in_file: &Path,
    cards_out_file: &Path,
) -> Result<(), String> {
    let raw = std::fs::read_to_string(entries_in_file).map_err(|e| e.to_string())?;
    let entries: Vec<crate::segment::Entry> =
        serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    let mut cards = Vec::new();
    let mut seen_card_keys = HashSet::new();
    if !entries.is_empty() {
        println!("loading decompose model from {} ...", model_path.display());
        let engine = TextEngine::load(model_path)?;
        println!("splitting {} entries into cards...", entries.len());
        for entry in &entries {
            match enumerate_items(&engine, &entry.headword, &entry.raw_entry) {
                Ok(items) => {
                    println!("  {}: {} item(s)", entry.headword, items.len());
                    for item in &items {
                        match fill_item(&engine, &entry.headword, &entry.raw_entry, item) {
                            Ok(card) => match card_rejection_reason(&card, &mut seen_card_keys) {
                                Some(reason) => {
                                    println!("    \"{}\": skipped ({reason})", item.label);
                                }
                                None => cards.push(card),
                            },
                            Err(e) => println!("    \"{}\": skipped ({e})", item.label),
                        }
                    }
                }
                Err(e) => println!("  {}: skipped ({e})", entry.headword),
            }
        }
    }
    if let Some(parent) = cards_out_file.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(&cards).map_err(|e| e.to_string())?;
    std::fs::write(cards_out_file, json).map_err(|e| e.to_string())?;
    println!(
        "wrote {} card(s) to {}",
        cards.len(),
        cards_out_file.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(english: &str, target_text: &str) -> Card {
        Card {
            english: english.to_string(),
            target_text: target_text.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn rejects_an_empty_field() {
        let mut seen = HashSet::new();
        assert_eq!(
            card_rejection_reason(&card("", "x"), &mut seen),
            Some("empty or self-referential")
        );
        assert_eq!(
            card_rejection_reason(&card("x", ""), &mut seen),
            Some("empty or self-referential")
        );
    }

    #[test]
    fn rejects_english_eq_target_text_case_insensitively() {
        let mut seen = HashSet::new();
        assert_eq!(
            card_rejection_reason(&card("Abandon", "abandon"), &mut seen),
            Some("empty or self-referential")
        );
    }

    #[test]
    fn rejects_a_leftover_tilde_in_either_field() {
        let mut seen = HashSet::new();
        assert_eq!(
            card_rejection_reason(&card("abhorrent", "~·rent"), &mut seen),
            Some("unreconstructed ~ left in output")
        );
        assert_eq!(
            card_rejection_reason(&card("~'d", "real translation"), &mut seen),
            Some("unreconstructed ~ left in output")
        );
    }

    #[test]
    fn accepts_a_good_card_and_records_it() {
        let mut seen = HashSet::new();
        assert_eq!(
            card_rejection_reason(&card("abandon", "yeka"), &mut seen),
            None
        );
        assert_eq!(seen.len(), 1);
    }

    #[test]
    fn rejects_an_exact_repeat_of_an_already_accepted_card() {
        let mut seen = HashSet::new();
        assert_eq!(
            card_rejection_reason(&card("ablaze", "-vutha"), &mut seen),
            None
        );
        assert_eq!(
            card_rejection_reason(&card("ablaze", "-vutha"), &mut seen),
            Some("duplicate of an already-created card")
        );
    }

    #[test]
    fn treats_the_duplicate_check_case_insensitively() {
        let mut seen = HashSet::new();
        assert_eq!(
            card_rejection_reason(&card("Ablaze", "-vutha"), &mut seen),
            None
        );
        assert_eq!(
            card_rejection_reason(&card("ablaze", "-VUTHA"), &mut seen),
            Some("duplicate of an already-created card")
        );
    }

    #[test]
    fn rejects_self_reference_hidden_behind_a_leading_hyphen() {
        let mut seen = HashSet::new();
        assert_eq!(
            card_rejection_reason(&card("the nqa", "-the nqa"), &mut seen),
            Some("empty or self-referential")
        );
    }

    #[test]
    fn rejects_a_duplicate_hidden_behind_parens_and_a_leading_hyphen() {
        let mut seen = HashSet::new();
        assert_eq!(
            card_rejection_reason(&card("ablaze (with)", "-khazimla"), &mut seen),
            None
        );
        assert_eq!(
            card_rejection_reason(&card("ablaze with", "khazimla"), &mut seen),
            Some("duplicate of an already-created card")
        );
    }
}
