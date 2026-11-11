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

const TRIVIAL_FRAGMENT_LABELS: &[&str] = &[
    "with", "the", "a", "an", "of", "in", "on", "by", "to", "and", "or", "but", "for", "at",
];

fn normalize_for_comparison(s: &str) -> String {
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

fn target_text_is_a_near_duplicate(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let (shorter, longer) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    !shorter.is_empty() && longer.contains(shorter) && longer.len() - shorter.len() <= 3
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
/// learn). When neither exact key matches, a slower fallback scan
/// additionally catches a same-English card whose `target_text` is only
/// a near-duplicate (see `target_text_is_a_near_duplicate`) rather
/// than identical — deliberately gated on the English side matching
/// first, so two genuinely different cards that happen to share a
/// short, legitimately-reused Xhosa stem are never merged just because
/// their translations overlap.
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

    let full_key = format!("{normalized_full}|{normalized_target}");
    let base_key = format!("{normalized_base}|{normalized_target}");

    let existing_idx = seen_card_keys
        .get(&full_key)
        .or_else(|| seen_card_keys.get(&base_key))
        .copied()
        .or_else(|| {
            cards.iter().position(|existing| {
                let existing_full = normalize_for_comparison(&existing.english);
                let (existing_base_str, _) = split_trailing_parenthetical(&existing.english);
                let existing_base = normalize_for_comparison(existing_base_str);
                (existing_full == normalized_full || existing_base == normalized_base)
                    && target_text_is_a_near_duplicate(
                        &normalize_for_comparison(&existing.target_text),
                        &normalized_target,
                    )
            })
        });
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

const CHUNK_THRESHOLD_CHARS: usize = 300;

const CHUNK_OVERLAP_CHARS: usize = 80;

fn split_into_overlapping_chunks(raw_entry: &str) -> Option<(String, String)> {
    if raw_entry.len() <= CHUNK_THRESHOLD_CHARS {
        return None;
    }
    let mut midpoint = raw_entry.len() / 2;
    while !raw_entry.is_char_boundary(midpoint) {
        midpoint += 1;
    }
    let split_at = raw_entry[..midpoint]
        .rfind(';')
        .or_else(|| raw_entry.find(';'))?;
    let first_half = raw_entry[..=split_at].trim().to_string();
    let mut overlap_start = split_at.saturating_sub(CHUNK_OVERLAP_CHARS);
    while !raw_entry.is_char_boundary(overlap_start) {
        overlap_start += 1;
    }
    let second_half = raw_entry[overlap_start..].trim().to_string();
    Some((first_half, second_half))
}

/// Pass 2a — enumerate one already-correctly-read entry's distinct
/// learnable units (not their fields yet, just what they are).
/// Chunks a genuinely long entry into two overlapping halves (see
/// `split_into_overlapping_chunks`) and enumerates each independently,
/// union-merging the results — short entries (the great majority) are
/// completely unaffected, going through `enumerate_items_single_call`
/// exactly once just as before.
const DERIVED_WORD_POS_TAGS: &[&str] = &[
    "n.", "v.", "vt.", "vi.", "adj.", "adv.", "prep.", "conj.", "pron.", "interj.", "num.",
];

fn starts_a_new_structural_unit(piece: &str) -> bool {
    if piece.starts_with('(') || piece.starts_with('~') {
        return true;
    }
    if piece.starts_with("eg.") || piece.starts_with("e.g.") {
        return true;
    }
    if let Some(comma_idx) = piece.find(',') {
        let word = &piece[..comma_idx];
        let looks_like_a_word =
            !word.is_empty() && word.chars().all(|c| c.is_alphabetic() || c == '-');
        if looks_like_a_word {
            let after_comma = piece[comma_idx + 1..].trim_start();
            if DERIVED_WORD_POS_TAGS
                .iter()
                .any(|tag| after_comma.starts_with(tag))
            {
                return true;
            }
        }
    }
    false
}

/// Rough, deliberately conservative lower bound on how many distinct
/// learnable units `raw_entry` structurally contains — counts the
/// semicolon-delimited spans that start with a clear marker (reusing
/// `starts_a_new_structural_unit`, the same signal an earlier,
/// reverted attempt used to *constrain* enumeration — see
/// `enumerate_items`'s own doc comment), with one span of safety
/// margin subtracted.
fn marker_based_minimum_item_count(raw_entry: &str) -> usize {
    let marker_spans = raw_entry
        .split(';')
        .map(str::trim)
        .filter(|piece| !piece.is_empty())
        .enumerate()
        .filter(|(i, piece)| *i == 0 || starts_a_new_structural_unit(piece))
        .count();
    marker_spans.saturating_sub(1)
}

pub fn enumerate_items(
    engine: &TextEngine,
    headword: &str,
    raw_entry: &str,
) -> Result<Vec<Item>, String> {
    let mut items = match split_into_overlapping_chunks(raw_entry) {
        None => enumerate_items_single_call(engine, headword, raw_entry)?,
        Some((first_half, second_half)) => {
            let mut items = enumerate_items_single_call(engine, headword, &first_half)?;
            items.extend(enumerate_items_single_call(engine, headword, &second_half)?);
            items
        }
    };
    let missed = find_missed_synonyms(engine, headword, raw_entry, &items)?;
    if !missed.is_empty() {
        println!(
            "  {headword}: missed-synonym check found {} more item(s)",
            missed.len()
        );
        items.extend(missed);
    }
    let floor = marker_based_minimum_item_count(raw_entry);
    if items.len() < floor {
        println!(
            "  WARNING: \"{headword}\" found only {} item(s), below the {floor} this \
             entry's own visible structure suggests — worth a manual look",
            items.len()
        );
    }
    Ok(items)
}

/// A single, narrowly-targeted second pass checking specifically
/// whether any sense above lists more than one alternative
/// target-language translation where only some of those alternatives
/// made it into that item's own excerpt
fn find_missed_synonyms(
    engine: &TextEngine,
    headword: &str,
    raw_entry: &str,
    items: &[Item],
) -> Result<Vec<Item>, String> {
    if items.is_empty() {
        return Ok(Vec::new());
    }
    let items_so_far = items
        .iter()
        .map(|item| format!("label \"{}\" — \"{}\"", item.label, item.source_excerpt))
        .collect::<Vec<_>>()
        .join("\n");
    let prompt = format!(
        "Below is one raw, correctly-transcribed dictionary entry and the items \
already identified for it. Check specifically for ONE kind of gap: does any \
sense above list more than one alternative target-language translation \
(e.g. two or more words separated by commas or semicolons) where only SOME \
of those alternatives ended up in that item's own excerpt below? This is \
the only thing to check for — not whether a sense, example, or derived \
word was missed entirely, only whether an already-found sense is missing \
one of its own listed alternatives.\n\n\
If you find any, output one new item per missing alternative, in the same \
\"label\"/\"source_excerpt\" shape as before. The \"label\" field must be \
copied EXACTLY, character for character, from one of the existing labels \
quoted below — never a number, never a new label you make up yourself. \
The \"source_excerpt\" is the missing alternative itself. If every \
alternative is already covered, output an empty array — do not invent an \
item just to have something to report.\n\n\
Entry (headword: \"{headword}\"):\n{raw_entry}\n\n\
Items already found:\n{items_so_far}"
    );
    let schema = enumerate_schema();
    let text = engine.generate(&prompt, Some(&schema))?;
    let context = format!("{headword} (missed-synonym check)");
    let extra_items = extract_json_array(&text, &context)?;
    let existing_labels: std::collections::HashSet<String> = items
        .iter()
        .map(|i| i.label.trim().to_lowercase())
        .collect();
    Ok(extra_items
        .into_iter()
        .filter_map(|v| serde_json::from_value::<Item>(v).ok())
        .filter(|i| !i.label.is_empty() && !i.source_excerpt.is_empty())
        .filter(|i| existing_labels.contains(&i.label.trim().to_lowercase()))
        .collect())
}

fn enumerate_items_single_call(
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
fn label_is_not_copied_from_its_own_excerpt(item: &Item) -> bool {
    let (base_label, _) = split_trailing_parenthetical(&item.label);
    let normalized_label = normalize_for_comparison(base_label);
    normalized_label.chars().count() < 4
        || !normalize_for_comparison(&item.source_excerpt).contains(&normalized_label)
}

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
word), \"part_of_speech\" (the full part-of-speech name ONLY, e.g. \"noun\", \
\"verb\", \"adjective\", \"adverb\" — never an abbreviation, a definition, \
an example, or a full sentence; if truly unknown use \"\"), \"notes\" (brief \
extra context if useful, else \
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

#[derive(serde::Serialize)]
struct SkippedItem {
    headword: String,
    label: String,
    english: Option<String>,
    target_text: Option<String>,
    reason: String,
}

fn needs_review_path(cards_out_file: &Path) -> std::path::PathBuf {
    let stem = cards_out_file
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("cards");
    let dir = cards_out_file.parent().unwrap_or_else(|| Path::new("."));
    dir.join(format!("{stem}.needs-review.json"))
}

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
    fn target_text_is_a_near_duplicate_catches_a_one_character_prefix_variant() {
        assert!(target_text_is_a_near_duplicate(
            "khupha isisu",
            "ukhupha isisu"
        ));
    }

    #[test]
    fn target_text_is_a_near_duplicate_rejects_a_short_stem_inside_an_unrelated_longer_phrase() {
        assert!(!target_text_is_a_near_duplicate(
            "kho",
            "kukho iinyoka ezininzi"
        ));
    }

    #[test]
    fn rejects_a_near_duplicate_target_text_under_the_same_english() {
        let mut cards = Vec::new();
        let mut seen = HashMap::new();
        assert_eq!(check(&mut cards, &mut seen, "abort", "khupha isisu"), None);
        assert_eq!(
            check(&mut cards, &mut seen, "abort", "ukhupha isisu"),
            Some("duplicate of an already-created card")
        );
        assert_eq!(cards.len(), 1);
    }

    #[test]
    fn keeps_a_near_duplicate_target_text_under_a_different_english() {
        let mut cards = Vec::new();
        let mut seen = HashMap::new();
        assert_eq!(check(&mut cards, &mut seen, "abort", "khupha isisu"), None);
        assert_eq!(
            check(&mut cards, &mut seen, "aborted fetus", "isisu esiphumileyo"),
            None
        );
        assert_eq!(cards.len(), 2);
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
    fn short_entries_are_not_chunked() {
        assert_eq!(split_into_overlapping_chunks("abode, n. ikhaya."), None);
    }

    #[test]
    fn marker_count_matches_abuse_real_sense_count_conservatively() {
        let raw_entry = "abuse, vt. (of authority, confidence, etc.) -sebenzisa kakubi; \
            (in words) -thuka, -tshabisa; (old use) -phatha kakubi; ~, n. ukusebenzisa \
            kakubi; impatho embi (im- iim-); isithuko, isinyeliso; abusive, adj. \
            -nyelisa(yo); abusively, adv. ngokunyelisayo.";
        assert_eq!(marker_based_minimum_item_count(raw_entry), 5);
    }

    #[test]
    fn marker_count_never_overestimates_abode_which_has_no_semicolons_at_all() {
        let raw_entry = "abode, n. (old or liter. use) ikhaya (ili- ama-), umzi, \
            indlu (in- izin-).";
        assert_eq!(marker_based_minimum_item_count(raw_entry), 0);
    }

    #[test]
    fn marker_count_is_zero_for_a_single_clause_entry() {
        assert_eq!(marker_based_minimum_item_count("abscess, n. ithumba."), 0);
    }

    #[test]
    fn splits_the_real_about_prep_entry_without_cutting_any_sense_in_half() {
        let raw_entry = "about, prep. (with verbs of movement) apha naphaya (plus \
            locative); eg. he was walking ~ the town; wayehamba-hamba apha naphaya \
            edolphini; (indicating position or state) alpha naphaya (plus locative); \
            eg. books are lying ~ her room: kukho iincwadi apha naphaya egumbini \
            lakhe; (near to) kufuphi na-; (expressing 'concerning, regarding, in \
            connection with') nga-; eg. he does not care ~ his clothes: \
            akazikha-thazi ngempahla yakhe; what are you talking ~?: nithetha \
            ngantoni?; go/set ~ something: -enza.";
        let (first_half, second_half) = split_into_overlapping_chunks(raw_entry).unwrap();
        assert!(first_half.contains("(with verbs of movement) apha naphaya (plus locative)"));
        assert!(first_half.contains("he was walking ~ the town"));
        assert!(second_half.contains("(near to) kufuphi na-"));
        assert!(second_half.contains("go/set ~ something: -enza."));
        assert!(first_half.len() + second_half.len() > raw_entry.len());
    }

    #[test]
    fn split_into_overlapping_chunks_never_panics_on_short_or_delimiter_free_long_text() {
        assert_eq!(split_into_overlapping_chunks(""), None);
        let no_semicolons = "a".repeat(400);
        assert_eq!(split_into_overlapping_chunks(&no_semicolons), None);
    }

    #[test]
    fn split_into_overlapping_chunks_is_safe_on_multi_byte_utf8_near_the_midpoint() {
        let raw_entry = format!(
            "abbé, n. {}; eg. umfundisi waseFransi ~·té: something; {}",
            "x".repeat(140),
            "y".repeat(160)
        );
        let result = split_into_overlapping_chunks(&raw_entry);
        assert!(result.is_some());
    }

    #[test]
    fn rejects_an_enumerated_item_whose_label_is_copied_from_its_own_excerpt() {
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
        let already_accepted = vec![card("abruptly", "ngequbuliso")];
        let bogus = card("abruptness", "ngequbuliso");
        assert!(target_text_reused_from_a_different_sense(
            &bogus,
            &already_accepted
        ));
    }

    #[test]
    fn a_unique_translation_is_not_flagged_as_reused() {
        let already_accepted = vec![card("accelerate", "khawulezisa")];
        let legit = card("accelerator", "umcephe");
        assert!(!target_text_reused_from_a_different_sense(
            &legit,
            &already_accepted
        ));
    }

    #[test]
    fn an_exact_repeat_of_the_same_card_is_not_flagged_as_reused() {
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
        let other_entrys_cards: Vec<Card> = vec![];
        let card_here = card("abruptness", "ngequbuliso");
        assert!(!target_text_reused_from_a_different_sense(
            &card_here,
            &other_entrys_cards
        ));
    }

    proptest::proptest! {
        #[test]
        fn normalize_for_comparison_is_idempotent(s in ".*") {
            let once = normalize_for_comparison(&s);
            let twice = normalize_for_comparison(&once);
            proptest::prop_assert_eq!(once, twice);
        }
        #[test]
        fn normalize_for_comparison_never_panics(s in ".*") {
            let _ = normalize_for_comparison(&s);
        }

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
