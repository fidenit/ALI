//! Claude Code 엔진: 이 컴퓨터에 깔린 Claude Code(`claude`)를 화면 없이 돌려서 한 턴을 맡긴다.
//!
//! ALI 가 Claude API 를 직접 부르는 대신 Claude Code 가 모델을 부르고 도구도 Claude Code 가 실행한다.
//! 그래서 사용자의 Claude Code 로그인, CLAUDE.md, 스킬, 훅, MCP 서버, 권한 설정이 그대로 적용된다.
//! ALI 는 화면 편집 언어를 지시문으로 덧붙이고, 흘러나오는 글자를 받아 화면을 그린다.
//!
//! 한 턴은 그 자체로 완결이다 (판의 상태와 최근 대화를 ALI 가 매번 함께 보낸다). 그래서 세션을 이어 가지 않는다.
//! 파일·명령·웹 도구는 Claude Code 것을 쓰고, ALI 자체 도구(경로 찾기, 기억 노트, 판 읽기, 작업자)는
//! 이 파일 아래쪽의 작은 MCP 서버로 내준다.

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
2. ALI's own tools are reached through the MCP server named `ali`: where the text above names route_find, place_find, note_search, note_read, note_write, note_append, board_read, browser_text, browser_eval, job_start, job_list, job_tell, job_stop or job_result, call `mcp__ali__<name>` with the same input. The other ALI tools named above (screen, shell, read_file, write_file, edit_file, list_dir, find_text, view_image, web_search, web_fetch) do not exist here: use your Claude Code tools instead (Bash or PowerShell, Read, Write, Edit, Glob, Grep, WebSearch, WebFetch). There is no `screen` tool because your text output already is the screen.
3. If a tool is refused for lack of permission, do not retry it and do not ask the user to grant it in a card: say in one short `say` that Claude Code's permission setting blocked it and that it can be changed under Keys at the bottom left.
4. Before your first tool call, output one short `say` line telling the user what you are about to do, so the screen is not silent while tools run.
";

fn launch(program: &str) -> Command {
    let mut c = Command::new(program);
    // 다른 Claude Code 세션 안에서 띄웠을 때 그 표식이 따라가지 않게 한다.
    c.env_remove("CLAUDECODE").env_remove("CLAUDE_CODE_ENTRYPOINT");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // 콘솔 창을 띄우지 않는다
    }
    c
}

/// 이 컴퓨터의 Claude Code: (실행할 이름이나 경로, 버전). 한 번만 알아본다.
/// PATH 에서 먼저 찾고, 없으면 흔한 설치 자리를 본다. 화면에서 바로 띄운 프로그램은 터미널과 PATH 가 달라서
/// (특히 macOS) 터미널에서는 되는 `claude` 가 안 보일 수 있다. Windows 에서 npm 으로 깐 것은 claude.cmd 다.
fn found() -> Option<(String, String)> {
    static V: OnceLock<Option<(String, String)>> = OnceLock::new();
    V.get_or_init(|| {
        let mut places: Vec<String> = Vec::new();
        match std::env::var("LYBOARD_CLAUDE") {
            Ok(p) => places.push(p),
            Err(_) => {
                places.push("claude".into());
                let home = std::env::var(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).unwrap_or_default();
                if cfg!(windows) {
                    places.push("claude.cmd".into());
                    places.push(format!("{home}\\.local\\bin\\claude.exe"));
                    places.push(format!("{}\\npm\\claude.cmd", std::env::var("APPDATA").unwrap_or_default()));
                } else {
                    places.extend([format!("{home}/.local/bin/claude"), format!("{home}/.claude/local/claude"), "/opt/homebrew/bin/claude".into(), "/usr/local/bin/claude".into()]);
                }
            }
        }
        places.into_iter().find_map(|p| {
            let out = launch(&p).arg("--version").stdin(Stdio::null()).output().ok()?;
            if !out.status.success() {
                return None;
            }
            let version = String::from_utf8_lossy(&out.stdout).split_whitespace().next()?.to_string();
            Some((p, version))
        })
    })
    .clone()
}

/// 키 창에서 고른 권한 방식: default(Claude Code 설정 그대로), acceptEdits(파일 수정 허용), bypassPermissions(전부 허용).
pub fn permission() -> String {
    let saved = std::fs::read_to_string(crate::sys::base_dir().join("claude-permission.txt")).unwrap_or_default();
    match saved.trim() {
        "acceptEdits" => "acceptEdits".into(),
        "bypassPermissions" => "bypassPermissions".into(),
        _ => "default".into(),
    }
}

fn command() -> Command {
    // 못 찾았으면 지정한 이름(없으면 claude)으로 띄워 본다. 실패하면 그 오류가 사용자에게 보인다.
    launch(&found().map(|f| f.0).unwrap_or_else(|| std::env::var("LYBOARD_CLAUDE").unwrap_or_else(|_| "claude".into())))
}

/// 이 컴퓨터에 Claude Code 가 있으면 그 버전.
pub fn version() -> Option<String> {
    found().map(|f| f.1)
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
    // Claude Code 의 도구는 꼭 필요한 것만 싣는다. 전부 실으면 턴마다 보내는 양이 늘어 첫 반응이 늦어진다 (실측 10.6초 → 6.4초).
    // 사용자가 붙여 둔 MCP 서버의 도구는 그대로 남는다. ToolSearch 를 빼면 안 된다: 그것이 없으면 MCP 도구의 설명이
    // 전부 매 턴 실려 간다 (도구 200개짜리 서버가 붙은 PC에서 턴당 39만 토큰 → 4만 토큰).
    if !extra.contains("--tools") {
        cmd.args(["--tools", if cfg!(windows) { "Bash,PowerShell,Read,Edit,Write,Glob,Grep,WebSearch,WebFetch,ToolSearch" } else { "Bash,Read,Edit,Write,Glob,Grep,WebSearch,WebFetch,ToolSearch" }]);
    }
    // 권한: 기본은 사용자의 Claude Code 설정 그대로다 (허용해 두지 않은 파일 수정과 명령은 거절된다).
    // 키 창에서 풀어 줄 수 있다.
    let mode = permission();
    if !extra.contains("--permission-mode") && mode != "default" {
        cmd.args(["--permission-mode", &mode]);
    }
    // ALI 자체 도구를 MCP 로 내준다.
    if let Some(config) = mcp_config(stop) {
        cmd.arg("--mcp-config").arg(config).args(["--allowedTools", "mcp__ali"]);
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
                crate::sys::log("cc.log", &format!("결과 turns={} usage={} model_usage={}", v["num_turns"], v["usage"], v["modelUsage"]));
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

// ---------------------------------------------------------------- ALI 도구를 내주는 MCP 서버
//
// Claude Code 는 다른 프로세스라서 ALI 의 도구(경로 찾기, 기억 노트, 판 읽기, 작업자)를 직접 부를 수 없다.
// 그래서 ALI 안에 작은 MCP 서버를 띄우고 (이 컴퓨터 안에서만 열리는 주소, 실행할 때마다 바뀌는 암호),
// Claude Code 에게 그 주소를 알려 준다. 도구는 ALI 프로세스 안에서 실행되므로 화면 쪽과도 그대로 이어진다.

/// Claude Code 에 내주는 도구. 파일·명령·웹 도구는 Claude Code 것이 있으므로 내주지 않는다.
const SHARED: [&str; 14] = [
    "route_find", "place_find", "note_search", "note_read", "note_write", "note_append", "board_read", "browser_text", "browser_eval", "job_start", "job_list", "job_tell", "job_stop",
    "job_result",
];

/// 지금 턴의 멈춤 신호 (도구가 오래 걸릴 때 멈춤 버튼이 듣게 한다).
static STOP: Mutex<Option<Arc<AtomicBool>>> = Mutex::new(None);

/// 서버를 (처음이면) 띄우고, Claude Code 에 넘길 설정 파일의 경로를 돌려준다.
fn mcp_config(stop: &Arc<AtomicBool>) -> Option<std::path::PathBuf> {
    *STOP.lock().unwrap_or_else(|e| e.into_inner()) = Some(stop.clone());
    static SERVER: OnceLock<Option<(u16, String)>> = OnceLock::new();
    let (port, token) = SERVER
        .get_or_init(|| {
            let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).ok()?;
            let port = listener.local_addr().ok()?.port();
            // 암호: 이 컴퓨터의 다른 프로그램이 주소를 알아내도 도구를 부르지 못하게 한다.
            use std::hash::{BuildHasher, Hasher};
            let token: String = (0..4).map(|_| format!("{:016x}", std::collections::hash_map::RandomState::new().build_hasher().finish())).collect();
            let expect = format!("Bearer {token}");
            std::thread::spawn(move || {
                for conn in listener.incoming().flatten() {
                    let expect = expect.clone();
                    std::thread::spawn(move || serve(conn, &expect));
                }
            });
            Some((port, token))
        })
        .clone()?;
    let path = crate::sys::data_dir().join("cc-mcp.json");
    let config = serde_json::json!({ "mcpServers": { "ali": { "type": "http", "url": format!("http://127.0.0.1:{port}/mcp"), "headers": { "Authorization": format!("Bearer {token}") } } } });
    std::fs::write(&path, config.to_string()).ok()?;
    Some(path)
}

/// 요청 하나를 받아 답한다 (연결 하나에 요청 하나).
fn serve(mut conn: std::net::TcpStream, expect: &str) {
    let _ = conn.set_read_timeout(Some(std::time::Duration::from_secs(20)));
    let mut reader = BufReader::new(match conn.try_clone() {
        Ok(c) => c,
        Err(_) => return,
    });
    let mut first = String::new();
    if reader.read_line(&mut first).is_err() {
        return;
    }
    let (mut length, mut authorized) = (0usize, false);
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            match k.trim().to_ascii_lowercase().as_str() {
                "content-length" => length = v.trim().parse().unwrap_or(0),
                "authorization" => authorized = v.trim() == expect,
                _ => {}
            }
        }
    }
    let mut reply = |status: &str, body: &str| {
        let _ = write!(conn, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        let _ = conn.flush();
    };
    // 요청을 끝까지 읽고 나서 답한다. 읽기 전에 닫으면 상대는 거절 대신 끊긴 연결을 본다.
    let mut body = vec![0u8; length.min(4_000_000)];
    if reader.read_exact(&mut body).is_err() {
        return reply("400 Bad Request", "{}");
    }
    if !authorized {
        return reply("401 Unauthorized", "{}");
    }
    if !first.starts_with("POST") {
        return reply("405 Method Not Allowed", "{}");
    }
    let Ok(req) = serde_json::from_slice::<Value>(&body) else { return reply("400 Bad Request", "{}") };
    // 알림(id 없음)에는 내용 없이 받았다고만 답한다.
    if req.get("id").is_none() {
        return reply("202 Accepted", "");
    }
    let result = match req["method"].as_str().unwrap_or("") {
        "initialize" => serde_json::json!({
            "protocolVersion": req["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18"),
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "ali", "version": env!("CARGO_PKG_VERSION") },
        }),
        "ping" => serde_json::json!({}),
        "tools/list" => {
            let all = crate::tools::definitions(false);
            let tools: Vec<Value> = all
                .as_array()
                .into_iter()
                .flatten()
                .filter(|t| SHARED.contains(&t["name"].as_str().unwrap_or("")))
                .map(|t| serde_json::json!({ "name": t["name"], "description": t["description"], "inputSchema": t["input_schema"] }))
                .collect();
            serde_json::json!({ "tools": tools })
        }
        "tools/call" => {
            let name = req["params"]["name"].as_str().unwrap_or("");
            let input = &req["params"]["arguments"];
            let (text, is_error) = if SHARED.contains(&name) { call(name, input) } else { (format!("없는 도구: {name}"), true) };
            serde_json::json!({ "content": [{ "type": "text", "text": text }], "isError": is_error })
        }
        other => {
            let e = serde_json::json!({ "jsonrpc": "2.0", "id": req["id"], "error": { "code": -32601, "message": format!("unknown method {other}") } });
            return reply("200 OK", &e.to_string());
        }
    };
    reply("200 OK", &serde_json::json!({ "jsonrpc": "2.0", "id": req["id"], "result": result }).to_string());
}

/// 도구 하나를 실행한다. 화면 쪽에 있는 것(판, 브라우저 카드)은 화면 스레드에 물어본다.
fn call(name: &str, input: &Value) -> (String, bool) {
    if let Some(stop) = STOP.lock().unwrap_or_else(|e| e.into_inner()).clone() {
        crate::tools::set_stop(stop);
    }
    crate::jobs::to_ui(Msg::Activity(crate::tools::describe(name, input)));
    let ask = |msg: Box<dyn FnOnce(std::sync::mpsc::Sender<String>) -> Msg>, secs: u64| -> (String, bool) {
        let (tx, rx) = std::sync::mpsc::channel();
        if !crate::jobs::to_ui(msg(tx)) {
            return ("오류: 화면에 닿지 못함".into(), true);
        }
        match rx.recv_timeout(std::time::Duration::from_secs(secs)) {
            Ok(out) => (out.clone(), out.starts_with("오류:")),
            Err(_) => ("오류: 화면에서 답이 오지 않음".into(), true),
        }
    };
    match name {
        "board_read" => {
            let topic = input["topic"].as_str().unwrap_or("").trim().to_string();
            ask(Box::new(move |reply| Msg::Board { topic, reply }), 15)
        }
        "browser_text" | "browser_eval" => {
            let script = if name == "browser_text" {
                "document.title + '\\n' + location.href + '\\n\\n' + (document.body ? document.body.innerText : '').slice(0, 30000)".to_string()
            } else {
                input["script"].as_str().unwrap_or("").to_string()
            };
            let card = input["card"].as_str().unwrap_or("").to_string();
            ask(Box::new(move |reply| Msg::Browser { card, script, reply }), 20)
        }
        _ => crate::tools::execute(name, input),
    }
}
