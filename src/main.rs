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
mod phone;
mod pics;
mod route;
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

/// 창의 이름. 확인용으로 띄운 창은 사용자가 실제 창과 헷갈리지 않게 이름을 다르게 한다.
fn title() -> &'static str {
    if std::env::var("LYBOARD_AUTOPLAY").is_ok() { "ALI 시험용 (쓰지 마세요)" } else { TITLE }
}
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
    /// 키를 넣는 창이 떠 있는가 (키 없이 처음 켰을 때 뜬다)
    key_popup: bool,
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
    shot_sent: Option<std::time::Instant>,
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
    /// 웹 카드 위치를 마지막으로 맞췄을 때의 창 크기. Mac 은 창 아래쪽 기준 좌표라 창 크기가 바뀌면 다시 맞춘다.
    web_win: egui::Vec2,
    /// 말로 대화하는 중인가 (마이크 스위치). 켜져 있으면 듣기 → 답하기 → 다시 듣기를 되풀이한다.
    converse: bool,
    mic: Option<audio::Mic>,
    mic_since: std::time::Instant,
    /// 이번 녹음에서 말소리가 들렸는가, 그리고 조용해지기 시작한 때
    heard: bool,
    quiet_since: Option<std::time::Instant>,
    /// AI 가 말하는 동안 마이크에 들어오는 소리의 평균 크기 (스피커 소리가 되돌아 들어오는 양)
    echo: f32,
    /// AI 가 말하는 동안 평균보다 크게 들리기 시작한 때 (사용자가 끼어드는 중일 수 있다)
    barge_since: Option<std::time::Instant>,
    /// 사용자가 말을 끊었다: 이번 턴의 남은 말은 소리 내지 않고 화면에만 반영한다.
    hush: bool,
    /// 이번 턴의 시간 재기 (data/timing.jsonl 에 남긴다)
    tm: TurnTiming,
    /// 받아쓰기를 맡긴 때와 그 소리의 길이(초)
    stt_started: Option<(std::time::Instant, f32)>,
    /// 마지막 받아쓰기에 걸린 시간 (다음 턴 기록에 붙인다)
    stt_done: Option<(u64, f32)>,
    /// 이번 턴의 say 가운데 아직 소리 내지 않은 마지막 것 (끝까지 아무 말도 안 했으면 그것을 읽는다)
    quiet_say: Option<String>,
    /// 기다리는 중: 사용자가 "잠깐만", "확인해 볼게"라고 했다. 부르거나 하던 이야기로 돌아올 때까지 말하지 않고 진행하지 않는다.
    standby: bool,
    /// 기다리는 동안 들린 말 (사용자가 혼잣말하거나 읽거나 다른 사람과 한 말). 다시 부르면 함께 전한다.
    asides: Vec<String>,
    /// 기다리는 중에 들린 말이 나를 부른 것인지 작은 모델이 판단한 결과 (말, 부른 것인가)
    wake_rx: Option<std::sync::mpsc::Receiver<(String, bool)>>,
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
    /// 답하는 중에 사용자가 더 적은 말. 도구를 쓰는 사이에 모델에게 전해지고, 그 전에 턴이 끝나면 다음 턴으로 간다.
    pub queue: Arc<std::sync::Mutex<Vec<String>>>,
    /// 지금 도는 턴을 멈추라는 신호
    pub stop: Arc<std::sync::atomic::AtomicBool>,
    /// 이번 턴에 AI 가 한마디(say)를 했는가
    pub turn_said: bool,
    probe: Option<Probe>,
    gif: Option<Gif>,
    promo: Option<Promo>,
    /// 초기화를 한 번 눌러 확인을 기다리는 중이면 그 시각
    pub reset_armed: Option<std::time::Instant>,
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

/// 보이지 않는 창으로 페이지 하나를 열어 글을 읽어 오는 일 (한 번에 하나).
struct Probe {
    url: String,
    script: String,
    needle: String,
    reply: Sender<String>,
    view: Option<wry::WebView>,
    since: std::time::Instant,
    polled: std::time::Instant,
    tx: Sender<String>,
    got: Receiver<String>,
    best: String,
}

/// 확인·소개용 녹화: 화면을 일정 간격으로 찍어 GIF 로 만든다 (LYBOARD_GIF=파일.gif).
/// 찍은 그림은 바로 뒤쪽 스레드로 보내 한 장씩 GIF 에 적는다 (메모리에 쌓아 두지 않는다).
struct Gif {
    tx: Option<Sender<(f64, usize, usize, Vec<u8>)>>,
    began: std::time::Instant,
    last: std::time::Instant,
    /// 마지막으로 무슨 일이 있었던 때 (이 뒤로 조용하면 녹화를 끝낸다)
    busy_at: std::time::Instant,
    /// 화면 내용이 바뀐 것을 알아보는 값과, 바뀐 뒤 촘촘히 찍을 시한
    sig: (usize, usize, u64, usize, bool),
    hot_until: std::time::Instant,
}

/// 찍힌 그림을 받아 GIF 로 적는 스레드. 가로 `width` 로 줄이고, 앞 장과 같은 그림은 버리고, 긴 멈춤은 줄인다.
fn gif_writer(path: String, width: u32) -> Sender<(f64, usize, usize, Vec<u8>)> {
    use image::codecs::gif::{GifEncoder, Repeat};
    let (tx, rx) = channel::<(f64, usize, usize, Vec<u8>)>();
    std::thread::spawn(move || {
        let Ok(file) = std::fs::File::create(&path) else { return };
        let mut enc = GifEncoder::new_with_speed(std::io::BufWriter::new(file), 10);
        let _ = enc.set_repeat(Repeat::Infinite);
        let mut held: Option<(f64, image::RgbaImage)> = None;
        let write = |enc: &mut GifEncoder<std::io::BufWriter<std::fs::File>>, img: image::RgbaImage, secs: f64| {
            // 아무 일도 없는 구간은 0.7초로 줄인다 (기다리는 시간이 길면 보기 지루하다).
            let ms = (secs * 1000.0).clamp(60.0, 700.0) as u64;
            let frame = image::Frame::from_parts(img, 0, 0, image::Delay::from_saturating_duration(std::time::Duration::from_millis(ms)));
            let _ = enc.encode_frame(frame);
        };
        while let Ok((t, w, h, rgba)) = rx.recv() {
            let Some(full) = image::RgbaImage::from_raw(w as u32, h as u32, rgba) else { continue };
            let height = (h as f32 * width as f32 / w as f32).round() as u32;
            let small = image::imageops::resize(&full, width, height, image::imageops::FilterType::Triangle);
            match held.take() {
                Some((t0, prev)) if prev.as_raw() == small.as_raw() => held = Some((t0, prev)),
                Some((t0, prev)) => {
                    write(&mut enc, prev, t - t0);
                    held = Some((t, small));
                }
                None => held = Some((t, small)),
            }
        }
        if let Some((_, last)) = held {
            // 마지막 장면은 2.5초 머문다.
            let frame = image::Frame::from_parts(last, 0, 0, image::Delay::from_saturating_duration(std::time::Duration::from_millis(2500)));
            let _ = enc.encode_frame(frame);
        }
        drop(enc);
        let _ = std::fs::write(format!("{path}.done"), "done");
    });
    tx
}

/// 소개 영상용 장면 (LYBOARD_PROMO=로고 그림 파일). 실제 화면 위에 여는 장면과 닫는 장면을 덧그린다.
/// 여는 장면: 글자의 벽 → 한 문장 → 로고. 그 뒤 실제 화면이 드러나 시연이 돈다. 시연이 끝나면 닫는 장면.
struct Promo {
    logo: String,
    began: std::time::Instant,
    outro: Option<std::time::Instant>,
}

/// 여는 장면이 화면을 덮고 있는 시간 (초). 이 뒤에 시연이 시작된다.
const PROMO_INTRO: f32 = 5.9;
const PROMO_OUTRO: f32 = 4.6;

impl App {
    /// 소개 장면이 아직 움직이고 있는가 (녹화가 이 동안 끝나지 않게 한다).
    fn promo_moving(&self) -> bool {
        match &self.promo {
            Some(p) => p.began.elapsed().as_secs_f32() < PROMO_INTRO + 0.3 || p.outro.map(|o| o.elapsed().as_secs_f32() < PROMO_OUTRO).unwrap_or(true),
            None => false,
        }
    }

    fn promo_frame(&mut self, ctx: &egui::Context, talking: bool) {
        let idle = self.autoplay.is_empty() && !self.busy && !talking && self.react_at.is_none() && self.fake_input.is_empty() && self.idle_since.elapsed().as_millis() > 1800;
        let Some(p) = &mut self.promo else { return };
        let t = p.began.elapsed().as_secs_f32();
        if p.outro.is_none() && t > PROMO_INTRO + 2.0 && idle {
            p.outro = Some(std::time::Instant::now());
        }
        let screen = ctx.content_rect();
        let paint = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("promo")));
        let c = screen.center();
        let fade = |x: f32, a: f32, b: f32| ((x - a) / (b - a)).clamp(0.0, 1.0);
        let back = |x: f32| {
            let (c1, c3) = (1.70158_f32, 2.70158_f32);
            1.0 + c3 * (x - 1.0).powi(3) + c1 * (x - 1.0).powi(2)
        };
        let logo = crate::pics::show(ctx, &p.logo);
        let text = |pos: egui::Pos2, s: &str, font: egui::FontId, color: egui::Color32, a: f32| {
            if a > 0.01 {
                paint.text(pos, egui::Align2::CENTER_CENTER, s, font, color.gamma_multiply(a));
            }
        };
        // 글자 사이를 벌려서 적는다 (로고 옆의 이름처럼).
        let spaced = |pos: egui::Pos2, s: &str, size: f32, gap: f32, color: egui::Color32, a: f32| {
            let n = s.chars().count() as f32;
            for (i, ch) in s.chars().enumerate() {
                let x = pos.x + (i as f32 - (n - 1.0) / 2.0) * gap;
                text(egui::pos2(x, pos.y), &ch.to_string(), ui::bold(size), color, a);
            }
        };

        if t < PROMO_INTRO + 0.1 {
            let cover = 1.0 - fade(t, PROMO_INTRO - 0.6, PROMO_INTRO);
            paint.rect_filled(screen, 0.0, ui::BG().gamma_multiply(cover));
            // 글자의 벽: 문단처럼 보이는 회색 줄들이 위로 흘러간다. 그러다 한가운데로 빨려 들어가 사라진다.
            let wall = fade(t, 0.0, 0.5) * (1.0 - fade(t, 2.9, 3.3));
            let pull = ui::ease_out_cubic(fade(t, 2.5, 3.3));
            if wall > 0.01 {
                let rows = 40;
                let pitch = 22.0;
                for row in 0..rows {
                    let y = screen.top() - pitch + (row as f32 * pitch - t * 120.0).rem_euclid(rows as f32 * pitch);
                    let mut seed = (row as u32).wrapping_mul(2_654_435_761).wrapping_add(12_345);
                    let mut x = screen.left() + 70.0;
                    // 문단의 마지막 줄은 짧게 끝난다.
                    let end = screen.right() - 70.0 - if row % 6 == 5 { screen.width() * 0.45 } else { 0.0 };
                    while x < end {
                        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                        let w = 22.0 + (seed >> 24) as f32 * 0.32;
                        let r = egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(w.min(end - x), 9.0));
                        let r = egui::Rect::from_center_size(r.center().lerp(c, pull), r.size() * (1.0 - pull));
                        paint.rect_filled(r, 3.0, ui::DIM().gamma_multiply(0.30 * wall * cover));
                        x += w + 9.0;
                    }
                }
            }
            // 한 문장
            let say = fade(t, 0.6, 1.1) * (1.0 - fade(t, 2.4, 2.9));
            if say > 0.01 {
                let plate = egui::Rect::from_center_size(c, egui::vec2(660.0, 150.0));
                paint.rect_filled(plate, 28.0, ui::BG().gamma_multiply(0.92 * say));
                text(c - egui::vec2(0.0, 24.0), "A screen full of text", ui::bold(38.0), ui::TEXT(), say);
                text(c + egui::vec2(0.0, 26.0), "is the worst way to use AI.", ui::bold(38.0), ui::TEXT(), say);
            }
            // 로고와 이름
            let mark = fade(t, 3.1, 3.6) * cover;
            if let crate::pics::Shown::Ready(id, size) = logo {
                let h = screen.height() * 0.26 * (0.80 + 0.20 * back(fade(t, 3.1, 3.8)));
                let r = egui::Rect::from_center_size(c - egui::vec2(0.0, 56.0), egui::vec2(h * size[0] / size[1], h));
                paint.image(id, r, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE.gamma_multiply(mark));
            }
            spaced(c + egui::vec2(0.0, 84.0), "FIDENIT", 26.0, 25.0, ui::TEXT(), fade(t, 3.7, 4.2) * cover);
            text(c + egui::vec2(0.0, 126.0), "presents  ALI  ·  Adaptive Live Interface", ui::font(15.0), ui::DIM(), fade(t, 4.2, 4.7) * cover);
        }

        if let Some(o) = p.outro {
            let u = o.elapsed().as_secs_f32();
            paint.rect_filled(screen, 0.0, ui::BG().gamma_multiply(0.96 * fade(u, 0.0, 0.6)));
            if let crate::pics::Shown::Ready(id, size) = crate::pics::show(ctx, &p.logo) {
                let h = 86.0;
                let r = egui::Rect::from_center_size(c - egui::vec2(0.0, 150.0), egui::vec2(h * size[0] / size[1], h));
                paint.image(id, r, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE.gamma_multiply(fade(u, 0.3, 0.8)));
            }
            text(c - egui::vec2(0.0, 44.0), "ALI", ui::bold(88.0 * (0.92 + 0.08 * back(fade(u, 0.5, 1.2)))), ui::TEXT(), fade(u, 0.5, 1.0));
            text(c + egui::vec2(0.0, 30.0), "Adaptive Live Interface", ui::font(19.0), ui::DIM(), fade(u, 0.8, 1.3));
            text(c + egui::vec2(0.0, 92.0), "Less reading. More done.", ui::bold(25.0), ui::TEXT(), fade(u, 1.2, 1.7));
            spaced(egui::pos2(c.x, screen.bottom() - 64.0), "FIDENIT", 15.0, 15.0, ui::DIM(), fade(u, 1.6, 2.1));
            text(egui::pos2(c.x, screen.bottom() - 38.0), "for the world", ui::font(12.5), ui::FAINT(), fade(u, 1.8, 2.3));
        }
        ctx.request_repaint();
    }
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
            key_popup: false,
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
            shot_sent: None,
            shot_delay: std::env::var("LYBOARD_SHOT_AT").ok().and_then(|v| v.parse().ok()).unwrap_or(3500),
            got_output: false,
            sent_at: std::time::Instant::now(),
            pre: None,
            review: Vec::new(),
            webviews: Default::default(),
            web_win: egui::Vec2::ZERO,
            converse: false,
            mic: None,
            mic_since: std::time::Instant::now(),
            heard: false,
            quiet_since: None,
            echo: 0.0,
            barge_since: None,
            hush: false,
            tm: TurnTiming::default(),
            stt_started: None,
            stt_done: None,
            quiet_say: None,
            standby: false,
            asides: Vec::new(),
            wake_rx: None,
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
            reset_armed: None,
            probe: None,
            promo: std::env::var("LYBOARD_PROMO").ok().map(|logo| Promo { logo, began: std::time::Instant::now(), outro: None }),
            gif: std::env::var("LYBOARD_GIF").ok().map(|path| {
                let now = std::time::Instant::now();
                let width = std::env::var("LYBOARD_GIF_WIDTH").ok().and_then(|v| v.parse().ok()).unwrap_or(900);
                Gif { tx: Some(gif_writer(path, width)), began: now, last: now, busy_at: now, sig: (0, 0, 0, 0, false), hot_until: now }
            }),
            stop: Default::default(),
            turn_said: false,
            queue: Default::default(),
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
        app.key_popup = app.cfg.key.is_none() && std::env::var("LYBOARD_DEMO").is_err() && std::env::var("LYBOARD_NO_KEY_WINDOW").is_err();
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
            let idle = |q: &mut std::collections::VecDeque<Vec<egui::Event>>, n: usize| (0..n).for_each(|_| q.push_back(Vec::new()));
            idle(&mut app.fake_input, 120);
            app.fake_marks.push_back((app.fake_input.len(), "시작".into()));
            for step in script.split(';').map(str::trim).filter(|s| !s.is_empty()) {
                app.queue_fake(step);
                app.fake_marks.push_back((app.fake_input.len(), format!("{step} (손 뗀 뒤)")));
            }
        }
        // 확인용: LYBOARD_PROBE=주소 로 주면 그 페이지를 보이지 않는 창으로 읽어 data\probe.log 에 남긴다.
        if let Ok(url) = std::env::var("LYBOARD_PROBE") {
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(2));
                let (tx, rx) = channel();
                let script = std::env::var("LYBOARD_PROBE_JS").unwrap_or_else(|_| route::PAGE_TEXT.to_string());
                let needle = std::env::var("LYBOARD_PROBE_NEEDLE").unwrap_or_default();
                jobs::to_ui(llm::Msg::Probe { url, script, needle, reply: tx });
                let text = rx.recv_timeout(std::time::Duration::from_secs(30)).unwrap_or_else(|_| "응답 없음".into());
                sys::log("probe.log", &format!("읽은 값: {:?}\n{text}", route::read_summary(&text)));
            });
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
            .filter(|c| c.fields.get("text").map(|t| !t.starts_with("멈춤") && !t.starts_with("실패") && !t.starts_with("Stopped") && !t.starts_with("Failed")).unwrap_or(true))
            .map(|c| c.id.clone())
            .collect();
        for id in stale {
            self.apply_line(&format!("set {id} text {}", sys::tr("멈춤: 프로그램이 꺼져서 끝내지 못함", "Stopped: the program was closed before it finished")), Author::Ai);
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
                    if line.trim_start().starts_with("say ") {
                        self.turn_said = true;
                    }
                    let mut words = line.split_whitespace();
                    let (cmd, id) = (words.next().unwrap_or(""), words.next().unwrap_or(""));
                    // 웹 카드는 제 크기로 화면 안에 다 들어와야 페이지가 보인다. 만들거나 주소를 바꾸면 그렇게 보이는 자리로 간다.
                    if matches!(cmd, "card" | "set") && self.board.card(id).map(|c| c.kind == core::Kind::Web).unwrap_or(false) {
                        self.view.show_card(id.to_string());
                    }
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
                    // 문자·전화 확인 카드에서 고른 것이면, 여기서 실제로 보내거나 건다 (AI 를 거치지 않는다).
                    let w: Vec<&str> = line.split_whitespace().collect();
                    if let ["set", card, "chosen", option] = w.as_slice() {
                        if let Some(ops) = phone::confirm(card, option) {
                            for op in ops {
                                self.apply_line(&op, Author::Ai);
                            }
                        }
                    }
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
                    parts.push(format!("{}: {name}", sys::tr("선택", "Chose")));
                }
                ("upd", Some(core::Kind::Input)) if cells.len() >= 3 => parts.push(format!("{}: {}", cells[0], cells[2])),
                ("upd", Some(core::Kind::Checklist)) if cells.len() >= 2 => parts.push(format!("{}: {}", if cells[0] == "done" { sys::tr("체크", "Checked") } else { sys::tr("체크 해제", "Unchecked") }, cells[1])),
                ("set", Some(core::Kind::Input)) => {}
                ("restore", _) => parts.push(sys::tr("보관함에서 꺼냄", "Brought back from the shelf").to_string()),
                _ => {}
            }
        }
        if parts.is_empty() { format!("({}) {}", sys::tr("화면에서 직접 조작", "on the board"), self.turn_events) } else { format!("({})", parts.join(" · ")) }
    }

    /// 초기화: 화면, 대화, 기억, 기록을 전부 지우고 처음 상태로 돌아간다. API 키와 음성 설정만 남는다. 되돌릴 수 없다.
    pub fn reset_all(&mut self) {
        if self.busy || jobs::running() > 0 {
            self.error = Some(sys::tr("답하는 중이거나 작업자가 일하는 중에는 초기화할 수 없습니다. 끝난 뒤에 다시 눌러 주세요.", "Cannot reset while it is answering or a worker is running. Try again when they finish.").into());
            return;
        }
        self.flush_speech();
        self.log = None; // 기록 파일을 놓아야 지울 수 있다
        // data 폴더: 음성 켬/끔과 화면 밝기 설정만 남기고 전부 지운다.
        if let Ok(rd) = std::fs::read_dir(sys::data_dir()) {
            for e in rd.flatten() {
                if e.file_name() == "voice" || e.file_name() == "theme.txt" {
                    continue;
                }
                let p = e.path();
                let _ = if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
            }
        }
        vault::wipe();
        route::forget();
        pics::forget();
        self.view = ui::View::default();
        self.webviews.clear();
        self.recent.clear();
        self.events.clear();
        self.rejects.clear();
        self.review.clear();
        self.pending.clear();
        self.pics.clear();
        self.job_news.clear();
        self.queue.lock().unwrap_or_else(|e| e.into_inner()).clear();
        self.touched.clear();
        self.touched_on.clear();
        self.touch_seq = 0;
        self.log_date.clear();
        self.selected = None;
        self.stats = None;
        self.error = None;
        self.tidy = None;
        self.tidy_rest = 0;
        self.task_topic = None;
        self.react_at = None;
        self.say_live = None;
        self.caption = None;
        self.show_shelf = false;
        self.input.clear();
        self.start_real_mode(); // 빈 보드와 빈 기억으로 다시 시작한다
    }

    /// 흉내 낸 마우스·글자 입력 한 걸음을 줄 세운다 ("click 230 190", "drag x y dx dy", "wheel x y 줄수", "type 글").
    fn queue_fake(&mut self, step: &str) {
        let m = egui::Modifiers::NONE;
        let w: Vec<&str> = step.split_whitespace().collect();
        let n: Vec<f32> = w.iter().skip(1).filter_map(|x| x.parse().ok()).collect();
        let q = &mut self.fake_input;
        match (w.first().copied(), n.as_slice()) {
            (Some("drag"), [x, y, dx, dy]) => {
                let p = egui::pos2(*x, *y);
                q.push_back(vec![egui::Event::PointerMoved(p)]);
                q.push_back(vec![egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers: m }]);
                for i in 1..=20 {
                    q.push_back(vec![egui::Event::PointerMoved(p + egui::vec2(*dx, *dy) * (i as f32 / 20.0))]);
                }
                for _ in 0..12 {
                    q.push_back(Vec::new());
                }
                q.push_back(vec![egui::Event::PointerButton { pos: p + egui::vec2(*dx, *dy), button: egui::PointerButton::Primary, pressed: false, modifiers: m }]);
            }
            (Some("click"), [x, y]) => {
                // 짧게 눌렀다 뗀다 (오래 누르면 클릭으로 치지 않는다)
                let p = egui::pos2(*x, *y);
                q.push_back(vec![egui::Event::PointerMoved(p)]);
                for _ in 0..6 {
                    q.push_back(Vec::new());
                }
                q.push_back(vec![egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers: m }]);
                q.push_back(Vec::new());
                q.push_back(vec![egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: false, modifiers: m }]);
            }
            (Some("type"), _) => {
                q.push_back(vec![egui::Event::Text(step.trim_start_matches("type").trim().to_string())]);
            }
            (Some("wheel"), [x, y, amount]) => {
                q.push_back(vec![egui::Event::PointerMoved(egui::pos2(*x, *y))]);
                for _ in 0..10 {
                    q.push_back(vec![egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Line, delta: egui::vec2(0.0, *amount / 10.0), phase: egui::TouchPhase::Move, source: egui::MouseWheelSource::Wheel, modifiers: m }]);
                }
            }
            _ => {}
        }
        for _ in 0..90 {
            q.push_back(Vec::new());
        }
    }

    /// 사진 하나를 입력창에 붙인다.
    pub fn add_pic(&mut self, ctx: &egui::Context, pic: Result<pics::Pic, String>) {
        match pic {
            Ok(_) if self.pics.len() >= 6 => self.error = Some(sys::tr("사진은 한 번에 6장까지 붙일 수 있습니다", "Up to six pictures at a time").into()),
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
        if sys::english() {
            t += "<lang>en</lang>\n";
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
        self.standby = false; // 글로 쓰거나 다시 부르면 기다리기를 끝낸다
        let input = self.input.trim().to_string();
        if self.busy {
            // 답하는 중에도 받는다. 줄을 세워 두면 하던 일 사이에 전해지거나, 끝나자마자 이어서 처리된다.
            if !input.is_empty() {
                self.queue.lock().unwrap_or_else(|e| e.into_inner()).push(input);
                self.input.clear();
            }
            return;
        }
        if input.is_empty() && self.pics.is_empty() {
            return;
        }
        let input = if input.is_empty() { sys::tr("(사진을 보냄)", "(sent pictures)").to_string() } else { input };
        self.input.clear();
        self.start_turn(ctx, input);
    }

    /// 한 턴을 시작한다. `input` 이 비어 있으면 화면 조작만으로 시작된 턴이다.
    fn start_turn(&mut self, ctx: &egui::Context, input: String) {
        self.flush_speech(); // 말하는 중에 새로 말을 걸면 하던 말을 멈춘다
        self.hush = false;
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
        let pics_note = if images.is_empty() { String::new() } else { if sys::english() { format!("({} pictures) ", images.len()) } else { format!("(사진 {}장) ", images.len()) } };
        if std::env::var("LYBOARD_DEBUG").is_ok() {
            debug_log("turn.log", &format!("{text}\n=====\n")); // 확인용: 모델에게 보낸 것 그대로
        }
        self.react_at = None;
        self.busy = true;
        self.error = None;
        let auto = input.is_empty(); // 사용자가 말한 것이 아니라 프로그램이 시작한 턴
        self.cur_input = if let Some(names) = &self.job_turn {
            format!("({}) {names}", sys::tr("맡긴 작업이 끝남", "delegated work finished"))
        } else if self.tidying {
            sys::tr("(분류와 주제 정리)", "(tidying topics)").to_string()
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
        self.tm = TurnTiming { stt: self.stt_done.take(), ..Default::default() };
        self.quiet_say = None;
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
        self.stop = Default::default();
        self.turn_said = false;
        llm::run_turn(self.cfg.clone(), text, images, self.queue.clone(), self.stop.clone(), pre, self.turn_no, self.tx.clone(), move || ctx.request_repaint());
        self.turn_no += 1;
    }

    /// AI 가 낸 한 줄을 받는다. 음성이 꺼져 있으면 바로 화면에 반영하고,
    /// 켜져 있으면 "문장 → 그 문장의 카드" 순서로 줄을 세워 말하는 속도에 맞춰 내보낸다.
    fn ai_line(&mut self, line: &str) {
        let t = line.trim();
        if !self.voice || self.hush || self.standby {
            if !t.starts_with("tell ") {
                self.apply_line(line, Author::Ai);
            }
            return;
        }
        // 한 턴에 소리 내는 말은 짧게: say 는 아직 아무 말도 안 했을 때만 읽는다 (일 시작 전 한마디).
        // 그 뒤의 say 는 화면에만 내고, 턴이 끝날 때까지 다른 말이 없었으면 그때 읽는다 (finish_turn).
        let said_any = !self.turn_spoken.is_empty() || !self.speech.is_empty();
        if let Some(sv) = t.strip_prefix("say ").map(str::trim).filter(|v| !v.is_empty()) {
            if said_any {
                self.quiet_say = Some(sv.to_string());
                if let Some(last) = self.speech.back_mut() {
                    last.1.push(line.to_string());
                } else {
                    self.apply_line(line, Author::Ai);
                }
                return;
            }
        }
        if t.starts_with("tell ") {
            self.quiet_say = None; // 정리한 대답이 나왔으니 남은 한마디는 읽지 않는다
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
                self.tm.first_voice.get_or_insert(self.sent_at.elapsed().as_millis() as u64);
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

    /// 기다리기를 끝내고 이 말로 대화를 이어 간다. 기다리는 동안 들린 말도 함께 전한다.
    fn wake_up(&mut self, ctx: &egui::Context, text: String) {
        self.standby = false;
        let asides = std::mem::take(&mut self.asides);
        sys::log("stt.log", &format!("기다리기 끝: {text}"));
        self.input = if asides.is_empty() {
            text
        } else {
            format!("{text}\n(기다리는 동안 사용자가 혼잣말하거나 읽은 말: {})", asides.join(" / "))
        };
        self.send(ctx);
    }

    /// 사용자가 말을 끊었다: 지금 말을 멈추고, 이번 턴의 남은 말도 소리 내지 않는다.
    fn silence(&mut self) {
        self.flush_speech();
        self.hush = true;
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
        // 일 시작 전 한마디만 하고 결과를 말하지 않았으면, 마지막 한마디(결과)를 읽어 준다.
        if let Some(last) = self.quiet_say.take() {
            if self.voice && !self.hush && !self.turn_spoken.iter().any(|x| x.trim() == last) {
                self.speech.push_back((last, Vec::new()));
                self.advance_speech();
            }
        }
        if !self.cfg.demo() {
            self.tm.write(&self.cur_input, self.sent_at, self.voice, &self.turn_spoken, self.error.as_deref());
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
                            self.apply_line(sys::tr("topic jobs 맡긴 작업", "topic jobs Delegated work"), Author::Ai);
                        }
                        "jobs".to_string()
                    }
                };
                self.apply_line(&format!("card {id} progress {topic} 3x2 {}", clip(&title, 40)), Author::Ai);
                self.apply_line(&format!("set {id} pct 0"), Author::Ai);
                self.apply_line(&format!("set {id} text {}", sys::tr("작업자가 시작함", "Worker started")), Author::Ai);
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
                        self.apply_line(&format!("set {id} text {}", sys::tr("끝남 · 결과를 정리하는 중", "Done · writing up the result")), Author::Ai);
                    } else {
                        self.apply_line(&format!("set {id} text {}", sys::tr("실패: 끝내지 못함", "Failed: could not finish")), Author::Ai);
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
                    self.tm.activity(&a, self.sent_at);
                    if !a.is_empty() {
                        self.turn_actions.push(a.clone());
                        self.got_output = true;
                    }
                    self.activity = a;
                }
                llm::Msg::Pre(p) => {
                    self.tm.pre_ms.get_or_insert(self.sent_at.elapsed().as_millis() as u64);
                    // 음성 대화에서는 판단 모델의 결정이 나오자마자(약 0.2초) 먼저 대답한다.
                    if let Some(ack) = p.ack.clone() {
                        if self.voice && !self.hush {
                            self.tm.first_voice.get_or_insert(self.sent_at.elapsed().as_millis() as u64);
                            self.tts.speak(&ack);
                            self.caption = Some(ack.clone());
                            self.turn_spoken.push(ack);
                        } else if !self.turn_said {
                            // 글로 대화할 때도 받았다는 대답을 먼저 보여 준다. AI 의 한마디가 오면 그것으로 바뀐다.
                            self.say_live = Some(ack);
                        }
                    }
                    self.pre = Some(p);
                }
                llm::Msg::Job(ev) => self.job_event(ev),
                llm::Msg::Probe { url, script, needle, reply } => {
                    if self.probe.is_some() {
                        let _ = reply.send("오류: 다른 페이지를 읽는 중".into());
                    } else {
                        let (tx, got) = channel();
                        let now = std::time::Instant::now();
                        self.probe = Some(Probe { url, script, needle, reply, view: None, since: now, polled: now, tx, got, best: String::new() });
                    }
                }
                llm::Msg::Keys(i, keys) => self.past.add_keys(i, &keys),
                llm::Msg::Added(said) => {
                    // 대화 기록에서 이 말들이 지금 턴에 합쳐진 것으로 보이게 한다.
                    for s in said {
                        self.cur_input += &format!("\n+ {s}");
                    }
                    self.hush = false; // 덧붙인 말에 대한 답은 다시 소리로 듣는다
                }
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
                    self.tm.mark_first(self.sent_at);
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
                        self.tm.mark_first(self.sent_at);
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
                    self.tm.stats = Some(stats.clone());
                    self.stats = Some(stats);
                    self.finish_turn();
                }
                llm::Msg::Error(e) if e == agent::STOPPED => {
                    // 사용자가 멈춘 것은 오류가 아니다.
                    self.queue.lock().unwrap_or_else(|e| e.into_inner()).clear();
                    self.apply_line(sys::tr("say 멈췄어요.", "say Stopped."), Author::Ai);
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
    /// 마이크는 AI 가 일하는 동안에도, 말하는 동안에도 열어 둔다. 일하는 중에 한 말은 하던 일에 전해지고,
    /// 말하는 중에 사용자가 끼어들면 AI 는 말을 멈추고 듣는다.
    fn voice_input(&mut self, ctx: &egui::Context, talking: bool) {
        if let Some(rx) = &self.wake_rx {
            match rx.try_recv() {
                Ok((text, yes)) => {
                    self.wake_rx = None;
                    if yes {
                        self.wake_up(ctx, text);
                    } else {
                        sys::log("stt.log", &format!("기다리는 중, 나를 부른 말이 아니라고 봄: {text}"));
                        self.asides.push(text);
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(50)),
                Err(_) => self.wake_rx = None,
            }
        }
        // 받아쓰기 결과가 왔는가
        if let Some(rx) = &self.stt {
            match rx.try_recv() {
                Ok(Ok(text)) => {
                    self.stt = None;
                    self.stt_done = self.stt_started.take().map(|(t, secs)| (t.elapsed().as_millis() as u64, secs));
                    if self.standby && !text.trim().is_empty() {
                        // 기다리는 중: 나를 부른 말인지 본다. 분명히 부르면 바로, 아니면 작은 모델에게 묻는다.
                        if is_wake_word(&text) {
                            self.wake_up(ctx, text);
                        } else if let Some(key) = self.cfg.key.clone() {
                            let (tx, rx) = channel();
                            self.wake_rx = Some(rx);
                            let context: String = self.recent.iter().rev().take(2).rev().map(|(u, a)| format!("사용자: {u}\n비서: {a}\n")).collect();
                            let asides = self.asides.join(" / ");
                            let wake = ctx.clone();
                            std::thread::spawn(move || {
                                let yes = decide::addressed(&key, &context, &asides, &text);
                                let _ = tx.send((text, yes));
                                wake.request_repaint();
                            });
                        }
                    } else if is_wait_word(&text) {
                        // "잠깐만", "확인해 볼게": 말을 멈추고 부를 때까지 기다린다.
                        self.silence();
                        self.standby = true;
                        self.asides.clear();
                        sys::log("stt.log", &format!("기다리기 시작: {text}"));
                    } else if is_stop_word(&text) {
                        // "그만", "멈춰": 말도 일도 바로 멈춘다. 새 요청으로 보내지 않는다.
                        self.silence();
                        if self.busy {
                            self.queue.lock().unwrap_or_else(|e| e.into_inner()).clear();
                            self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
                        }
                    } else if self.turn_spoken.iter().rev().take(3).any(|said| similar(said, &text)) {
                        // 스피커에서 나온 AI 자기 목소리를 받아쓴 것: 버린다.
                        sys::log("stt.log", &format!("자기 목소리로 보고 버림: {text}"));
                    } else if !text.is_empty() {
                        self.input = text;
                        self.send(ctx);
                    }
                }
                Ok(Err(e)) => {
                    self.stt = None;
                    self.error = Some(format!("{}: {e}", sys::tr("받아쓰기 실패", "Transcription failed")));
                }
                Err(_) => ctx.request_repaint_after(std::time::Duration::from_millis(50)),
            }
        }
        if !self.converse {
            self.mic = None;
            return;
        }
        // 받아쓰는 중이 아니면 언제나 듣는다 (일하는 중에도, 말하는 중에도).
        let free = self.stt.is_none() && self.react_at.is_none();
        match &self.mic {
            None if free => {
                self.mic = Some(audio::Mic::start());
                self.mic_since = std::time::Instant::now();
                self.heard = false;
                self.quiet_since = None;
                self.echo = 0.0;
                self.barge_since = None;
            }
            None => {}
            Some(mic) => {
                if let Some(e) = mic.error() {
                    self.error = Some(format!("{}: {e}", sys::tr("마이크를 쓸 수 없음", "Microphone unavailable")));
                    self.converse = false;
                    self.mic = None;
                    return;
                }
                let level = mic.level();
                if talking && !self.heard {
                    // AI 가 말하는 중: 스피커 소리가 마이크로 되돌아 들어온다. 그 평균보다 뚜렷하게 큰 소리가
                    // 0.3초 넘게 이어지면 사용자가 끼어든 것으로 보고 말을 멈춘다.
                    self.echo = self.echo * 0.95 + level * 0.05;
                    let loud = level > (MIC_THRESHOLD * 1.5).max(self.echo * 2.0);
                    match (loud, self.barge_since) {
                        (true, None) => self.barge_since = Some(std::time::Instant::now()),
                        (false, _) => self.barge_since = None,
                        _ => {}
                    }
                    if self.barge_since.map(|t| t.elapsed().as_millis() > 200).unwrap_or(false) {
                        mic.keep_last(0.5); // 끼어든 말의 시작은 남기고, 그 앞의 AI 목소리는 버린다
                        self.silence();
                        self.heard = true;
                        self.quiet_since = None;
                        self.mic_since = std::time::Instant::now();
                        self.barge_since = None;
                    } else {
                        // 끼어들지 않았으면 AI 목소리를 쌓아 두지 않는다.
                        mic.keep_last(0.6);
                        self.mic_since = std::time::Instant::now();
                    }
                    ctx.request_repaint_after(std::time::Duration::from_millis(40));
                    return;
                }
                // 말소리가 들리다가 1.1초 조용하면 말이 끝난 것으로 본다.
                let loud = level > MIC_THRESHOLD;
                if loud && !self.heard && self.busy {
                    // 일하는 중에 사용자가 말하기 시작했다: 남은 말은 소리 내지 않는다.
                    self.hush = true;
                }
                if loud {
                    self.heard = true;
                    self.quiet_since = None;
                } else if self.heard && self.quiet_since.is_none() {
                    self.quiet_since = Some(std::time::Instant::now());
                }
                if !self.heard {
                    // 말이 시작되기 전의 조용한 소리는 오래 쌓아 두지 않는다.
                    mic.keep_last(0.6);
                    self.mic_since = std::time::Instant::now();
                }
                let ended = self.quiet_since.map(|t| t.elapsed().as_millis() > 1100).unwrap_or(false);
                let too_long = self.mic_since.elapsed().as_secs() > 45;
                if ended || too_long {
                    let pcm = self.mic.take().map(|m| m.finish()).unwrap_or_default();
                    if self.heard && pcm.len() > 16000 / 3 {
                        let (tx, rx) = channel();
                        self.stt = Some(rx);
                        self.stt_started = Some((std::time::Instant::now(), pcm.len() as f32 / 16000.0));
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

    /// 보이지 않는 창에서 페이지를 읽는 일을 한 걸음 진행한다. 원하는 글(거리와 시간)이 나오면 돌려주고 창을 없앤다.
    fn run_probe(&mut self, frame: &eframe::Frame, ctx: &egui::Context) {
        let Some(p) = &mut self.probe else { return };
        if p.view.is_none() {
            // 화면 밖에, 보이지 않게 만든다. 페이지는 넓은 화면인 줄 안다.
            let bounds = wry::Rect { position: wry::dpi::LogicalPosition::new(-3000.0, -3000.0).into(), size: wry::dpi::LogicalSize::new(1280.0, 900.0).into() };
            match wry::WebViewBuilder::new().with_bounds(bounds).with_url(&p.url).with_focused(false).build_as_child(frame) {
                Ok(v) => p.view = Some(v),
                Err(e) => {
                    let _ = p.reply.send(format!("오류: 페이지를 열지 못함 ({e})"));
                    self.probe = None;
                    return;
                }
            }
        }
        while let Ok(text) = p.got.try_recv() {
            // 스크립트의 결과는 JSON 문자열로 온다.
            let text = serde_json::from_str::<String>(&text).unwrap_or(text);
            let ready = if p.needle.is_empty() { route::read_summary(&text).is_some() } else { text.contains(&p.needle) };
            if ready {
                let _ = p.reply.send(text);
                self.probe = None;
                return;
            }
            if text.len() > p.best.len() {
                p.best = text;
            }
        }
        if p.since.elapsed().as_secs() >= 20 {
            let _ = p.reply.send(format!("오류: 시간 안에 경로가 나오지 않음\n{}", p.best.chars().take(1500).collect::<String>()));
            self.probe = None;
            return;
        }
        if p.since.elapsed().as_millis() > 1800 && p.polled.elapsed().as_millis() > 500 {
            p.polled = std::time::Instant::now();
            let tx = p.tx.clone();
            if let Some(v) = &p.view {
                let _ = v.evaluate_script_with_callback(&p.script, move |r| {
                    let _ = tx.send(r);
                });
            }
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
    }

    /// 웹 카드마다 브라우저 화면을 하나씩 두고, 카드가 움직이면 따라가게 한다.
    fn sync_webviews(&mut self, frame: &eframe::Frame, slots: Vec<ui::WebSlot>, zoom: f32) {
        self.webviews.retain(|id, _| slots.iter().any(|s| &s.id == id));
        let win = self.wake.content_rect().size();
        let resized = win != self.web_win;
        self.web_win = win;
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
                    if w.rect != s.rect || (resized && cfg!(target_os = "macos")) {
                        let _ = w.view.set_bounds(bounds);
                        w.rect = s.rect;
                    }
                    if w.visible != s.visible {
                        let _ = w.view.set_visible(s.visible);
                        w.visible = s.visible;
                    }
                }
                None => {
                    // 화면에 안 보이는 자리에 있어도 페이지는 만들어 둔다 (AI 가 읽고 다룰 수 있어야 한다). 보일 때만 드러낸다.
                    let built = wry::WebViewBuilder::new().with_bounds(bounds).with_url(&url).with_focused(false).with_visible(s.visible).build_as_child(frame);
                    match built {
                        Ok(view) => {
                            self.webviews.insert(s.id.clone(), Web { view, url, rect: s.rect, visible: s.visible });
                        }
                        Err(e) => self.error = Some(format!("브라우저 화면을 띄우지 못함: {e}")),
                    }
                }
            }
        }
    }

    fn status_line(&self) -> String {
        let mut parts = Vec::new();
        if self.cfg.demo() && self.promo.is_none() {
            parts.push(if std::env::var("LYBOARD_DEMO").is_ok() { sys::tr("시연 모드 · 무엇을 입력해도 준비된 장면이 재생됩니다", "Demo mode · any input plays the prepared scenes") } else { sys::tr("데모 · API 키 없음", "Demo · no API key") }.to_string());
        }
        if let Some(s) = self.stats.as_ref().filter(|_| self.promo.is_none()) {
            parts.push(s.model.clone());
            parts.push(if sys::english() { format!("first {:.1}s · total {:.1}s", s.first_ms as f32 / 1000.0, s.total_ms as f32 / 1000.0) } else { format!("첫 반응 {:.1}초 · 전체 {:.1}초", s.first_ms as f32 / 1000.0, s.total_ms as f32 / 1000.0) });
            if s.input_tokens > 0 {
                parts.push(if sys::english() { format!("in {} (cached {}) · out {} tokens", s.input_tokens, s.cached_tokens, s.output_tokens) } else { format!("입력 {} (캐시 {}) · 출력 {} 토큰", s.input_tokens, s.cached_tokens, s.output_tokens) });
            }
        }
        let text: usize = self.board.cards.iter().map(card_chars).sum();
        parts.push(if sys::english() { format!("{} cards · {} characters", self.board.cards.len(), text) } else { format!("카드 {} · 화면 글자 {}", self.board.cards.len(), text) });
        let workers = jobs::running();
        if workers > 0 {
            parts.push(if sys::english() { format!("{workers} workers running") } else { format!("작업자 {workers}명이 일하는 중") });
        }
        parts.push(if sys::english() { format!("zoom {:.0}% (scroll to zoom, drag empty space to move, double-click to see all)", self.view.zoom * 100.0) } else { format!("확대 {:.0}% (스크롤로 확대·축소, 빈 곳을 끌어 이동, 두 번 눌러 전체 보기)", self.view.zoom * 100.0) });
        if !self.rejects.is_empty() {
            parts.push(format!("{} {}", sys::tr("거부된 명령", "rejected commands"), self.rejects.len()));
        }
        if let Some(s) = &self.selected {
            parts.push(format!("{}: {s}", sys::tr("선택", "selected")));
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
                self.shot_sent = Some(std::time::Instant::now());
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::ScreenshotCallback::new(move |img| save_bmp(&path, &img))));
            }
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        }
        // 화면 그림은 다음 프레임에 도착한다. 화면이 가만히 있으면 다음 프레임이 오지 않으므로 잠깐 더 그리게 한다.
        if self.shot_sent.map(|t| t.elapsed().as_millis() < 1500).unwrap_or(false) {
            ui.ctx().request_repaint();
        }
        // 녹화: 0.11초마다 화면을 찍는다. 할 일이 다 끝나고 2.5초 조용하면 끝낸다 (길어도 3분).
        let promo_moving = self.promo_moving();
        if let Some(g) = &mut self.gif {
            let talking = self.tts.is_speaking() || !self.speech.is_empty();
            if self.busy || talking || self.react_at.is_some() || !self.autoplay.is_empty() || !self.job_news.is_empty() || !self.fake_input.is_empty() || self.probe.is_some() || promo_moving {
                g.busy_at = std::time::Instant::now();
            }
            let over = g.began.elapsed().as_secs() > 180 || (g.began.elapsed().as_secs() >= 4 && g.busy_at.elapsed().as_millis() > 2500);
            if over {
                g.tx = None; // 보내는 쪽을 닫으면 적는 스레드가 마무리한다
            } else if {
                // 화면이 바뀌는 동안(과 그 뒤 1.6초의 전환)은 촘촘히, 기다리는 동안은 드문드문 찍는다.
                let sig = (self.turn_raw.len(), self.board.cards.len(), self.board.cards.iter().map(|c| c.rev).sum::<u64>(), self.recent.len() + self.board.say.len(), self.busy);
                if sig != g.sig {
                    g.sig = sig;
                    g.hot_until = std::time::Instant::now() + std::time::Duration::from_millis(1600);
                }
                let gap = if std::time::Instant::now() < g.hot_until || !self.fake_input.is_empty() || promo_moving { std::env::var("LYBOARD_GIF_GAP").ok().and_then(|v| v.parse().ok()).unwrap_or(66) } else { 1500 };
                g.last.elapsed().as_millis() >= gap
            } {
                g.last = std::time::Instant::now();
                if let Some(tx) = g.tx.clone() {
                    let t = g.began.elapsed().as_secs_f64();
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::ScreenshotCallback::new(move |img| {
                        let rgba: Vec<u8> = img.pixels.iter().flat_map(|p| p.to_array()).collect();
                        let _ = tx.send((t, img.size[0], img.size[1], rgba));
                    })));
                }
            }
            if g.tx.is_some() {
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(40));
            }
        }
        self.pump();
        let ctx = ui.ctx().clone();
        if !self.titlebar_done {
            // 창이 만들어진 뒤에야 적용되므로 될 때까지 시도한다.
            let (bg, dim) = (ui::BG(), ui::DIM());
            self.titlebar_done = sys::dark_titlebar(title(), (bg.r(), bg.g(), bg.b()), (dim.r(), dim.g(), dim.b()), !ui::is_light());
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
        // 답하는 중에 적어 둔 말이 전해지지 못한 채 턴이 끝났으면, 바로 다음 턴으로 보낸다.
        if !self.busy && self.react_at.is_none() {
            let waiting: Vec<String> = self.queue.lock().unwrap_or_else(|e| e.into_inner()).drain(..).collect();
            if !waiting.is_empty() {
                self.start_turn(&ctx, waiting.join("\n"));
            }
        }
        // 맡긴 작업이 끝났으면, 사용자가 말하고 있지 않을 때 그 결과를 화면에 정리하는 턴을 돈다.
        if self.standby {
            // 기다리는 중에는 작업 결과 정리나 구성 정리를 하지 않는다 (부르면 그때 한다).
        } else if !self.job_news.is_empty() {
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
            // "+" 로 시작하는 입력은 앞의 턴이 도는 도중(시작 4초 뒤)에 넣는다: 답하는 중에 더 말하는 것을 시험한다.
            if self.busy && self.autoplay.front().map(|n| n.starts_with('+')).unwrap_or(false) && self.sent_at.elapsed().as_millis() > 4000 {
                self.input = self.autoplay.pop_front().unwrap_or_default().trim_start_matches('+').to_string();
                self.send(&ctx);
            }
            let intro_done = self.promo.as_ref().map(|p| p.began.elapsed().as_secs_f32() > PROMO_INTRO - 0.4).unwrap_or(true);
            if intro_done && !self.busy && !talking && self.job_news.is_empty() && self.idle_since.elapsed().as_millis() > 1200 {
                // 확인용 대본에서는 "|" 가 입력 구분자라서, 조작 줄 안의 "|" 는 "¦" 로 적는다.
                let next = self.autoplay.pop_front().unwrap_or_default().replace('¦', "|");
                if let Some(step) = next.strip_prefix('@') {
                    // "@" 로 시작하면 마우스·글자 입력을 흉내 낸다 (앞의 턴이 끝난 뒤에 실행된다).
                    self.queue_fake(step);
                    self.idle_since = std::time::Instant::now() + std::time::Duration::from_secs(2);
                } else if let Some(line) = next.strip_prefix('!') {
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
            .frame(egui::Frame::new().fill(ui::BG()).inner_margin(egui::Margin { left: 18, right: 18, top: 18, bottom: 0 }))
            .show(ui, |ui| {
                let mut lines = Vec::new();
                let mut clicked = None;
                // 판단 모델이 "이 주제의 이야기"라고 본 구역은 답이 오기 전부터 강조한다.
                let hot = if self.busy { self.pre.as_ref().and_then(|p| p.topic.clone()) } else { None };
                let mut webs = Vec::new();
                let moving =
                    ui::board(ui, &self.board, &mut self.view, &self.selected, &self.pending, &mut lines, &mut clicked, hot.as_deref(), &mut webs);
                self.sync_webviews(frame, webs, ctx.zoom_factor());
                self.run_probe(frame, &ctx);
                for l in lines {
                    // 화면 쪽에만 해당하는 부탁 (보드를 바꾸는 조작이 아니다)
                    if let Some(id) = l.strip_prefix("@show ") {
                        self.view.show_card(id.to_string());
                        continue;
                    }
                    self.apply_line(&l, Author::User);
                }
                if let Some(id) = clicked {
                    self.selected = if self.selected.as_deref() == Some(id.as_str()) { None } else { Some(id) };
                }
                if moving {
                    ctx.request_repaint();
                }
            });
        self.key_window(&ctx);
        self.promo_frame(&ctx, talking);
    }
}

fn setup_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    // (파일, 묶음 파일 안의 번호). Mac 의 Apple SD Gothic Neo 는 한 파일(.ttc)에 굵기별로 들어 있다: 0 보통, 6 굵게.
    let load = |list: &[(&str, u32)]| {
        list.iter().find_map(|(path, index)| {
            let bytes = std::fs::read(path).ok()?;
            let mut data = egui::FontData::from_owned(bytes);
            data.index = *index;
            Some(data)
        })
    };
    let regular = [
        ("C:\\Windows\\Fonts\\malgun.ttf", 0),
        ("/System/Library/Fonts/AppleSDGothicNeo.ttc", 0),
        ("/System/Library/Fonts/Supplemental/AppleGothic.ttf", 0),
    ];
    let heavy = [("C:\\Windows\\Fonts\\malgunbd.ttf", 0), ("/System/Library/Fonts/AppleSDGothicNeo.ttc", 6)];
    if let Some(data) = load(&regular) {
        fonts.font_data.insert("kr".into(), Arc::new(data));
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts.families.entry(family).or_default().insert(0, "kr".into());
        }
    }
    // 제목과 숫자에 쓰는 굵은 글꼴. 없으면 보통 글꼴로 대신한다.
    let mut bold = fonts.families.get(&egui::FontFamily::Proportional).cloned().unwrap_or_default();
    if let Some(data) = load(&heavy) {
        fonts.font_data.insert("kr-bold".into(), Arc::new(data));
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
            .with_title(title())
            // 확인용: 창의 크기 (LYBOARD_SIZE="가로,세로")
            .with_inner_size(std::env::var("LYBOARD_SIZE").ok().and_then(|v| v.split_once(',').and_then(|(w, h)| Some([w.trim().parse::<f32>().ok()?, h.trim().parse::<f32>().ok()?]))).unwrap_or([1560.0, 920.0]))
            .with_active(!quiet)
            // 확인용: 창을 띄울 위치 (LYBOARD_POS="x,y")
            .with_position(std::env::var("LYBOARD_POS").ok().and_then(|v| v.split_once(',').and_then(|(x, y)| Some([x.trim().parse::<f32>().ok()?, y.trim().parse::<f32>().ok()?]))).unwrap_or([80.0, 60.0])),
        ..Default::default()
    };
    eframe::run_native(
        title(),
        options,
        Box::new(|cc| {
            setup_fonts(&cc.egui_ctx);
            sys::load_language();
            // 저장해 둔 선택이 없으면 OS 설정(밝게/어둡게)을 따른다.
            let light = match std::fs::read_to_string(theme_path()).map(|s| s.trim().to_string()).as_deref() {
                Ok("light") => true,
                Ok("dark") => false,
                _ => cc.egui_ctx.system_theme() == Some(egui::Theme::Light),
            };
            set_theme(&cc.egui_ctx, light, false);
            Ok(Box::new(App::new(&cc.egui_ctx)))
        }),
    )
}

/// 말을 멈추라는 짧은 말인가 ("그만", "멈춰", "스톱", "조용히 해" 등).
fn is_stop_word(text: &str) -> bool {
    let t: String = text.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
    if t.is_empty() || t.chars().count() > 10 {
        return false;
    }
    ["그만", "멈춰", "멈춰요", "멈춤", "스톱", "stop", "조용", "쉿", "됐어", "됐다", "닥쳐", "그만해", "그만하라고", "shutup", "quiet", "enough", "thatsenough"].iter().any(|w| t.contains(w))
}

/// 잠시 기다려 달라는 말인가 ("잠깐만", "기다려 봐", "확인해 볼게", "읽어 볼게").
fn is_wait_word(text: &str) -> bool {
    let t: String = text.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
    if t.is_empty() || t.chars().count() > 20 {
        return false;
    }
    // "잠깐, 이거 찾아봐"처럼 시키는 말은 기다리라는 말이 아니다.
    if ["줘", "찾아봐", "해봐", "알려", "보여", "만들어", "가져와", "써봐", "봐봐", "find", "show", "tellme", "make", "search", "get"].iter().any(|w| t.contains(w)) {
        return false;
    }
    ["잠깐", "잠시만", "기다려", "기다리", "볼게", "볼께", "보고올게", "체크해볼", "생각좀", "생각해볼", "holdon", "wait", "onesec", "onesecond", "letmecheck", "letmeread", "letmethink", "justasec"].iter().any(|w| t.contains(w))
}

/// 기다리는 중에 분명히 나를 부르는 말인가 ("알리", "다 봤어", "이제 얘기해").
fn is_wake_word(text: &str) -> bool {
    let t: String = text.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
    // 영어 이름은 낱말 하나로 들렸을 때만 (quality, Italian 같은 낱말 속의 ali 는 아니다).
    if text.split(|c: char| !c.is_alphanumeric()).any(|w| w.eq_ignore_ascii_case("ali")) {
        return true;
    }
    ["알리", "다봤어", "다했어", "다읽었어", "확인했어", "이제얘기해", "이제말해", "얘기해봐", "말해봐", "됐어이제", "자이제", "돌아왔어", "imback", "imdone", "okaydone", "allright", "goahead", "youthere"].iter().any(|w| t.contains(w))
}

/// 두 글이 거의 같은가 (두 글자 묶음이 얼마나 겹치는가). AI 자기 목소리를 받아쓴 것을 가려낼 때 쓴다.
fn similar(a: &str, b: &str) -> bool {
    let grams = |s: &str| {
        let c: Vec<char> = s.chars().filter(|c| c.is_alphanumeric()).collect();
        c.windows(2).map(|w| (w[0], w[1])).collect::<std::collections::HashSet<_>>()
    };
    let (ga, gb) = (grams(a), grams(b));
    if gb.len() < 3 {
        return false;
    }
    let common = gb.iter().filter(|g| ga.contains(g)).count();
    common as f32 / gb.len() as f32 > 0.6
}

/// 한 턴에 걸린 시간. 턴이 끝나면 data/timing.jsonl 에 한 줄로 남긴다 (밀리초, 턴 시작부터).
#[derive(Default)]
struct TurnTiming {
    /// 받아쓰기 (걸린 시간, 소리 길이 초) - 말로 한 턴일 때
    stt: Option<(u64, f32)>,
    /// 판단 모델의 결정이 도착한 때
    pre_ms: Option<u64>,
    /// 화면에 첫 출력이 나온 때
    first_ms: Option<u64>,
    /// 첫 소리를 내기 시작한 때
    first_voice: Option<u64>,
    /// 도구 (무엇, 시작, 걸린 시간)
    tools: Vec<(String, u64, u64)>,
    open: Option<(String, u64)>,
    stats: Option<llm::Stats>,
}

impl TurnTiming {
    fn mark_first(&mut self, start: std::time::Instant) {
        self.first_ms.get_or_insert(start.elapsed().as_millis() as u64);
    }

    /// 도구 표시가 바뀔 때마다 부른다: 앞 도구를 닫고 새 도구를 연다.
    fn activity(&mut self, what: &str, start: std::time::Instant) {
        let now = start.elapsed().as_millis() as u64;
        if let Some((w, t)) = self.open.take() {
            self.tools.push((w, t, now - t));
        }
        if !what.is_empty() {
            self.open = Some((what.to_string(), now));
        }
    }

    fn write(&mut self, input: &str, start: std::time::Instant, voice: bool, spoken: &[String], error: Option<&str>) {
        self.activity("", start);
        let total = start.elapsed().as_millis() as u64;
        let (date, time) = sys::now_local();
        let s = self.stats.clone().unwrap_or_default();
        let line = serde_json::json!({
            "at": format!("{date} {time}"),
            "input": input.chars().take(200).collect::<String>(),
            "voice": voice,
            "stt_ms": self.stt.map(|x| x.0),
            "speech_secs": self.stt.map(|x| (x.1 * 10.0).round() / 10.0),
            "pre_ms": self.pre_ms,
            "first_screen_ms": self.first_ms,
            "first_voice_ms": self.first_voice,
            "total_ms": total,
            "model": s.model,
            "api_first_ms": s.first_ms,
            "tokens": { "input": s.input_tokens, "cached": s.cached_tokens, "output": s.output_tokens },
            "tools": self.tools.iter().map(|(w, t, d)| serde_json::json!({ "what": w, "at_ms": t, "ms": d })).collect::<Vec<_>>(),
            "spoken": spoken,
            "error": error,
        });
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(sys::data_dir().join("timing.jsonl")) {
            use std::io::Write;
            let _ = writeln!(f, "{line}");
        }
    }
}

fn theme_path() -> std::path::PathBuf {
    sys::data_dir().join("theme.txt")
}

/// 밝은 화면과 어두운 화면을 바꾼다. `save` 면 다음 실행에도 그대로 쓰도록 남긴다.
pub fn set_theme(ctx: &egui::Context, light: bool, save: bool) {
    ui::set_light(light);
    let theme = if light { egui::Theme::Light } else { egui::Theme::Dark };
    ctx.set_theme(theme);
    ctx.style_mut_of(theme, |s| {
        s.visuals.selection.bg_fill = if light { egui::Color32::from_rgb(196, 214, 240) } else { egui::Color32::from_rgb(70, 71, 77) };
        s.visuals.selection.stroke = egui::Stroke::new(1.0, ui::DIM());
        s.visuals.text_cursor.stroke = egui::Stroke::new(1.5, ui::TEXT());
    });
    // Mac 의 창 제목 줄도 같이 바꾼다.
    ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(if light { egui::SystemTheme::Light } else { egui::SystemTheme::Dark }));
    if save {
        let _ = std::fs::write(theme_path(), if light { "light" } else { "dark" });
    }
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

#[cfg(test)]
mod voice_tests {
    use super::{is_stop_word, similar};

    #[test]
    fn stop_words() {
        for w in ["그만.", "그만해!", "멈춰", "스톱", "조용히 해", "Stop.", "말 그만하라고"] {
            assert!(is_stop_word(w), "{w}");
        }
        for w in ["그만 찾고 다른 걸 보여줘 공모전 말고", "이 영상 만들 수 있어?", ""] {
            assert!(!is_stop_word(w), "{w}");
        }
    }

    #[test]
    fn wait_words() {
        use super::{is_wait_word, is_wake_word};
        for w in ["잠깐만.", "잠깐 기다려 봐", "내가 확인해 볼게", "이거 읽어 볼게요", "잠시만요"] {
            assert!(is_wait_word(w), "{w}");
        }
        for w in ["잠깐, 이거 찾아봐", "잠깐만 그거 보여줘", "공모전 조사해 줘"] {
            assert!(!is_wait_word(w), "{w}");
        }
        assert!(is_wake_word("알리, 다 봤어"));
        assert!(!is_wake_word("여기 마감이 언제지"));
        assert!(is_wake_word("Ali, I'm back"));
        assert!(!is_wake_word("the quality is bad"));
        assert!(is_wait_word("Hold on."));
        assert!(!is_wait_word("wait, find the deadline"));
    }

    #[test]
    fn own_voice() {
        assert!(similar("되감기 여행 구성을 그림으로 보여 드릴게요.", "되감기 여행 구성을 그림으로 보여드릴게요"));
        assert!(!similar("되감기 여행 구성을 그림으로 보여 드릴게요.", "한국 손에서 클로즈업 뭔 소리야"));
    }
}
