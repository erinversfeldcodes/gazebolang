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
use std::collections::HashMap;
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

/// A bare part-of-speech tag in parentheses (e.g. `"abhor (verb)"`) adds
/// no sense-distinguishing information — the card already has its own
/// `part_of_speech` field — so it's dropped entirely before comparison,
/// unlike a real qualifying parenthetical (`"ablaze (excited)"`,
/// `"ablaze (with)"`) which genuinely marks a distinct sense and must be
/// kept. Confirmed necessary in a real run: `"abhor (verb)"`/`"thiya"`
/// slipped past the duplicate check against an already-created
/// `"abhor"`/`"-thiya"` card, differing only by this redundant tag.
const POS_TAG_WORDS: &[&str] = &[
    "n",
    "v",
    "vt",
    "vi",
    "adj",
    "adjective",
    "adv",
    "adverb",
    "noun",
    "verb",
    "prep",
    "preposition",
    "pron",
    "pronoun",
    "conj",
    "conjunction",
    "interj",
    "interjection",
];

fn strip_redundant_pos_parenthetical(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut depth = 0u32;
    let mut paren_content = String::new();
    for c in s.chars() {
        match c {
            '(' => {
                depth += 1;
                paren_content.clear();
            }
            ')' if depth > 0 => {
                depth -= 1;
                let tag = paren_content.trim().trim_end_matches('.').to_lowercase();
                if !POS_TAG_WORDS.contains(&tag.as_str()) {
                    result.push('(');
                    result.push_str(&paren_content);
                    result.push(')');
                }
            }
            _ if depth > 0 => paren_content.push(c),
            _ => result.push(c),
        }
    }
    result
}

/// A handful of common English function words that are never themselves a
/// real, standalone flashcard concept in this pipeline — every real card's
/// `english` is a dictionary headword, sense, or phrase, never a bare
/// preposition/article/conjunction on its own. Confirmed necessary in a
/// real run: enumerate split `"ablaze (with)"` into two items, one
/// correctly labeled and one reduced to just `"with"`, which fill then
/// happily gave a real-looking card — a duplicate in substance, but not
/// in form, since "with" naturally doesn't equal "ablaze with".
const TRIVIAL_FRAGMENT_LABELS: &[&str] = &[
    "with", "the", "a", "an", "of", "in", "on", "by", "to", "and", "or", "but", "for", "at",
];

/// Returns a short, human-readable reason to reject a drafted card, or
/// `None` if it should be kept. Not fully pure — a kept card's key is
/// recorded into `seen_card_keys` so a later duplicate of it is caught too
/// — but synchronous and needs no model/network call, so it's unit-tested
/// on its own rather than only indirectly through a full decompose run.
/// Strips cosmetic differences that don't change a card's real identity
/// — a leading/trailing "-" marking a Xhosa bound stem, a redundant
/// part-of-speech tag, and parentheses around a qualifying word — before
/// two cards are compared. Confirmed necessary in a real run against the
/// production model: `"ablaze with"`/`"khazimla"` slipped past the
/// duplicate check against an already-created `"ablaze (with)"`/
/// `"-khazimla"` card, and `"the nqa"`/`"-the nqa"` slipped past the
/// self-referential check, both differing only by this kind of
/// formatting, not by content.
fn normalize_for_comparison(s: &str) -> String {
    // Trimming hyphens can expose a space that was on the far side of
    // one (e.g. "- 0" -> " 0") — a pre-existing bug a real proptest
    // run caught: without the final `.trim()`, that leftover space
    // survives the first pass but gets cleaned up as a matter of
    // course by `split_whitespace` on any *second* pass, breaking
    // idempotence. The plain `.trim()` here closes that gap.
    strip_redundant_pos_parenthetical(s)
        .chars()
        .filter(|c| *c != '(' && *c != ')')
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_matches('-')
        .trim()
        .to_lowercase()
}

/// Splits `english` into its base word/phrase and a trailing qualifying
/// parenthetical, if it has one worth keeping separately — e.g. `"abate
/// (of wind, storm, pain)"` -> (`"abate"`, `Some("of wind, storm,
/// pain)")`. A parenthetical that's just a redundant part-of-speech tag
/// (already handled by `strip_redundant_pos_parenthetical`) doesn't
/// count: the whole original string is returned unsplit, with no
/// qualifier, so it's still correctly recognized as a duplicate via
/// `normalize_for_comparison`'s own POS-tag stripping, without a
/// meaningless "verb"/"noun" ending up in a card's notes.
fn split_trailing_parenthetical(english: &str) -> (&str, Option<&str>) {
    let trimmed = english.trim_end();
    let Some(before_close) = trimmed.strip_suffix(')') else {
        return (trimmed, None);
    };
    let Some(open) = before_close.rfind('(') else {
        return (trimmed, None);
    };
    let base = trimmed[..open].trim_end();
    let content = before_close[open + 1..].trim();
    let is_pos_tag = POS_TAG_WORDS.contains(&content.trim_end_matches('.').to_lowercase().as_str());
    if base.is_empty() || content.is_empty() || is_pos_tag {
        return (trimmed, None);
    }
    (base, Some(content))
}

/// Folds `qualifier` into `card`'s own dictionary-details notes, unless
/// an equivalent note is already there — used both when a brand-new
/// card's own qualifying parenthetical is moved out of `english` and
/// into notes, and when a later near-duplicate's differently-worded
/// qualifier (e.g. "(floods)" matching an already-kept "(of floods)")
/// is folded into the card it duplicates, rather than discarded with
/// the rest of that duplicate.
fn merge_qualifier_into_notes(card: &mut Card, qualifier: &str) {
    let qualifier = qualifier.trim();
    if qualifier.is_empty() {
        return;
    }
    let already_present = card
        .notes
        .split(';')
        .any(|existing| existing.trim().eq_ignore_ascii_case(qualifier));
    if already_present {
        return;
    }
    if card.notes.trim().is_empty() {
        card.notes = qualifier.to_string();
    } else {
        card.notes = format!("{}; {qualifier}", card.notes.trim());
    }
}

/// `card` is mutated in place when kept: a qualifying parenthetical (see
/// `split_trailing_parenthetical`) is moved out of `english` and into
/// `notes`, whether this is the first card to carry that qualifier or a
/// later near-duplicate's differently-worded one being folded into an
/// already-kept card instead (`cards[existing_idx]` in that branch).
/// `seen_card_keys` maps a dedup key to that card's index in `cards` —
/// looked up under *two* keys so either kind of "same card" is caught:
/// the existing full-string key (post `normalize_for_comparison`, which
/// already merges a case like "ablaze with" into "ablaze (with)" by
/// dropping the parens' own characters) and a *base-word-only* key
/// (post `split_trailing_parenthetical`, which additionally merges
/// "abate" and "abate (of wind, storm, pain)" — confirmed necessary in a
/// real run: these are the same card, with a qualifier that's a
/// usage-context note rather than a distinct sense, and keeping both
/// as separate cards is pure redundancy, not two different things to
/// learn).
pub fn card_rejection_reason(
    card: &mut Card,
    cards: &mut [Card],
    seen_card_keys: &mut HashMap<String, usize>,
) -> Option<&'static str> {
    let english = card.english.trim().to_string();
    let target_text = card.target_text.trim().to_string();
    if english.is_empty() || target_text.is_empty() {
        return Some("empty or self-referential");
    }
    if english.contains('~') || target_text.contains('~') {
        return Some("unreconstructed ~ left in output");
    }
    let normalized_full = normalize_for_comparison(&english);
    let normalized_target = normalize_for_comparison(&target_text);
    if normalized_full == normalized_target {
        return Some("empty or self-referential");
    }
    if TRIVIAL_FRAGMENT_LABELS.contains(&normalized_full.as_str()) {
        return Some("english is just a stray function word, not a real card");
    }

    let (base_english, qualifier) = split_trailing_parenthetical(&english);
    let normalized_base = normalize_for_comparison(base_english);

    // Keyed on the pair actually shown to the learner (english +
    // target_text), not the enumerated item's label — two different
    // enumerate labels can still land on the identical card when the
    // entry text overlaps, and that's the duplicate worth catching.
    let full_key = format!("{normalized_full}|{normalized_target}");
    let base_key = format!("{normalized_base}|{normalized_target}");

    let existing_idx = seen_card_keys
        .get(&full_key)
        .or_else(|| seen_card_keys.get(&base_key))
        .copied();
    if let Some(existing_idx) = existing_idx {
        if let Some(q) = qualifier {
            merge_qualifier_into_notes(&mut cards[existing_idx], q);
        }
        return Some("duplicate of an already-created card");
    }

    if let Some(q) = qualifier {
        card.english = base_english.to_string();
        merge_qualifier_into_notes(card, q);
    }
    seen_card_keys.insert(full_key, cards.len());
    seen_card_keys.insert(base_key, cards.len());
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
the entry below. And when a sense gives more than one alternative \
target-language translation (e.g. several words separated by commas), \
that's still only ONE item — put every alternative in that one item's \
own excerpt, never split any one of them off into a separate item of \
its own.\n\n\
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

/// A real run showed this step occasionally promoting one bare
/// alternative target-language translation, among several listed for a
/// single sense (e.g. "krwada; ngequbuliso, ngebhaqo", three
/// alternative Xhosa translations of "abruptly"), into a fake item of
/// its own — labeled "ngequbuliso (noun)", the Xhosa word copied
/// straight into English position, as if it were a distinct derived
/// word. A genuine label is always reconstructed English prose, which
/// essentially never collides letter-for-letter with unrelated
/// target-language text, so a label that — once its own trailing
/// qualifier like "(noun)" is set aside — turns out to just BE a chunk
/// of its own `source_excerpt` is a reliable sign of exactly this
/// failure.
///
/// This used to be grounds to drop the item outright at enumeration —
/// reverted after a later real run showed that was far too blunt: it's
/// *also* the normal shape of a perfectly good item whenever the
/// dictionary happens to spell a derived word out in full right in its
/// own excerpt ("accelerator, n. …"), or whenever an item's only real
/// content *is* an untranslated worked example the model labeled
/// lazily instead of reconstructing ("he goes, ~ a ship: ukhwela
/// enqanaweni" for "aboard") — both real cases where dropping the item
/// threw away the entry's only genuinely learnable content for that
/// sense. This is now just a "suspect" signal, checked at the fill
/// stage (see `run_decompose`) against the one thing that actually
/// distinguishes the original bug — the same translation getting
/// reused for a second, different sense — rather than against the
/// label's wording alone.
fn label_is_not_copied_from_its_own_excerpt(item: &Item) -> bool {
    let (base_label, _) = split_trailing_parenthetical(&item.label);
    let normalized_label = normalize_for_comparison(base_label);
    // Too short a label risks coincidental overlap with unrelated
    // excerpt text; the real case this guards against is a whole word,
    // not a short fragment, so there's no real precision cost in only
    // checking labels long enough for the overlap to be meaningful.
    normalized_label.chars().count() < 4
        || !normalize_for_comparison(&item.source_excerpt).contains(&normalized_label)
}

/// The one thing that actually distinguishes the real bug
/// `label_is_not_copied_from_its_own_excerpt` flags as suspect from the
/// far more common legitimate case that signal alone can't tell apart
/// from it: a dictionary entry reusing the identical translation for
/// two genuinely *different* senses, by coincidence, within its own
/// few lines is essentially never real — confirmed real case: the
/// bogus "abruptness" card reused "ngequbuliso" from the entry's own
/// already-accepted "abruptly" card. `cards_this_entry` must only be
/// cards already accepted from the *same* entry — a translation
/// legitimately recurring across two unrelated entries elsewhere in
/// the dictionary is not this bug.
fn target_text_reused_from_a_different_sense(card: &Card, cards_this_entry: &[Card]) -> bool {
    let target = normalize_for_comparison(&card.target_text);
    let english = normalize_for_comparison(&card.english);
    !target.is_empty()
        && cards_this_entry.iter().any(|existing| {
            normalize_for_comparison(&existing.target_text) == target
                && normalize_for_comparison(&existing.english) != english
        })
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
speech abbreviation baked in. If, and only if, this item's own label \
already reconstructed a DIFFERENT ENGLISH word derived from the headword \
by a suffix (e.g. the entry wrote \"~ment\" and the label spelled out \
\"abatement\"), \"english\" must be that reconstructed English word, not \
the bare headword — but if this item is simply an alternate \
target-language translation, a worked example, or any other sense of \
the headword itself, \"english\" stays the headword/phrase as normal; \
never use target-language text as \"english\"), \"target_text\" \
(the exact target-language \
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

/// A card or whole entry that didn't make it into the final output,
/// kept around for a human to glance at rather than only a line in a
/// console that's long since scrolled past by the time anyone looks —
/// every skip reason already printed during a run was, before this,
/// otherwise unrecoverable the moment the process exited.
#[derive(serde::Serialize)]
struct SkippedItem {
    headword: String,
    label: String,
    english: Option<String>,
    target_text: Option<String>,
    reason: String,
}

/// The sidecar path a run's skipped items (if any) are written to —
/// see `segment::needs_review_path`, which this mirrors; kept as its
/// own small copy rather than a shared helper, since the two run on
/// different data shapes (entries vs. cards) and the path logic itself
/// is only a couple of lines.
fn needs_review_path(cards_out_file: &Path) -> std::path::PathBuf {
    let stem = cards_out_file
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("cards");
    let dir = cards_out_file.parent().unwrap_or_else(|| Path::new("."));
    dir.join(format!("{stem}.needs-review.json"))
}

/// Fills in and accepts (or rejects, with a reason) one previously
/// enumerated item — the per-item body of `run_decompose`'s own loop,
/// pulled out only to stay under clippy's line-count lint.
/// `entry_start_idx` is where `cards` stood before this entry's own
/// items started being processed, so `target_text_reused_from_a_different_sense`
/// only ever compares against cards from the *same* entry.
fn process_item(
    engine: &TextEngine,
    entry: &crate::segment::Entry,
    item: &Item,
    entry_start_idx: usize,
    cards: &mut Vec<Card>,
    seen_card_keys: &mut HashMap<String, usize>,
    skipped: &mut Vec<SkippedItem>,
) {
    match fill_item(engine, &entry.headword, &entry.raw_entry, item) {
        Ok(mut card) => {
            if !label_is_not_copied_from_its_own_excerpt(item)
                && target_text_reused_from_a_different_sense(&card, &cards[entry_start_idx..])
            {
                println!(
                    "    \"{}\": skipped (translation reused from a different sense in the same entry)",
                    item.label
                );
                skipped.push(SkippedItem {
                    headword: entry.headword.clone(),
                    label: item.label.clone(),
                    english: Some(card.english),
                    target_text: Some(card.target_text),
                    reason: "translation reused from a different sense in the same entry"
                        .to_string(),
                });
                return;
            }
            match card_rejection_reason(&mut card, cards, seen_card_keys) {
                Some(reason) => {
                    println!("    \"{}\": skipped ({reason})", item.label);
                    skipped.push(SkippedItem {
                        headword: entry.headword.clone(),
                        label: item.label.clone(),
                        english: Some(card.english),
                        target_text: Some(card.target_text),
                        reason: reason.to_string(),
                    });
                }
                None => cards.push(card),
            }
        }
        Err(e) => {
            println!("    \"{}\": skipped ({e})", item.label);
            skipped.push(SkippedItem {
                headword: entry.headword.clone(),
                label: item.label.clone(),
                english: None,
                target_text: None,
                reason: e,
            });
        }
    }
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
    let mut seen_card_keys = HashMap::new();
    let mut skipped = Vec::new();
    if !entries.is_empty() {
        println!("loading decompose model from {} ...", model_path.display());
        let engine = TextEngine::load(model_path)?;
        println!("splitting {} entries into cards...", entries.len());
        for entry in &entries {
            let entry_start_idx = cards.len();
            match enumerate_items(&engine, &entry.headword, &entry.raw_entry) {
                Ok(items) => {
                    println!("  {}: {} item(s)", entry.headword, items.len());
                    for item in &items {
                        process_item(
                            &engine,
                            entry,
                            item,
                            entry_start_idx,
                            &mut cards,
                            &mut seen_card_keys,
                            &mut skipped,
                        );
                    }
                }
                Err(e) => {
                    println!("  {}: skipped ({e})", entry.headword);
                    skipped.push(SkippedItem {
                        headword: entry.headword.clone(),
                        label: String::new(),
                        english: None,
                        target_text: None,
                        reason: e,
                    });
                }
            }
        }
    }
    if let Some(parent) = cards_out_file.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    if !skipped.is_empty() {
        let review_path = needs_review_path(cards_out_file);
        let review_json = serde_json::to_string_pretty(&skipped).map_err(|e| e.to_string())?;
        std::fs::write(&review_path, review_json).map_err(|e| e.to_string())?;
        println!(
            "wrote {} skipped item{} needing review to {}",
            skipped.len(),
            if skipped.len() == 1 { "" } else { "s" },
            review_path.display()
        );
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

    /// Mirrors `run_decompose`'s own usage exactly: builds a card,
    /// checks it, and pushes it onto `cards` only if kept — so a test
    /// can inspect a kept card's own (possibly-mutated) fields
    /// afterward via `cards[..]`, not just the rejection reason.
    fn check(
        cards: &mut Vec<Card>,
        seen: &mut HashMap<String, usize>,
        english: &str,
        target_text: &str,
    ) -> Option<&'static str> {
        let mut c = card(english, target_text);
        let reason = card_rejection_reason(&mut c, cards, seen);
        if reason.is_none() {
            cards.push(c);
        }
        reason
    }

    #[test]
    fn rejects_an_empty_field() {
        let mut cards = Vec::new();
        let mut seen = HashMap::new();
        assert_eq!(
            check(&mut cards, &mut seen, "", "x"),
            Some("empty or self-referential")
        );
        assert_eq!(
            check(&mut cards, &mut seen, "x", ""),
            Some("empty or self-referential")
        );
    }

    #[test]
    fn rejects_english_eq_target_text_case_insensitively() {
        let mut cards = Vec::new();
        let mut seen = HashMap::new();
        assert_eq!(
            check(&mut cards, &mut seen, "Abandon", "abandon"),
            Some("empty or self-referential")
        );
    }

    #[test]
    fn rejects_a_leftover_tilde_in_either_field() {
        let mut cards = Vec::new();
        let mut seen = HashMap::new();
        assert_eq!(
            check(&mut cards, &mut seen, "abhorrent", "~·rent"),
            Some("unreconstructed ~ left in output")
        );
        assert_eq!(
            check(&mut cards, &mut seen, "~'d", "real translation"),
            Some("unreconstructed ~ left in output")
        );
    }

    #[test]
    fn accepts_a_good_card_and_records_it() {
        let mut cards = Vec::new();
        let mut seen = HashMap::new();
        assert_eq!(check(&mut cards, &mut seen, "abandon", "yeka"), None);
        assert_eq!(cards.len(), 1);
    }

    #[test]
    fn rejects_an_exact_repeat_of_an_already_accepted_card() {
        let mut cards = Vec::new();
        let mut seen = HashMap::new();
        assert_eq!(check(&mut cards, &mut seen, "ablaze", "-vutha"), None);
        assert_eq!(
            check(&mut cards, &mut seen, "ablaze", "-vutha"),
            Some("duplicate of an already-created card")
        );
    }

    #[test]
    fn treats_the_duplicate_check_case_insensitively() {
        let mut cards = Vec::new();
        let mut seen = HashMap::new();
        assert_eq!(check(&mut cards, &mut seen, "Ablaze", "-vutha"), None);
        assert_eq!(
            check(&mut cards, &mut seen, "ablaze", "-VUTHA"),
            Some("duplicate of an already-created card")
        );
    }

    #[test]
    fn rejects_self_reference_hidden_behind_a_leading_hyphen() {
        let mut cards = Vec::new();
        let mut seen = HashMap::new();
        assert_eq!(
            check(&mut cards, &mut seen, "the nqa", "-the nqa"),
            Some("empty or self-referential")
        );
    }

    #[test]
    fn rejects_a_duplicate_hidden_behind_parens_and_a_leading_hyphen() {
        let mut cards = Vec::new();
        let mut seen = HashMap::new();
        assert_eq!(
            check(&mut cards, &mut seen, "ablaze (with)", "-khazimla"),
            None
        );
        assert_eq!(
            check(&mut cards, &mut seen, "ablaze with", "khazimla"),
            Some("duplicate of an already-created card")
        );
    }

    #[test]
    fn rejects_a_duplicate_hidden_behind_a_redundant_pos_tag() {
        let mut cards = Vec::new();
        let mut seen = HashMap::new();
        assert_eq!(check(&mut cards, &mut seen, "abhor", "-thiya"), None);
        assert_eq!(
            check(&mut cards, &mut seen, "abhor (verb)", "thiya"),
            Some("duplicate of an already-created card")
        );
    }

    #[test]
    fn a_qualifying_parenthetical_is_treated_as_the_same_card_with_the_qualifier_moved_to_notes() {
        // Confirmed necessary in a real run against the production
        // model (the "abate" entry): a bare sense and a parenthetical-
        // qualified alternate of the *same* word and the *same*
        // translation are the same card, not two different ones — the
        // qualifier is usage-context, not a distinct meaning, so it's
        // preserved in the dictionary-details notes rather than lost.
        let mut cards = Vec::new();
        let mut seen = HashMap::new();
        assert_eq!(check(&mut cards, &mut seen, "ablaze", "-vutha"), None);
        assert_eq!(
            check(&mut cards, &mut seen, "ablaze (excited)", "-vutha"),
            Some("duplicate of an already-created card")
        );
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].english, "ablaze");
        assert_eq!(cards[0].notes, "excited");
    }

    #[test]
    fn the_qualifier_is_moved_to_notes_even_on_a_cards_first_occurrence() {
        let mut cards = Vec::new();
        let mut seen = HashMap::new();
        assert_eq!(
            check(
                &mut cards,
                &mut seen,
                "abate (of wind, storm, pain)",
                "-damba"
            ),
            None
        );
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].english, "abate");
        assert_eq!(cards[0].notes, "of wind, storm, pain");
    }

    #[test]
    fn a_differently_worded_qualifier_on_a_later_duplicate_is_folded_into_the_kept_cards_notes() {
        // The exact pair reported from a real run: "abate (floods)" and
        // "abate (of floods)" are the same card with cosmetically
        // different qualifier wording — both should survive in notes
        // rather than the second one's wording being silently dropped.
        let mut cards = Vec::new();
        let mut seen = HashMap::new();
        assert_eq!(
            check(&mut cards, &mut seen, "abate (of floods)", "wutha"),
            None
        );
        assert_eq!(
            check(&mut cards, &mut seen, "abate (floods)", "wutha"),
            Some("duplicate of an already-created card")
        );
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].english, "abate");
        assert_eq!(cards[0].notes, "of floods; floods");
    }

    #[test]
    fn a_bare_occurrence_after_a_qualified_one_adds_nothing_further_to_notes() {
        let mut cards = Vec::new();
        let mut seen = HashMap::new();
        assert_eq!(
            check(
                &mut cards,
                &mut seen,
                "abate (of wind, storm, pain)",
                "-damba"
            ),
            None
        );
        assert_eq!(
            check(&mut cards, &mut seen, "abate", "damba"),
            Some("duplicate of an already-created card")
        );
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].notes, "of wind, storm, pain");
    }

    #[test]
    fn rejects_a_card_whose_english_is_just_a_stray_function_word() {
        let mut cards = Vec::new();
        let mut seen = HashMap::new();
        assert_eq!(
            check(&mut cards, &mut seen, "with", "khazimla"),
            Some("english is just a stray function word, not a real card")
        );
    }

    fn item(label: &str, source_excerpt: &str) -> Item {
        Item {
            label: label.to_string(),
            source_excerpt: source_excerpt.to_string(),
        }
    }

    #[test]
    fn rejects_an_enumerated_item_whose_label_is_copied_from_its_own_excerpt() {
        // Real failure: enumerate_items split one alternative
        // target-language translation, among several listed for
        // "abruptly", off into its own fake item — labeling it with
        // the Xhosa word itself, copied straight into English position.
        assert!(!label_is_not_copied_from_its_own_excerpt(&item(
            "ngequbuliso (noun)",
            "ngequbuliso, ngebhaqo"
        )));
    }

    #[test]
    fn accepts_an_enumerated_item_with_a_real_english_label() {
        assert!(label_is_not_copied_from_its_own_excerpt(&item(
            "abruptness (noun)",
            "~ness, n."
        )));
        assert!(label_is_not_copied_from_its_own_excerpt(&item(
            "abruptly (adv.)",
            "~-ly, adv. krwada"
        )));
    }

    #[test]
    fn label_check_leaves_a_short_label_alone_to_avoid_coincidental_overlap() {
        assert!(label_is_not_copied_from_its_own_excerpt(&item(
            "it (pronoun)",
            "it was late"
        )));
    }

    #[test]
    fn detects_the_real_ngequbuliso_reuse_across_two_different_senses() {
        // Real failure: the bogus "abruptness" card reused "ngequbuliso"
        // from the entry's own already-accepted "abruptly" card.
        let already_accepted = vec![card("abruptly", "ngequbuliso")];
        let bogus = card("abruptness", "ngequbuliso");
        assert!(target_text_reused_from_a_different_sense(
            &bogus,
            &already_accepted
        ));
    }

    #[test]
    fn a_unique_translation_is_not_flagged_as_reused() {
        // Real case this must NOT catch: "accelerator" legitimately has
        // its own, unique translation — nothing else in the entry uses
        // "umcephe".
        let already_accepted = vec![card("accelerate", "khawulezisa")];
        let legit = card("accelerator", "umcephe");
        assert!(!target_text_reused_from_a_different_sense(
            &legit,
            &already_accepted
        ));
    }

    #[test]
    fn an_exact_repeat_of_the_same_card_is_not_flagged_as_reused() {
        // Exact (english, target_text) duplicates are `card_rejection_reason`'s
        // own job — this check is specifically for the *same* translation
        // attached to a *different* sense, not a plain repeat.
        let already_accepted = vec![card("abrupt", "krwada")];
        let repeat = card("abrupt", "krwada");
        assert!(!target_text_reused_from_a_different_sense(
            &repeat,
            &already_accepted
        ));
    }

    #[test]
    fn an_empty_target_text_is_not_flagged_as_reused() {
        let already_accepted = vec![card("abruptly", "")];
        let other = card("abruptness", "");
        assert!(!target_text_reused_from_a_different_sense(
            &other,
            &already_accepted
        ));
    }

    #[test]
    fn reuse_in_an_unrelated_earlier_entry_is_not_flagged() {
        // `cards_this_entry` must only ever be passed cards from the
        // *same* entry — a coincidental match against an unrelated
        // entry elsewhere in the dictionary isn't this bug, and this
        // function trusts its caller to have already scoped the slice.
        let other_entrys_cards: Vec<Card> = vec![];
        let card_here = card("abruptness", "ngequbuliso");
        assert!(!target_text_reused_from_a_different_sense(
            &card_here,
            &other_entrys_cards
        ));
    }

    proptest::proptest! {
        /// Everything `normalize_for_comparison` does — stripping a
        /// redundant pos-tag parenthetical, dropping parens, collapsing
        /// whitespace, trimming hyphens, lowercasing — produces a form
        /// that's already fully normalized: running it again on its
        /// own output must always be a no-op, for any input whatsoever.
        #[test]
        fn normalize_for_comparison_is_idempotent(s in ".*") {
            let once = normalize_for_comparison(&s);
            let twice = normalize_for_comparison(&once);
            proptest::prop_assert_eq!(once, twice);
        }

        /// This runs on real, possibly-garbled model output — it must
        /// never panic, no matter how adversarial the input (empty,
        /// all parens, unmatched parens, arbitrary Unicode).
        #[test]
        fn normalize_for_comparison_never_panics(s in ".*") {
            let _ = normalize_for_comparison(&s);
        }

        /// The whole point of `seen_card_keys`: once a genuinely new
        /// card has been accepted, checking the *exact same* card again
        /// must always report it as a duplicate — never silently
        /// accepted twice, for any non-degenerate (english, target_text)
        /// pair.
        #[test]
        fn accepting_a_card_then_rechecking_it_always_reports_a_duplicate(
            english in "[a-zA-Z]{2,10}",
            target_text in "[a-zA-Z]{2,10}",
        ) {
            proptest::prop_assume!(english.to_lowercase() != target_text.to_lowercase());
            proptest::prop_assume!(!TRIVIAL_FRAGMENT_LABELS.contains(&english.to_lowercase().as_str()));
            let mut cards = Vec::new();
            let mut seen = HashMap::new();
            proptest::prop_assert_eq!(check(&mut cards, &mut seen, &english, &target_text), None);
            proptest::prop_assert_eq!(
                check(&mut cards, &mut seen, &english, &target_text),
                Some("duplicate of an already-created card")
            );
        }
    }
}
