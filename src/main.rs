#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod agent;
mod audio;
mod chat;
mod core;
mod decide;
mod eleven;
mod jev;
mod lang;
mod llm;
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
        };
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
            }
            app.start_real_mode();
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
                    let by = if who == "U" { Author::User } else { Author::Ai };
                    if let Ok(Some(op)) = lang::parse(line) {
                        let _ = self.board.apply(op, by);
                    }
                }
            }
            self.board.notes.clear();
        }
        self.log = std::fs::OpenOptions::new().create(true).append(true).open(log_path()).ok();
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
                if let Some(f) = &mut self.log {
                    let _ = writeln!(f, "{}\t{}", if by == Author::User { "U" } else { "A" }, line.trim());
                }
                // AI 가 어떤 카드를 건드렸으면, 그 카드의 주제가 화면에 보이게 한다.
                if by == Author::Ai {
                    let mut words = line.split_whitespace();
                    let (cmd, id) = (words.next().unwrap_or(""), words.next().unwrap_or(""));
                    if matches!(cmd, "card" | "set" | "add" | "upd") {
                        if let Some(c) = self.board.card(id).filter(|c| c.kind != core::Kind::Decision) {
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
                    } else if line.starts_with("set ") && line.contains(" chosen ") {
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

    fn turn_text(&mut self, input: &str) -> String {
        let mut t = format!("<board>\n{}</board>\n", lang::serialize(&self.board));
        if !self.recent.is_empty() {
            t += "<recent>\n";
            for (u, a) in self.recent.iter().rev().take(10).rev() {
                t += &format!("사용자: {u}\nsay: {a}\n");
            }
            t += "</recent>\n";
        }
        if !self.events.is_empty() {
            t += &format!("<events>\n{}\n</events>\n", self.events.join("\n"));
        }
        // 판단 모델이 지난 화면을 보고 짚은 것
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
        if self.busy || input.is_empty() {
            return;
        }
        self.input.clear();
        self.start_turn(ctx, input);
    }

    /// 한 턴을 시작한다. `input` 이 비어 있으면 화면 조작만으로 시작된 턴이다.
    fn start_turn(&mut self, ctx: &egui::Context, input: String) {
        self.flush_speech(); // 말하는 중에 새로 말을 걸면 하던 말을 멈춘다
        self.turn_events = self.events.join(" / ");
        let text = self.turn_text(&input);
        self.react_at = None;
        self.busy = true;
        self.error = None;
        self.cur_input = if input.is_empty() { format!("(화면에서 직접 조작) {}", self.turn_events) } else { input };
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
            input: if self.turn_events.is_empty() || !self.cur_input.starts_with("(화면에서") { self.cur_input.clone() } else { String::new() },
            topics: self.board.topics.iter().map(|t| (t.id.clone(), t.name.clone())).collect(),
            notes: vault::notes(),
            recent: self.recent.iter().rev().take(3).rev().map(|(u, a)| format!("사용자: {u}\n응답: {a}\n")).collect(),
            voice: self.voice,
        };
        let ctx = ctx.clone();
        llm::run_turn(self.cfg.clone(), text, pre, self.turn_no, self.tx.clone(), move || ctx.request_repaint());
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
        // 메인 모델이 화면을 고친 뒤: 판단 모델이 화면만 보고 카드마다 "무엇인지 분명한가", "얼마나 중요한가"를 본다.
        if !self.cfg.demo() && jev::key().is_some() && !self.board.cards.is_empty() {
            let board = lang::serialize(&self.board);
            let cards: Vec<(String, String, String)> = self
                .board
                .cards
                .iter()
                .map(|c| {
                    let topic = self.board.topics.iter().find(|t| t.id == c.topic).map(|t| t.name.clone()).unwrap_or_default();
                    (c.id.clone(), topic, c.title.clone())
                })
                .collect();
            let tx = self.tx.clone();
            std::thread::spawn(move || {
                if let Some(post) = decide::after(&board, &cards) {
                    let _ = tx.send(llm::Msg::Review(post));
                }
            });
        }
        // 이 턴을 오늘의 기록 노트에 남긴다 (시연 모드는 남기지 않는다).
        if !self.cfg.demo() {
            vault::log_turn(&vault::TurnRecord {
                input: self.cur_input.clone(),
                say: self.board.say.clone(),
                spoken: self.turn_spoken.clone(),
                actions: self.turn_actions.clone(),
                raw: self.turn_raw.clone(),
                error: self.error.clone(),
            });
        }
        self.recent.push((std::mem::take(&mut self.cur_input), self.board.say.clone()));
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
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
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
        if !self.autoplay.is_empty() {
            if !self.busy && !talking && self.idle_since.elapsed().as_millis() > 1200 {
                // 확인용 대본에서는 "|" 가 입력 구분자라서, 조작 줄 안의 "|" 는 "¦" 로 적는다.
                let next = self.autoplay.pop_front().unwrap_or_default().replace('¦', "|");
                if let Some(line) = next.strip_prefix('!') {
                    // "!" 로 시작하면 입력이 아니라 사용자의 화면 조작으로 넣는다 (클릭을 흉내 내는 확인용).
                    self.apply_line(line, Author::User);
                    self.idle_since = std::time::Instant::now() + std::time::Duration::from_secs(2);
                } else {
                    self.input = next;
                    self.send(&ctx);
                }
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }

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
            .with_active(!quiet),
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
