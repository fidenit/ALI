//! 맡긴 작업. 메인 AI 가 작업 지시서를 쓰면, 지시서마다 작업자(따로 도는 대화)가 하나씩 붙어 동시에 일한다.
//! 메인은 기다리지 않고 사용자와 계속 대화하고, 작업자가 끝나면 보고서가 메인에게 돌아온다.
//!
//!   지시서와 결과  보관함 `작업/지시서/<작업ID> <제목>.md`
//!   진행 상황      화면의 진행 카드 (카드 ID = 작업 ID)

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde_json::Value;

use crate::llm::{Config, Msg};
use crate::{agent, sys, tools, vault};

const WORKER: &str = include_str!("worker.md");
/// 작업자 하나가 도구를 주고받는 횟수의 상한
const MAX_STEPS: usize = 200;

pub enum Event {
    Started { id: String, title: String, topic: Option<String> },
    Progress { id: String, pct: Option<u8>, text: Option<String> },
    Finished { id: String, title: String, ok: bool, report: String },
}

struct Job {
    id: String,
    title: String,
    dir: String,
    note: String,
    /// 진행 중 / 끝남 / 실패 / 중단함
    state: &'static str,
    /// 가장 최근에 한 일 한 줄
    last: String,
    report: String,
    started: Instant,
    took_secs: u64,
    stop: Arc<AtomicBool>,
    inbox: Arc<Mutex<Vec<String>>>,
}

static JOBS: Mutex<Vec<Job>> = Mutex::new(Vec::new());
type Ui = (Sender<Msg>, Box<dyn Fn() + Send>);
static UI: Mutex<Option<Ui>> = Mutex::new(None);

fn jobs() -> std::sync::MutexGuard<'static, Vec<Job>> {
    JOBS.lock().unwrap_or_else(|e| e.into_inner())
}

/// 화면 쪽으로 소식을 보낼 길을 정해 둔다 (프로그램이 뜰 때 한 번).
pub fn init(tx: Sender<Msg>, wake: impl Fn() + Send + 'static) {
    *UI.lock().unwrap_or_else(|e| e.into_inner()) = Some((tx, Box::new(wake)));
}

/// 화면 쪽 스레드에 요청을 보낸다 (도구가 브라우저 같은 화면 쪽 기능을 써야 할 때).
pub fn to_ui(m: Msg) -> bool {
    match UI.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        Some((tx, wake)) => {
            let ok = tx.send(m).is_ok();
            wake();
            ok
        }
        None => false,
    }
}

fn emit(ev: Event) {
    if let Some((tx, wake)) = UI.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        let _ = tx.send(Msg::Job(ev));
        wake();
    }
}

fn next_id() -> String {
    let path = sys::data_dir().join("job.seq");
    let n = std::fs::read_to_string(&path).ok().and_then(|s| s.trim().parse::<u32>().ok()).unwrap_or(0) + 1;
    let _ = std::fs::write(&path, n.to_string());
    format!("job{n}")
}

pub fn running() -> usize {
    jobs().iter().filter(|j| j.state == "진행 중").count()
}

/// 지금 돌고 있는 작업 (매 턴 메인 모델에게 보여 준다).
pub fn summary() -> String {
    let mut out = String::new();
    for j in jobs().iter().filter(|j| j.state == "진행 중") {
        out += &format!("{} [{}분째] {} — 지금: {}\n", j.id, j.started.elapsed().as_secs() / 60, j.title, if j.last.is_empty() { "시작하는 중" } else { &j.last });
    }
    out
}

fn set<F: FnOnce(&mut Job)>(id: &str, f: F) {
    if let Some(j) = jobs().iter_mut().find(|j| j.id == id) {
        f(j);
    }
}

/// 메인 모델이 부르는 작업 도구.
pub fn tool(name: &str, input: &Value) -> Result<String, String> {
    let id = input["id"].as_str().unwrap_or("").trim().to_string();
    match name {
        "job_start" => start(input),
        "job_list" => {
            let all = jobs();
            if all.is_empty() {
                return Ok("맡긴 작업 없음".into());
            }
            Ok(all
                .iter()
                .map(|j| {
                    let time = if j.state == "진행 중" { format!("{}분째", j.started.elapsed().as_secs() / 60) } else { format!("{}초 걸림", j.took_secs) };
                    format!("{} [{} · {time}] {} — {} (작업 폴더: {}, 지시서: {})", j.id, j.state, j.title, if j.last.is_empty() { "-" } else { &j.last }, j.dir, j.note)
                })
                .collect::<Vec<_>>()
                .join("\n"))
        }
        "job_tell" => {
            let msg = input["message"].as_str().unwrap_or("").trim().to_string();
            let all = jobs();
            let j = all.iter().find(|j| j.id == id).ok_or_else(|| format!("'{id}' 라는 작업이 없음"))?;
            if j.state != "진행 중" {
                return Err(format!("{id} 는 이미 {} 상태라 전할 수 없음. 이어서 시키려면 job_start 로 새 작업을 맡긴다", j.state));
            }
            j.inbox.lock().unwrap_or_else(|e| e.into_inner()).push(msg);
            Ok(format!("{id} 에게 전함 (작업자가 지금 하던 단계를 마치는 대로 읽는다)"))
        }
        "job_stop" => {
            let all = jobs();
            let j = all.iter().find(|j| j.id == id).ok_or_else(|| format!("'{id}' 라는 작업이 없음"))?;
            j.stop.store(true, Ordering::Relaxed);
            Ok(format!("{id} 에게 멈추라고 함 (지금 하던 단계를 마치면 멈춘다)"))
        }
        "job_result" => {
            let all = jobs();
            let j = all.iter().find(|j| j.id == id).ok_or_else(|| format!("'{id}' 라는 작업이 없음"))?;
            Ok(if j.state == "진행 중" { format!("아직 진행 중. 지금: {}", j.last) } else { format!("[{}]\n{}", j.state, j.report) })
        }
        other => Err(format!("모르는 도구 '{other}'")),
    }
}

fn start(input: &Value) -> Result<String, String> {
    let title = input["title"].as_str().unwrap_or("").trim().to_string();
    let order = input["order"].as_str().unwrap_or("").trim().to_string();
    if title.is_empty() || order.is_empty() {
        return Err("title 과 order 가 필요함".into());
    }
    let mut cfg = Config::load();
    let key = cfg.key.clone().ok_or("API 키가 없어 작업자를 띄울 수 없음")?;
    cfg.effort = match input["effort"].as_str() {
        Some(e) if ["low", "medium", "high"].contains(&e) => e.to_string(),
        _ => "medium".into(),
    };
    let home = std::env::var(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).unwrap_or_else(|_| ".".into());
    let mut dir = input["dir"].as_str().map(str::trim).filter(|d| !d.is_empty()).unwrap_or(&home).to_string();
    if !std::path::Path::new(&dir).is_dir() {
        std::fs::create_dir_all(&dir).map_err(|e| format!("작업 폴더를 만들 수 없음: {e}"))?;
    }
    let id = next_id();
    // 같은 저장소를 여러 작업자가 동시에 고치면 서로 덮어쓴다. 요청하면 작업자마다 따로 떼어 낸 작업 폴더(git worktree)를 준다.
    let mut note_extra = String::new();
    if input["worktree"].as_bool().unwrap_or(false) {
        let top = tools::shell("git rev-parse --show-toplevel", Some(&dir), 20).map_err(|e| format!("worktree 를 쓰려면 git 저장소여야 함: {e}"))?;
        let top = top.trim().replace('/', if cfg!(windows) { "\\" } else { "/" });
        let wt = format!("{}-{id}", top.trim_end_matches(['\\', '/']));
        tools::shell(&format!("git worktree add -b 'ali/{id}' '{wt}'"), Some(&top), 120).map_err(|e| format!("git worktree 를 만들지 못함: {e}"))?;
        note_extra = format!("- 원래 저장소: {top}\n- 따로 떼어 낸 작업 폴더 (브랜치 ali/{id}): {wt}\n");
        dir = wt;
    }
    let safe: String = title.chars().map(|c| if "\\/:*?\"<>|#^[]".contains(c) { ' ' } else { c }).take(40).collect();
    let note = format!("작업/지시서/{id} {}.md", safe.trim());
    let (date, time) = sys::now_local();
    let _ = vault::write(
        &note,
        &format!("---\ndescription: 작업 지시서 {id} — {title}\ncreated: {date}\ntags:\n  - ali/작업\n---\n# {title}\n\n- 작업 ID: {id}\n- 맡긴 때: {date} {time}\n- 작업 폴더: {dir}\n{note_extra}\n## 지시\n\n{order}\n"),
    );
    let stop = Arc::new(AtomicBool::new(false));
    let inbox = Arc::new(Mutex::new(Vec::new()));
    jobs().push(Job {
        id: id.clone(),
        title: title.clone(),
        dir: dir.clone(),
        note: note.clone(),
        state: "진행 중",
        last: String::new(),
        report: String::new(),
        started: Instant::now(),
        took_secs: 0,
        stop: stop.clone(),
        inbox: inbox.clone(),
    });
    emit(Event::Started { id: id.clone(), title: title.clone(), topic: input["topic"].as_str().map(str::to_string).filter(|t| !t.is_empty()) });
    let first = format!(
        "<order id=\"{id}\" title=\"{title}\">\n{order}\n</order>\n<env>\n지금: {date} {time}\n운영체제: {}\n작업 폴더: {dir}\n{note_extra}지시서 노트: {note}\n기억 보관함 폴더: {}\n</env>",
        if cfg!(windows) { "Windows (shell 은 Windows PowerShell 5.1)" } else { "macOS" },
        vault::root().display()
    );
    let (jid, jtitle, jdir, jnote) = (id.clone(), title.clone(), dir.clone(), note.clone());
    std::thread::spawn(move || {
        let id = jid;
        let started = Instant::now();
        let send = |m: Msg| {
            if let Msg::Activity(a) = m {
                if !a.is_empty() {
                    set(&id, |j| j.last = a);
                }
            }
        };
        let check = || {
            if stop.load(Ordering::Relaxed) {
                return Err("중단함".to_string());
            }
            let said: Vec<String> = inbox.lock().unwrap_or_else(|e| e.into_inner()).drain(..).collect();
            Ok(if said.is_empty() { None } else { Some(format!("<from_main>\n{}\n</from_main>", said.join("\n"))) })
        };
        let tool = |name: &str, input: &Value| -> Option<(String, bool)> {
            match name {
                "progress" => {
                    let text = input["text"].as_str().map(|t| t.replace('\n', " ")).filter(|t| !t.trim().is_empty());
                    if let Some(t) = &text {
                        set(&id, |j| j.last = t.clone());
                    }
                    emit(Event::Progress { id: id.clone(), pct: input["pct"].as_u64().map(|p| p.min(100) as u8), text });
                    Some(("화면에 반영됨".into(), false))
                }
                // 작업 폴더를 따로 주지 않은 명령은 이 작업의 폴더에서 실행한다.
                "shell" if input["cwd"].as_str().map(|c| c.trim().is_empty()).unwrap_or(true) => {
                    let mut with = input.clone();
                    with["cwd"] = jdir.clone().into();
                    Some(tools::execute("shell", &with))
                }
                _ => None,
            }
        };
        let role = agent::Role { system: WORKER, tools: tools::definitions(true), max_steps: MAX_STEPS, check: &check, tool: &tool };
        let result = agent::run_as(&cfg, &key, &role, &first, &[], &send);
        let secs = started.elapsed().as_secs();
        let (state, ok, report) = match result {
            Ok((stats, text)) => {
                sys::log("jobs.log", &format!("{id} 끝남: {secs}초, 입력 {} (캐시 {}) 출력 {} 토큰", stats.input_tokens, stats.cached_tokens, stats.output_tokens));
                ("끝남", true, if text.trim().is_empty() { "결과: 작업자가 보고서 없이 끝냄".to_string() } else { text.trim().to_string() })
            }
            Err(e) => {
                sys::log("jobs.log", &format!("{id} 실패: {secs}초, {e}"));
                if stop.load(Ordering::Relaxed) { ("중단함", false, "결과: 도중에 멈춤 (메인이 중단시킴)".to_string()) } else { ("실패", false, format!("결과: 작업자가 끝내지 못함\n오류: {e}")) }
            }
        };
        let (_, time) = sys::now_local();
        let _ = vault::append(&jnote, &format!("\n## 결과 ({state}, {time}, {secs}초)\n\n{report}"));
        set(&id, |j| {
            j.state = state;
            j.report = report.clone();
            j.took_secs = secs;
        });
        emit(Event::Finished { id: id.clone(), title: jtitle, ok, report });
    });
    Ok(format!("작업 {id} 을 작업자에게 맡김. 작업 폴더: {dir}. 지시서: {note}. 화면에 진행 카드 {id} 가 자동으로 생겼다. 끝나면 보고서가 <job_done> 으로 온다. 기다리지 말고 이 턴을 마친다."))
}
