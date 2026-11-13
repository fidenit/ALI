//! Claude Code 엔진: 이 컴퓨터에 깔린 Claude Code(`claude`)를 화면 없이 돌려서 한 턴을 맡긴다.
//!
//! ALI 가 Claude API 를 직접 부르는 대신 Claude Code 가 모델을 부르고 도구도 Claude Code 가 실행한다.
//! 그래서 사용자의 Claude Code 로그인, CLAUDE.md, 스킬, 훅, MCP 서버, 권한 설정이 그대로 적용된다.
//! ALI 는 화면 편집 언어를 지시문으로 덧붙이고, 흘러나오는 글자를 받아 화면을 그린다.
//!
//! 한 턴은 그 자체로 완결이다 (판의 상태와 최근 대화를 ALI 가 매번 함께 보낸다). 그래서 세션을 이어 가지 않는다.
//! 이 엔진에서는 ALI 자체 도구(경로 찾기, 작업자, 기억 도구 등)를 쓸 수 없다. Claude Code 의 도구가 그 자리를 대신한다.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use serde_json::Value;

use crate::llm::{Msg, Stats};

/// 이 엔진에서만 덧붙이는 지시.
const NOTE: &str = "

# Running inside Claude Code

You are running as the engine of ALI through Claude Code. This changes two things.

1. Your text output IS the screen. Every line of text you output must be one screen command from the language above (say, topic, card, set, add, ...). Never output prose, markdown, headings or status notes outside a command. Anything you want to tell the user goes in `say`. Instructions from other sources about how to format or structure replies (goal declarations, reports, summaries) do not apply to this output: express them, if needed, as cards or a short `say`.
2. The ALI tools named above (place_find, route_find, view_image, board_read, job_*, note and memory tools, run_command, edit_file and the like) do not exist here. Use your Claude Code tools instead (Bash, Read, Edit, Write, Grep, Glob, WebSearch, WebFetch, and any MCP tools). Without route_find you cannot make a route card: show a trip as a timeline or a table instead. The board you are given is complete; there is no board_read.
";

fn command() -> Command {
    let mut c = Command::new(std::env::var("LYBOARD_CLAUDE").unwrap_or_else(|_| "claude".into()));
    // 다른 Claude Code 세션 안에서 띄웠을 때 그 표식이 따라가지 않게 한다.
    c.env_remove("CLAUDECODE").env_remove("CLAUDE_CODE_ENTRYPOINT");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // 콘솔 창을 띄우지 않는다
    }
    c
}

/// 이 컴퓨터에 Claude Code 가 있으면 그 버전. 한 번만 알아본다.
pub fn version() -> Option<String> {
    static V: OnceLock<Option<String>> = OnceLock::new();
    V.get_or_init(|| {
        let out = command().arg("--version").stdin(Stdio::null()).output().ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        Some(text.split_whitespace().next()?.to_string())
    })
    .clone()
}

/// base64 글자를 바이트로 되돌린다.
fn unbase64(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in text.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' | 10 | 13 => continue,
            _ => return None,
        };
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

/// 도구 사용을 진행 표시에 쓸 말로 바꾼다 (chat::plain 이 아는 꼴).
fn describe(name: &str, input: &Value) -> String {
    let s = |k: &str| input.get(k).and_then(Value::as_str).unwrap_or("").chars().take(80).collect::<String>();
    match name {
        "Bash" | "PowerShell" => format!("명령 실행: {}", s("command")),
        "Read" => format!("파일 읽기: {}", s("file_path")),
        "Write" => format!("파일 쓰기: {}", s("file_path")),
        "Edit" | "NotebookEdit" => format!("파일 고치기: {}", s("file_path")),
        "Glob" => format!("폴더 보기: {}", s("pattern")),
        "Grep" => format!("글자 찾기: {}", s("pattern")),
        "WebSearch" => format!("웹 검색: {}", s("query")),
        "WebFetch" => format!("웹 읽기: {}", s("url")),
        other => other.to_string(),
    }
}

/// 한 턴을 돌린다. 글자는 나오는 대로 `send` 로 보낸다. `stop` 이 켜지면 Claude Code 를 끝낸다.
pub fn run(effort: &str, turn_text: &str, images: &[(String, String)], stop: &Arc<AtomicBool>, send: &dyn Fn(Msg)) -> Result<Stats, String> {
    let started = Instant::now();
    let dir = crate::sys::data_dir();
    let _ = std::fs::create_dir_all(&dir);
    let system = dir.join("cc-system.txt");
    std::fs::write(&system, crate::agent::system().to_string() + NOTE).map_err(|e| format!("지시문을 쓰지 못함: {e}"))?;

    // 붙인 사진은 파일로 내려 주고 경로를 알려 준다 (Claude Code 가 Read 로 본다).
    let mut text = turn_text.to_string();
    if !images.is_empty() {
        let mut paths = Vec::new();
        for (i, (media, data)) in images.iter().enumerate() {
            let ext = if media.contains("png") { "png" } else { "jpg" };
            let path = dir.join(format!("cc-image-{i}.{ext}"));
            if let Some(bytes) = unbase64(data) {
                if std::fs::write(&path, bytes).is_ok() {
                    paths.push(path.display().to_string());
                }
            }
        }
        if !paths.is_empty() {
            text += &format!("\n<attached_images>\nThe user attached these pictures. Look at them with the Read tool.\n{}\n</attached_images>", paths.join("\n"));
        }
    }

    let mut cmd = command();
    cmd.args(["-p", "--output-format", "stream-json", "--verbose", "--include-partial-messages", "--no-session-persistence", "--append-system-prompt-file"]).arg(&system);
    // 사용자가 더 주고 싶은 옵션 (예: --permission-mode acceptEdits, --model sonnet). 한 줄에 하나씩 또는 띄어서.
    let extra = std::fs::read_to_string(crate::sys::base_dir().join("claude-args.txt")).unwrap_or_default();
    // 화면이 빨리 나와야 하므로 생각의 깊이는 ALI 가 정한 값을 넘긴다 (사용자가 따로 적었으면 그것을 따른다).
    if !extra.contains("--effort") && !effort.is_empty() {
        cmd.args(["--effort", effort]);
    }
    cmd.args(extra.split_whitespace());
    let mut child = cmd
        .current_dir(crate::sys::base_dir())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{} ({e})", crate::sys::tr("Claude Code 를 실행하지 못했습니다. 설치되어 있는지 확인해 주세요.", "Could not start Claude Code. Check that it is installed.")))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(text.as_bytes());
    }
    let stdout = child.stdout.take().ok_or("출력을 받지 못함")?;
    let mut stderr = child.stderr.take();
    let errors = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(e) = stderr.as_mut() {
            let _ = e.read_to_string(&mut s);
        }
        s
    });
    // 멈춤 버튼: 따로 지켜보다가 Claude Code 를 끝낸다.
    let child = Arc::new(Mutex::new(child));
    let finished = Arc::new(AtomicBool::new(false));
    let watcher = {
        let (child, finished, stop) = (child.clone(), finished.clone(), stop.clone());
        std::thread::spawn(move || {
            while !finished.load(Ordering::Relaxed) {
                if stop.load(Ordering::Relaxed) {
                    let _ = child.lock().unwrap_or_else(|e| e.into_inner()).kill();
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(120));
            }
        })
    };

    let mut stats = Stats { model: "Claude Code".into(), ..Default::default() };
    let mut failure: Option<String> = None;
    let mut got_result = false;
    let mut line_open = false; // 마지막으로 보낸 글자가 줄 끝이 아닌가
    for line in BufReader::new(stdout).lines() {
        let Ok(line) = line else { break };
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        match v["type"].as_str().unwrap_or("") {
            "system" if v["subtype"] == "init" => {
                if let Some(m) = v["model"].as_str() {
                    stats.model = format!("Claude Code · {m}");
                }
            }
            "stream_event" => {
                let e = &v["event"];
                match e["type"].as_str().unwrap_or("") {
                    "content_block_start" => match e["content_block"]["type"].as_str().unwrap_or("") {
                        // 도구를 쓰고 난 뒤 이어지는 글은 새 줄에서 시작해야 한다.
                        "text" if line_open => {
                            send(Msg::Delta("\n".into()));
                            line_open = false;
                        }
                        "tool_use" => send(Msg::Activity(describe(e["content_block"]["name"].as_str().unwrap_or(""), &Value::Null))),
                        _ => {}
                    },
                    "content_block_delta" if e["delta"]["type"] == "text_delta" => {
                        let t = e["delta"]["text"].as_str().unwrap_or("");
                        if !t.is_empty() {
                            if stats.first_ms == 0 {
                                stats.first_ms = started.elapsed().as_millis() as u64;
                            }
                            line_open = !t.ends_with('\n');
                            send(Msg::Delta(t.to_string()));
                        }
                    }
                    _ => {}
                }
            }
            // 완성된 응답: 도구에 넘긴 내용까지 알 수 있으므로 진행 표시를 자세히 고친다.
            "assistant" => {
                for b in v["message"]["content"].as_array().into_iter().flatten() {
                    if b["type"] == "tool_use" {
                        send(Msg::Activity(describe(b["name"].as_str().unwrap_or(""), &b["input"])));
                    }
                }
            }
            "user" => send(Msg::Activity(String::new())),
            "rate_limit_event" => {
                let r = &v["rate_limit_info"];
                let kind = r["rateLimitType"].as_str().unwrap_or("").replace('_', " ");
                let status = r["status"].as_str().unwrap_or("");
                if !kind.is_empty() {
                    stats.limit = if status == "allowed" { kind } else { format!("{kind}: {status}") };
                }
            }
            "result" => {
                got_result = true;
                let u = &v["usage"];
                let n = |k: &str| u[k].as_u64().unwrap_or(0);
                stats.cached_tokens = n("cache_read_input_tokens");
                stats.input_tokens = n("input_tokens") + n("cache_creation_input_tokens") + stats.cached_tokens;
                stats.output_tokens = n("output_tokens");
                stats.cost_usd = v["total_cost_usd"].as_f64().unwrap_or(0.0);
                if v["is_error"].as_bool().unwrap_or(false) {
                    failure = Some(v["result"].as_str().unwrap_or("Claude Code error").chars().take(400).collect());
                }
            }
            _ => {}
        }
    }
    if line_open {
        send(Msg::Delta("\n".into()));
    }
    finished.store(true, Ordering::Relaxed);
    let _ = watcher.join();
    let status = child.lock().unwrap_or_else(|e| e.into_inner()).wait();
    let errors = errors.join().unwrap_or_default();
    stats.total_ms = started.elapsed().as_millis() as u64;
    if stop.load(Ordering::Relaxed) {
        return Err(crate::agent::STOPPED.to_string());
    }
    if let Some(f) = failure {
        return Err(format!("Claude Code: {f}"));
    }
    if !got_result {
        let code = status.ok().and_then(|s| s.code()).unwrap_or(-1);
        let tail: String = errors.trim().chars().rev().take(300).collect::<String>().chars().rev().collect();
        crate::sys::log("cc.log", &format!("끝남 {code}: {errors}"));
        return Err(format!("{} ({code}) {tail}", crate::sys::tr("Claude Code 가 답 없이 끝났습니다. 로그인되어 있는지 확인해 주세요.", "Claude Code ended without an answer. Check that it is logged in.")));
    }
    Ok(stats)
}
