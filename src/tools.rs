//! AI 가 실제 일을 하는 데 쓰는 도구. 전부 묻지 않고 바로 실행한다 (사용자가 그렇게 정함).

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::vault;

thread_local! {
    /// 이 스레드에서 도는 턴을 사용자가 멈추라고 했는지 보는 신호 (메인 턴의 스레드에만 걸린다)
    static STOP: std::cell::RefCell<Option<std::sync::Arc<std::sync::atomic::AtomicBool>>> = const { std::cell::RefCell::new(None) };
}

pub fn set_stop(flag: std::sync::Arc<std::sync::atomic::AtomicBool>) {
    STOP.with(|s| *s.borrow_mut() = Some(flag));
}

/// 사용자가 지금 턴을 멈추라고 했는가.
pub fn stopped() -> bool {
    STOP.with(|s| s.borrow().as_ref().map(|f| f.load(std::sync::atomic::Ordering::Relaxed)).unwrap_or(false))
}

const SHELL_NOTE: &str = if cfg!(windows) { "Windows PowerShell 5.1" } else { "sh" };

/// API 에 보내는 도구 정의. 순서와 내용이 바뀌면 캐시가 깨지므로 고정해 둔다.
pub fn definitions(worker: bool) -> Value {
    let tool = |name: &str, desc: &str, props: Value, required: &[&str]| {
        json!({
            "name": name,
            "description": desc,
            "input_schema": { "type": "object", "properties": props, "required": required, "additionalProperties": false },
        })
    };
    let all = json!([
        tool(
            "screen",
            "도구로 일하는 도중에 화면(보드)을 고친다. ops 에 화면 수정 명령을 한 줄에 하나씩 넣는다. 일하는 중간에 쓴 일반 텍스트는 사용자에게 보이지 않으므로, 진행 상황이나 중간 결과를 보여 주려면 반드시 이 도구를 쓴다. 거부된 명령이 있으면 이유가 돌아온다.",
            json!({ "ops": { "type": "string", "description": "화면 수정 명령들. 줄바꿈으로 구분." } }),
            &["ops"],
        ),
        tool(
            "shell",
            &format!("사용자의 컴퓨터에서 명령을 실행한다 ({SHELL_NOTE}). 표준 출력과 오류 출력, 종료 코드가 돌아온다. 파일 찾기, 프로그램 실행, 시스템 정보 확인, git 등 무엇이든 쓴다."),
            json!({
                "command": { "type": "string" },
                "cwd": { "type": "string", "description": "작업 폴더 (생략하면 사용자 홈 폴더)" },
                "timeout_sec": { "type": "integer", "description": "기본 120" },
            }),
            &["command"],
        ),
        tool(
            "read_file",
            "텍스트 파일을 읽는다. 큰 파일은 from_line 과 lines 로 필요한 부분만 읽는다 (이때는 줄 번호가 붙어 나온다).",
            json!({
                "path": { "type": "string", "description": "절대 경로" },
                "from_line": { "type": "integer", "description": "읽기 시작할 줄 (1부터)" },
                "lines": { "type": "integer", "description": "읽을 줄 수" },
            }),
            &["path"],
        ),
        tool(
            "route_find",
            "들르는 곳들을 순서대로 잇는 자동차 길을 찾는다. 구간별 거리와 시간, 각 곳의 좌표가 돌아오고, 길의 모양이 route 카드용으로 저장된다. 경로나 동선을 보여 달라는 요청에는 지도 페이지를 띄우지 말고 이 도구로 찾은 뒤 route 카드로 직접 그린다.",
            json!({
                "card": { "type": "string", "description": "이 경로를 그릴 route 카드의 ID (새로 지어도 되고, 있는 카드의 것을 다시 써도 된다)" },
                "stops": {
                    "type": "array",
                    "description": "들르는 곳들, 가는 순서대로 (2~12곳)",
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": { "type": "string", "description": "카드에 보일 짧은 이름" },
                            "find": { "type": "string", "description": "위치를 찾을 검색어 (생략하면 name 으로 찾는다). 상호가 안 나오면 동네 이름이나 도로명 주소" },
                            "lat": { "type": "number" },
                            "lng": { "type": "number" },
                        },
                        "required": ["name"],
                    },
                },
            }),
            &["card", "stops"],
        ),
        tool(
            "place_find",
            "장소 이름이나 주소로 좌표(위도, 경도)를 찾는다 (오픈스트리트맵). 지도에 여러 곳을 잇는 경로를 띄우려면 좌표가 필요하다. 상호가 안 나오면 동네 이름이나 도로명 주소로 다시 찾는다.",
            json!({ "query": { "type": "string", "description": "장소 이름 또는 주소. 예: 비발디파크, 양양 낙산사, 강원 양양군 손양면 상운리" } }),
            &["query"],
        ),
        tool(
            "view_image",
            "그림 파일을 직접 본다 (png, jpg, webp, gif, bmp). 사용자가 붙인 사진을 다시 보거나, 화면 캡처·도면·사진 파일의 내용을 확인할 때 쓴다.",
            json!({ "path": { "type": "string", "description": "절대 경로" } }),
            &["path"],
        ),
        tool(
            "edit_file",
            "파일의 일부만 바꾼다. old 에 지금 파일에 있는 글을 그대로 넣고(공백과 들여쓰기까지 같아야 한다) new 에 바꿀 글을 넣는다. old 는 파일에서 한 군데만 가리켜야 하므로 앞뒤 줄을 충분히 포함한다. 기존 파일을 고칠 때는 write_file 로 통째로 덮어쓰지 말고 이 도구를 쓴다.",
            json!({
                "path": { "type": "string", "description": "절대 경로" },
                "old": { "type": "string" },
                "new": { "type": "string" },
                "all": { "type": "boolean", "description": "true 면 old 가 여러 군데 있어도 전부 바꾼다" },
            }),
            &["path", "old", "new"],
        ),
        tool(
            "find_text",
            "폴더 아래의 모든 텍스트 파일에서 글자를 찾는다 (대소문자 무시, .git·target·node_modules 등은 건너뜀). 어느 파일의 몇째 줄에 있는지 돌아온다. 코드에서 함수나 설정이 어디 있는지 찾을 때 쓴다.",
            json!({
                "path": { "type": "string", "description": "찾을 폴더의 절대 경로" },
                "text": { "type": "string" },
                "ext": { "type": "string", "description": "이 확장자의 파일만 (예: rs). 생략하면 전부" },
            }),
            &["path", "text"],
        ),
        tool(
            "write_file",
            "파일을 만들거나 통째로 덮어쓴다. 폴더가 없으면 만든다.",
            json!({ "path": { "type": "string", "description": "절대 경로" }, "content": { "type": "string" } }),
            &["path", "content"],
        ),
        tool("list_dir", "폴더의 내용을 본다.", json!({ "path": { "type": "string", "description": "절대 경로" } }), &["path"]),
        tool(
            "note_search",
            "기억 보관함(옵시디언)의 모든 노트와 과거 기록에서 글자를 찾는다. 사용자가 예전에 말한 것, 예전에 한 일을 떠올려야 할 때 쓴다.",
            json!({ "query": { "type": "string" } }),
            &["query"],
        ),
        tool("note_read", "보관함의 노트 하나를 읽는다.", json!({ "path": { "type": "string", "description": "보관함 기준 경로. 예: 기억/사용자.md" } }), &["path"]),
        tool(
            "note_write",
            "보관함에 노트를 만들거나 통째로 고쳐 쓴다. 오래 남길 사실은 기억/ 아래, 끝낸 일의 요약은 작업/ 아래에 둔다.",
            json!({ "path": { "type": "string" }, "content": { "type": "string", "description": "옵시디언 마크다운. 맨 위 속성에 description 한 줄을 넣는다." } }),
            &["path", "content"],
        ),
        tool("note_append", "보관함의 노트 끝에 내용을 덧붙인다 (없으면 만든다).", json!({ "path": { "type": "string" }, "content": { "type": "string" } }), &["path", "content"]),
        tool(
            "browser_text",
            "화면의 web 카드에 떠 있는 페이지의 제목, 주소, 본문 글을 읽는다. 페이지가 제대로 떴는지, 무슨 내용인지 확인할 때 쓴다.",
            json!({ "card": { "type": "string", "description": "web 카드의 ID" } }),
            &["card"],
        ),
        tool(
            "browser_eval",
            "화면의 web 카드에 떠 있는 페이지 안에서 자바스크립트를 실행하고 마지막 식의 값을 받는다. 요소를 누르거나(document.querySelector(…).click()), 입력란을 채우거나, 화면의 특정 값을 읽거나, 스크롤할 때 쓴다. 다른 주소로 가려면 이 도구 대신 화면 명령 `set <카드> url <주소>`를 쓴다.",
            json!({ "card": { "type": "string", "description": "web 카드의 ID" }, "script": { "type": "string" } }),
            &["card", "script"],
        ),
        tool(
            "job_start",
            "일을 작업자에게 맡긴다. 작업자는 따로 도는 AI 로, order(작업 지시서)만 받고 지금까지의 대화는 모른다. 부르면 바로 돌아오고 작업자는 뒤에서 일한다. 여러 번 부르면 작업자 여럿이 동시에 일한다. 화면에 진행 카드가 자동으로 생기고, 끝나면 보고서가 다음 턴에 <job_done> 으로 온다.",
            json!({
                "title": { "type": "string", "description": "사용자가 알아볼 작업 이름 (진행 카드의 제목이 된다, 40자 이내)" },
                "order": { "type": "string", "description": "작업 지시서. 이것만 읽고 일할 수 있게 쓴다: 목표, 배경, 대상 파일·폴더의 절대 경로, 해야 할 것, 하지 말 것, 완료 기준, 확인 방법." },
                "dir": { "type": "string", "description": "작업 폴더의 절대 경로 (생략하면 사용자 홈 폴더)" },
                "topic": { "type": "string", "description": "진행 카드를 놓을 주제 ID (생략하면 '맡긴 작업' 주제)" },
                "worktree": { "type": "boolean", "description": "true 면 git 저장소에서 이 작업만의 브랜치와 작업 폴더를 따로 떼어 준다. 같은 저장소를 여러 작업자가 동시에 고칠 때 쓴다." },
                "effort": { "type": "string", "enum": ["low", "medium", "high"], "description": "얼마나 깊이 생각하며 일할지. 기본 medium, 어려운 코딩은 high" },
            }),
            &["title", "order"],
        ),
        tool(
            "board_read",
            "보드에서 주제 하나의 카드 내용을 전부 읽는다. <board> 가 일부만 왔을 때(# 로 시작하는 안내 줄이 붙은 주제), 그 주제의 카드를 고치거나 내용을 답해야 하면 먼저 이것으로 읽는다.",
            json!({ "topic": { "type": "string", "description": "주제 ID" } }),
            &["topic"],
        ),
        tool("job_list", "맡긴 작업들의 상태를 본다.", json!({}), &[]),
        tool(
            "job_tell",
            "진행 중인 작업자에게 추가 지시나 사용자의 답을 전한다.",
            json!({ "id": { "type": "string", "description": "작업 ID (예: job3)" }, "message": { "type": "string" } }),
            &["id", "message"],
        ),
        tool("job_stop", "진행 중인 작업을 멈춘다.", json!({ "id": { "type": "string" } }), &["id"]),
        tool("job_result", "끝난 작업의 보고서를 다시 본다.", json!({ "id": { "type": "string" } }), &["id"]),
        tool(
            "progress",
            "사용자가 보는 화면의 진행 카드를 고친다. 단계가 넘어갈 때마다 부른다.",
            json!({
                "text": { "type": "string", "description": "지금 하는 일. 사용자가 알아볼 말로 60자 이내" },
                "pct": { "type": "integer", "description": "전체 중 어디까지 왔는지 (0~100)" },
            }),
            &["text"],
        ),
        { "type": "web_search_20260209", "name": "web_search", "max_uses": 8 },
        { "type": "web_fetch_20260209", "name": "web_fetch", "max_uses": 8 },
    ]);
    // 작업자는 화면과 브라우저를 직접 다루지 않고, 일을 다시 남에게 맡기지도 않는다. 메인은 progress 를 쓰지 않는다.
    let keep = |name: &str| {
        if worker { !(name == "screen" || name == "board_read" || name == "route_find" || name.starts_with("browser_") || name.starts_with("job_")) } else { name != "progress" }
    };
    Value::Array(all.as_array().unwrap().iter().filter(|t| keep(t["name"].as_str().unwrap_or(""))).cloned().collect())
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max * 2 / 3).collect();
    let tail: String = s.chars().rev().take(max / 3).collect::<Vec<_>>().into_iter().rev().collect();
    format!("{head}\n… (가운데 생략, 전체 {}자) …\n{tail}", s.chars().count())
}

fn arg<'a>(input: &'a Value, key: &str) -> Result<&'a str, String> {
    input[key].as_str().ok_or_else(|| format!("'{key}' 가 필요함"))
}

/// 화면 아래에 띄울 한 줄 설명 ("명령 실행: git status").
pub fn describe(name: &str, input: &Value) -> String {
    let short = |key: &str| -> String {
        let s = input[key].as_str().unwrap_or("").replace('\n', " ");
        if s.chars().count() > 70 { s.chars().take(70).collect::<String>() + "…" } else { s }
    };
    match name {
        "shell" => format!("명령 실행: {}", short("command")),
        "read_file" => format!("파일 읽기: {}", short("path")),
        "write_file" => format!("파일 쓰기: {}", short("path")),
        "list_dir" => format!("폴더 보기: {}", short("path")),
        "edit_file" => format!("파일 고치기: {}", short("path")),
        "view_image" => format!("그림 보기: {}", short("path")),
        "place_find" => format!("장소 찾기: {}", short("query")),
        "route_find" => "길 찾기".to_string(),
        "find_text" => format!("글자 찾기: {}", short("text")),
        "job_start" => format!("작업 맡기기: {}", short("title")),
        "job_list" => "맡긴 작업 확인".to_string(),
        "board_read" => format!("보드 읽기: {}", short("topic")),
        "job_tell" => format!("작업자에게 전하기: {}", short("id")),
        "job_stop" => format!("작업 멈추기: {}", short("id")),
        "job_result" => format!("작업 결과 보기: {}", short("id")),
        "progress" => format!("진행: {}", short("text")),
        "note_search" => format!("기억 찾기: {}", short("query")),
        "note_read" => format!("기억 읽기: {}", short("path")),
        "note_write" => format!("기억 쓰기: {}", short("path")),
        "note_append" => format!("기억 덧붙이기: {}", short("path")),
        "browser_text" => format!("페이지 읽기: {}", short("card")),
        "browser_eval" => format!("페이지 조작: {}", short("script")),
        "web_search" => format!("웹 검색: {}", short("query")),
        "web_fetch" => format!("웹 페이지 읽기: {}", short("url")),
        other => other.to_string(),
    }
}

/// 도구 하나를 실행한다. 반환값은 (결과 글, 오류인가). `screen` 은 화면 쪽에서 따로 처리한다.
pub fn execute(name: &str, input: &Value) -> (String, bool) {
    let result: Result<String, String> = (|| match name {
        "shell" => shell(arg(input, "command")?, input["cwd"].as_str(), input["timeout_sec"].as_u64().unwrap_or(120)),
        "read_file" => {
            let bytes = std::fs::read(arg(input, "path")?).map_err(|e| e.to_string())?;
            let text = String::from_utf8_lossy(&bytes);
            let (from, count) = (input["from_line"].as_u64(), input["lines"].as_u64());
            if from.is_none() && count.is_none() {
                return Ok(clip(&text, 60_000));
            }
            let from = from.unwrap_or(1).max(1) as usize;
            let total = text.lines().count();
            let part: Vec<String> =
                text.lines().enumerate().skip(from - 1).take(count.unwrap_or(400) as usize).map(|(n, l)| format!("{:>5}  {l}", n + 1)).collect();
            Ok(format!("{}\n(전체 {total}줄)", clip(&part.join("\n"), 60_000)))
        }
        "edit_file" => {
            let path = arg(input, "path")?;
            let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
            let (mut old, mut new) = (arg(input, "old")?.to_string(), arg(input, "new")?.to_string());
            if old.is_empty() {
                return Err("old 가 비어 있음".into());
            }
            // 파일의 줄 끝이 CRLF 인데 LF 로 적어 왔으면 맞춰 준다.
            if !text.contains(&old) && text.contains("\r\n") {
                old = old.replace("\r\n", "\n").replace('\n', "\r\n");
                new = new.replace("\r\n", "\n").replace('\n', "\r\n");
            }
            let found = text.matches(&old).count();
            let all = input["all"].as_bool().unwrap_or(false);
            if found == 0 {
                return Err("old 의 글이 파일에 없음. 공백과 들여쓰기까지 같아야 한다. read_file 로 그 부분을 다시 읽고 그대로 옮겨 적는다".into());
            }
            if found > 1 && !all {
                return Err(format!("old 의 글이 {found}군데 있음. 앞뒤 줄을 더 넣어 한 군데만 가리키게 하거나 all 을 true 로 준다"));
            }
            let out = if all { text.replace(&old, &new) } else { text.replacen(&old, &new, 1) };
            std::fs::write(path, out).map_err(|e| e.to_string())?;
            Ok(format!("고침: {path} ({}군데)", if all { found } else { 1 }))
        }
        "find_text" => {
            let root = std::path::PathBuf::from(arg(input, "path")?);
            let needle = arg(input, "text")?.to_lowercase();
            if needle.is_empty() {
                return Err("text 가 비어 있음".into());
            }
            let ext = input["ext"].as_str().map(|e| e.trim_start_matches('.').to_lowercase()).filter(|e| !e.is_empty());
            let mut hits = Vec::new();
            let mut stack = vec![root.clone()];
            while let Some(dir) = stack.pop() {
                let Ok(rd) = std::fs::read_dir(&dir) else { continue };
                for e in rd.flatten() {
                    let p = e.path();
                    let name = e.file_name().to_string_lossy().to_string();
                    if p.is_dir() {
                        if ![".git", "target", "node_modules", ".venv", "dist", "build", "__pycache__"].contains(&name.as_str()) {
                            stack.push(p);
                        }
                        continue;
                    }
                    if let Some(want) = &ext {
                        if p.extension().map(|x| x.to_string_lossy().to_lowercase()) != Some(want.clone()) {
                            continue;
                        }
                    }
                    if e.metadata().map(|m| m.len() > 2_000_000).unwrap_or(true) {
                        continue;
                    }
                    let Ok(text) = std::fs::read_to_string(&p) else { continue }; // 글이 아닌 파일은 건너뛴다
                    for (n, line) in text.lines().enumerate() {
                        if line.to_lowercase().contains(&needle) {
                            let snippet: String = line.trim().chars().take(160).collect();
                            hits.push(format!("{}:{}: {snippet}", p.strip_prefix(&root).unwrap_or(&p).display(), n + 1));
                        }
                    }
                }
            }
            if hits.is_empty() {
                return Ok("찾는 글이 없음".into());
            }
            hits.sort();
            let total = hits.len();
            hits.truncate(120);
            Ok(format!("{}\n(총 {total}건)", hits.join("\n")))
        }
        "route_find" => crate::route::find(input),
        "place_find" => {
            let query = arg(input, "query")?;
            let encoded: String = query.bytes().map(|b| if b.is_ascii_alphanumeric() { (b as char).to_string() } else { format!("%{b:02X}") }).collect();
            let url = format!("https://nominatim.openstreetmap.org/search?q={encoded}&format=jsonv2&limit=6&accept-language=ko");
            let mut resp = crate::agent::http().get(&url).header("user-agent", "ALI-desktop/0.1").call().map_err(|e| format!("연결 실패: {e}"))?;
            let text = resp.body_mut().read_to_string().map_err(|e| e.to_string())?;
            let found: Value = serde_json::from_str(&text).map_err(|_| format!("장소 찾기 응답을 읽지 못함: {}", clip(&text, 200)))?;
            let rows: Vec<String> = found
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|p| {
                            let num = |k: &str| p[k].as_str().and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
                            format!("{} — 위도 {:.6}, 경도 {:.6} ({})", p["display_name"].as_str().unwrap_or("?"), num("lat"), num("lon"), p["type"].as_str().unwrap_or(""))
                        })
                        .collect()
                })
                .unwrap_or_default();
            if rows.is_empty() {
                return Ok(format!("'{query}' 로는 찾지 못함. 상호 대신 동네 이름이나 도로명 주소로 다시 찾아 본다"));
            }
            Ok(rows.join("\n"))
        }
        "job_start" | "job_list" | "job_tell" | "job_stop" | "job_result" => crate::jobs::tool(name, input),
        "write_file" => {
            let path = std::path::PathBuf::from(arg(input, "path")?);
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            let content = arg(input, "content")?;
            std::fs::write(&path, content).map_err(|e| e.to_string())?;
            Ok(format!("저장함: {} ({}자)", path.display(), content.chars().count()))
        }
        "list_dir" => {
            let mut rows = Vec::new();
            for e in std::fs::read_dir(arg(input, "path")?).map_err(|e| e.to_string())?.flatten() {
                let meta = e.metadata().ok();
                let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
                let size = meta.map(|m| m.len()).unwrap_or(0);
                rows.push(if is_dir { format!("[폴더] {}", e.file_name().to_string_lossy()) } else { format!("{} ({size} 바이트)", e.file_name().to_string_lossy()) });
            }
            rows.sort();
            let total = rows.len();
            rows.truncate(400);
            Ok(format!("{}\n(총 {total}개)", rows.join("\n")))
        }
        "note_search" => Ok(vault::search(arg(input, "query")?)),
        "note_read" => vault::read(arg(input, "path")?).map(|t| clip(&t, 40_000)),
        "note_write" => vault::write(arg(input, "path")?, arg(input, "content")?),
        "note_append" => vault::append(arg(input, "path")?, arg(input, "content")?),
        other => Err(format!("모르는 도구 '{other}'")),
    })();
    match result {
        Ok(s) => (if s.is_empty() { "(출력 없음)".into() } else { s }, false),
        Err(e) => (e, true),
    }
}

pub fn shell(command: &str, cwd: Option<&str>, timeout_sec: u64) -> Result<String, String> {
    let home = std::env::var(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).unwrap_or_else(|_| ".".into());
    let mut cmd;
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd = Command::new("powershell.exe");
        // 출력이 한글로 깨지지 않게 UTF-8 로 맞춘다.
        let wrapped = format!("[Console]::OutputEncoding = [Text.Encoding]::UTF8; $ProgressPreference = 'SilentlyContinue'; {command}");
        cmd.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", &wrapped]);
        cmd.creation_flags(0x0800_0000); // 콘솔 창을 띄우지 않는다
    }
    #[cfg(not(windows))]
    {
        cmd = Command::new("sh");
        cmd.args(["-c", command]);
    }
    let mut child = cmd
        .current_dir(cwd.filter(|c| !c.is_empty()).unwrap_or(&home))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("실행 실패: {e}"))?;
    // 출력이 많아도 막히지 않도록 따로 읽는다.
    let mut out_pipe = child.stdout.take().unwrap();
    let mut err_pipe = child.stderr.take().unwrap();
    let out_t = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = out_pipe.read_to_end(&mut b);
        b
    });
    let err_t = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = err_pipe.read_to_end(&mut b);
        b
    });
    let started = Instant::now();
    let limit = Duration::from_secs(timeout_sec.clamp(1, 1800));
    let status = loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(s) => break Some(s),
            None if started.elapsed() > limit || stopped() => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            None => std::thread::sleep(Duration::from_millis(40)),
        }
    };
    let out = String::from_utf8_lossy(&out_t.join().unwrap_or_default()).to_string();
    let err = String::from_utf8_lossy(&err_t.join().unwrap_or_default()).to_string();
    let mut text = clip(out.trim_end(), 20_000);
    if !err.trim().is_empty() {
        text += &format!("\n[오류 출력]\n{}", clip(err.trim_end(), 6_000));
    }
    match status {
        Some(s) if s.success() => Ok(text),
        Some(s) => Err(format!("{text}\n[종료 코드 {}]", s.code().unwrap_or(-1))),
        None if stopped() => Err(format!("{text}\n[사용자가 멈춰서 중단함]")),
        None => Err(format!("{text}\n[{timeout_sec}초 제한 시간 초과로 중단함]")),
    }
}
