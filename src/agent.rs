//! 한 턴의 작업 루프: 모델이 도구를 요청하면 실행해서 결과를 돌려주기를, 모델이 일을 끝낼 때까지 반복한다.

use std::io::{BufRead, BufReader};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::llm::{Config, Msg, Stats};

/// 지시문. 부품 표는 등록된 부품 목록에서 만들어 끼워 넣는다 (프로그램이 떠 있는 동안 한 번만 만든다: 바뀌면 캐시가 깨진다).
pub fn system() -> &'static str {
    static BUILT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    BUILT.get_or_init(|| include_str!("prompt.md").replace("{{PARTS}}", crate::core::parts_table().trim_end()))
}
/// 사용자가 멈춰서 턴이 끝났다는 표시
pub const STOPPED: &str = "__stopped";
const URL: &str = "https://api.anthropic.com/v1/messages";
/// 한 턴에서 도구를 주고받는 횟수의 상한
const MAX_STEPS: usize = 80;

/// 누가 일하는가: 메인(화면을 고치는 쪽)인지 맡은 작업자인지에 따라 지시문과 도구가 다르다.
pub struct Role<'a> {
    pub system: &'a str,
    pub tools: Value,
    pub max_steps: usize,
    /// 도구를 한 차례 쓴 뒤마다 불린다. Err 면 거기서 멈추고, Ok(Some(글)) 이면 그 글을 모델에게 함께 전한다.
    pub check: &'a dyn Fn() -> Result<Option<String>, String>,
    /// 이 역할만의 도구 처리. None 이면 공통 도구로 실행한다.
    pub tool: &'a dyn Fn(&str, &Value) -> Option<(String, bool)>,
}

pub fn http() -> ureq::Agent {
    ureq::Agent::config_builder()
        // OS 의 TLS 구현과 OS 인증서 저장소를 쓴다 (Windows: Schannel, Mac: Secure Transport).
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .provider(ureq::tls::TlsProvider::NativeTls)
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .build(),
        )
        .http_status_as_error(false)
        .build()
        .into()
}

/// 도구 없이 한 번 묻고 답을 글로 받는다 (기억 정리 같은 작은 뒷일에 쓴다).
pub fn ask(key: &str, model: &str, system: &str, user: &str, max_tokens: u32) -> Result<String, String> {
    let body = json!({ "model": model, "max_tokens": max_tokens, "system": system, "messages": [{ "role": "user", "content": user }] }).to_string();
    let mut resp = http()
        .post(URL)
        .header("content-type", "application/json")
        .header("x-api-key", key)
        .header("anthropic-version", "2026-10-21")
        .send(body)
        .map_err(|e| format!("연결 실패: {e}"))?;
    let status = resp.status().as_u16();
    let text = resp.body_mut().read_to_string().unwrap_or_default();
    if status != 200 {
        return Err(api_error(status, &text));
    }
    let v: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    Ok(v["content"].as_array().map(|a| a.iter().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("")).unwrap_or_default())
}

fn body(cfg: &Config, role: &Role, messages: &[Value], fallbacks: bool, drop_block: bool) -> String {
    let mut v = json!({
        "model": cfg.model,
        "max_tokens": 32000,
        "stream": true,
        "output_config": { "effort": cfg.effort },
        // 지시문은 매 턴 같으므로 여기까지를 캐시한다.
        "system": [{ "type": "text", "text": role.system, "cache_control": { "type": "ephemeral" } }],
        "tools": role.tools,
        // 한 턴 안에서 도구를 여러 번 쓰는 동안 쌓이는 내용도 캐시한다.
        "cache_control": { "type": "ephemeral" },
        "messages": messages,
    });
    if drop_block {
        // 앞선 생각 블록이 "대화가 달라졌다"는 이유로 거부됐을 때: 그 블록만 버리고 이어 가게 한다.
        v["thinking"] = json!({ "type": "adaptive", "block_binding": { "prefix_mismatch_behavior": "drop_block" } });
    }
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

/// `images` 는 사용자가 이번 말에 붙인 사진 (형식, base64).
/// `added` 는 일하는 도중에 사용자가 덧붙인 말을 꺼내 오는 함수 (없으면 None). 도구를 한 차례 쓴 뒤마다 불린다.
pub fn run(cfg: &Config, key: &str, turn_text: &str, images: &[(String, String)], added: &dyn Fn() -> Result<Option<String>, String>, send: &dyn Fn(Msg)) -> Result<Stats, String> {
    let role = Role { system: system(), tools: crate::tools::definitions(false), max_steps: MAX_STEPS, check: &|| added(), tool: &|_, _| None };
    run_as(cfg, key, &role, turn_text, images, send).map(|(stats, _)| stats)
}

/// 모델이 일을 끝낼 때까지 돌린다. 마지막 응답의 글도 함께 돌려준다 (작업자의 보고서).
pub fn run_as(cfg: &Config, key: &str, role: &Role, turn_text: &str, images: &[(String, String)], send: &dyn Fn(Msg)) -> Result<(Stats, String), String> {
    let agent = http();
    let started = Instant::now();
    let mut stats = Stats { model: cfg.model.clone(), ..Default::default() };
    let mut fallbacks = true;
    let mut drop_block = false;
    let mut messages = if images.is_empty() {
        vec![json!({ "role": "user", "content": turn_text })]
    } else {
        // 사진을 먼저, 글을 뒤에 둔다.
        let mut content: Vec<Value> =
            images.iter().map(|(media, data)| json!({ "type": "image", "source": { "type": "base64", "media_type": media, "data": data } })).collect();
        content.push(json!({ "type": "text", "text": turn_text }));
        vec![json!({ "role": "user", "content": content })]
    };

    for _ in 0..role.max_steps {
        let reply = stream_once(&agent, cfg, key, role, &messages, &mut fallbacks, &mut drop_block, &mut stats, started, send)?;
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
                    } else if name == "view_image" {
                        // 그림 파일을 열어 그림 그대로 모델에게 보여 준다.
                        send(Msg::Activity(crate::tools::describe(name, input)));
                        match crate::pics::from_file(input["path"].as_str().unwrap_or("")) {
                            Ok(pic) => {
                                results.push(json!({ "type": "tool_result", "tool_use_id": b["id"], "content": [
                                    { "type": "image", "source": { "type": "base64", "media_type": pic.media, "data": pic.b64() } },
                                    { "type": "text", "text": format!("{}x{} 픽셀로 보냄", pic.w, pic.h) },
                                ] }));
                                continue;
                            }
                            Err(e) => (e, true),
                        }
                    } else if name == "board_read" {
                        send(Msg::Activity(crate::tools::describe(name, input)));
                        let (rtx, rrx) = std::sync::mpsc::channel();
                        send(Msg::Board { topic: input["topic"].as_str().unwrap_or("").trim().to_string(), reply: rtx });
                        match rrx.recv_timeout(Duration::from_secs(15)) {
                            Ok(out) => (out.clone(), out.starts_with("오류:")),
                            Err(_) => ("오류: 보드를 읽지 못함".into(), true),
                        }
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
                        (role.tool)(name, input).unwrap_or_else(|| crate::tools::execute(name, input))
                    };
                    results.push(json!({ "type": "tool_result", "tool_use_id": b["id"], "content": out, "is_error": is_error }));
                }
                // 멈추라는 신호나 중간에 전할 말이 있는지 본다.
                if let Some(more) = (role.check)()? {
                    results.push(json!({ "type": "text", "text": more }));
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
                let text: Vec<&str> = content.iter().filter_map(|b| if b["type"] == "text" { b["text"].as_str() } else { None }).collect();
                return Ok((stats, text.join("\n")));
            }
        }
    }
    Err(format!("한 번에 {}단계를 넘겨 중단했습니다", role.max_steps))
}

#[allow(clippy::too_many_arguments)]
fn stream_once(
    agent: &ureq::Agent,
    cfg: &Config,
    key: &str,
    role: &Role,
    messages: &[Value],
    fallbacks: &mut bool,
    drop_block: &mut bool,
    stats: &mut Stats,
    started: Instant,
    send: &dyn Fn(Msg),
) -> Result<Reply, String> {
    let post = |fallbacks: bool, drop_block: bool| {
        let mut req = agent
            .post(URL)
            .header("content-type", "application/json")
            .header("x-api-key", key)
            .header("anthropic-version", "2026-10-21");
        let betas: Vec<&str> = [(fallbacks, "server-side-fallback-2026-07-01"), (drop_block, "thinking-binding-controls-2026-08-01")].iter().filter(|b| b.0).map(|b| b.1).collect();
        if !betas.is_empty() {
            req = req.header("anthropic-beta", betas.join(","));
        }
        req.send(body(cfg, role, messages, fallbacks, drop_block)).map_err(|e| format!("연결 실패: {e}"))
    };
    let mut resp = post(*fallbacks, *drop_block)?;
    for _ in 0..2 {
        if resp.status().as_u16() != 400 {
            break;
        }
        let text = resp.body_mut().read_to_string().unwrap_or_default();
        if !*drop_block && text.contains("block_binding") {
            // 앞서 받은 생각 블록이 "그때와 대화 내용이 다르다"고 거부됐다. 그 블록만 버리게 하고 이어 간다 (이 턴이 끝날 때까지 유지).
            // 원인을 찾을 수 있게 보낸 내용을 남긴다.
            crate::sys::log("api.log", &format!("생각 블록 거부, 버리고 다시 보냄: {}", api_error(400, &text)));
            let _ = std::fs::write(crate::sys::data_dir().join("bind-error.json"), json!({ "error": text, "messages": messages }).to_string());
            *drop_block = true;
        } else if *fallbacks && text.contains("fallback") {
            *fallbacks = false; // 이 계정·모델에서 fallbacks 가 안 받아들여지면 빼고 다시 보낸다
        } else {
            return Err(api_error(400, &text));
        }
        resp = post(*fallbacks, *drop_block)?;
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
        if crate::tools::stopped() {
            return Err(STOPPED.into());
        }
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
                    if raw.trim().is_empty() {
                        // 입력이 조각으로 오지 않았으면 처음 받은 것을 그대로 둔다 (지우면 받은 내용을 고친 것이 되어 다음 요청이 거부된다).
                        if !blocks[idx]["input"].is_object() {
                            blocks[idx]["input"] = json!({});
                        }
                    } else {
                        blocks[idx]["input"] = serde_json::from_str(&raw).unwrap_or_else(|_| json!({ "__invalid_json": raw }));
                    }
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
