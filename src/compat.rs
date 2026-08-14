use serde::Serialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::error::{GatewayError, Result};

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

pub fn normalize_openai_tools(tools: &[Value]) -> Vec<Value> {
    tools
        .iter()
        .filter_map(|tool| {
            if tool.get("type").and_then(Value::as_str) != Some("function") {
                return None;
            }
            let function = tool.get("function").unwrap_or(tool);
            let name = function.get("name")?.as_str()?;
            Some(json!({
                "name": name,
                "description": function.get("description").cloned().unwrap_or_else(|| json!("")),
                "parameters": function.get("parameters").cloned().unwrap_or_else(|| json!({"type":"object","properties":{}})),
            }))
        })
        .collect()
}

pub fn append_tool_instructions(prompt: &mut String, tools: &[Value], tool_choice: Option<&Value>) {
    let tools = normalize_openai_tools(tools);
    if tools.is_empty() {
        return;
    }
    let required = matches!(tool_choice.and_then(Value::as_str), Some("required"))
        || tool_choice
            .and_then(|value| value.get("type"))
            .and_then(Value::as_str)
            == Some("function");
    let suffix = if prompt.ends_with("\n\nAssistant:") {
        prompt.truncate(prompt.len() - "\n\nAssistant:".len());
        "\n\nAssistant:"
    } else if prompt.ends_with("\n\nModel:") {
        prompt.truncate(prompt.len() - "\n\nModel:".len());
        "\n\nModel:"
    } else {
        ""
    };
    prompt.push_str("\n\nTool-use instructions:\nAvailable client functions:\n");
    prompt.push_str(&serde_json::to_string(&tools).unwrap_or_default());
    prompt.push_str(
        "\nIf a function is needed, return only valid JSON in this exact form: {\"tool_calls\":[{\"name\":\"function_name\",\"arguments\":{}}]}. Do not wrap it in Markdown. The client, not you, executes functions. Otherwise answer normally.",
    );
    if required {
        prompt.push_str(
            " A function call is required for this turn; do not answer with ordinary text.",
        );
    }
    prompt.push_str(suffix);
}

pub fn tool_choice_required(tool_choice: Option<&Value>) -> bool {
    matches!(tool_choice.and_then(Value::as_str), Some("required"))
        || tool_choice
            .and_then(|value| value.get("type"))
            .and_then(Value::as_str)
            == Some("function")
}

pub fn extract_tool_calls(text: &str) -> Vec<ToolCall> {
    let trimmed = text.trim();
    let candidate = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .unwrap_or(trimmed)
        .strip_suffix("```")
        .unwrap_or(trimmed)
        .trim();
    let value = serde_json::from_str::<Value>(candidate).ok().or_else(|| {
        let start = candidate.find('{')?;
        let end = candidate.rfind('}')?;
        serde_json::from_str(&candidate[start..=end]).ok()
    });
    let Some(value) = value else {
        return Vec::new();
    };
    let calls: Vec<&Value> = if let Some(calls) = value.as_array() {
        calls.iter().collect()
    } else if let Some(calls) = value
        .get("tool_calls")
        .and_then(Value::as_array)
        .or_else(|| value.get("function_calls").and_then(Value::as_array))
    {
        calls.iter().collect()
    } else if let Some(call) = value.get("function_call") {
        vec![call]
    } else if value.get("name").and_then(Value::as_str).is_some() {
        vec![&value]
    } else {
        Vec::new()
    };
    calls
        .into_iter()
        .filter_map(|call| {
            let function = call.get("function").unwrap_or(call);
            let name = function.get("name")?.as_str()?.to_owned();
            let arguments = function
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let arguments = arguments
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| arguments.to_string());
            Some(ToolCall {
                id: call
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("call_{}", Uuid::new_v4().simple())),
                name,
                arguments,
            })
        })
        .collect()
}

pub fn responses_input_to_prompt(input: &Value, instructions: Option<&str>) -> Result<String> {
    let mut sections = Vec::new();
    if let Some(instructions) = instructions.filter(|value| !value.trim().is_empty()) {
        sections.push(format!("System instructions:\n{}", instructions.trim()));
    }
    match input {
        Value::String(text) => sections.push(format!("User:\n{text}")),
        Value::Array(items) => {
            for item in items {
                let kind = item
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("message");
                if kind == "function_call_output" {
                    let call_id = item
                        .get("call_id")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown");
                    let output = value_to_text(item.get("output").unwrap_or(&Value::Null));
                    sections.push(format!("Tool result for {call_id}:\n{output}"));
                    continue;
                }
                if kind == "function_call" {
                    let name = item
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown");
                    let arguments = value_to_text(item.get("arguments").unwrap_or(&Value::Null));
                    sections.push(format!("Assistant requested function {name}:\n{arguments}"));
                    continue;
                }
                let role = item.get("role").and_then(Value::as_str).unwrap_or("user");
                let content = value_to_text(item.get("content").unwrap_or(item));
                if !content.trim().is_empty() {
                    sections.push(format!("{}:\n{}", role_label(role), content.trim()));
                }
            }
        }
        _ => {
            return Err(GatewayError::InvalidRequest(
                "input must be a string or array".into(),
            ));
        }
    }
    if sections.is_empty() {
        return Err(GatewayError::InvalidRequest("input cannot be empty".into()));
    }
    Ok(format!("{}\n\nAssistant:", sections.join("\n\n")))
}

pub fn gemini_contents_to_prompt(body: &Value) -> Result<String> {
    let mut sections = Vec::new();
    if let Some(system) = body.get("systemInstruction") {
        let text = value_to_text(system.get("parts").unwrap_or(system));
        if !text.is_empty() {
            sections.push(format!("System instructions:\n{text}"));
        }
    }
    let contents = body
        .get("contents")
        .and_then(Value::as_array)
        .ok_or_else(|| GatewayError::InvalidRequest("contents must be a non-empty array".into()))?;
    for content in contents {
        let role = content
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("user");
        let text = value_to_text(content.get("parts").unwrap_or(content));
        if !text.is_empty() {
            sections.push(format!("{}:\n{text}", role_label(role)));
        }
    }
    if sections.is_empty() {
        return Err(GatewayError::InvalidRequest(
            "contents cannot be empty".into(),
        ));
    }
    Ok(format!("{}\n\nModel:", sections.join("\n\n")))
}

pub fn gemini_tools(body: &Value) -> Vec<Value> {
    body.get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|tool| {
            tool.get("functionDeclarations")
                .or_else(|| tool.get("function_declarations"))
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .filter_map(|function| {
            Some(json!({
                "type": "function",
                "name": function.get("name")?.clone(),
                "description": function.get("description").cloned().unwrap_or_else(|| json!("")),
                "parameters": function.get("parameters").cloned().unwrap_or_else(|| json!({"type":"object","properties":{}})),
            }))
        })
        .collect()
}

pub fn gemini_tool_required(body: &Value) -> bool {
    body.get("toolConfig")
        .or_else(|| body.get("tool_config"))
        .and_then(|config| {
            config
                .get("functionCallingConfig")
                .or_else(|| config.get("function_calling_config"))
        })
        .and_then(|config| config.get("mode"))
        .and_then(Value::as_str)
        .is_some_and(|mode| matches!(mode.to_ascii_uppercase().as_str(), "ANY" | "VALIDATED"))
}

pub fn value_to_text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .map(value_to_text)
            .filter(|v| !v.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Object(map) => {
            if let Some(text) = map.get("text").and_then(Value::as_str) {
                return text.to_owned();
            }
            if let Some(text) = map.get("output_text").and_then(Value::as_str) {
                return text.to_owned();
            }
            if let Some(output) = map.get("output") {
                return value_to_text(output);
            }
            if let Some(url) = map
                .get("image_url")
                .and_then(|v| v.as_str().or_else(|| v.get("url").and_then(Value::as_str)))
            {
                return format!("[User image: {url}]");
            }
            map.get("content")
                .map(value_to_text)
                .unwrap_or_else(|| value.to_string())
        }
        _ => value.to_string(),
    }
}

fn role_label(role: &str) -> &'static str {
    match role.to_ascii_lowercase().as_str() {
        "system" | "developer" => "System instructions",
        "assistant" | "model" => "Assistant",
        "tool" | "function" => "Tool result",
        _ => "User",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_openai_and_compact_tool_calls() {
        let compact = extract_tool_calls(
            r#"{"tool_calls":[{"name":"weather","arguments":{"city":"Paris"}}]}"#,
        );
        assert_eq!(compact[0].name, "weather");
        assert_eq!(compact[0].arguments, r#"{"city":"Paris"}"#);
        let openai = extract_tool_calls(
            r#"{"tool_calls":[{"id":"call_1","function":{"name":"clock","arguments":"{}"}}]}"#,
        );
        assert_eq!(openai[0].id, "call_1");
        let direct = extract_tool_calls(r#"{"name":"weather","arguments":{"city":"Paris"}}"#);
        assert_eq!(direct[0].name, "weather");
    }

    #[test]
    fn flattens_responses_function_output() {
        let prompt = responses_input_to_prompt(
            &json!([
                {"role":"user","content":"weather?"},
                {"type":"function_call_output","call_id":"call_1","output":"sunny"}
            ]),
            None,
        )
        .unwrap();
        assert!(prompt.contains("Tool result for call_1:\nsunny"));
    }

    #[test]
    fn parses_compatibility_request_fixtures() {
        let responses: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/responses_function_roundtrip.json"
        ))
        .unwrap();
        let prompt = responses_input_to_prompt(responses.get("input").unwrap(), None).unwrap();
        assert!(prompt.contains("Tool result for call_fixture"));

        let gemini: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/gemini_function_request.json"
        ))
        .unwrap();
        assert_eq!(gemini_tools(&gemini)[0]["name"], "weather");
        assert!(
            gemini_contents_to_prompt(&gemini)
                .unwrap()
                .contains("Weather in Paris?")
        );
    }

    #[test]
    fn tool_instructions_precede_assistant_marker() {
        let mut prompt = "User:\nhello\n\nAssistant:".to_owned();
        append_tool_instructions(
            &mut prompt,
            &[json!({"type":"function","name":"ping","parameters":{"type":"object"}})],
            Some(&json!("required")),
        );
        assert!(prompt.ends_with("\n\nAssistant:"));
        assert!(
            prompt.find("Tool-use instructions").unwrap() < prompt.rfind("Assistant:").unwrap()
        );
        assert!(tool_choice_required(Some(&json!("required"))));
    }
}
