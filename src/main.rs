#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod agent;
mod audio;
mod chat;
mod core;
mod decide;
mod eleven;
mod jev;
mod jobs;
mod lang;
mod llm;
mod memory;
mod pics;
mod sys;
mod tools;
mod tts;
mod ui;
mod vault;

use std::io::Write;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use eframe::egui;

use crate::core::{Author, Board};

const TITLE: &str = "ALI";
/// 이 크기보다 큰 소리가 들어오면 말하는 중으로 본다.
const MIC_THRESHOLD: f32 = 0.02;
/// 매 턴 그대로 보여 주는 최근 대화의 수. 그보다 오래된 것은 관련 있을 때만 찾아서 보여 준다.
const RECENT: usize = 10;
/// 보드를 명령으로 적은 것이 이 글자 수를 넘으면, 지금 이야기와 먼 주제는 줄여서 모델에게 보낸다.
const BOARD_FULL: usize = 12_000;
/// 이 날수만큼 손대지 않은 주제는 보관하자고 짚는다.
const STALE_DAYS: i64 = 14;

/// "2026-11-12" 같은 날짜를 날수로 바꾼다 (두 날짜의 차이를 세는 데만 쓴다).
fn days_of(date: &str) -> i64 {
    let mut p = date.split('-').map(|x| x.parse::<i64>().unwrap_or(0));
    let (y, m, d) = (p.next().unwrap_or(0), p.next().unwrap_or(1), p.next().unwrap_or(1));
    let (y, m) = if m <= 2 { (y - 1, m + 12) } else { (y, m) };
    365 * y + y / 4 - y / 100 + y / 400 + (153 * (m - 3) + 2) / 5 + d
}

fn recent_count() -> usize {
    // 확인용: LYBOARD_RECENT=2 처럼 줄이면 오래된 대화를 찾아오는 동작을 짧은 대화로도 시험할 수 있다.
    std::env::var("LYBOARD_RECENT").ok().and_then(|v| v.parse().ok()).unwrap_or(RECENT)
}

fn log_path() -> std::path::PathBuf {
    sys::data_dir().join("ops.log")
}
fn voice_path() -> std::path::PathBuf {
    sys::data_dir().join("voice")
}

struct App {
    board: Board,
    view: ui::View,
    cfg: llm::Config,
    tx: Sender<llm::Msg>,
    rx: Receiver<llm::Msg>,
    stream: lang::LineStream,
    busy: bool,
    turn_no: usize,
    input: String,
    cur_input: String,
    say_live: Option<String>,
    /// 처음 쓰는 사람이 키를 넣는 칸
    key_input: String,
    jev_input: String,
    /// 화면 조작에 AI 가 반응할 예정 시각
    react_at: Option<std::time::Instant>,
    /// 사용자가 조작했고 AI 의 반응을 기다리는 카드
    pending: std::collections::HashSet<String>,
    /// 최근 (사용자 입력, AI 의 한마디)
    recent: Vec<(String, String)>,
    /// 지난 턴 이후 사용자가 화면에서 직접 한 조작
    events: Vec<String>,
    /// 거부된 AI 명령과 이유
    rejects: Vec<String>,
    selected: Option<String>,
    stats: Option<llm::Stats>,
    error: Option<String>,
    log: Option<std::fs::File>,
    focus_input: bool,
    /// 확인용 자동 입력 (환경 변수 LYBOARD_AUTOPLAY="입력1|입력2"). 키보드를 건드리지 않고 장면을 재생한다.
    autoplay: std::collections::VecDeque<String>,
    idle_since: std::time::Instant,
    /// 음성 켬/끔. 꺼져 있으면 지금까지처럼 글로만 동작한다.
    voice: bool,
    tts: tts::Tts,
    /// 아직 말하지 않은 문장과, 그 문장이 읽힐 때 화면에 나갈 명령들
    speech: std::collections::VecDeque<(String, Vec<String>)>,
    /// 지금 읽고 있는 문장 (화면 아래 자막)
    caption: Option<String>,
    /// AI 가 지금 하고 있는 일 한 줄 ("명령 실행: …")
    activity: String,
    /// 이번 턴의 기록 (끝나면 보관함의 오늘 노트에 남긴다)
    turn_raw: String,
    turn_actions: Vec<String>,
    turn_spoken: Vec<String>,
    turn_events: String,
    titlebar_done: bool,
    shot: Option<String>,
    shot_from: std::time::Instant,
    shot_delay: u64,
    /// 이번 턴에서 모델의 출력이나 도구 사용이 시작됐는가 (아직이면 "기다리는 중" 표시)
    got_output: bool,
    sent_at: std::time::Instant,
    /// 판단 모델이 먼저 내린 결정 (요청의 종류, 주제)
    pre: Option<decide::Pre>,
    /// 판단 모델이 지난 화면을 보고 짚은 것 (다음 턴에 메인 모델에게 전한다)
    review: Vec<String>,
    /// 웹 카드 위에 올려 둔 브라우저 화면
    webviews: std::collections::HashMap<String, Web>,
    /// 말로 대화하는 중인가 (마이크 스위치). 켜져 있으면 듣기 → 답하기 → 다시 듣기를 되풀이한다.
    converse: bool,
    mic: Option<audio::Mic>,
    mic_since: std::time::Instant,
    /// 이번 녹음에서 말소리가 들렸는가, 그리고 조용해지기 시작한 때
    heard: bool,
    quiet_since: Option<std::time::Instant>,
    /// 받아쓰기 결과를 기다리는 중
    stt: Option<Receiver<Result<String, String>>>,
    /// 화면을 다시 그리게 깨우는 손잡이 (다른 스레드에서 쓴다)
    wake: egui::Context,
    /// 판단 모델이 짚은 "구성에서 고칠 것". 있으면 한가할 때 구성 정리 턴을 돌린다.
    tidy: Option<Vec<String>>,
    /// 지금 도는 턴이 구성 정리인가
    tidying: bool,
    /// 지금까지의 모든 턴 (껐다 켜도 남는다). 최근 것은 그대로, 오래된 것은 관련 있을 때 찾아서 모델에게 보인다.
    past: memory::Past,
    /// 주제마다 마지막으로 손댄 차례 (보드가 커지면 최근에 다룬 주제만 자세히 모델에게 보낸다)
    touched: std::collections::HashMap<String, u64>,
    touch_seq: u64,
    /// 주제마다 마지막으로 손댄 날짜 (오래 손대지 않은 주제를 보관하자고 짚는 데 쓴다)
    touched_on: std::collections::HashMap<String, String>,
    /// 조작 기록에 마지막으로 적은 날짜, 그리고 기록을 되살리는 동안의 "그때 날짜"
    log_date: String,
    replay_date: Option<String>,
    /// 이번 턴에 쓸 옛 대화 후보
    recall_cands: Vec<(f32, usize, String)>,
    /// 확인용 입력 대본: 프레임마다 넣을 마우스 사건 (LYBOARD_INPUT). 실제 마우스를 건드리지 않고 끌기와 휠을 시험한다.
    fake_input: std::collections::VecDeque<Vec<egui::Event>>,
    fake_marks: std::collections::VecDeque<(usize, String)>,
    fake_frame: usize,
    /// 입력창에 붙여 둔 사진 (보내면 이번 말과 함께 간다)
    pub pics: Vec<(pics::Pic, egui::TextureHandle)>,
    /// 방금까지 사용자 앞에 있던 일의 주제 (다음 일을 고를 때 같은 주제의 것부터 가져온다)
    task_topic: Option<String>,
    /// 왼쪽 구역에 보관함 목록을 펼쳐 놓았는가
    pub show_shelf: bool,
    /// 끝난 작업의 보고서 (작업 ID, 제목, 성공했는가, 보고서). 다음 턴에 메인 모델에게 전한다.
    job_news: Vec<(String, String, bool, String)>,
    /// 지금 도는 턴이 작업 결과를 정리하는 턴이면 그 이름
    job_turn: Option<String>,
    /// 구성 정리를 다시 하기까지 남은 턴 수
    tidy_rest: u8,
}

struct Web {
    view: wry::WebView,
    url: String,
    rect: egui::Rect,
    visible: bool,
}

/// 카드에 적힌 주소를 브라우저가 열 수 있는 주소로 바꾼다 (내 컴퓨터의 파일 경로도 받는다).
fn to_url(s: &str) -> String {
    let s = s.trim();
    let lower = s.to_lowercase();
    if ["http://", "https://", "file:", "about:", "data:"].iter().any(|p| lower.starts_with(p)) {
        return s.to_string();
    }
    let bytes = s.as_bytes();
    let is_path = (bytes.len() > 2 && bytes[1] == b':') || s.starts_with('/') || s.starts_with('\\');
    if is_path {
        let p = s.replace('\\', "/").replace('%', "%25").replace(' ', "%20").replace('#', "%23");
        return format!("file:///{}", p.trim_start_matches('/'));
    }
    format!("https://{s}")
}



impl App {
    fn new(ctx: &egui::Context) -> Self {
        let cfg = llm::Config::load();
        let wake = ctx.clone();
        let (tx, rx) = channel();
        let mut app = App {
            board: Board::default(),
            view: ui::View::default(),
            cfg,
            tx,
            rx,
            stream: lang::LineStream::default(),
            busy: false,
            turn_no: 0,
            input: String::new(),
            cur_input: String::new(),
            say_live: None,
            key_input: String::new(),
            jev_input: String::new(),
            react_at: None,
            pending: Default::default(),
            recent: Vec::new(),
            events: Vec::new(),
            rejects: Vec::new(),
            selected: None,
            stats: None,
            error: None,
            log: None,
            focus_input: true,
            autoplay: std::env::var("LYBOARD_AUTOPLAY")
                .map(|v| v.split('|').map(str::to_string).filter(|s| !s.is_empty()).collect())
                .unwrap_or_default(),
            idle_since: std::time::Instant::now(),
            voice: std::env::var("LYBOARD_VOICE").is_ok()
                || std::fs::read_to_string(voice_path()).map(|s| s.trim() == "on").unwrap_or(false),
            tts: tts::Tts::new(move || wake.request_repaint()),
            speech: Default::default(),
            caption: None,
            activity: String::new(),
            turn_raw: String::new(),
            turn_actions: Vec::new(),
            turn_spoken: Vec::new(),
            turn_events: String::new(),
            titlebar_done: false,
            shot: std::env::var("LYBOARD_SHOT").ok(),
            shot_from: std::time::Instant::now(),
            shot_delay: std::env::var("LYBOARD_SHOT_AT").ok().and_then(|v| v.parse().ok()).unwrap_or(3500),
            got_output: false,
            sent_at: std::time::Instant::now(),
            pre: None,
            review: Vec::new(),
            webviews: Default::default(),
            converse: false,
            mic: None,
            mic_since: std::time::Instant::now(),
            heard: false,
            quiet_since: None,
            stt: None,
            wake: ctx.clone(),
            tidy: None,
            tidying: false,
            past: memory::Past::default(),
            touched: Default::default(),
            touch_seq: 0,
            touched_on: Default::default(),
            log_date: String::new(),
            replay_date: None,
            recall_cands: Vec::new(),
            task_topic: None,
            pics: Vec::new(),
            fake_input: Default::default(),
            fake_marks: Default::default(),
            fake_frame: 0,
            show_shelf: std::env::var("LYBOARD_SHELF").is_ok(), // 확인용: 보관함 목록을 펼친 채로 시작
            job_news: Vec::new(),
            job_turn: None,
            tidy_rest: 0,
        };
        {
            let wake = ctx.clone();
            jobs::init(app.tx.clone(), move || wake.request_repaint());
        }
        sys::register_obsidian_vault(&vault::root()); // 보관함 폴더를 만들고 옵시디언에 등록해 둔다
        // 확인용: 사람이 말하지 않아도 받아쓰기와 마이크를 시험할 수 있게 한다. 결과는 data\stt.log 에 남는다.
        if let Ok(path) = std::env::var("LYBOARD_STT_FILE") {
            // 16kHz·16비트·한 채널 소리 파일을 받아쓴다.
            std::thread::spawn(move || {
                let bytes = std::fs::read(&path).unwrap_or_default();
                let pcm: Vec<i16> = bytes.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
                if let Err(e) = eleven::transcribe(&pcm) {
                    sys::log("stt.log", &format!("받아쓰기 시험 실패: {e}"));
                }
            });
        }
        if std::env::var("LYBOARD_MIC_TEST").is_ok() {
            // 마이크를 2초 열어 소리가 들어오는지 본다.
            std::thread::spawn(|| {
                let mic = audio::Mic::start();
                std::thread::sleep(std::time::Duration::from_secs(2));
                let (err, level) = (mic.error(), mic.level());
                let pcm = mic.finish();
                sys::log("stt.log", &format!("마이크 시험: 오류={err:?}  받은 소리 {:.1}초  마지막 크기 {level:.4}", pcm.len() as f32 / 16000.0));
            });
        }
        // 시연 모드는 매번 빈 보드에서 시작한다. 실제 모드는 조작 기록을 재생해 보드를 되살린다.
        if !app.cfg.demo() {
            if std::env::args().any(|a| a == "--fresh") {
                let _ = std::fs::remove_file(log_path());
                memory::forget_turns();
            }
            app.start_real_mode();
        }
        // 확인용: 지금 보드의 구성을 판단 모델에게 한 번 물어보고 결과를 data\jev.log 에 남긴다.
        if std::env::var("LYBOARD_STRUCT").is_ok() {
            let topics: Vec<(String, String, String, String)> = app
                .board
                .topics
                .iter()
                .filter(|t| app.board.cards.iter().any(|c| c.topic == t.id))
                .map(|t| {
                    let titles: Vec<&str> = app.board.cards.iter().filter(|c| c.topic == t.id).map(|c| c.title.as_str()).collect();
                    (t.id.clone(), t.name.clone(), t.group.clone().unwrap_or_default(), titles.join(", "))
                })
                .collect();
            let groups: Vec<(String, String)> = app.board.groups.iter().map(|g| (g.id.clone(), g.name.clone())).collect();
            std::thread::spawn(move || {
                let hints = decide::structure(&topics, &groups, &[]);
                sys::log("jev.log", &format!("구성 시험 결과: {hints:?}"));
            });
        }
        // 확인용: LYBOARD_INPUT="drag 700 400 -900 0;wheel 700 400 -600;drag …" 처럼 주면 그 마우스 동작을 프로그램 안에서 흉내 낸다.
        // 동작 하나가 끝나고 화면이 가라앉을 때마다 보는 범위를 data\view.log 에 남긴다.
        if let Ok(script) = std::env::var("LYBOARD_INPUT") {
            let m = egui::Modifiers::NONE;
            let idle = |q: &mut std::collections::VecDeque<Vec<egui::Event>>, n: usize| (0..n).for_each(|_| q.push_back(Vec::new()));
            idle(&mut app.fake_input, 120);
            app.fake_marks.push_back((app.fake_input.len(), "시작".into()));
            for step in script.split(';').map(str::trim).filter(|s| !s.is_empty()) {
                let w: Vec<&str> = step.split_whitespace().collect();
                let n: Vec<f32> = w.iter().skip(1).filter_map(|x| x.parse().ok()).collect();
                let q = &mut app.fake_input;
                match (w.first().copied(), n.as_slice()) {
                    (Some("drag"), [x, y, dx, dy]) => {
                        let p = egui::pos2(*x, *y);
                        q.push_back(vec![egui::Event::PointerMoved(p)]);
                        q.push_back(vec![egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers: m }]);
                        for i in 1..=20 {
                            q.push_back(vec![egui::Event::PointerMoved(p + egui::vec2(*dx, *dy) * (i as f32 / 20.0))]);
                        }
                        idle(q, 12);
                        app.fake_marks.push_back((q.len(), format!("{step} (누르고 있는 중)")));
                        q.push_back(vec![egui::Event::PointerButton { pos: p + egui::vec2(*dx, *dy), button: egui::PointerButton::Primary, pressed: false, modifiers: m }]);
                    }
                    (Some("wheel"), [x, y, amount]) => {
                        q.push_back(vec![egui::Event::PointerMoved(egui::pos2(*x, *y))]);
                        for _ in 0..10 {
                            q.push_back(vec![egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Line, delta: egui::vec2(0.0, *amount / 10.0), phase: egui::TouchPhase::Move, source: egui::MouseWheelSource::Wheel, modifiers: m }]);
                        }
                    }
                    _ => {}
                }
                idle(q, 150);
                app.fake_marks.push_back((q.len(), format!("{step} (손 뗀 뒤)")));
            }
        }
        // 확인용: 시작하자마자 이 주제 쪽으로 날아간다.
        if let Ok(g) = std::env::var("LYBOARD_GOTO") {
            app.view.want_topic(g);
        }
        app
    }

    /// 실제 모드로 들어간다: 저장된 조작 기록을 재생해 보드를 되살리고, 이후의 조작을 기록하기 시작한다.
    /// 처음 쓰는 사람이 키를 넣은 직후에도 불린다.
    fn start_real_mode(&mut self) {
        self.board = Board::default();
        if let Ok(text) = std::fs::read_to_string(log_path()) {
            for l in text.lines() {
                if let Some((who, line)) = l.split_once('\t') {
                    if who == "T" {
                        // 날짜 표시 줄: 이 뒤의 조작은 이 날짜에 있었던 것이다.
                        self.replay_date = Some(line.trim().to_string());
                        self.log_date = line.trim().to_string();
                        continue;
                    }
                    let by = if who == "U" { Author::User } else { Author::Ai };
                    if let Ok(Some(op)) = lang::parse(line) {
                        if self.board.apply(op, by).is_ok() {
                            self.touch(line);
                        }
                    }
                }
            }
            self.board.notes.clear();
        }
        self.log = std::fs::OpenOptions::new().create(true).append(true).open(log_path()).ok();
        self.replay_date = None;
        // 지난번까지의 대화를 되살린다.
        let (past, tail) = memory::Past::load(RECENT);
        self.past = past;
        self.recent = tail.into_iter().map(|t| (t.user, t.say)).collect();
        // 프로그램이 꺼지면 작업자도 함께 멈춘다. 끝나지 못한 진행 카드에 그 사실을 적어 둔다.
        let stale: Vec<String> = self
            .board
            .cards
            .iter()
            .filter(|c| c.id.starts_with("job") && c.kind == core::Kind::Progress && c.fields.get("pct").map(|p| p != "100").unwrap_or(true))
            .filter(|c| c.fields.get("text").map(|t| !t.starts_with("멈춤") && !t.starts_with("실패")).unwrap_or(true))
            .map(|c| c.id.clone())
            .collect();
        for id in stale {
            self.apply_line(&format!("set {id} text 멈춤: 프로그램이 꺼져서 끝내지 못함"), Author::Ai);
        }
        self.focus_input = true;
    }

    /// 모든 변화가 지나가는 단 하나의 문. AI 와 사용자의 조작이 같은 형식으로 여기를 통과한다.
    fn apply_line(&mut self, line: &str, by: Author) {
        let result = match lang::parse(line) {
            Ok(Some(op)) => self.board.apply(op, by),
            Ok(None) => return,
            Err(e) => Err(e),
        };
        match result {
            Ok(()) => {
                self.touch(line);
                let today = sys::now_local().0;
                if let Some(f) = &mut self.log {
                    if self.log_date != today {
                        let _ = writeln!(f, "T\t{today}");
                        self.log_date = today;
                    }
                    let _ = writeln!(f, "{}\t{}", if by == Author::User { "U" } else { "A" }, line.trim());
                }
                // AI 가 어떤 카드를 건드렸으면, 그 카드의 주제가 화면에 보이게 한다.
                if by == Author::Ai {
                    let mut words = line.split_whitespace();
                    let (cmd, id) = (words.next().unwrap_or(""), words.next().unwrap_or(""));
                    if cmd == "restore" {
                        self.view.want_topic(id.to_string());
                    }
                    if matches!(cmd, "card" | "set" | "add" | "upd") {
                        if let Some(c) = self.board.card(id).filter(|c| !c.kind.asks()) {
                            self.view.want_topic(c.topic.clone());
                        }
                    }
                }
                if by == Author::User {
                    self.events.push(line.to_string());
                    // 내용에 대한 조작이면 사용자가 말을 걸지 않아도 AI 가 반응한다.
                    // 체크는 연달아 누를 수 있으니 잠깐 기다렸다 한 번에, 선택지는 바로.
                    let wait = if line.starts_with("upd ") {
                        Some(900)
                    } else if line.starts_with("set ") && (line.contains(" chosen ") || line.contains(" sent ")) {
                        Some(250)
                    } else {
                        None
                    };
                    if let Some(ms) = wait {
                        self.react_at = Some(std::time::Instant::now() + std::time::Duration::from_millis(ms));
                        // 누른 즉시 그 카드에 "처리 중" 표시를 켠다. AI 의 반응이 끝나면 꺼진다.
                        if let Some(card) = line.split_whitespace().nth(1) {
                            self.pending.insert(card.to_string());
                        }
                    }
                }
            }
            Err(e) if by == Author::Ai => {
                debug_log("rejects.log", &format!("{} → {e}\n", line.trim()));
                self.rejects.push(format!("{} → {e}", line.trim()));
            }
            Err(_) => {}
        }
    }

    /// 사용자가 화면에서 한 조작을 대화 기록에 남길 짧은 말로 바꾼다 (명령어 그대로는 읽기 어렵다).
    fn events_label(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        for line in self.turn_events.split(" / ") {
            let mut w = line.split_whitespace();
            let (cmd, id, third) = (w.next().unwrap_or(""), w.next().unwrap_or(""), w.next().unwrap_or(""));
            let cells: Vec<&str> = line.splitn(4, ' ').nth(3).unwrap_or("").split('|').map(str::trim).collect();
            let card = self.board.card(id);
            let kind = card.map(|c| c.kind);
            match (cmd, kind) {
                ("set", Some(core::Kind::Decision)) if third == "chosen" => {
                    let option = line.split_whitespace().nth(3).unwrap_or("");
                    let name = card.and_then(|c| c.items.iter().find(|(i, _)| i == option)).map(|(_, c)| c[0].clone()).unwrap_or_else(|| option.to_string());
                    parts.push(format!("선택: {name}"));
                }
                ("upd", Some(core::Kind::Input)) if cells.len() >= 3 => parts.push(format!("{}: {}", cells[0], cells[2])),
                ("upd", Some(core::Kind::Checklist)) if cells.len() >= 2 => parts.push(format!("{}: {}", if cells[0] == "done" { "체크" } else { "체크 해제" }, cells[1])),
                ("set", Some(core::Kind::Input)) => {}
                ("restore", _) => parts.push("보관함에서 꺼냄".to_string()),
                _ => {}
            }
        }
        if parts.is_empty() { format!("(화면에서 직접 조작) {}", self.turn_events) } else { format!("({})", parts.join(" · ")) }
    }

    /// 사진 하나를 입력창에 붙인다.
    pub fn add_pic(&mut self, ctx: &egui::Context, pic: Result<pics::Pic, String>) {
        match pic {
            Ok(_) if self.pics.len() >= 6 => self.error = Some("사진은 한 번에 6장까지 붙일 수 있습니다".into()),
            Ok(p) => {
                let small = egui::ColorImage::from_rgba_unmultiplied([p.thumb.0, p.thumb.1], &p.thumb.2);
                let tex = ctx.load_texture(format!("pic{}", self.pics.len()), small, Default::default());
                self.pics.push((p, tex));
                self.error = None;
                self.focus_input = true;
            }
            Err(e) => self.error = Some(e),
        }
    }

    /// 붙여넣기(Ctrl+V)로 온 그림과 창에 끌어다 놓은 그림 파일을 받는다.
    fn take_pics(&mut self, ctx: &egui::Context) {
        let pasted: Vec<std::sync::Arc<egui::ColorImage>> =
            ctx.input(|i| i.events.iter().filter_map(|e| if let egui::Event::PasteImage(img) = e { Some(img.clone()) } else { None }).collect());
        for img in pasted {
            let rgba: Vec<u8> = img.pixels.iter().flat_map(|p| p.to_srgba_unmultiplied()).collect();
            self.add_pic(ctx, pics::from_rgba(img.size[0], img.size[1], rgba));
        }
        let dropped: Vec<std::path::PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        for path in dropped {
            self.add_pic(ctx, pics::from_file(&path.display().to_string()));
        }
        // 확인용: LYBOARD_PIC=파일 경로 로 주면 시작할 때 그 그림을 붙인 것으로 친다.
        if let Ok(path) = std::env::var("LYBOARD_PIC") {
            if self.turn_no == 0 && self.pics.is_empty() && !self.busy {
                self.add_pic(ctx, pics::from_file(&path));
                // SAFETY: 확인용으로 한 번만 쓰고 지운다. 다른 스레드가 이 변수를 읽지 않는다.
                unsafe { std::env::remove_var("LYBOARD_PIC") };
            }
        }
    }

    /// 사용자가 처리할 일(결정 카드)을 하나 골라 앞에 가져다 놓는다. 사용자가 일을 찾아다니는 것이 아니라 일이 사용자를 찾아온다.
    /// 앞에 있던 일이 처리되어 사라졌으면 다음 일을 가져오고, 보드는 그 일이 속한 구역을 바로 옆에 붙여 보여 준다.
    fn pick_task(&mut self) {
        let asks: Vec<(String, String)> = self.board.cards.iter().filter(|c| c.kind.asks()).map(|c| (c.id.clone(), c.topic.clone())).collect();
        if let Some(cur) = &self.view.task {
            if asks.iter().any(|a| &a.0 == cur) {
                return;
            }
            self.view.task = None;
        }
        // 방금 한 답을 AI 가 반영하는 동안에는 화면이 그 일의 구역에 머문다. 반영이 끝나면 다음 일을 가져온다.
        if self.busy || self.react_at.is_some() {
            return;
        }
        // 방금 처리한 일과 같은 주제의 일이 남아 있으면 그것부터, 없으면 가장 오래 기다린 것부터.
        let next = asks.iter().find(|a| Some(&a.1) == self.task_topic.as_ref()).or_else(|| asks.first());
        if let Some((id, topic)) = next {
            self.view.task = Some(id.clone());
            self.task_topic = Some(topic.clone());
            self.view.dock_topic(topic.clone());
        }
    }

    /// 지금 일은 미뤄 두고 다음 일을 앞에 가져온다.
    pub fn next_task(&mut self) {
        let asks: Vec<(String, String)> = self.board.cards.iter().filter(|c| c.kind.asks()).map(|c| (c.id.clone(), c.topic.clone())).collect();
        if asks.len() < 2 {
            return;
        }
        let at = asks.iter().position(|a| Some(&a.0) == self.view.task.as_ref()).unwrap_or(0);
        let (id, topic) = asks[(at + 1) % asks.len()].clone();
        self.view.task = Some(id);
        self.task_topic = Some(topic.clone());
        self.view.dock_topic(topic);
    }

    /// 이 줄이 다룬 주제를 "방금 손댄 주제"로 적어 둔다.
    fn touch(&mut self, line: &str) {
        let Some(id) = line.split_whitespace().nth(1) else { return };
        let topic = match self.board.card(id) {
            Some(c) => c.topic.clone(),
            None if self.board.topics.iter().any(|t| t.id == id) => id.to_string(),
            None => return,
        };
        self.touch_seq += 1;
        // 날짜가 적히기 전의 옛 기록은 언제 것인지 알 수 없으므로 오늘로 친다 (오늘부터 나이를 센다).
        let date = self.replay_date.clone().unwrap_or_else(|| sys::now_local().0);
        self.touched_on.insert(topic.clone(), date);
        self.touched.insert(topic, self.touch_seq);
    }

    /// 모델에게 보낼 보드. 작을 때는 전부 보낸다. 오래 써서 커지면 지금 이야기와 가까운 주제만 자세히,
    /// 나머지는 카드 제목만 또는 주제 이름만 보낸다 (사람이 화면을 축소해서 보는 것과 같다). 모델은 필요하면 board_read 로 더 읽는다.
    fn board_text(&self, input: &str) -> String {
        let full = lang::serialize(&self.board);
        let limit = std::env::var("LYBOARD_BOARD_FULL").ok().and_then(|v| v.parse().ok()).unwrap_or(BOARD_FULL);
        if full.chars().count() <= limit {
            return format!("<board>\n{full}</board>\n");
        }
        let mut order: Vec<(&str, u64)> = self.board.topics.iter().map(|t| (t.id.as_str(), self.touched.get(&t.id).copied().unwrap_or(0))).collect();
        order.sort_by(|a, b| b.1.cmp(&a.1));
        let mut level: std::collections::HashMap<&str, u8> = Default::default();
        for (n, (id, _)) in order.iter().enumerate() {
            level.insert(*id, if n < 6 { 2 } else if n < 20 { 1 } else { 0 });
        }
        // 사용자가 짚고 있는 카드의 주제, 답을 기다리는 결정이 있는 주제, 이번 말에 이름이 나온 주제는 늘 자세히.
        if let Some(c) = self.selected.as_deref().and_then(|s| self.board.card(s)) {
            level.insert(c.topic.as_str(), 2);
        }
        for c in self.board.cards.iter().filter(|c| c.kind.asks()) {
            level.insert(c.topic.as_str(), 2);
        }
        let said = memory::grams(input);
        for t in &self.board.topics {
            let name = memory::grams(&t.name);
            let shared = name.iter().filter(|g| said.binary_search(g).is_ok()).count();
            if shared >= 2 || (!name.is_empty() && shared == name.len()) {
                level.insert(t.id.as_str(), 2);
            }
        }
        let text = lang::serialize_with(&self.board, &|t| level.get(t).copied().unwrap_or(0));
        format!("<board detail=\"일부만\">\n{text}</board>\n")
    }

    fn turn_text(&mut self, input: &str) -> String {
        let mut t = self.board_text(input);
        // 늘 기억하고 있는 것
        let core = memory::core();
        if !core.is_empty() {
            t += &format!("<core>\n{core}\n</core>\n");
        }
        // 최근 대화보다 오래된 턴 가운데 이번 말과 관련 있는 것을 찾아 함께 보낸다.
        let older = self.past.len().saturating_sub(recent_count());
        self.recall_cands = if input.is_empty() { Vec::new() } else { self.past.candidates(input, older, 12, 0.15) };
        // 보관함: 화면에서 내려 둔 주제들. 이번 말과 이름이나 카드 제목이 겹치는 것을 앞에 적는다.
        if !self.board.shelf.is_empty() {
            let said = memory::grams(input);
            let mut rows: Vec<(usize, String)> = self
                .board
                .shelf
                .iter()
                .enumerate()
                .map(|(n, s)| {
                    let titles: Vec<&str> = s.cards.iter().take(8).map(|c| c.title.as_str()).collect();
                    let words = memory::grams(&format!("{} {}", s.topic.name, titles.join(" ")));
                    let shared = words.iter().filter(|g| said.binary_search(g).is_ok()).count();
                    // 겹치는 것 먼저, 그다음 최근에 보관한 것 먼저
                    (shared * 100_000 + n, format!("{} {} (카드 {}장: {}){}", s.topic.id, s.topic.name, s.cards.len(), titles.join(", "), if shared >= 2 { "  ← 이번 말과 관련 있어 보임" } else { "" }))
                })
                .collect();
            rows.sort_by(|a, b| b.0.cmp(&a.0));
            let more = rows.len().saturating_sub(40);
            let lines: Vec<String> = rows.into_iter().take(40).map(|r| r.1).collect();
            t += &format!("<archived>\n{}\n{}</archived>\n", lines.join("\n"), if more > 0 { format!("(이 밖에 {more}개)\n") } else { String::new() });
        }
        if !self.recent.is_empty() {
            t += "<recent>\n";
            for (u, a) in self.recent.iter().rev().take(recent_count()).rev() {
                t += &format!("사용자: {u}\nsay: {a}\n");
            }
            t += "</recent>\n";
        }
        // 맡긴 작업: 돌고 있는 것과, 방금 끝나 보고서가 온 것
        let running = jobs::summary();
        if !running.is_empty() {
            t += &format!("<jobs>\n{running}</jobs>\n");
        }
        self.job_turn = None;
        if !self.job_news.is_empty() {
            let names: Vec<String> = self.job_news.iter().map(|n| n.1.clone()).collect();
            for (id, title, ok, report) in self.job_news.drain(..) {
                t += &format!("<job_done id=\"{id}\" title=\"{title}\" state=\"{}\">\n{report}\n</job_done>\n", if ok { "끝남" } else { "끝내지 못함" });
            }
            if input.is_empty() && self.events.is_empty() {
                self.job_turn = Some(names.join(", "));
            }
        }
        if !self.events.is_empty() {
            t += &format!("<events>\n{}\n</events>\n", self.events.join("\n"));
        }
        // 판단 모델이 지난 화면을 보고 짚은 것
        // 구성 정리 턴: 판단 모델이 짚은 것을 전한다.
        // 사용자가 말했거나 화면을 조작해서 시작된 턴에는 끼워 넣지 않는다 (그 턴은 그 요청만 처리한다).
        let own_turn = input.is_empty() && self.events.is_empty() && self.job_turn.is_none();
        if let Some(hints) = if own_turn { self.tidy.take() } else { None } {
            t += &format!("<tidy>
{}
</tidy>
", hints.join("
"));
            self.tidying = true;
        }
        if !self.review.is_empty() {
            t += &format!("<review>\n{}\n</review>\n", std::mem::take(&mut self.review).join("\n"));
        }
        let mut rejected = std::mem::take(&mut self.rejects);
        rejected.append(&mut self.board.notes);
        if !rejected.is_empty() {
            t += &format!("<rejected>\n{}\n</rejected>\n", rejected.join("\n"));
        }
        if let Some(s) = &self.selected {
            t += &format!("<selected>{s}</selected>\n");
        }
        if self.voice {
            t += "<voice>on</voice>\n";
        }
        self.events.clear();
        // 기억 보관함의 노트 목록과 지금의 환경
        let notes = vault::index();
        if !notes.is_empty() {
            t += &format!("<memory>\n{notes}</memory>\n");
        }
        let (date, time) = sys::now_local();
        let home = std::env::var(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).unwrap_or_default();
        t += &format!(
            "<env>\n지금: {date} {time}\n운영체제: {}\n사용자 홈 폴더: {home}\n기억 보관함 폴더: {}\n</env>\n",
            if cfg!(windows) { "Windows" } else { "macOS" },
            vault::root().display()
        );
        t + &format!("<input>{input}</input>")
    }

    fn send(&mut self, ctx: &egui::Context) {
        let input = self.input.trim().to_string();
        if self.busy || (input.is_empty() && self.pics.is_empty()) {
            return;
        }
        let input = if input.is_empty() { "(사진을 보냄)".to_string() } else { input };
        self.input.clear();
        self.start_turn(ctx, input);
    }

    /// 한 턴을 시작한다. `input` 이 비어 있으면 화면 조작만으로 시작된 턴이다.
    fn start_turn(&mut self, ctx: &egui::Context, input: String) {
        self.flush_speech(); // 말하는 중에 새로 말을 걸면 하던 말을 멈춘다
        self.turn_events = self.events.join(" / ");
        let mut text = self.turn_text(&input);
        // 붙여 둔 사진: 이번 말과 함께 보내고, 나중에 다시 볼 수 있게 보관함에도 남긴다.
        let mut images = Vec::new();
        let mut saved = Vec::new();
        if !input.is_empty() {
            let (date, time) = sys::now_local();
            let dir = vault::root().join("첨부");
            let _ = std::fs::create_dir_all(&dir);
            for (n, (pic, _)) in self.pics.drain(..).enumerate() {
                let path = dir.join(format!("{date} {}-{}.{}", time.replace(':', ""), n + 1, pic.ext()));
                if std::fs::write(&path, &pic.data).is_ok() {
                    saved.push(path.display().to_string());
                }
                images.push((pic.media.to_string(), pic.b64()));
            }
        }
        if !images.is_empty() {
            text += &format!("\n<images>\n사용자가 이번 말에 사진 {}장을 붙였다 (이 메시지 앞에 들어 있다). 저장된 곳:\n{}\n</images>", images.len(), saved.join("\n"));
        }
        let pics_note = if images.is_empty() { String::new() } else { format!("(사진 {}장) ", images.len()) };
        if std::env::var("LYBOARD_DEBUG").is_ok() {
            debug_log("turn.log", &format!("{text}\n=====\n")); // 확인용: 모델에게 보낸 것 그대로
        }
        self.react_at = None;
        self.busy = true;
        self.error = None;
        let auto = input.is_empty(); // 사용자가 말한 것이 아니라 프로그램이 시작한 턴
        self.cur_input = if let Some(names) = &self.job_turn {
            format!("(맡긴 작업이 끝남) {names}")
        } else if self.tidying {
            "(분류와 주제 정리)".to_string()
        } else if auto {
            self.events_label()
        } else {
            format!("{pics_note}{input}")
        };
        self.turn_raw.clear();
        self.turn_actions.clear();
        self.turn_spoken.clear();
        self.activity.clear();
        self.stream = lang::LineStream::default();
        self.got_output = false;
        self.pre = None;
        self.sent_at = std::time::Instant::now();
        // 판단 모델이 먼저 볼 재료
        let pre = llm::PreInput {
            input: if auto { String::new() } else { self.cur_input.clone() },
            topics: self.board.topics.iter().map(|t| (t.id.clone(), t.name.clone())).collect(),
            // 핵심 기억은 어차피 매 턴 통째로 가므로 미리 읽을 후보에서 뺀다.
            notes: vault::notes().into_iter().filter(|(p, _)| p != memory::CORE).collect(),
            recent: self.recent.iter().rev().take(3).rev().map(|(u, a)| format!("사용자: {u}\n응답: {a}\n")).collect(),
            voice: self.voice,
            recall: std::mem::take(&mut self.recall_cands),
        };
        let ctx = ctx.clone();
        llm::run_turn(self.cfg.clone(), text, images, pre, self.turn_no, self.tx.clone(), move || ctx.request_repaint());
        self.turn_no += 1;
    }

    /// AI 가 낸 한 줄을 받는다. 음성이 꺼져 있으면 바로 화면에 반영하고,
    /// 켜져 있으면 "문장 → 그 문장의 카드" 순서로 줄을 세워 말하는 속도에 맞춰 내보낸다.
    fn ai_line(&mut self, line: &str) {
        let t = line.trim();
        if !self.voice {
            if !t.starts_with("tell ") {
                self.apply_line(line, Author::Ai);
            }
            return;
        }
        let spoken = t.strip_prefix("tell ").or_else(|| t.strip_prefix("say ")).map(str::trim).filter(|s| !s.is_empty());
        if let Some(s) = spoken {
            // say 는 화면 아래 한마디도 함께 바꾼다
            let lines = if t.starts_with("say ") { vec![line.to_string()] } else { Vec::new() };
            self.speech.push_back((s.to_string(), lines));
        } else if let Some(last) = self.speech.back_mut() {
            last.1.push(line.to_string()); // 아직 읽지 않은 문장에 딸린 명령: 그 문장 차례까지 기다린다
        } else {
            self.apply_line(line, Author::Ai); // 지금 읽는 문장에 딸린 명령: 바로 나간다
        }
        self.advance_speech();
    }

    /// 앞 문장을 다 읽었으면 다음 문장을 읽기 시작하고 그 문장의 카드를 띄운다.
    fn advance_speech(&mut self) {
        if self.tts.is_speaking() {
            return;
        }
        match self.speech.pop_front() {
            Some((sentence, lines)) => {
                self.tts.speak(&sentence);
                self.turn_spoken.push(sentence.clone());
                self.caption = Some(sentence);
                for l in lines {
                    self.apply_line(&l, Author::Ai);
                }
            }
            None => self.caption = None,
        }
    }

    /// 말을 멈추고, 아직 화면에 나가지 않은 명령을 전부 반영한다 (보드가 어긋나지 않도록).
    fn flush_speech(&mut self) {
        self.tts.stop();
        self.caption = None;
        let rest: Vec<String> = self.speech.drain(..).flat_map(|(_, lines)| lines).collect();
        for l in rest {
            self.apply_line(&l, Author::Ai);
        }
    }

    fn set_voice(&mut self, on: bool) {
        self.voice = on;
        if !on {
            self.flush_speech();
        }
        let _ = std::fs::write(voice_path(), if on { "on" } else { "off" });
    }

    fn finish_turn(&mut self) {
        if let Some(rest) = self.stream.finish() {
            self.ai_line(&rest);
        }
        for c in &mut self.board.cards {
            c.live = None;
        }
        self.say_live = None;
        self.busy = false;
        if self.react_at.is_none() {
            self.pending.clear();
        }
        self.idle_since = std::time::Instant::now();
        self.activity.clear();
        self.pre = None;
        // 구성 정리 턴인데 고칠 것이 없어 아무것도 내지 않았으면, 대화에도 기록에도 남기지 않는다.
        let silent = self.tidying && self.turn_raw.trim().is_empty() && self.error.is_none();
        // 이번 턴에 손댄 카드와 주제 (보드가 아무리 커져도 검토와 기록은 이것만 본다)
        let mut touched_cards: Vec<String> = Vec::new();
        for l in self.turn_raw.lines() {
            if let Some(id) = l.split_whitespace().nth(1) {
                if self.board.card(id).is_some() && !touched_cards.iter().any(|x| x == id) {
                    touched_cards.push(id.to_string());
                }
            }
        }
        let mut hot_topics: Vec<String> = Vec::new();
        for id in &touched_cards {
            let t = self.board.card(id).map(|c| c.topic.clone()).unwrap_or_default();
            if !hot_topics.contains(&t) {
                hot_topics.push(t);
            }
        }
        let topic_names: Vec<String> = hot_topics.iter().filter_map(|id| self.board.topics.iter().find(|t| &t.id == id).map(|t| t.name.clone())).collect();
        // 메인 모델이 화면을 고친 뒤: 판단 모델이 화면만 보고 카드마다 "무엇인지 분명한가", "얼마나 중요한가"를 본다.
        if !self.cfg.demo() && !self.board.cards.is_empty() {
            // 오래 손대지 않은 주제: 보관하자고 짚는다 (답을 기다리는 결정이 있는 주제는 빼고).
            let limit = std::env::var("LYBOARD_STALE_DAYS").ok().and_then(|v| v.parse().ok()).unwrap_or(STALE_DAYS);
            let today = days_of(&sys::now_local().0);
            let mut stale: Vec<(i64, String)> = self
                .board
                .topics
                .iter()
                .filter(|t| !hot_topics.contains(&t.id))
                .filter(|t| !self.board.cards.iter().any(|c| c.topic == t.id && (c.kind.asks() || c.id.starts_with("job"))))
                .filter_map(|t| {
                    let age = today - days_of(self.touched_on.get(&t.id)?);
                    (age >= limit).then(|| (age, format!("주제 {}({})는 {age}일 동안 손대지 않음. 끝났거나 한동안 볼 일이 없으면 archive 로 보관할 것", t.id, t.name)))
                })
                .collect();
            stale.sort_by(|a, b| b.0.cmp(&a.0));
            let stale: Vec<String> = stale.into_iter().take(6).map(|s| s.1).collect();
            let board = lang::serialize_with(&self.board, &|t| if hot_topics.iter().any(|h| h == t) { 2 } else { 0 });
            let cards: Vec<(String, String, String)> = touched_cards
                .iter()
                .rev()
                .take(20)
                .filter_map(|id| self.board.card(id))
                .map(|c| {
                    let topic = self.board.topics.iter().find(|t| t.id == c.topic).map(|t| t.name.clone()).unwrap_or_default();
                    (c.id.clone(), topic, c.title.clone())
                })
                .collect();
            // 그리고 분류와 주제의 구성도 본다: 합칠 주제, 나눌 분류, 겹치는 분류.
            let topics: Vec<(String, String, String, String)> = self
                .board
                .topics
                .iter()
                .filter(|t| self.board.cards.iter().any(|c| c.topic == t.id))
                .map(|t| {
                    let titles: Vec<&str> = self.board.cards.iter().filter(|c| c.topic == t.id).take(6).map(|c| c.title.as_str()).collect();
                    (t.id.clone(), t.name.clone(), t.group.clone().unwrap_or_default(), titles.join(", "))
                })
                .collect();
            let groups: Vec<(String, String)> = self.board.groups.iter().map(|g| (g.id.clone(), g.name.clone())).collect();
            let tx = self.tx.clone();
            let wake = self.wake.clone();
            let hot = hot_topics.clone();
            std::thread::spawn(move || {
                if let Some(post) = decide::after(&board, &cards) {
                    let _ = tx.send(llm::Msg::Review(post));
                }
                let mut hints = decide::structure(&topics, &groups, &hot);
                hints.extend(stale);
                if !hints.is_empty() {
                    let _ = tx.send(llm::Msg::Tidy(hints));
                }
                wake.request_repaint();
            });
        }
        // 이 턴을 기억에 남긴다: 턴 기록에 덧붙이고, 뒤에서 작은 모델이 오래 남길 사실을 골라 핵심 기억에 적는다.
        if !self.cfg.demo() && !silent {
            let (date, time) = sys::now_local();
            let turn = memory::Turn {
                at: format!("{date} {time}"),
                user: self.cur_input.clone(),
                say: self.board.say.clone(),
                did: self.turn_actions.clone(),
                ops: self.turn_raw.trim().chars().take(2000).collect(),
                topics: topic_names.join(", "),
            };
            let turn_index = self.past.len();
            self.past.push(&turn);
            if let (Some(key), false) = (self.cfg.key.clone(), self.tidying) {
                let events = self.turn_events.clone();
                let (tx, wake) = (self.tx.clone(), self.wake.clone());
                std::thread::spawn(move || {
                    if let Some(keys) = memory::keep(&key, &turn, &events) {
                        let _ = tx.send(llm::Msg::Keys(turn_index, keys));
                        wake.request_repaint();
                    }
                });
            }
        }
        // 방금 턴이 구성 정리였으면 한동안 다시 정리하지 않는다 (정리가 정리를 부르지 않게).
        if self.tidying {
            self.tidying = false;
            self.tidy_rest = 3;
        } else {
            self.tidy_rest = self.tidy_rest.saturating_sub(1);
        }
        // 이 턴을 오늘의 기록 노트에 남긴다 (시연 모드는 남기지 않는다).
        if !self.cfg.demo() && !silent {
            vault::log_turn(&vault::TurnRecord {
                input: self.cur_input.clone(),
                say: self.board.say.clone(),
                spoken: self.turn_spoken.clone(),
                actions: self.turn_actions.clone(),
                raw: self.turn_raw.clone(),
                error: self.error.clone(),
            });
        }
        if !silent {
            self.recent.push((std::mem::take(&mut self.cur_input), self.board.say.clone()));
        }
    }

    /// 맡긴 작업의 소식을 화면에 반영한다. 작업마다 진행 카드가 하나씩 있다 (카드 ID = 작업 ID).
    fn job_event(&mut self, ev: jobs::Event) {
        let clip = |s: &str, n: usize| s.replace('\n', " ").chars().take(n).collect::<String>();
        match ev {
            jobs::Event::Started { id, title, topic } => {
                let topic = match topic.filter(|t| self.board.topics.iter().any(|x| &x.id == t)) {
                    Some(t) => t,
                    None => {
                        if !self.board.topics.iter().any(|t| t.id == "jobs") {
                            self.apply_line("topic jobs 맡긴 작업", Author::Ai);
                        }
                        "jobs".to_string()
                    }
                };
                self.apply_line(&format!("card {id} progress {topic} 3x2 {}", clip(&title, 40)), Author::Ai);
                self.apply_line(&format!("set {id} pct 0"), Author::Ai);
                self.apply_line(&format!("set {id} text 작업자가 시작함"), Author::Ai);
            }
            jobs::Event::Progress { id, pct, text } => {
                if self.board.card(&id).is_none() {
                    return;
                }
                if let Some(p) = pct {
                    self.apply_line(&format!("set {id} pct {}", p.min(99)), Author::Ai);
                }
                if let Some(t) = text {
                    self.apply_line(&format!("set {id} text {}", clip(&t, 60)), Author::Ai);
                }
            }
            jobs::Event::Finished { id, title, ok, report } => {
                if self.board.card(&id).is_some() {
                    if ok {
                        self.apply_line(&format!("set {id} pct 100"), Author::Ai);
                        self.apply_line(&format!("set {id} text 끝남 · 결과를 정리하는 중"), Author::Ai);
                    } else {
                        self.apply_line(&format!("set {id} text 실패: 끝내지 못함"), Author::Ai);
                    }
                }
                self.job_news.push((id, title, ok, report));
                self.idle_since = std::time::Instant::now();
            }
        }
        self.rejects.clear(); // 프로그램이 낸 명령의 거부는 모델에게 알릴 일이 아니다
    }

    fn pump(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                llm::Msg::Activity(a) => {
                    if !a.is_empty() {
                        self.turn_actions.push(a.clone());
                        self.got_output = true;
                    }
                    self.activity = a;
                }
                llm::Msg::Pre(p) => {
                    // 음성 대화에서는 판단 모델의 결정이 나오자마자(약 0.2초) 먼저 대답한다.
                    if let Some(ack) = p.ack.clone().filter(|_| self.voice) {
                        self.tts.speak(&ack);
                        self.caption = Some(ack.clone());
                        self.turn_spoken.push(ack);
                    }
                    self.pre = Some(p);
                }
                llm::Msg::Job(ev) => self.job_event(ev),
                llm::Msg::Keys(i, keys) => self.past.add_keys(i, &keys),
                llm::Msg::Board { topic, reply } => {
                    let found = self.board.topics.iter().any(|t| t.id == topic);
                    let _ = reply.send(if found {
                        self.touch_seq += 1;
                        self.touched.insert(topic.clone(), self.touch_seq);
                        lang::serialize_with(&self.board, &|t| if t == topic { 2 } else { 0 }).lines().filter(|l| !l.starts_with("group ") && !l.starts_with("put ") && !l.starts_with('#') && (!l.starts_with("topic ") || l.split_whitespace().nth(1) == Some(topic.as_str()))).collect::<Vec<_>>().join("
")
                    } else {
                        format!("오류: '{topic}' 라는 주제가 없음")
                    });
                }
                llm::Msg::Tidy(hints) => {
                    if self.tidy_rest == 0 {
                        self.tidy = Some(hints);
                    }
                }
                llm::Msg::Review(post) => {
                    self.review = post
                        .unclear
                        .iter()
                        .map(|(id, p)| format!("카드 {id}: 화면만 봐서는 무엇에 관한 것인지 알기 어려움 (분명할 확률 {p:.2})"))
                        .collect();
                    self.view.set_importance(post.importance);
                }
                llm::Msg::Browser { card, script, reply } => match self.webviews.get(&card) {
                    Some(w) => {
                        let done = reply.clone();
                        if let Err(e) = w.view.evaluate_script_with_callback(&script, move |result| {
                            let _ = done.send(result);
                        }) {
                            let _ = reply.send(format!("오류: {e}"));
                        }
                    }
                    None => {
                        let _ = reply.send(format!("오류: '{card}' 카드에 떠 있는 페이지가 없음 (web 카드를 만들고 url 을 정한 뒤 다시 시도)"));
                    }
                },
                llm::Msg::Screen { ops, reply } => {
                    self.got_output = true;
                    // 일하는 도중의 화면 수정. 거부된 것이 있으면 이유를 바로 돌려준다.
                    self.turn_raw += &format!("{ops}\n");
                    debug_log("raw.log", &format!("{ops}\n"));
                    let before = self.rejects.len();
                    for l in ops.lines() {
                        self.ai_line(l);
                    }
                    let new: Vec<String> = self.rejects.drain(before..).collect();
                    let mut notes = std::mem::take(&mut self.board.notes);
                    notes.extend(new);
                    let _ = reply.send(if notes.is_empty() { "반영됨".to_string() } else { format!("일부 거부되거나 잘림:\n{}", notes.join("\n")) });
                }
                llm::Msg::Delta(d) => {
                    if !d.trim().is_empty() {
                        self.got_output = true;
                    }
                    self.turn_raw += &d;
                    debug_log("raw.log", &d);
                    for chunk in self.stream.feed(&d) {
                        match chunk {
                            lang::Chunk::Line(l) => {
                                self.say_live = None;
                                self.ai_line(&l);
                            }
                            // 음성일 때는 말하는 순서에 맞춰 나가야 하므로 미리 보여 주지 않는다.
                            lang::Chunk::Partial(_) if self.voice => {}
                            lang::Chunk::Partial(p) => {
                                // 줄이 끝나기 전에도 글자가 흘러 들어오는 것을 보여 준다.
                                if let Some((id, field, value)) = lang::partial_text(&p) {
                                    if id.is_empty() {
                                        self.say_live = Some(value);
                                    } else if let Some(c) = self.board.cards.iter_mut().find(|c| c.id == id) {
                                        c.live = Some((field, value));
                                    }
                                }
                            }
                        }
                    }
                }
                llm::Msg::Done(stats) => {
                    self.stats = Some(stats);
                    self.finish_turn();
                }
                llm::Msg::Error(e) => {
                    debug_log("rejects.log", &format!("[오류] {e}\n"));
                    self.error = Some(e);
                    self.finish_turn();
                }
            }
        }
    }

    /// 말로 대화하기. 마이크 스위치가 켜져 있는 동안: 듣는다 → 말이 끝나면 글로 바꿔 보낸다 → AI 가 답한다 → 다시 듣는다.
    /// AI 가 말하는 동안에는 마이크를 닫아서 자기 목소리를 듣지 않게 한다.
    fn voice_input(&mut self, ctx: &egui::Context, talking: bool) {
        // 받아쓰기 결과가 왔는가
        if let Some(rx) = &self.stt {
            match rx.try_recv() {
                Ok(Ok(text)) => {
                    self.stt = None;
                    if !text.is_empty() {
                        self.input = text;
                        self.send(ctx);
                    }
                }
                Ok(Err(e)) => {
                    self.stt = None;
                    self.error = Some(format!("받아쓰기 실패: {e}"));
                }
                Err(_) => ctx.request_repaint_after(std::time::Duration::from_millis(50)),
            }
        }
        if !self.converse {
            self.mic = None;
            return;
        }
        let free = !self.busy && !talking && self.stt.is_none() && self.react_at.is_none();
        match &self.mic {
            None if free => {
                self.mic = Some(audio::Mic::start());
                self.mic_since = std::time::Instant::now();
                self.heard = false;
                self.quiet_since = None;
            }
            None => {}
            Some(mic) => {
                if let Some(e) = mic.error() {
                    self.error = Some(format!("마이크를 쓸 수 없음: {e}"));
                    self.converse = false;
                    self.mic = None;
                    return;
                }
                // 말소리가 들리다가 1.1초 조용하면 말이 끝난 것으로 본다.
                let loud = mic.level() > MIC_THRESHOLD;
                if loud {
                    self.heard = true;
                    self.quiet_since = None;
                } else if self.heard && self.quiet_since.is_none() {
                    self.quiet_since = Some(std::time::Instant::now());
                }
                let ended = self.quiet_since.map(|t| t.elapsed().as_millis() > 1100).unwrap_or(false);
                let too_long = self.mic_since.elapsed().as_secs() > 45;
                if ended || too_long {
                    let pcm = self.mic.take().map(|m| m.finish()).unwrap_or_default();
                    if self.heard && pcm.len() > 16000 / 3 {
                        let (tx, rx) = channel();
                        self.stt = Some(rx);
                        let wake = ctx.clone();
                        std::thread::spawn(move || {
                            let _ = tx.send(eleven::transcribe(&pcm));
                            wake.request_repaint();
                        });
                    }
                }
                ctx.request_repaint_after(std::time::Duration::from_millis(40));
            }
        }
    }

    /// 웹 카드마다 브라우저 화면을 하나씩 두고, 카드가 움직이면 따라가게 한다.
    fn sync_webviews(&mut self, frame: &eframe::Frame, slots: Vec<ui::WebSlot>, zoom: f32) {
        self.webviews.retain(|id, _| slots.iter().any(|s| &s.id == id));
        for s in slots {
            if s.url.trim().is_empty() {
                continue;
            }
            let url = to_url(&s.url);
            let bounds = wry::Rect {
                position: wry::dpi::LogicalPosition::new((s.rect.left() * zoom) as f64, (s.rect.top() * zoom) as f64).into(),
                size: wry::dpi::LogicalSize::new((s.rect.width() * zoom) as f64, (s.rect.height() * zoom) as f64).into(),
            };
            match self.webviews.get_mut(&s.id) {
                Some(w) => {
                    if w.url != url {
                        let _ = w.view.load_url(&url);
                        w.url = url;
                    }
                    if w.rect != s.rect {
                        let _ = w.view.set_bounds(bounds);
                        w.rect = s.rect;
                    }
                    if w.visible != s.visible {
                        let _ = w.view.set_visible(s.visible);
                        w.visible = s.visible;
                    }
                }
                None if s.visible => {
                    let built = wry::WebViewBuilder::new().with_bounds(bounds).with_url(&url).with_focused(false).build_as_child(frame);
                    match built {
                        Ok(view) => {
                            self.webviews.insert(s.id.clone(), Web { view, url, rect: s.rect, visible: true });
                        }
                        Err(e) => self.error = Some(format!("브라우저 화면을 띄우지 못함: {e}")),
                    }
                }
                None => {}
            }
        }
    }

    fn status_line(&self) -> String {
        let mut parts = Vec::new();
        if self.cfg.demo() {
            parts.push(if std::env::var("LYBOARD_DEMO").is_ok() { "시연 모드 · 무엇을 입력해도 준비된 장면이 재생됩니다" } else { "API 키를 넣으면 시작합니다" }.to_string());
        }
        if let Some(s) = &self.stats {
            parts.push(s.model.clone());
            parts.push(format!("첫 반응 {:.1}초 · 전체 {:.1}초", s.first_ms as f32 / 1000.0, s.total_ms as f32 / 1000.0));
            if s.input_tokens > 0 {
                parts.push(format!("입력 {} (캐시 {}) · 출력 {} 토큰", s.input_tokens, s.cached_tokens, s.output_tokens));
            }
        }
        let text: usize = self.board.cards.iter().map(card_chars).sum();
        parts.push(format!("카드 {} · 화면 글자 {}", self.board.cards.len(), text));
        let workers = jobs::running();
        if workers > 0 {
            parts.push(format!("작업자 {workers}명이 일하는 중"));
        }
        parts.push(format!("확대 {:.0}% (스크롤로 확대·축소, 빈 곳을 끌어 이동, 두 번 눌러 전체 보기)", self.view.zoom * 100.0));
        if !self.rejects.is_empty() {
            parts.push(format!("거부된 명령 {}", self.rejects.len()));
        }
        if let Some(s) = &self.selected {
            parts.push(format!("선택: {s}"));
        }
        parts.join("   ·   ")
    }
}

/// 모델이 실제로 내보낸 원문과 거부된 명령을 data 폴더에 남긴다 (수정 언어를 다듬는 근거 자료).
fn debug_log(name: &str, text: &str) {
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(sys::data_dir().join(name)) {
        let _ = f.write_all(text.as_bytes());
    }
}

/// 화면에 실제로 보이는 글자 수 (이 프로그램의 성과 지표).
fn card_chars(c: &core::Card) -> usize {
    c.title.chars().count()
        + c.fields.values().map(|v| v.chars().count()).sum::<usize>()
        + c.items.iter().flat_map(|(_, cells)| cells.iter()).map(|v| v.chars().count()).sum::<usize>()
}

impl eframe::App for App {
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        if self.fake_input.is_empty() && self.fake_marks.is_empty() {
            return;
        }
        if let Some(events) = self.fake_input.pop_front() {
            raw.events.extend(events);
        }
        self.fake_frame += 1;
        while self.fake_marks.front().map(|m| m.0 <= self.fake_frame).unwrap_or(false) {
            let (_, name) = self.fake_marks.pop_front().unwrap();
            if let Some((seen, limit)) = self.view.seen() {
                let out = (limit.min.x - seen.min.x).max(seen.max.x - limit.max.x).max(limit.min.y - seen.min.y).max(seen.max.y - limit.max.y);
                let fits = seen.width() >= limit.width() - 1.0 || seen.height() >= limit.height() - 1.0;
                sys::log(
                    "view.log",
                    &format!(
                        "{name}: 확대 {:.0}%  보는 범위 x {:.0}~{:.0} y {:.0}~{:.0}  한계 x {:.0}~{:.0} y {:.0}~{:.0}  한계 밖으로 나간 거리 {:.0}{}",
                        self.view.zoom * 100.0,
                        seen.min.x, seen.max.x, seen.min.y, seen.max.y,
                        limit.min.x, limit.max.x, limit.min.y, limit.max.y,
                        out.max(0.0),
                        if fits { " (전체 보기라 보는 범위가 한계보다 넓은 축이 있음)" } else { "" }
                    ),
                );
            }
        }
        ctx.request_repaint();
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        // 확인용: LYBOARD_SHOT=파일.bmp 가 있으면 잠시 뒤 화면을 그 파일로 저장한다 (PC 가 잠겨 있어도 된다).
        if let Some(path) = self.shot.clone() {
            if self.shot_from.elapsed().as_millis() as u64 > self.shot_delay {
                self.shot = None;
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::ScreenshotCallback::new(move |img| save_bmp(&path, &img))));
            }
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        }
        self.pump();
        let ctx = ui.ctx().clone();
        if !self.titlebar_done {
            // 창이 만들어진 뒤에야 적용되므로 될 때까지 시도한다.
            self.titlebar_done = sys::dark_titlebar(TITLE, (16, 17, 19), (138, 139, 145));
        }
        if let Some(at) = self.react_at {
            if !self.busy && std::time::Instant::now() >= at {
                self.start_turn(&ctx, String::new());
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(60));
            }
        }
        if self.voice {
            self.advance_speech();
        }
        let talking = self.tts.is_speaking() || !self.speech.is_empty();
        self.voice_input(&ctx, talking);
        // 맡긴 작업이 끝났으면, 사용자가 말하고 있지 않을 때 그 결과를 화면에 정리하는 턴을 돈다.
        if !self.job_news.is_empty() {
            let quiet = !self.busy && self.react_at.is_none() && !talking && self.stt.is_none() && !(self.mic.is_some() && self.heard);
            if quiet && self.input.trim().is_empty() && self.idle_since.elapsed().as_millis() > 700 {
                self.start_turn(&ctx, String::new());
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(200));
            }
        } else if self.tidy.is_some() {
            let quiet = !self.busy && self.react_at.is_none() && !talking && self.stt.is_none() && !(self.mic.is_some() && self.heard);
            if quiet && self.input.trim().is_empty() && self.autoplay.is_empty() && self.idle_since.elapsed().as_millis() > 2500 {
                self.start_turn(&ctx, String::new());
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(300));
            }
        }
        if !self.autoplay.is_empty() {
            if !self.busy && !talking && self.job_news.is_empty() && self.idle_since.elapsed().as_millis() > 1200 {
                // 확인용 대본에서는 "|" 가 입력 구분자라서, 조작 줄 안의 "|" 는 "¦" 로 적는다.
                let next = self.autoplay.pop_front().unwrap_or_default().replace('¦', "|");
                if let Some(line) = next.strip_prefix('!') {
                    // "!" 로 시작하면 입력이 아니라 사용자의 화면 조작으로 넣는다 (클릭을 흉내 내는 확인용).
                    // 한 번의 조작이 여러 줄이면 ⏎ 로 이어 적는다 (입력 카드의 보내기처럼).
                    for l in line.split('⏎') {
                        self.apply_line(l.trim(), Author::User);
                    }
                    self.idle_since = std::time::Instant::now() + std::time::Duration::from_secs(2);
                } else {
                    self.input = next;
                    self.send(&ctx);
                }
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }

        self.take_pics(&ctx);
        self.pick_task();
        self.chat_panel(ui, &ctx, talking);

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(ui::BG).inner_margin(egui::Margin { left: 18, right: 18, top: 18, bottom: 0 }))
            .show(ui, |ui| {
                let mut lines = Vec::new();
                let mut clicked = None;
                // 판단 모델이 "이 주제의 이야기"라고 본 구역은 답이 오기 전부터 강조한다.
                let hot = if self.busy { self.pre.as_ref().and_then(|p| p.topic.clone()) } else { None };
                let mut webs = Vec::new();
                let moving =
                    ui::board(ui, &self.board, &mut self.view, &self.selected, &self.pending, &mut lines, &mut clicked, hot.as_deref(), &mut webs);
                self.sync_webviews(frame, webs, ctx.zoom_factor());
                for l in lines {
                    self.apply_line(&l, Author::User);
                }
                if let Some(id) = clicked {
                    self.selected = if self.selected.as_deref() == Some(id.as_str()) { None } else { Some(id) };
                }
                if moving {
                    ctx.request_repaint();
                }
            });
    }
}

fn setup_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let candidates = ["C:\\Windows\\Fonts\\malgun.ttf", "/System/Library/Fonts/Supplemental/AppleGothic.ttf"];
    for path in candidates {
        if let Ok(bytes) = std::fs::read(path) {
            fonts.font_data.insert("kr".into(), Arc::new(egui::FontData::from_owned(bytes)));
            for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                fonts.families.entry(family).or_default().insert(0, "kr".into());
            }
            break;
        }
    }
    // 제목과 숫자에 쓰는 굵은 글꼴. 없으면 보통 글꼴로 대신한다.
    let mut bold = fonts.families.get(&egui::FontFamily::Proportional).cloned().unwrap_or_default();
    if let Ok(bytes) = std::fs::read("C:\\Windows\\Fonts\\malgunbd.ttf") {
        fonts.font_data.insert("kr-bold".into(), Arc::new(egui::FontData::from_owned(bytes)));
        bold.insert(0, "kr-bold".into());
    }
    fonts.families.insert(egui::FontFamily::Name("bold".into()), bold);
    ctx.set_fonts(fonts);
}

fn main() -> eframe::Result {
    // 자동 재생으로 띄울 때는 사용자가 쓰던 창의 입력을 가로채지 않도록 포커스를 가져오지 않는다.
    let quiet = std::env::var("LYBOARD_AUTOPLAY").is_ok();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(TITLE)
            .with_inner_size([1560.0, 920.0])
            .with_active(!quiet)
            // 확인용: 창을 띄울 위치 (LYBOARD_POS="x,y")
            .with_position(std::env::var("LYBOARD_POS").ok().and_then(|v| v.split_once(',').and_then(|(x, y)| Some([x.trim().parse::<f32>().ok()?, y.trim().parse::<f32>().ok()?]))).unwrap_or([80.0, 60.0])),
        ..Default::default()
    };
    eframe::run_native(
        TITLE,
        options,
        Box::new(|cc| {
            setup_fonts(&cc.egui_ctx);
            cc.egui_ctx.set_theme(egui::Theme::Dark);
            cc.egui_ctx.style_mut_of(egui::Theme::Dark, |s| {
                s.visuals.selection.bg_fill = egui::Color32::from_rgb(70, 71, 77);
                s.visuals.selection.stroke = egui::Stroke::new(1.0, ui::DIM);
                s.visuals.text_cursor.stroke = egui::Stroke::new(1.5, ui::TEXT);
            });
            Ok(Box::new(App::new(&cc.egui_ctx)))
        }),
    )
}

/// 화면 그림을 24비트 BMP 로 저장한다 (확인용. 외부 라이브러리 없이 쓸 수 있는 가장 단순한 그림 형식).
fn save_bmp(path: &str, img: &egui::ColorImage) {
    let (w, h) = (img.size[0], img.size[1]);
    let row = (w * 3).div_ceil(4) * 4;
    let mut out = Vec::with_capacity(54 + row * h);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&((54 + row * h) as u32).to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&(h as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    out.extend_from_slice(&[0u8; 24]);
    for y in (0..h).rev() {
        let start = out.len();
        for x in 0..w {
            let c = img.pixels[y * w + x];
            out.extend_from_slice(&[c.b(), c.g(), c.r()]);
        }
        out.resize(start + row, 0);
    }
    let _ = std::fs::write(path, out);
}
