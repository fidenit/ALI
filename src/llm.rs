//! 한 턴을 별도 스레드에서 돌리고, 일어나는 일을 화면 쪽으로 넘긴다.
//! 실제 호출과 도구 실행은 agent.rs 에 있다. API 키가 없으면 같은 경로로 시연 대본을 흘려보낸다.

use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

pub enum Msg {
    /// 모델이 낸 글 조각 (화면 수정 명령)
    Delta(String),
    /// 지금 하고 있는 일 한 줄 ("명령 실행: …"). 빈 문자열이면 끝났다는 뜻.
    Activity(String),
    /// 일하는 도중에 화면을 고쳐 달라는 요청. 거부된 명령의 이유를 `reply` 로 돌려준다.
    Screen { ops: String, reply: Sender<String> },
    /// 웹 카드 안의 페이지에서 스크립트를 실행해 달라는 요청. 결과를 `reply` 로 돌려준다.
    Browser { card: String, script: String, reply: Sender<String> },
    /// 판단 모델이 메인 모델보다 먼저 내린 결정 (요청의 종류, 주제 등)
    Pre(crate::decide::Pre),
    /// 판단 모델이 화면을 검토한 결과
    Review(crate::decide::Post),
    /// 판단 모델이 분류·주제의 구성에서 고칠 만한 것을 짚었다
    Tidy(Vec<String>),
    /// 맡긴 작업의 소식 (시작, 진행, 끝)
    Job(crate::jobs::Event),
    /// 보드에서 주제 하나의 내용을 전부 읽어 달라는 요청 (보드가 커서 줄여 보냈을 때)
    Board { topic: String, reply: Sender<String> },
    /// 기억 정리가 끝나서, 턴 하나에 "찾을 때 쓸 낱말"이 붙었다 (턴 번호, 낱말들)
    Keys(usize, String),
    Done(Stats),
    Error(String),
}

#[derive(Default, Clone)]
pub struct Stats {
    pub model: String,
    pub input_tokens: u64,
    pub cached_tokens: u64,
    pub output_tokens: u64,
    pub first_ms: u64,
    pub total_ms: u64,
}

#[derive(Clone)]
pub struct Config {
    pub key: Option<String>,
    pub model: String,
    pub effort: String,
}

impl Config {
    pub fn load() -> Self {
        let env = |k: &str| std::env::var(k).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        let key = env("ANTHROPIC_API_KEY").or_else(|| {
            std::fs::read_to_string(crate::sys::base_dir().join("key.txt")).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
        });
        // LYBOARD_DEMO 가 있으면 키가 있어도 시연 대본으로 돈다 (화면 확인용, API 비용 없음).
        let key = if env("LYBOARD_DEMO").is_some() { None } else { key };
        Config {
            key,
            model: env("LYBOARD_MODEL").unwrap_or_else(|| "claude-opus-5-5".into()),
            // 화면 반응 속도가 중요해서 기본은 low. 형태 선택 품질이 아쉬우면 medium 으로 올린다.
            effort: env("LYBOARD_EFFORT").unwrap_or_else(|| "low".into()),
        }
    }
    pub fn demo(&self) -> bool {
        self.key.is_none()
    }
}

/// 판단 모델이 먼저 볼 재료.
#[derive(Default)]
pub struct PreInput {
    pub input: String,
    /// 화면에 있는 주제 (ID, 이름)
    pub topics: Vec<(String, String)>,
    /// 기억 노트 (경로, 설명)
    pub notes: Vec<(String, String)>,
    pub recent: String,
    /// 음성이 켜져 있는가
    pub voice: bool,
    /// 이번 말과 글자가 겹치는 옛 대화 후보 (점수, 턴 번호, 내용)
    pub recall: Vec<(f32, usize, String)>,
}

/// 한 턴을 별도 스레드에서 실행한다. `wake` 는 화면을 다시 그리게 깨우는 함수.
pub fn run_turn(cfg: Config, turn_text: String, pre: PreInput, turn_no: usize, tx: Sender<Msg>, wake: impl Fn() + Send + 'static) {
    std::thread::spawn(move || {
        let send = |m: Msg| {
            let _ = tx.send(m);
            wake();
        };
        let (mut cfg, mut turn_text) = (cfg, turn_text);
        // 메인 모델보다 먼저: 판단 모델이 요청의 종류, 주제, 필요한 생각의 깊이, 필요한 기억을 정한다.
        if !cfg.demo() && !pre.input.is_empty() {
            if let Some(p) = crate::decide::before(&pre.input, &pre.topics, &pre.notes, &pre.recent) {
                if let Some(e) = &p.effort {
                    cfg.effort = e.clone();
                }
                let mut loaded = String::new();
                for path in &p.notes {
                    if let Ok(text) = crate::vault::read(path) {
                        let text: String = text.chars().take(6000).collect();
                        loaded += &format!("<note path=\"{path}\">\n{text}\n</note>\n");
                    }
                }
                if !loaded.is_empty() {
                    turn_text += &format!("\n<notes>\n{loaded}</notes>");
                }
                // 음성 대화: 답을 기다리는 동안 먼저 대답한다. 메인 모델에게는 이미 한 말을 알려서 같은 인사를 되풀이하지 않게 한다.
                let mut p = p;
                if !pre.voice {
                    p.ack = None;
                }
                if let Some(ack) = &p.ack {
                    turn_text += &format!("\n<already_said>{ack}</already_said>");
                }
                send(Msg::Pre(p));
            }
        }
        // 옛 대화: 후보 가운데 정말 필요한 것만 골라 함께 보낸다.
        if !cfg.demo() && !pre.recall.is_empty() {
            let picked = crate::decide::relevant(&pre.input, &pre.recent, &pre.recall);
            if !picked.is_empty() {
                turn_text += &format!("\n<recall>\n{}</recall>", picked.join(""));
            }
        }
        let result = match &cfg.key {
            Some(key) => crate::agent::run(&cfg, key, &turn_text, &send),
            None => demo(turn_no, &send),
        };
        match result {
            Ok(stats) => send(Msg::Done(stats)),
            Err(e) => send(Msg::Error(e)),
        }
    });
}

// ---------------------------------------------------------------------------
// 시연 모드: 미리 쓴 대본을 실제 스트림처럼 조금씩 흘려보낸다.

const SCENES: [&str; 3] = [
    "say 제주 3박 4일 초안을 잡았어요. 숙소만 골라 주세요.
topic trip 제주 여행
card s1 status trip 준비 상태
set s1 state run
set s1 text 항공 확정 · 숙소 미정
tell 예상 비용은 구십사만 원이고 예산 안에 들어옵니다.
card m1 metric trip 예상 총비용
set m1 value 94
set m1 unit 만원
set m1 delta -6 만원
set m1 text 2인 기준, 예산 100만원 대비
card c1 chart trip 4x3 비용 구성
set c1 unit 만원
add c1 a 항공 | 22
add c1 b 숙소 | 36
add c1 c 렌터카 | 14
add c1 d 식비 | 16
add c1 e 기타 | 6
tell 일정은 나흘로 짰고 출발 전에 할 일이 세 가지 남았습니다.
card t1 timeline trip 3x4 일정
add t1 d1 1일차 | 도착 · 애월 해안도로 | todo
add t1 d2 2일차 | 한라산 영실 코스 | todo
add t1 d3 3일차 | 성산 · 우도 | todo
add t1 d4 4일차 | 동문시장 · 출발 | todo
card l1 checklist trip 3x3 출발 전
add l1 i1 done | 항공권 결제
add l1 i2 todo | 숙소 예약
add l1 i3 todo | 렌터카 예약
add l1 i4 todo | 한라산 탐방 예약
tell 숙소는 세 곳 중에서 골라 주세요.
card d1 decision trip 4x3 숙소 결정
set d1 text 어느 쪽으로 예약할까요?
add d1 o1 애월 바다 앞 | 1박 13만원, 공항 30분, 조용함
add d1 o2 서귀포 시내 | 1박 11만원, 맛집 가까움, 주차 불편
add d1 o3 성산 독채 | 1박 15만원, 일출봉 도보, 공항 멀다
focus d1
",
    "say 숙소를 반영하고 예약을 진행하고 있어요.
del d1
set s1 state ok
set s1 text 항공 · 숙소 확정
upd l1 i2 done | 숙소 예약
set m1 value 98
set m1 delta -2 만원
upd c1 b 숙소 | 40
card p1 progress trip 3x3 예약 진행
set p1 pct 60
set p1 text 렌터카 조회 중
add p1 a done | 숙소 결제
add p1 b done | 확인 메일 수신
add p1 c run | 렌터카 가격 비교
add p1 d todo | 한라산 탐방 예약
focus p1
",
    "say 노트북 후보 셋을 비교했어요.
topic nb 노트북 교체
card n1 table nb 6x3 후보 비교
set n1 cols 모델 | 무게 | 배터리 | 가격 | 한 줄 평
add n1 r1 A 14형 | 1.24kg | 18시간 | 189만원 | 가볍고 오래감
add n1 r2 B 16형 | 1.95kg | 12시간 | 239만원 | 화면 크고 성능 높음
add n1 r3 C 13형 | 1.10kg | 15시간 | 159만원 | 가장 싸고 포트 적음
card n2 chart nb 3x2 가격
set n2 unit 만원
add n2 a A 14형 | 189
add n2 b B 16형 | 239
add n2 c C 13형 | 159
card n3 decision nb 3x2 우선순위
set n3 text 무엇이 가장 중요한가요?
add n3 o1 휴대성 | 매일 들고 다님
add n3 o2 성능 | 영상 편집, 빌드
add n3 o3 가격 | 150만원대까지
focus n3
",
];

fn demo(turn_no: usize, send: &dyn Fn(Msg)) -> Result<Stats, String> {
    let started = Instant::now();
    let script = SCENES[turn_no % SCENES.len()];
    std::thread::sleep(Duration::from_millis(500));
    let first_ms = started.elapsed().as_millis() as u64;
    let chars: Vec<char> = script.chars().collect();
    for piece in chars.chunks(3) {
        send(Msg::Delta(piece.iter().collect()));
        std::thread::sleep(Duration::from_millis(9));
    }
    Ok(Stats {
        model: "시연 대본".into(),
        output_tokens: (chars.len() / 2) as u64,
        first_ms,
        total_ms: started.elapsed().as_millis() as u64,
        ..Default::default()
    })
}
