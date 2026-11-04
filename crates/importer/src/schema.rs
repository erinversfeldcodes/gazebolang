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
    // maxLength on every field (especially the short ones like
    // part_of_speech) isn't just a sanity bound — without it, a verbose
    // model will write paragraph-length commentary into whichever
    // open-ended string field comes first, rather than actually using
    // the dedicated notes/example_target/image_query fields that follow
    // it. Confirmed in a real run against the production decompose
    // model: part_of_speech ended up containing several sentences,
    // including literal "Notes:"/"Example Target:"/"Image Query:" text,
    // while those real fields were all left empty.
    json!({
        "type": "object",
        "properties": {
            "english": { "type": "string", "maxLength": 80 },
            "target_text": { "type": "string", "maxLength": 80 },
            // An enum, not a free string: a maxLength-bounded string still
            // let the model pad or truncate mid-word once it ran past
            // whatever it actually wanted to say (confirmed in a real run).
            // Forcing one exact literal choice removes that failure mode
            // entirely.
            "part_of_speech": {
                "type": "string",
                "enum": [
                    "", "n.", "v.", "vt.", "vi.", "adj.", "adv.", "prep.",
                    "conj.", "pron.", "interj.", "num.",
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
