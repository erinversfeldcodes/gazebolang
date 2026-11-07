//! Smaller local models sometimes still wrap output in a markdown fence or
//! add a sentence before/after it even under grammar-constrained decoding —
//! pull out the first matching bracketed span rather than requiring the
//! whole response to be clean JSON.
use serde_json::Value;

pub fn extract_json_array(text: &str, context: &str) -> Result<Vec<Value>, String> {
    let (start, end) = match (text.find('['), text.rfind(']')) {
        (Some(s), Some(e)) if e >= s => (s, e),
        _ => {
            return Err(format!(
                "couldn't find a JSON array in {context}'s response:\n{text}"
            ));
        }
    };
    match serde_json::from_str::<Value>(&text[start..=end]) {
        Ok(Value::Array(arr)) => Ok(arr),
        Ok(_) => Err(format!(
            "couldn't parse {context}'s response: not an array\n---\n{text}"
        )),
        Err(e) => Err(format!(
            "couldn't parse {context}'s response: {e}\n---\n{text}"
        )),
    }
}

pub fn extract_json_object(
    text: &str,
    context: &str,
) -> Result<serde_json::Map<String, Value>, String> {
    let (start, end) = match (text.find('{'), text.rfind('}')) {
        (Some(s), Some(e)) if e >= s => (s, e),
        _ => {
            return Err(format!(
                "couldn't find a JSON object in {context}'s response:\n{text}"
            ));
        }
    };
    match serde_json::from_str::<Value>(&text[start..=end]) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(_) => Err(format!(
            "couldn't parse {context}'s response: not an object\n---\n{text}"
        )),
        Err(e) => Err(format!(
            "couldn't parse {context}'s response: {e}\n---\n{text}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extract_json_array_parses_a_clean_array() {
        assert_eq!(
            extract_json_array(r#"[{"a":1}]"#, "ctx").unwrap(),
            vec![json!({"a": 1})]
        );
    }

    #[test]
    fn extract_json_array_tolerates_a_markdown_fence_and_surrounding_prose() {
        let text = "Sure, here it is:\n```json\n[{\"a\":1},{\"b\":2}]\n```\nHope that helps!";
        assert_eq!(
            extract_json_array(text, "ctx").unwrap(),
            vec![json!({"a": 1}), json!({"b": 2})]
        );
    }

    #[test]
    fn extract_json_array_rejects_a_lone_object_not_an_array() {
        let err = extract_json_array(r#"{"a":1}"#, "ctx").unwrap_err();
        assert!(err.contains("couldn't find a JSON array"), "{err}");
    }

    #[test]
    fn extract_json_array_rejects_unparseable_text() {
        let err = extract_json_array("not json at all", "ctx").unwrap_err();
        assert!(err.contains("couldn't find a JSON array"), "{err}");
    }

    #[test]
    fn extract_json_object_parses_a_clean_object() {
        let obj = extract_json_object(r#"{"a":1}"#, "ctx").unwrap();
        assert_eq!(obj.get("a"), Some(&json!(1)));
    }

    #[test]
    fn extract_json_object_rejects_an_array_not_an_object() {
        let err = extract_json_object("[1,2]", "ctx").unwrap_err();
        assert!(err.contains("couldn't find a JSON object"), "{err}");
    }
}
