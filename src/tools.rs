//! AI 가 실제 일을 하는 데 쓰는 도구. 전부 묻지 않고 바로 실행한다 (사용자가 그렇게 정함).

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::vault;

const SHELL_NOTE: &str = if cfg!(windows) { "Windows PowerShell 5.1" } else { "sh" };

/// API 에 보내는 도구 정의. 순서와 내용이 바뀌면 캐시가 깨지므로 고정해 둔다.
pub fn definitions() -> Value {
    let tool = |name: &str, desc: &str, props: Value, required: &[&str]| {
        json!({
            "name": name,
            "description": desc,
            "input_schema": { "type": "object", "properties": props, "required": required, "additionalProperties": false },
        })
    };
    json!([
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
        tool("read_file", "텍스트 파일을 읽는다.", json!({ "path": { "type": "string", "description": "절대 경로" } }), &["path"]),
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
        { "type": "web_search_20260209", "name": "web_search", "max_uses": 8 },
        { "type": "web_fetch_20260209", "name": "web_fetch", "max_uses": 8 },
    ])
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
            Ok(clip(&String::from_utf8_lossy(&bytes), 60_000))
        }
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

fn shell(command: &str, cwd: Option<&str>, timeout_sec: u64) -> Result<String, String> {
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
            None if started.elapsed() > limit => {
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
        None => Err(format!("{text}\n[{timeout_sec}초 제한 시간 초과로 중단함]")),
    }
}
