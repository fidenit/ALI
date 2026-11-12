//! 기록과 기억을 옵시디언 보관함으로 남긴다.
//!
//! 보관함은 프로그램 폴더 안의 `vault` 폴더이고, 내용은 전부 보통 마크다운 파일이다.
//! 옵시디언에서 "폴더를 보관함으로 열기"로 이 폴더를 열면 그대로 보인다 (옵시디언이 없어도 프로그램은 동작한다).
//! 옵시디언 형식을 따른다: 맨 위 YAML 속성(--- 사이), `[[노트 이름]]` 링크, 태그, 접히는 콜아웃.
//!
//!   기록/2026-11-12.md   프로그램이 자동으로 쓰는 하루치 기록 (일일 노트 형식)
//!   기억/…               AI 가 직접 쓰고 고치는 오래 남길 사실
//!   작업/…               AI 가 일을 마치고 남기는 결과 요약

use std::path::{Path, PathBuf};

use crate::sys;

pub fn root() -> PathBuf {
    let custom = std::fs::read_to_string(sys::base_dir().join("vault.txt")).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let r = custom.map(PathBuf::from).unwrap_or_else(|| sys::base_dir().join("vault"));
    for sub in ["기록", "기억", "작업"] {
        let _ = std::fs::create_dir_all(r.join(sub));
    }
    r
}

/// 보관함 안의 경로로 바꾼다. 보관함 밖을 가리키면 거부한다. 확장자가 없으면 .md 를 붙인다.
fn resolve(rel: &str) -> Result<PathBuf, String> {
    let rel = rel.trim().trim_start_matches(['/', '\\']);
    if rel.is_empty() || rel.contains("..") || rel.contains(':') {
        return Err(format!("보관함 안의 상대 경로여야 함: '{rel}'"));
    }
    let mut p = root().join(rel.replace('\\', "/"));
    if p.extension().is_none() {
        p.set_extension("md");
    }
    Ok(p)
}

fn rel_name(p: &Path) -> String {
    p.strip_prefix(root()).unwrap_or(p).to_string_lossy().replace('\\', "/")
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue; // .obsidian, .trash
        }
        if p.is_dir() {
            walk(&p, out);
        } else if p.extension().map(|x| x == "md").unwrap_or(false) {
            out.push(p);
        }
    }
}

/// 노트의 한 줄 설명: 속성의 description, 없으면 본문 첫 줄.
fn describe(text: &str) -> String {
    let mut in_front = false;
    let mut first_body = String::new();
    for (i, line) in text.lines().enumerate() {
        let t = line.trim();
        if i == 0 && t == "---" {
            in_front = true;
            continue;
        }
        if in_front {
            if t == "---" {
                in_front = false;
            } else if let Some(d) = t.strip_prefix("description:") {
                return d.trim().trim_matches('"').to_string();
            }
            continue;
        }
        if !t.is_empty() && first_body.is_empty() {
            first_body = t.trim_start_matches('#').trim().chars().take(80).collect();
        }
    }
    first_body
}

/// 기억·작업 노트를 최근에 고친 순서로 돌려준다. 기록(하루치 대화)과 작업 지시서는 수가 끝없이 늘어나므로 목록에서 빼고,
/// 필요할 때 note_search 로 찾는다. 뒤의 수는 목록에 다 싣지 못한 노트의 수.
fn listed() -> (Vec<(String, String)>, usize) {
    let mut files = Vec::new();
    walk(&root(), &mut files);
    let mut rows: Vec<(std::time::SystemTime, PathBuf)> = files
        .into_iter()
        .filter(|p| {
            let r = rel_name(p);
            !r.starts_with("기록/") && !r.starts_with("작업/지시서/")
        })
        .map(|p| (std::fs::metadata(&p).and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH), p))
        .collect();
    rows.sort_by(|a, b| b.0.cmp(&a.0));
    let more = rows.len().saturating_sub(LISTED);
    let out = rows.iter().take(LISTED).map(|(_, p)| (rel_name(p), describe(&std::fs::read_to_string(p).unwrap_or_default()))).collect();
    (out, more)
}

/// 목록에 싣는 노트의 수
const LISTED: usize = 80;

/// 기억·작업 노트의 목록 (매 턴 모델에게 보여 준다).
pub fn index() -> String {
    let (rows, more) = listed();
    let mut out: String = rows.iter().map(|(p, d)| format!("{p} — {d}\n")).collect();
    if more > 0 {
        out += &format!("(최근에 고친 {LISTED}개만 적음. 이 밖에 노트 {more}개와 모든 작업 지시서·날짜별 기록이 더 있다. note_search 로 찾는다)\n");
    }
    out
}

pub fn read(rel: &str) -> Result<String, String> {
    let p = resolve(rel)?;
    std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", rel_name(&p)))
}

/// 노트를 쓴다. 맨 위에 속성이 없으면 기본 속성을 붙인다.
pub fn write(rel: &str, content: &str) -> Result<String, String> {
    let p = resolve(rel)?;
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let (date, _) = sys::now_local();
    let body = if content.trim_start().starts_with("---") {
        content.to_string()
    } else {
        format!("---\ncreated: {date}\ntags:\n  - ly-board\n---\n{content}")
    };
    std::fs::write(&p, body).map_err(|e| e.to_string())?;
    Ok(format!("저장함: {}", rel_name(&p)))
}

pub fn append(rel: &str, content: &str) -> Result<String, String> {
    let p = resolve(rel)?;
    if !p.exists() {
        return write(rel, content);
    }
    let mut text = std::fs::read_to_string(&p).map_err(|e| e.to_string())?;
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text += content;
    text.push('\n');
    std::fs::write(&p, text).map_err(|e| e.to_string())?;
    Ok(format!("덧붙임: {}", rel_name(&p)))
}

/// 보관함 전체에서 글자를 찾는다 (대소문자 무시). 기록도 포함한다.
pub fn search(query: &str) -> String {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return "검색어가 비어 있음".into();
    }
    let mut files = Vec::new();
    walk(&root(), &mut files);
    files.sort();
    let mut out = String::new();
    let mut hits = 0;
    for p in files.iter().rev() {
        let Ok(text) = std::fs::read_to_string(p) else { continue };
        for (n, line) in text.lines().enumerate() {
            if line.to_lowercase().contains(&q) {
                let snippet: String = line.trim().chars().take(160).collect();
                out += &format!("{}:{}: {}\n", rel_name(p), n + 1, snippet);
                hits += 1;
                if hits >= 40 {
                    return out + "(40건에서 끊음)\n";
                }
            }
        }
    }
    if out.is_empty() { format!("'{query}' 에 해당하는 노트 없음") } else { out }
}

/// 한 턴의 기록.
pub struct TurnRecord {
    pub input: String,
    pub say: String,
    pub spoken: Vec<String>,
    pub actions: Vec<String>,
    pub raw: String,
    pub error: Option<String>,
}

/// 오늘의 기록 노트(일일 노트 형식)에 한 턴을 덧붙인다.
pub fn log_turn(r: &TurnRecord) {
    let (date, time) = sys::now_local();
    let path = root().join("기록").join(format!("{date}.md"));
    let mut text = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| format!("---\ntype: 기록\ndate: {date}\ntags:\n  - ali/기록\n---\n# {date}\n"));
    text += &format!("\n## {time}\n");
    text += &format!("- **입력** {}\n", r.input.replace('\n', " "));
    if !r.say.is_empty() {
        text += &format!("- **응답** {}\n", r.say);
    }
    for s in &r.spoken {
        text += &format!("- **말함** {s}\n");
    }
    if !r.actions.is_empty() {
        text += "- **한 일**\n";
        for a in &r.actions {
            text += &format!("    - {}\n", a.replace('\n', " "));
        }
    }
    if let Some(e) = &r.error {
        text += &format!("- **오류** {e}\n");
    }
    let raw = r.raw.trim();
    if !raw.is_empty() {
        // 접히는 콜아웃: 평소에는 접혀 있고, 눌러야 원문이 보인다.
        text += "\n> [!note]- 화면 명령 원문\n> ```\n";
        for l in raw.lines() {
            text += &format!("> {l}\n");
        }
        text += "> ```\n";
    }
    let _ = std::fs::write(&path, text);
}

/// 기억·작업 노트의 (경로, 한 줄 설명) 목록.
pub fn notes() -> Vec<(String, String)> {
    listed().0
}
