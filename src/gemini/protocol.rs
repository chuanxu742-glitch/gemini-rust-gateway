use std::collections::HashMap;

use serde_json::Value;

use super::framing::{PathPart, nested};

pub fn model_headers(model: &str) -> HashMap<&'static str, String> {
    let spec = match model {
        "gemini-3-pro" => Some(("9d8ca3786ebdfbea", 1)),
        "gemini-3-flash" => Some(("fbb127bbb056c959", 1)),
        "gemini-3-flash-thinking" => Some(("5bf011840784117a", 1)),
        "gemini-3-pro-plus" => Some(("e6fa609c3fa255c0", 4)),
        "gemini-3-flash-plus" => Some(("56fdd199312815e2", 4)),
        "gemini-3-flash-thinking-plus" => Some(("e051ce1aa80aa576", 4)),
        "gemini-3-pro-advanced" => Some(("e6fa609c3fa255c0", 2)),
        "gemini-3-flash-advanced" => Some(("56fdd199312815e2", 2)),
        "gemini-3-flash-thinking-advanced" => Some(("e051ce1aa80aa576", 2)),
        _ => None,
    };
    let mut headers = HashMap::new();
    if let Some((model_id, capacity)) = spec {
        headers.insert(
            "x-goog-ext-525001261-jspb",
            format!(r#"[1,null,null,null,"{model_id}",null,null,0,[4],null,null,{capacity}]"#),
        );
        headers.insert("x-goog-ext-73010989-jspb", "[0]".into());
        headers.insert("x-goog-ext-73010990-jspb", "[0]".into());
    }
    headers
}

pub fn update_output(output: &mut super::super::domain::output::GeminiOutput, parsed: &Value) {
    if let Some(metadata) = nested(parsed, &[PathPart::Index(1)]).and_then(Value::as_array) {
        output.cid = metadata.first().and_then(Value::as_str).map(str::to_owned);
        output.rid = metadata.get(1).and_then(Value::as_str).map(str::to_owned);
    }
    let Some(candidate) = nested(parsed, &[PathPart::Index(4), PathPart::Index(0)]) else {
        return;
    };
    output.rcid = nested(candidate, &[PathPart::Index(0)])
        .and_then(Value::as_str)
        .map(str::to_owned);
    if let Some(text) =
        nested(candidate, &[PathPart::Index(1), PathPart::Index(0)]).and_then(Value::as_str)
    {
        output.text = text.to_owned();
    }

    let mut image_urls = Vec::new();
    collect_generated_image_urls(
        nested(
            candidate,
            &[PathPart::Index(12), PathPart::Index(7), PathPart::Index(0)],
        ),
        &mut image_urls,
    );
    collect_generated_image_urls(
        nested(
            candidate,
            &[
                PathPart::Index(12),
                PathPart::Index(0),
                PathPart::Key("8"),
                PathPart::Index(0),
            ],
        ),
        &mut image_urls,
    );
    image_urls.sort();
    image_urls.dedup();
    if !image_urls.is_empty() {
        output.generated_images = image_urls;
    }

    if let Some(urls) = nested(
        candidate,
        &[
            PathPart::Index(12),
            PathPart::Index(59),
            PathPart::Index(0),
            PathPart::Index(0),
            PathPart::Index(0),
            PathPart::Index(0),
            PathPart::Index(7),
        ],
    )
    .and_then(Value::as_array)
        && let Some(url) = urls.get(1).and_then(Value::as_str)
    {
        output.generated_videos = vec![url.to_owned()];
    }

    if let Some(media) =
        nested(candidate, &[PathPart::Index(12), PathPart::Index(86)]).and_then(Value::as_array)
    {
        let mut urls = Vec::new();
        for item in media {
            if let Some(parts) =
                nested(item, &[PathPart::Index(1), PathPart::Index(7)]).and_then(Value::as_array)
                && let Some(url) = parts.get(1).and_then(Value::as_str)
            {
                urls.push(url.to_owned());
            }
        }
        if !urls.is_empty() {
            output.generated_media = urls;
        }
    }
}

fn collect_generated_image_urls(value: Option<&Value>, output: &mut Vec<String>) {
    let Some(images) = value.and_then(Value::as_array) else {
        return;
    };
    for image in images {
        if let Some(url) = nested(
            image,
            &[PathPart::Index(0), PathPart::Index(3), PathPart::Index(3)],
        )
        .and_then(Value::as_str)
        {
            output.push(url.to_owned());
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parses_text_candidate() {
        let parsed = json!([null, ["cid", "rid"], null, null, [["rcid", ["hello"]]]]);
        let mut output = super::super::super::domain::output::GeminiOutput::default();
        update_output(&mut output, &parsed);
        assert_eq!(output.text, "hello");
        assert_eq!(output.cid.as_deref(), Some("cid"));
        assert_eq!(output.rcid.as_deref(), Some("rcid"));
    }

    #[test]
    fn parses_image_to_image_candidate() {
        let mut image = vec![Value::Null; 4];
        image[3] = json!([null, null, null, "https://example.test/generated.png"]);
        let generated_image = json!([image]);
        let mut candidate = vec![Value::Null; 13];
        candidate[0] = json!("rcid");
        candidate[1] = json!(["done"]);
        candidate[12] = json!([{"8": [[generated_image]]}]);
        let parsed = json!([null, ["cid", "rid"], null, null, [candidate]]);
        let mut output = super::super::super::domain::output::GeminiOutput::default();
        update_output(&mut output, &parsed);
        assert_eq!(
            output.generated_images,
            vec!["https://example.test/generated.png"]
        );
    }

    #[test]
    fn parses_captured_text_fixture() {
        let parsed: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/protocol_text_candidate.json"
        ))
        .unwrap();
        let mut output = super::super::super::domain::output::GeminiOutput::default();
        update_output(&mut output, &parsed);
        assert_eq!(output.text, "fixture response text");
        assert_eq!(output.cid.as_deref(), Some("fixture-cid"));
    }

    #[test]
    fn parses_captured_image_fixture() {
        let parsed: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/protocol_image_candidate.json"
        ))
        .unwrap();
        let mut output = super::super::super::domain::output::GeminiOutput::default();
        update_output(&mut output, &parsed);
        assert_eq!(
            output.generated_images,
            vec!["https://example.test/fixture.png"]
        );
    }
}
