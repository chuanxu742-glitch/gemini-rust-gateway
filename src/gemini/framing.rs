use serde_json::Value;

/// Parse Google's length-prefixed streaming frames. Lengths are UTF-16 code
/// units, matching JavaScript String.length rather than UTF-8 byte length.
pub fn parse_frames(input: &str) -> (Vec<Value>, String) {
    let mut frames = Vec::new();
    let mut position = 0usize;

    while position < input.len() {
        while let Some(ch) = input[position..].chars().next() {
            if !ch.is_whitespace() {
                break;
            }
            position += ch.len_utf8();
        }
        if position >= input.len() {
            break;
        }

        let Some(newline_offset) = input[position..].find('\n') else {
            break;
        };
        let marker_end = position + newline_offset;
        let marker = &input[position..marker_end];
        if marker.is_empty() || !marker.bytes().all(|byte| byte.is_ascii_digit()) {
            break;
        }
        let Ok(expected_units) = marker.parse::<usize>() else {
            break;
        };

        // The provider counts the newline after the marker as part of the frame.
        let content_start = marker_end;
        let mut units = 0usize;
        let mut content_end = content_start;
        for ch in input[content_start..].chars() {
            let next = units + ch.len_utf16();
            if next > expected_units {
                break;
            }
            units = next;
            content_end += ch.len_utf8();
            if units == expected_units {
                break;
            }
        }
        if units < expected_units {
            break;
        }

        let chunk = input[content_start..content_end].trim();
        position = content_end;
        if chunk.is_empty() {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(chunk) {
            if let Value::Array(items) = value {
                frames.extend(items);
            } else {
                frames.push(value);
            }
        }
    }

    (frames, input[position..].to_owned())
}

pub fn nested<'a>(value: &'a Value, path: &[PathPart<'_>]) -> Option<&'a Value> {
    let mut current = value;
    for part in path {
        current = match part {
            PathPart::Index(index) => current.as_array()?.get(*index)?,
            PathPart::Key(key) => current.as_object()?.get(*key)?,
        };
    }
    Some(current)
}

#[derive(Clone, Copy)]
pub enum PathPart<'a> {
    Index(usize),
    Key(&'a str),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ascii_frame() {
        let payload = "\n[1,2]\n";
        let framed = format!("{}{}", payload.encode_utf16().count(), payload);
        let (frames, rest) = parse_frames(&framed);
        assert_eq!(frames, vec![Value::from(1), Value::from(2)]);
        assert!(rest.is_empty());
    }

    #[test]
    fn waits_for_utf16_frame_completion() {
        let payload = "\n[\"😀\"]\n";
        let framed = format!("{}{}", payload.encode_utf16().count(), payload);
        let split = framed.len() - 2;
        let (frames, rest) = parse_frames(&framed[..split]);
        assert!(frames.is_empty());
        assert_eq!(rest, framed[..split]);
    }
}
