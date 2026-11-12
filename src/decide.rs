//! 메인 모델의 앞뒤에 놓이는 작은 결정들. 전부 Jev(판단 전용 모델)에 맡긴다.
//!
//!   앞 (before): 입력이 들어온 직후 0.2초쯤에 끝난다.
//!     - 얼마나 깊이 생각할 일인가        → 메인 모델의 노력 수준을 정한다
//!     - 대화인가, 화면 수정인가, 작업인가  → 답이 오기 전에 화면에 바로 표시한다
//!     - 어느 주제의 이야기인가            → 그 주제 구역을 바로 강조한다
//!     - 어떤 기억 노트가 필요한가         → 미리 읽어서 메인 모델에게 같이 준다
//!
//!   뒤 (after): 메인 모델이 화면을 고친 뒤에 돈다.
//!     - 각 카드가 화면만 보고도 무엇인지 알 수 있는가 → 불분명한 카드를 다음 턴에 메인 모델에게 알린다
//!     - 각 카드가 지금 얼마나 중요한가              → 구역 안에서 중요한 카드를 앞에 놓는다
//!
//! Jev 키가 없거나 응답이 늦으면 전부 None 을 돌려주고, 프로그램은 기본값으로 동작한다.

use std::collections::HashMap;

use serde_json::{Map, Value};

use crate::jev;

#[derive(Debug, Clone, Default)]
pub struct Pre {
    /// 메인 모델의 노력 수준 ("low" | "medium" | "high")
    pub effort: Option<String>,
    /// 화면에 바로 띄울 분류 ("대화", "화면 수정", "작업", "기억 찾기")
    pub kind: Option<String>,
    /// 이 입력이 속한 기존 주제의 ID
    pub topic: Option<String>,
    /// 음성 대화에서 답을 기다리는 동안 먼저 건넬 한마디 ("네, 해볼게요.")
    pub ack: Option<String>,
    /// 미리 읽어 둘 기억 노트의 경로
    pub notes: Vec<String>,
}

pub fn before(input: &str, topics: &[(String, String)], notes: &[(String, String)], recent: &str) -> Option<Pre> {
    jev::key()?;
    let mut state = format!("사용자가 방금 한 말:\n{input}\n");
    if !recent.is_empty() {
        state += &format!("\n직전 대화:\n{recent}\n");
    }
    if !topics.is_empty() {
        state += "\n지금 화면에 있는 주제:\n";
        for (id, name) in topics {
            state += &format!("- {id}: {name}\n");
        }
    }
    let opt = |k: &str, v: &str| (k.to_string(), v.to_string());
    let mut q = Map::new();
    q.insert(
        "effort".into(),
        jev::choice(
            "이 요청을 제대로 처리하는 데 필요한 생각의 깊이",
            &[
                opt("low", "인사, 간단한 질문, 화면의 작은 수정"),
                opt("medium", "비교나 계획처럼 따져 볼 것이 있는 요청, 도구를 몇 번 쓰는 일"),
                opt("high", "복잡한 분석, 여러 단계의 긴 작업, 코드나 문서를 새로 만드는 일"),
            ],
        ),
    );
    q.insert(
        "kind".into(),
        jev::choice(
            "이 요청의 종류",
            &[
                opt("대화", "아는 것으로 답할 수 있는 질문이나 이야기"),
                opt("화면 수정", "이미 화면에 있는 카드를 고치거나 지우거나 정리해 달라는 요청"),
                opt("작업", "파일, 명령 실행, 웹 검색 등 컴퓨터에서 실제로 무언가를 해야 하는 요청"),
                opt("기억 찾기", "예전에 말한 것이나 예전에 한 일을 떠올려야 하는 요청"),
            ],
        ),
    );
    if !topics.is_empty() {
        let mut options: Vec<(String, String)> = topics.iter().map(|(id, name)| (id.clone(), name.clone())).collect();
        options.push(opt("new", "위 주제 어디에도 속하지 않는 새로운 이야기"));
        q.insert("topic".into(), jev::choice("이 말이 속하는 주제", &options));
    }
    for (i, (path, desc)) in notes.iter().take(24).enumerate() {
        q.insert(format!("note{i}"), jev::noul(&format!("이 요청을 처리하려면 '{path}' 노트({desc})의 내용이 필요하다")));
    }
    let started = std::time::Instant::now();
    let a = match jev::decide(&state, Value::Object(q)) {
        Ok(a) => a,
        Err(e) => {
            crate::sys::log("jev.log", &format!("실패: {e}"));
            return None;
        }
    };
    let ms = started.elapsed().as_millis();
    let pick = |name: &str, min: f64| a.get(name).filter(|x| x.confidence >= min).and_then(|x| x.choice.clone());
    let mut pre = Pre {
        ack: None,
        effort: pick("effort", 0.5),
        kind: pick("kind", 0.5),
        topic: pick("topic", 0.6).filter(|t| t != "new"),
        notes: notes
            .iter()
            .take(24)
            .enumerate()
            .filter(|(i, _)| a.get(&format!("note{i}")).and_then(|x| x.value).unwrap_or(0.0) >= 0.65)
            .map(|(_, (path, _))| path.clone())
            .collect(),
    };
    // 무엇을 얼마나 확신하고 골랐는지 남긴다 (기준값을 다듬는 근거).
    let raw = |name: &str| a.get(name).map(|x| format!("{}({:.2})", x.choice.clone().unwrap_or_default(), x.confidence)).unwrap_or_default();
    crate::sys::log(
        "jev.log",
        &format!(
            "앞 {ms}ms  입력=\"{}\"  깊이={}  종류={}  주제={}  미리 읽을 노트={:?}",
            input.chars().take(40).collect::<String>(),
            raw("effort"),
            raw("kind"),
            raw("topic"),
            pre.notes
        ),
    );
    // 일을 시킨 것이면 답이 오기 전에 먼저 대답할 말을 정한다 (음성 대화에서 쓴다).
    pre.ack = match pre.kind.as_deref() {
        Some("작업") => Some("네, 바로 해볼게요.".to_string()),
        Some("화면 수정") => Some("네, 고칠게요.".to_string()),
        Some("기억 찾기") => Some("잠깐만요, 찾아볼게요.".to_string()),
        _ => None,
    };
    Some(pre)
}

#[derive(Debug, Clone, Default)]
pub struct Post {
    /// 화면만 봐서는 무엇인지 알기 어려운 카드: (ID, 분명할 확률)
    pub unclear: Vec<(String, f64)>,
    /// 카드별 중요도 (0~1)
    pub importance: HashMap<String, f64>,
}

/// `board` 는 화면 전체(수정 언어), `cards` 는 (ID, 주제 이름, 제목).
pub fn after(board: &str, cards: &[(String, String, String)]) -> Option<Post> {
    jev::key()?;
    if cards.is_empty() {
        return None;
    }
    let state = format!(
        "한 화면에 여러 주제의 카드가 놓여 있다. 사용자는 대화 과정을 보지 않고 이 화면만 본다. 아래는 화면 전체를 명령 형식으로 적은 것이다.\n\n{board}"
    );
    let mut q = Map::new();
    for (id, topic, title) in cards.iter().take(20) {
        q.insert(
            format!("clear_{id}"),
            jev::noul(&format!("'{topic}' 주제의 카드 '{title}'({id})은 주제 이름과 카드 내용만 보고도 무엇에 관한 것인지 분명하다")),
        );
        q.insert(
            format!("imp_{id}"),
            jev::score(
                &format!("'{topic}' 주제의 카드 '{title}'({id})이 지금 사용자에게 중요한 정도"),
                &["참고용이라 안 봐도 된다", "알아 두면 좋다", "지금 꼭 봐야 하거나 사용자의 결정이 필요하다"],
            ),
        );
    }
    let started = std::time::Instant::now();
    let a = match jev::decide(&state, Value::Object(q)) {
        Ok(a) => a,
        Err(e) => {
            crate::sys::log("jev.log", &format!("실패: {e}"));
            return None;
        }
    };
    let ms = started.elapsed().as_millis();
    let mut post = Post::default();
    for (id, _, _) in cards.iter().take(20) {
        if let Some(p) = a.get(&format!("clear_{id}")).and_then(|x| x.value) {
            if p < 0.4 {
                post.unclear.push((id.clone(), p));
            }
        }
        if let Some(s) = a.get(&format!("imp_{id}")).and_then(|x| x.value) {
            // 점수는 단계의 번호(0, 1, 2) 위에서의 위치로 온다. 0~1 로 맞춘다.
            post.importance.insert(id.clone(), (s / 2.0).clamp(0.0, 1.0));
        }
    }
    let mut imp: Vec<String> = post.importance.iter().map(|(id, s)| format!("{id}={s:.2}")).collect();
    imp.sort();
    crate::sys::log("jev.log", &format!("뒤 {ms}ms  카드 {}장  불분명={:?}  중요도 {}", cards.len(), post.unclear, imp.join(" ")));
    Some(post)
}
