//! The three JSON Schemas passed to the embedded engine's grammar-constrained
//! sampling, ported verbatim from the original script's schema literals.
use serde_json::{Value, json};

pub fn segment_schema() -> Value {
    json!({
        "type": "array",
        "items": {
            "type": "object",
            "properties": {
                "headword": { "type": "string" },
                "raw_entry": { "type": "string" },
            },
            "required": ["headword", "raw_entry"],
        },
    })
}

pub fn enumerate_schema() -> Value {
    json!({
        "type": "array",
        "items": {
            "type": "object",
            "properties": {
                "label": { "type": "string" },
                "source_excerpt": { "type": "string" },
            },
            "required": ["label", "source_excerpt"],
        },
    })
}

pub fn fill_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "english": { "type": "string", "maxLength": 80 },
            "target_text": { "type": "string", "maxLength": 80 },
            "part_of_speech": {
                "type": "string",
                "enum": [
                    "", "noun", "verb", "transitive verb", "intransitive verb",
                    "adjective", "adverb", "preposition", "conjunction",
                    "pronoun", "interjection", "numeral",
                ],
            },
            "notes": { "type": "string", "maxLength": 300 },
            "example_target": { "type": "string", "maxLength": 150 },
            "example_gloss": { "type": "string", "maxLength": 150 },
            "image_query": { "type": "string", "maxLength": 120 },
        },
        "required": ["english", "target_text", "image_query"],
    })
}
