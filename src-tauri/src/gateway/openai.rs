//! OpenAI Chat Completions 兼容层：请求翻译（OpenAI → Anthropic）与响应翻译
//! （非流式 JSON + 流式 SSE 逐事件转换）。语义移植自 ZCodeHub app/openai_compat.py。
//!
//! 覆盖：system/developer、user（字符串/多模态块取文本与 image_url）、assistant
//! tool_calls、tool 角色回传、response_format JSON 指令、reasoning_effort、
//! stop_sequences、tools/tool_choice、流式 text_delta/input_json_delta/message_delta。

use serde_json::{json, Map, Value};

pub fn sse(payload: &Value) -> String {
    format!("data: {}\n\n", payload)
}

fn text_from_content(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|b| {
                if b.get("type").and_then(Value::as_str) == Some("text") {
                    b.get("text").and_then(Value::as_str).map(String::from)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

fn blocks_from_user_content(content: Option<&Value>) -> Value {
    match content {
        Some(Value::String(s)) => {
            json!([{ "type": "text", "text": s }])
        }
        Some(Value::Array(items)) => {
            let mut out: Vec<Value> = vec![];
            for item in items {
                match item.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        if let Some(t) = item.get("text").and_then(Value::as_str) {
                            out.push(json!({ "type": "text", "text": t }));
                        }
                    }
                    Some("image_url") => {
                        let url = item
                            .get("image_url")
                            .map(|u| {
                                u.as_str()
                                    .map(String::from)
                                    .unwrap_or_else(|| u.get("url").and_then(Value::as_str).unwrap_or("").to_string())
                            })
                            .unwrap_or_default();
                        // 官方 anthropic wire 只认 base64 data:；http(s) 图片本地网关不下载
                        if let Some(b64) = url.strip_prefix("data:").and_then(|rest| rest.split_once(',')) {
                            let (meta, data) = b64;
                            let media = meta
                                .split(';')
                                .next()
                                .unwrap_or("image/png")
                                .to_string();
                            out.push(json!({ "type": "image", "source": { "type": "base64", "media_type": media, "data": data } }));
                        }
                    }
                    _ => {}
                }
            }
            if out.is_empty() {
                out.push(json!({ "type": "text", "text": "" }));
            }
            Value::Array(out)
        }
        _ => json!([{ "type": "text", "text": "" }]),
    }
}

const STOP_REASON_MAP: &[(&str, &str)] = &[
    ("max_tokens", "length"),
    ("stop_sequence", "stop"),
    ("tool_use", "tool_calls"),
];

pub fn stop_reason_to_finish(v: Option<&str>) -> &'static str {
    let key = v.unwrap_or("");
    STOP_REASON_MAP
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, f)| *f)
        .unwrap_or("stop")
}

/// OpenAI 请求体 → Anthropic messages 体。非法时返回 Err(错误信息)。
pub fn request_to_anthropic(payload: &Value) -> Result<Value, String> {
    let model = payload
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or("必须提供 model 参数")?
        .to_string();
    let msgs = payload
        .get("messages")
        .and_then(Value::as_array)
        .filter(|m| !m.is_empty())
        .ok_or("必须提供 messages 数组")?;

    let mut system_parts: Vec<String> = vec![];
    let mut out_msgs: Vec<Value> = vec![];
    for msg in msgs {
        let Some(obj) = msg.as_object() else { continue };
        let role = obj.get("role").and_then(Value::as_str).unwrap_or("");
        let content = obj.get("content");
        match role {
            "system" | "developer" => {
                let text = text_from_content(content);
                if !text.is_empty() {
                    system_parts.push(text);
                }
            }
            "tool" => {
                out_msgs.push(json!({
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": obj.get("tool_call_id").and_then(Value::as_str).unwrap_or(""),
                        "content": text_from_content(content),
                    }],
                }));
            }
            "assistant" => {
                let mut blocks: Vec<Value> = vec![];
                let text = text_from_content(content);
                if !text.is_empty() {
                    blocks.push(json!({ "type": "text", "text": text }));
                }
                for tc in obj.get("tool_calls").and_then(Value::as_array).unwrap_or(&vec![]) {
                    let fn_obj = tc.get("function").and_then(Value::as_object);
                    let Some(fn_obj) = fn_obj else { continue };
                    let args = match fn_obj.get("arguments") {
                        Some(Value::String(s)) => serde_json::from_str::<Value>(s)
                            .unwrap_or_else(|_| json!({ "_raw": s })),
                        Some(v @ Value::Object(_)) => v.clone(),
                        _ => json!({}),
                    };
                    blocks.push(json!({
                        "type": "tool_use",
                        "id": tc.get("id").and_then(Value::as_str).unwrap_or(""),
                        "name": fn_obj.get("name").and_then(Value::as_str).unwrap_or(""),
                        "input": args,
                    }));
                }
                out_msgs.push(json!({
                    "role": "assistant",
                    "content": if blocks.is_empty() { json!([{ "type": "text", "text": "" }]) } else { Value::Array(blocks) },
                }));
            }
            // user 及未知角色一律按 user 处理
            _ => {
                out_msgs.push(json!({ "role": "user", "content": blocks_from_user_content(content) }));
            }
        }
    }

    // 结构化输出：anthropic wire 没有 response_format，用 system 指令逼近
    if let Some(fmt) = payload.get("response_format").and_then(Value::as_object) {
        match fmt.get("type").and_then(Value::as_str) {
            Some("json_object") => {
                system_parts.push("Respond with a single valid JSON object and nothing else.".into());
            }
            Some("json_schema") => {
                let schema = fmt.get("json_schema").and_then(|js| js.get("schema"));
                let mut directive =
                    "Respond with a single JSON value conforming to this JSON Schema and nothing else.".to_string();
                if let Some(schema) = schema {
                    directive.push_str(" Schema: ");
                    directive.push_str(&schema.to_string());
                }
                system_parts.push(directive);
            }
            _ => {}
        }
    }

    let max_tokens = payload
        .get("max_tokens")
        .or_else(|| payload.get("max_completion_tokens"))
        .and_then(Value::as_i64)
        .filter(|v| *v > 0)
        .unwrap_or(4096);

    let mut body = Map::new();
    body.insert("model".into(), Value::String(model));
    body.insert("messages".into(), Value::Array(out_msgs));
    body.insert("max_tokens".into(), Value::from(max_tokens));
    if let Some(Value::Object(md)) = payload.get("metadata") {
        body.insert("metadata".into(), Value::Object(md.clone()));
    }
    for k in ["session_id", "conversation_id"] {
        if let Some(v) = payload.get(k) {
            body.insert(k.into(), v.clone());
        }
    }
    if !system_parts.is_empty() {
        body.insert("system".into(), Value::String(system_parts.join("\n\n")));
    }

    // reasoning_effort → output_config.effort
    let effort = payload
        .get("reasoning_effort")
        .and_then(Value::as_str)
        .map(String::from)
        .or_else(|| {
            payload
                .get("reasoning")
                .and_then(|r| r.get("effort"))
                .and_then(Value::as_str)
                .map(String::from)
        });
    if let Some(e) = effort.filter(|e| ["low", "medium", "high", "max"].contains(&e.to_lowercase().as_str())) {
        body.insert("output_config".into(), json!({ "effort": e.to_lowercase() }));
    }
    if let Some(t) = payload.get("temperature").and_then(Value::as_f64) {
        body.insert("temperature".into(), Value::from(t));
    }
    if let Some(t) = payload.get("top_p").and_then(Value::as_f64) {
        body.insert("top_p".into(), Value::from(t));
    }
    match payload.get("stop") {
        Some(Value::String(s)) if !s.is_empty() => {
            body.insert("stop_sequences".into(), json!([s]));
        }
        Some(Value::Array(arr)) if !arr.is_empty() => {
            body.insert(
                "stop_sequences".into(),
                Value::Array(arr.iter().filter_map(Value::as_str).map(|s| Value::String(s.into())).collect()),
            );
        }
        _ => {}
    }
    if payload.get("stream").and_then(Value::as_bool) == Some(true) {
        body.insert("stream".into(), Value::Bool(true));
    }

    if let Some(tools) = payload.get("tools").and_then(Value::as_array) {
        let mapped: Vec<Value> = tools
            .iter()
            .filter_map(|t| {
                let f = t.get("function").and_then(Value::as_object)?;
                let name = f.get("name").and_then(Value::as_str).filter(|s| !s.is_empty())?;
                Some(json!({
                    "name": name,
                    "description": f.get("description").and_then(Value::as_str).unwrap_or(""),
                    "input_schema": f.get("parameters").cloned().unwrap_or_else(|| json!({ "type": "object" })),
                }))
            })
            .collect();
        if !mapped.is_empty() {
            body.insert("tools".into(), Value::Array(mapped));
        }
    }
    match payload.get("tool_choice") {
        Some(Value::String(s)) if s == "none" => {
            body.remove("tools");
        }
        Some(Value::String(s)) if s == "required" => {
            body.insert("tool_choice".into(), json!({ "type": "any" }));
        }
        Some(Value::Object(c)) if c.get("type").and_then(Value::as_str) == Some("function") => {
            if let Some(name) = c.get("function").and_then(|f| f.get("name")).and_then(Value::as_str) {
                body.insert("tool_choice".into(), json!({ "type": "tool", "name": name }));
            }
        }
        _ => {}
    }
    Ok(Value::Object(body))
}

/// Anthropic message 响应 → OpenAI chat.completion
pub fn response_to_openai(data: &Value, model: &str) -> Value {
    let mut text_parts: Vec<String> = vec![];
    let mut tool_calls: Vec<Value> = vec![];
    for block in data.get("content").and_then(Value::as_array).unwrap_or(&vec![]) {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(t) = block.get("text").and_then(Value::as_str) {
                    text_parts.push(t.to_string());
                }
            }
            Some("tool_use") => {
                tool_calls.push(json!({
                    "id": block.get("id").and_then(Value::as_str).unwrap_or(""),
                    "type": "function",
                    "function": {
                        "name": block.get("name").and_then(Value::as_str).unwrap_or(""),
                        "arguments": block.get("input").cloned().unwrap_or(Value::Null).to_string(),
                    },
                }));
            }
            _ => {}
        }
    }
    let usage = data.get("usage").cloned().unwrap_or(Value::Null);
    let in_tok = usage.get("input_tokens").and_then(Value::as_i64).unwrap_or(0);
    let out_tok = usage.get("output_tokens").and_then(Value::as_i64).unwrap_or(0);
    let mut message = Map::new();
    message.insert("role".into(), Value::String("assistant".into()));
    message.insert(
        "content".into(),
        if text_parts.is_empty() {
            Value::Null
        } else {
            Value::String(text_parts.join(""))
        },
    );
    if !tool_calls.is_empty() {
        message.insert("tool_calls".into(), Value::Array(tool_calls));
    }
    json!({
        "id": data.get("id").and_then(Value::as_str).map(String::from).unwrap_or_else(|| format!("chatcmpl-{}", uuid::Uuid::new_v4().simple())),
        "object": "chat.completion",
        "created": chrono::Utc::now().timestamp(),
        "model": data.get("model").and_then(Value::as_str).unwrap_or(model),
        "choices": [{
            "index": 0,
            "message": Value::Object(message),
            "finish_reason": stop_reason_to_finish(data.get("stop_reason").and_then(Value::as_str)),
        }],
        "usage": { "prompt_tokens": in_tok, "completion_tokens": out_tok, "total_tokens": in_tok + out_tok },
    })
}

/// Anthropic SSE 事件流 → OpenAI chat.completion.chunk 流（有状态转换器）。
/// 上游 error 事件返回 Err(错误消息)。
pub struct StreamConverter {
    pub chunk_id: String,
    pub created: i64,
    pub finish_reason: Option<String>,
    pub usage_prompt: Option<i64>,
    pub usage_completion: Option<i64>,
    pub model_name: String,
    tool_seq: usize,
}

impl StreamConverter {
    pub fn new(model: &str) -> StreamConverter {
        StreamConverter {
            chunk_id: format!("chatcmpl-{}", uuid::Uuid::new_v4().simple()),
            created: chrono::Utc::now().timestamp(),
            finish_reason: None,
            usage_prompt: None,
            usage_completion: None,
            model_name: model.to_string(),
            tool_seq: 0,
        }
    }

    fn usage_total(&self) -> Option<i64> {
        match (self.usage_prompt, self.usage_completion) {
            (Some(p), Some(c)) => Some(p + c),
            (Some(p), None) => Some(p),
            (None, Some(c)) => Some(c),
            _ => None,
        }
    }

    fn chunk_event(&self, delta: Value, finish_reason: Option<&str>) -> Value {
        let mut payload = json!({
            "id": self.chunk_id,
            "object": "chat.completion.chunk",
            "created": self.created,
            "choices": [{ "index": 0, "delta": delta, "finish_reason": finish_reason }],
        });
        if finish_reason.is_some() {
            payload["usage"] = json!({
                "prompt_tokens": self.usage_prompt,
                "completion_tokens": self.usage_completion,
                "total_tokens": self.usage_total(),
            });
            // model 字段在 finish chunk 一并补齐（保持每次 chunk 都带 model）
            payload["model"] = Value::String(self.model_name.clone());
        }
        payload
    }

    pub fn start_event(&self) -> Value {
        self.chunk_event(json!({ "role": "assistant", "content": "" }), None)
    }

    pub fn done(&self) -> &'static str {
        "data: [DONE]\n\n"
    }

    /// 喂一个 Anthropic SSE 事件，产出 0..n 个 OpenAI chunk JSON 字符串
    pub fn feed(&mut self, model: &str, evt: &Value) -> Result<Vec<String>, String> {
        self.model_name = model.to_string();
        let etype = evt.get("type").and_then(Value::as_str).unwrap_or("");
        match etype {
            "error" => {
                let err = evt.get("error").cloned().unwrap_or(Value::Null);
                let msg = err
                    .get("message")
                    .and_then(Value::as_str)
                    .or_else(|| err.get("type").and_then(Value::as_str))
                    .unwrap_or("上游流式错误")
                    .to_string();
                Err(msg)
            }
            "message_start" => {
                let msg = evt.get("message").cloned().unwrap_or(Value::Null);
                if let Some(id) = msg.get("id").and_then(Value::as_str) {
                    self.chunk_id = id.to_string();
                }
                if let Some(p) = msg.get("usage").and_then(|u| u.get("input_tokens")).and_then(Value::as_i64) {
                    self.usage_prompt = Some(p);
                }
                Ok(vec![])
            }
            "content_block_start" => {
                let block = evt.get("content_block").cloned().unwrap_or(Value::Null);
                if block.get("type").and_then(Value::as_str) == Some("tool_use") {
                    let idx = self.tool_seq;
                    self.tool_seq += 1;
                    let chunk = self.chunk_event(
                        json!({ "tool_calls": [{
                            "index": idx,
                            "id": block.get("id").and_then(Value::as_str).unwrap_or(""),
                            "type": "function",
                            "function": { "name": block.get("name").and_then(Value::as_str).unwrap_or(""), "arguments": "" },
                        }]}),
                        None,
                    );
                    return Ok(vec![sse(&chunk)]);
                }
                Ok(vec![])
            }
            "content_block_delta" => {
                let delta = evt.get("delta").cloned().unwrap_or(Value::Null);
                match delta.get("type").and_then(Value::as_str) {
                    Some("text_delta") => {
                        if let Some(t) = delta.get("text").and_then(Value::as_str) {
                            let chunk = self.chunk_event(json!({ "content": t }), None);
                            return Ok(vec![sse(&chunk)]);
                        }
                        Ok(vec![])
                    }
                    Some("input_json_delta") => {
                        if let Some(p) = delta.get("partial_json").and_then(Value::as_str) {
                            let chunk = self.chunk_event(
                                json!({ "tool_calls": [{
                                    "index": self.tool_seq.saturating_sub(1),
                                    "function": { "arguments": p },
                                }]}),
                                None,
                            );
                            return Ok(vec![sse(&chunk)]);
                        }
                        Ok(vec![])
                    }
                    _ => Ok(vec![]),
                }
            }
            "message_delta" => {
                let delta = evt.get("delta").cloned().unwrap_or(Value::Null);
                let finish = stop_reason_to_finish(delta.get("stop_reason").and_then(Value::as_str));
                self.finish_reason = Some(finish.to_string());
                if let Some(out) = evt.get("usage").and_then(|u| u.get("output_tokens")).and_then(Value::as_i64) {
                    self.usage_completion = Some(out);
                }
                let chunk = self.chunk_event(json!({}), Some(finish));
                Ok(vec![sse(&chunk)])
            }
            _ => {
                // 兼容未带 type 名的上游错误事件
                if let Some(err) = evt.get("error").filter(|v| v.is_object()) {
                    let msg = err
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("上游流式错误")
                        .to_string();
                    return Err(msg);
                }
                Ok(vec![]) // content_block_stop / message_stop / ping 等
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_translation_basic() {
        let payload = json!({
            "model": "GLM-5.3",
            "messages": [
                { "role": "system", "content": "be brief" },
                { "role": "user", "content": "hello" },
                { "role": "assistant", "content": "hi", "tool_calls": [
                    { "id": "c1", "type": "function", "function": { "name": "f", "arguments": "{\"a\":1}" } }
                ]},
                { "role": "tool", "tool_call_id": "c1", "content": "result" }
            ],
            "max_tokens": 100,
            "temperature": 0.5,
            "stop": ["END"],
            "stream": true
        });
        let body = request_to_anthropic(&payload).unwrap();
        assert_eq!(body["system"], "be brief");
        assert_eq!(body["max_tokens"], 100);
        assert_eq!(body["stream"], true);
        assert_eq!(body["stop_sequences"], json!(["END"]));
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(msgs[1]["content"][0]["type"], "text");
        assert_eq!(msgs[1]["content"][1]["type"], "tool_use");
        assert_eq!(msgs[2]["content"][0]["type"], "tool_result");
    }

    #[test]
    fn request_translation_tools_and_choice() {
        let payload = json!({
            "model": "m",
            "messages": [{ "role": "user", "content": "x" }],
            "tools": [{ "type": "function", "function": { "name": "get", "description": "d", "parameters": { "type": "object" } } }],
            "tool_choice": "required",
            "response_format": { "type": "json_object" }
        });
        let body = request_to_anthropic(&payload).unwrap();
        assert_eq!(body["tools"][0]["name"], "get");
        assert_eq!(body["tools"][0]["input_schema"]["type"], "object");
        assert_eq!(body["tool_choice"]["type"], "any");
        assert!(body["system"].as_str().unwrap().contains("JSON object"));
        // tool_choice none 移除 tools
        let p2 = json!({ "model": "m", "messages": [{ "role": "user", "content": "x" }], "tools": [{ "type": "function", "function": { "name": "g" } }], "tool_choice": "none" });
        let b2 = request_to_anthropic(&p2).unwrap();
        assert!(b2.get("tools").is_none());
    }

    #[test]
    fn request_requires_model_and_messages() {
        assert!(request_to_anthropic(&json!({ "messages": [] })).is_err());
        assert!(request_to_anthropic(&json!({ "model": "m" })).is_err());
    }

    #[test]
    fn response_translation_roundtrip() {
        let anthro = json!({
            "id": "msg_1",
            "model": "GLM-5.3",
            "content": [
                { "type": "text", "text": "hi " },
                { "type": "tool_use", "id": "t1", "name": "f", "input": { "a": 1 } }
            ],
            "stop_reason": "tool_use",
            "usage": { "input_tokens": 10, "output_tokens": 5 }
        });
        let out = response_to_openai(&anthro, "GLM-5.3");
        assert_eq!(out["object"], "chat.completion");
        let choice = &out["choices"][0];
        assert_eq!(choice["finish_reason"], "tool_calls");
        assert_eq!(choice["message"]["content"], "hi ");
        assert_eq!(choice["message"]["tool_calls"][0]["function"]["name"], "f");
        assert_eq!(out["usage"]["total_tokens"], 15);
    }

    #[test]
    fn stream_converter_full_pass() {
        let mut conv = StreamConverter::new("GLM-5.3");
        let mut out = vec![];
        out.push(sse(&conv.start_event()));
        for evt in [
            json!({ "type": "message_start", "message": { "id": "msg_x", "usage": { "input_tokens": 7 } } }),
            json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "text" } }),
            json!({ "type": "content_block_delta", "delta": { "type": "text_delta", "text": "hel" } }),
            json!({ "type": "content_block_delta", "delta": { "type": "text_delta", "text": "lo" } }),
            json!({ "type": "content_block_start", "index": 1, "content_block": { "type": "tool_use", "id": "t9", "name": "fn" } }),
            json!({ "type": "content_block_delta", "delta": { "type": "input_json_delta", "partial_json": "{\"x\":" } }),
            json!({ "type": "content_block_delta", "delta": { "type": "input_json_delta", "partial_json": "1}" } }),
            json!({ "type": "message_delta", "delta": { "stop_reason": "tool_use" }, "usage": { "output_tokens": 3 } }),
        ] {
            out.extend(conv.feed("GLM-5.3", &evt).unwrap());
        }
        out.push(conv.done().to_string());
        let all = out.join("");
        assert!(all.starts_with("data: {"));
        assert!(all.contains("\"content\":\"hel\""));
        assert!(all.contains("\"tool_calls\""));
        assert!(all.contains("\"finish_reason\":\"tool_calls\""));
        assert!(all.contains("\"prompt_tokens\":7"));
        assert!(all.contains("\"completion_tokens\":3"));
        assert!(all.ends_with("data: [DONE]\n\n"));
    }

    #[test]
    fn stream_converter_error_event() {
        let mut conv = StreamConverter::new("m");
        let err = conv.feed("m", &json!({ "type": "error", "error": { "type": "api_error", "message": "boom" } }));
        assert_eq!(err.unwrap_err(), "boom");
    }
}
