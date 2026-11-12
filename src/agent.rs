//! 한 턴의 작업 루프: 모델이 도구를 요청하면 실행해서 결과를 돌려주기를, 모델이 일을 끝낼 때까지 반복한다.

use std::io::{BufRead, BufReader};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::llm::{Config, Msg, Stats};

const SYSTEM: &str = include_str!("prompt.md");
const URL: &str = "https://api.anthropic.com/v1/messages";
/// 한 턴에서 도구를 주고받는 횟수의 상한
const MAX_STEPS: usize = 80;

fn body(cfg: &Config, messages: &[Value], fallbacks: bool) -> String {
    let mut v = json!({
        "model": cfg.model,
        "max_tokens": 32000,
        "stream": true,
        "output_config": { "effort": cfg.effort },
        // 지시문은 매 턴 같으므로 여기까지를 캐시한다.
        "system": [{ "type": "text", "text": SYSTEM, "cache_control": { "type": "ephemeral" } }],
        "tools": crate::tools::definitions(),
        // 한 턴 안에서 도구를 여러 번 쓰는 동안 쌓이는 내용도 캐시한다.
        "cache_control": { "type": "ephemeral" },
        "messages": messages,
    });
    if fallbacks {
        // 안전 분류기가 요청을 거절하면 서버가 다른 모델로 이어서 답하게 한다.
        v["fallbacks"] = "default".into();
    }
    v.to_string()
}

/// 한 번의 응답에서 받은 것.
struct Reply {
    /// 받은 내용 블록 전부. 다음 요청에 그대로 돌려보내야 한다 (생각 블록 포함, 고치면 안 됨).
    blocks: Vec<Value>,
    stop: String,
}

pub fn run(cfg: &Config, key: &str, turn_text: &str, send: &dyn Fn(Msg)) -> Result<Stats, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        // OS 의 TLS 구현과 OS 인증서 저장소를 쓴다 (Windows: Schannel, Mac: Secure Transport).
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .provider(ureq::tls::TlsProvider::NativeTls)
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .build(),
        )
        .http_status_as_error(false)
        .build()
        .into();
    let started = Instant::now();
    let mut stats = Stats { model: cfg.model.clone(), ..Default::default() };
    let mut fallbacks = true;
    let mut messages = vec![json!({ "role": "user", "content": turn_text })];

    for _ in 0..MAX_STEPS {
        let reply = stream_once(&agent, cfg, key, &messages, &mut fallbacks, &mut stats, started, send)?;
        send(Msg::Delta("\n".into())); // 응답 사이에 줄이 붙지 않게 한다
        // 빈 글 블록은 되돌려 보낼 수 없다.
        let content: Vec<Value> =
            reply.blocks.iter().filter(|b| !(b["type"] == "text" && b["text"].as_str().unwrap_or("").is_empty())).cloned().collect();
        match reply.stop.as_str() {
            "tool_use" => {
                messages.push(json!({ "role": "assistant", "content": content }));
                let mut results = Vec::new();
                for b in reply.blocks.iter().filter(|b| b["type"] == "tool_use") {
                    let name = b["name"].as_str().unwrap_or("");
                    let input = &b["input"];
                    let (out, is_error) = if let Some(raw) = input["__invalid_json"].as_str() {
                        (json!({ "INVALID_JSON": raw }).to_string(), true)
                    } else if name == "screen" {
                        // 화면은 화면 쪽 스레드가 고친다. 거부된 명령이 있으면 그 이유를 받아 온다.
                        let (rtx, rrx) = std::sync::mpsc::channel();
                        send(Msg::Screen { ops: input["ops"].as_str().unwrap_or("").to_string(), reply: rtx });
                        (rrx.recv_timeout(Duration::from_secs(15)).unwrap_or_else(|_| "반영됨".into()), false)
                    } else if name == "browser_text" || name == "browser_eval" {
                        // 브라우저 화면도 화면 쪽 스레드에 있다. 스크립트를 넘기고 결과를 기다린다.
                        send(Msg::Activity(crate::tools::describe(name, input)));
                        let script = if name == "browser_text" {
                            "document.title + '\\n' + location.href + '\\n\\n' + (document.body ? document.body.innerText : '').slice(0, 30000)".to_string()
                        } else {
                            input["script"].as_str().unwrap_or("").to_string()
                        };
                        let (rtx, rrx) = std::sync::mpsc::channel();
                        send(Msg::Browser { card: input["card"].as_str().unwrap_or("").to_string(), script, reply: rtx });
                        match rrx.recv_timeout(Duration::from_secs(20)) {
                            Ok(out) => (out.clone(), out.starts_with("오류:")),
                            Err(_) => ("오류: 20초 안에 페이지에서 답이 오지 않음".into(), true),
                        }
                    } else {
                        send(Msg::Activity(crate::tools::describe(name, input)));
                        crate::tools::execute(name, input)
                    };
                    results.push(json!({ "type": "tool_result", "tool_use_id": b["id"], "content": out, "is_error": is_error }));
                }
                // 도구 결과는 한 메시지에 모두 담아 돌려준다.
                messages.push(json!({ "role": "user", "content": results }));
                send(Msg::Activity(String::new()));
            }
            // 서버 쪽 도구(웹 검색 등)가 길어져 잠시 멈춘 것: 받은 것을 그대로 붙여 다시 보내면 이어서 한다.
            "pause_turn" => messages.push(json!({ "role": "assistant", "content": content })),
            "refusal" => return Err("모델이 이 요청을 거절했습니다".into()),
            "max_tokens" => return Err("출력이 한도에서 잘렸습니다".into()),
            _ => {
                stats.total_ms = started.elapsed().as_millis() as u64;
                return Ok(stats);
            }
        }
    }
    Err(format!("한 번에 {MAX_STEPS}단계를 넘겨 중단했습니다"))
}

#[allow(clippy::too_many_arguments)]
fn stream_once(
    agent: &ureq::Agent,
    cfg: &Config,
    key: &str,
    messages: &[Value],
    fallbacks: &mut bool,
    stats: &mut Stats,
    started: Instant,
    send: &dyn Fn(Msg),
) -> Result<Reply, String> {
    let post = |fallbacks: bool| {
        let mut req = agent
            .post(URL)
            .header("content-type", "application/json")
            .header("x-api-key", key)
            .header("anthropic-version", "2026-10-21");
        if fallbacks {
            req = req.header("anthropic-beta", "server-side-fallback-2026-07-01");
        }
        req.send(body(cfg, messages, fallbacks)).map_err(|e| format!("연결 실패: {e}"))
    };
    let mut resp = post(*fallbacks)?;
    if resp.status().as_u16() == 400 && *fallbacks {
        let text = resp.body_mut().read_to_string().unwrap_or_default();
        if !text.contains("fallback") {
            return Err(api_error(400, &text));
        }
        *fallbacks = false; // 이 계정·모델에서 fallbacks 가 안 받아들여지면 빼고 다시 보낸다
        resp = post(false)?;
    }
    let status = resp.status().as_u16();
    if status != 200 {
        let text = resp.body_mut().read_to_string().unwrap_or_default();
        return Err(api_error(status, &text));
    }

    let mut blocks: Vec<Value> = Vec::new();
    let mut partial: std::collections::HashMap<usize, String> = Default::default();
    let mut stop = String::new();
    let reader = BufReader::new(resp.into_body().into_reader());
    for line in reader.lines() {
        let line = line.map_err(|e| format!("스트림 끊김: {e}"))?;
        let Some(data) = line.strip_prefix("data:") else { continue };
        let Ok(v) = serde_json::from_str::<Value>(data.trim()) else { continue };
        let idx = v["index"].as_u64().unwrap_or(0) as usize;
        match v["type"].as_str().unwrap_or("") {
            "message_start" => {
                let u = &v["message"]["usage"];
                stats.cached_tokens = u["cache_read_input_tokens"].as_u64().unwrap_or(0);
                stats.input_tokens =
                    u["input_tokens"].as_u64().unwrap_or(0) + stats.cached_tokens + u["cache_creation_input_tokens"].as_u64().unwrap_or(0);
                if let Some(m) = v["message"]["model"].as_str() {
                    stats.model = m.to_string();
                }
            }
            "content_block_start" => {
                if blocks.len() <= idx {
                    blocks.resize(idx + 1, Value::Null);
                }
                blocks[idx] = v["content_block"].clone();
                match blocks[idx]["type"].as_str() {
                    Some("tool_use") => {
                        partial.insert(idx, String::new());
                    }
                    Some("server_tool_use") => {
                        partial.insert(idx, String::new());
                        let what = if blocks[idx]["name"] == "web_fetch" { "웹 페이지 읽는 중" } else { "웹 검색 중" };
                        send(Msg::Activity(what.into()));
                    }
                    _ => {}
                }
            }
            "content_block_delta" if idx < blocks.len() => {
                let d = &v["delta"];
                let append = |b: &mut Value, field: &str, more: &str| {
                    let cur = b[field].as_str().unwrap_or("").to_string();
                    b[field] = Value::String(cur + more);
                };
                match d["type"].as_str().unwrap_or("") {
                    "text_delta" => {
                        let t = d["text"].as_str().unwrap_or("");
                        if stats.first_ms == 0 {
                            stats.first_ms = started.elapsed().as_millis() as u64;
                        }
                        append(&mut blocks[idx], "text", t);
                        send(Msg::Delta(t.to_string()));
                    }
                    "input_json_delta" => {
                        if let Some(p) = partial.get_mut(&idx) {
                            p.push_str(d["partial_json"].as_str().unwrap_or(""));
                        }
                    }
                    "thinking_delta" => append(&mut blocks[idx], "thinking", d["thinking"].as_str().unwrap_or("")),
                    "signature_delta" => blocks[idx]["signature"] = d["signature"].clone(),
                    "citations_delta" => {
                        if !blocks[idx]["citations"].is_array() {
                            blocks[idx]["citations"] = json!([]);
                        }
                        blocks[idx]["citations"].as_array_mut().unwrap().push(d["citation"].clone());
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
                if let Some(raw) = partial.remove(&idx) {
                    blocks[idx]["input"] = if raw.trim().is_empty() {
                        json!({})
                    } else {
                        serde_json::from_str(&raw).unwrap_or_else(|_| json!({ "__invalid_json": raw }))
                    };
                }
            }
            "message_delta" => {
                if let Some(n) = v["usage"]["output_tokens"].as_u64() {
                    stats.output_tokens += n;
                }
                if let Some(s) = v["delta"]["stop_reason"].as_str() {
                    stop = s.to_string();
                }
            }
            "error" => return Err(format!("API 오류: {}", v["error"]["message"].as_str().unwrap_or("알 수 없음"))),
            _ => {}
        }
    }
    blocks.retain(|b| !b.is_null());
    Ok(Reply { blocks, stop })
}

fn api_error(status: u16, text: &str) -> String {
    let msg = serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
        .unwrap_or_else(|| text.chars().take(300).collect());
    format!("HTTP {status}: {msg}")
}
