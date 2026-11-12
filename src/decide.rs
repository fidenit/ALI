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
    let en = crate::sys::english();
    pre.ack = match pre.kind.as_deref() {
        Some("작업") => Some(if en { "Sure, on it." } else { "네, 바로 해볼게요." }.to_string()),
        Some("화면 수정") => Some(if en { "Okay, updating." } else { "네, 고칠게요." }.to_string()),
        Some("기억 찾기") => Some(if en { "One moment, let me look." } else { "잠깐만요, 찾아볼게요." }.to_string()),
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

/// 분류와 주제의 구성을 살핀다. 합칠 만한 주제, 나눌 만한 분류, 겹치는 분류를 짚어서 돌려준다.
/// 돌려준 것이 있으면 프로그램이 한가할 때 메인 모델에게 "구성 정리"를 시킨다.
///
/// `topics` 는 (ID, 이름, 분류 ID(없으면 빈 문자열), 카드 제목들), `groups` 는 (ID, 이름).
/// `hot` 은 이번 턴에 손댄 주제. 주제가 많아지면 모든 쌍을 볼 수 없으므로, 손댄 주제가 낀 쌍부터 본다.
pub fn structure(topics: &[(String, String, String, String)], groups: &[(String, String)], hot: &[String]) -> Vec<String> {
    let mut hints = Vec::new();
    // 따져 볼 것도 없이 분명한 것들
    if !groups.is_empty() {
        for (id, name, group, _) in topics.iter().filter(|t| t.2.is_empty()) {
            let _ = group;
            hints.push(format!("주제 {id}({name})가 어느 분류에도 들어 있지 않음"));
        }
        if groups.len() == 1 {
            hints.push("분류가 하나뿐이라 나눈 의미가 없음 (없애거나 둘 이상으로 나눌 것)".to_string());
        }
        for (gid, gname) in groups {
            let n = topics.iter().filter(|t| &t.2 == gid).count();
            if n == 1 && groups.len() > 2 {
                hints.push(format!("분류 {gid}({gname})에 주제가 하나뿐임 (다른 분류로 옮기고 없앨지 볼 것)"));
            }
        }
    }
    if jev::key().is_none() || topics.len() < 2 {
        return hints;
    }
    let mut state = String::from("한 사람이 AI 와 여러 가지 일을 하고 있고, 일마다 '주제'가 하나씩 있다. 주제는 '분류'로 묶여 있다.\n\n");
    for (gid, gname) in groups {
        state += &format!("분류 {gname}:\n");
        for (_, name, _, cards) in topics.iter().filter(|t| &t.2 == gid) {
            state += &format!("  - 주제 '{name}' (카드: {cards})\n");
        }
    }
    let loose: Vec<_> = topics.iter().filter(|t| t.2.is_empty()).collect();
    if !loose.is_empty() {
        state += "분류 없음:\n";
        for (_, name, _, cards) in &loose {
            state += &format!("  - 주제 '{name}' (카드: {cards})\n");
        }
    }
    let mut q = Map::new();
    // 같은 분류 안의 주제 쌍: 사실상 같은 일인가
    let mut pairs = Vec::new();
    for i in 0..topics.len() {
        for j in i + 1..topics.len() {
            if topics[i].2 == topics[j].2 {
                pairs.push((i, j));
            }
        }
    }
    pairs.sort_by_key(|(i, j)| !(hot.contains(&topics[*i].0) || hot.contains(&topics[*j].0)));
    pairs.truncate(18);
    for (n, (i, j)) in pairs.iter().enumerate() {
        q.insert(
            format!("same{n}"),
            jev::noul(&format!("주제 '{}'와 주제 '{}'는 같은 일의 일부여서 한 주제로 합치는 것이 낫다", topics[*i].1, topics[*j].1)),
        );
    }
    // 주제가 많이 든 분류: 성격이 다른 두 갈래로 나뉘는가
    let big: Vec<&(String, String)> = groups.iter().filter(|g| topics.iter().filter(|t| t.2 == g.0).count() >= 4).collect();
    for (n, (_, gname)) in big.iter().enumerate() {
        q.insert(format!("split{n}"), jev::noul(&format!("분류 '{gname}'에 든 주제들은 성격이 뚜렷이 다른 두 갈래로 나뉘어서, 분류를 둘로 나누는 것이 낫다")));
    }
    // 분류 쌍: 서로 겹치는가
    let mut gpairs = Vec::new();
    for i in 0..groups.len() {
        for j in i + 1..groups.len() {
            if gpairs.len() < 10 {
                gpairs.push((i, j));
            }
        }
    }
    for (n, (i, j)) in gpairs.iter().enumerate() {
        q.insert(format!("overlap{n}"), jev::noul(&format!("분류 '{}'와 분류 '{}'는 서로 겹쳐서 하나로 합치는 것이 낫다", groups[*i].1, groups[*j].1)));
    }
    if q.is_empty() {
        return hints;
    }
    let started = std::time::Instant::now();
    let a = match jev::decide(&state, Value::Object(q)) {
        Ok(a) => a,
        Err(e) => {
            crate::sys::log("jev.log", &format!("구성 검토 실패: {e}"));
            return hints;
        }
    };
    let p = |name: String| a.get(&name).and_then(|x| x.value).unwrap_or(0.0);
    // 기준값을 다듬을 수 있게 확률을 전부 남긴다.
    let mut seen: Vec<String> = pairs.iter().enumerate().map(|(n, (i, j))| format!("{:.2} 합침 {}+{}", p(format!("same{n}")), topics[*i].1, topics[*j].1)).collect();
    seen.extend(big.iter().enumerate().map(|(n, g)| format!("{:.2} 나눔 {}", p(format!("split{n}")), g.1)));
    seen.extend(gpairs.iter().enumerate().map(|(n, (i, j))| format!("{:.2} 겹침 {}+{}", p(format!("overlap{n}")), groups[*i].1, groups[*j].1)));
    seen.sort_by(|a, b| b.cmp(a));
    crate::sys::log("jev.log", &format!("구성 확률: {}", seen.join(" | ")));
    for (n, (i, j)) in pairs.iter().enumerate() {
        let v = p(format!("same{n}"));
        if v >= 0.35 {
            hints.push(format!("주제 {}({})와 {}({})는 같은 일일 수 있음, 합칠지 볼 것 (검토자 확률 {v:.2})", topics[*i].0, topics[*i].1, topics[*j].0, topics[*j].1));
        }
    }
    for (n, (gid, gname)) in big.iter().enumerate() {
        let v = p(format!("split{n}"));
        if v >= 0.40 {
            hints.push(format!("분류 {gid}({gname})는 성격이 다른 두 갈래일 수 있음, 나눌지 볼 것 (검토자 확률 {v:.2})"));
        }
    }
    for (n, (i, j)) in gpairs.iter().enumerate() {
        let v = p(format!("overlap{n}"));
        if v >= 0.45 {
            hints.push(format!("분류 {}({})와 {}({})는 겹칠 수 있음, 합칠지 볼 것 (검토자 확률 {v:.2})", groups[*i].0, groups[*i].1, groups[*j].0, groups[*j].1));
        }
    }
    crate::sys::log("jev.log", &format!("구성 {}ms  주제 {}  분류 {}  짚은 것 {:?}", started.elapsed().as_millis(), topics.len(), groups.len(), hints));
    hints
}

/// 글자가 겹쳐서 후보로 뽑힌 옛 대화들 가운데, 지금 말에 정말 필요한 것을 고른다.
/// `cands` 는 (글자 겹침 점수, 턴 번호, 내용). 고른 것의 내용을 시간 순서로 돌려준다.
/// 글자가 겹친다고 관련 있는 것은 아니고(같은 낱말, 다른 이야기), 덜 겹쳐도 꼭 필요한 것이 있어서 판단 모델에게 묻는다.
pub fn relevant(input: &str, recent: &str, cands: &[(f32, usize, String)]) -> Vec<String> {
    const MAX: usize = 5;
    let by_letters = || {
        let mut v: Vec<&(f32, usize, String)> = cands.iter().filter(|c| c.0 >= 0.3).take(MAX).collect();
        v.sort_by_key(|c| c.1);
        v.into_iter().map(|c| c.2.clone()).collect::<Vec<_>>()
    };
    if cands.is_empty() || jev::key().is_none() {
        return by_letters();
    }
    let mut state = format!("사용자가 방금 한 말:\n{input}\n");
    if !recent.is_empty() {
        state += &format!("\n직전 대화:\n{recent}\n");
    }
    let mut q = Map::new();
    for (n, (_, _, text)) in cands.iter().enumerate() {
        let short: String = text.chars().take(320).collect();
        q.insert(format!("r{n}"), jev::noul(&format!("아래 옛 대화에는 사용자가 방금 한 말에 답하거나 그 일을 하는 데 필요한 내용이 들어 있다.\n\n{short}")));
    }
    let started = std::time::Instant::now();
    let a = match jev::decide(&state, Value::Object(q)) {
        Ok(a) => a,
        Err(e) => {
            crate::sys::log("jev.log", &format!("옛 대화 고르기 실패: {e}"));
            return by_letters();
        }
    };
    let mut scored: Vec<(f64, &(f32, usize, String))> =
        cands.iter().enumerate().map(|(n, c)| (a.get(&format!("r{n}")).and_then(|x| x.value).unwrap_or(0.0), c)).collect();
    let seen: Vec<String> = scored.iter().map(|(p, c)| format!("{p:.2}/{:.2} #{}", c.0, c.1)).collect();
    crate::sys::log("jev.log", &format!("옛 대화 {}ms  (판단/글자 #턴) {}", started.elapsed().as_millis(), seen.join(" ")));
    // 판단 모델이 필요하다고 본 것, 또는 글자가 아주 많이 겹치는 것.
    scored.retain(|(p, c)| *p >= 0.35 || c.0 >= 0.55);
    scored.sort_by(|x, y| y.0.partial_cmp(&x.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(MAX);
    scored.sort_by_key(|(_, c)| c.1);
    scored.into_iter().map(|(_, c)| c.2.clone()).collect()
}

/// 기다리는 중에 들린 말이 비서를 부르거나 비서에게 하는 말인지 판단한다 (혼잣말, 읽는 소리, 다른 사람과의 대화가 아닌지).
/// 판단하지 못하면 부르지 않은 것으로 본다 (기다려 달라고 했으므로 조용히 있는 쪽이 낫다).
pub fn addressed(key: &str, context: &str, asides: &str, text: &str) -> bool {
    const SYSTEM: &str = "너는 음성 비서가 대답해야 할지 판단한다. 사용자는 비서에게 잠깐 기다리라고 해 두고 다른 일(뭔가 읽기, 확인하기, 다른 사람과 얘기하기, 혼잣말)을 하는 중이다. 이때 마이크에 들린 말 하나를 받는다.\n- 비서를 부르거나, 비서에게 묻거나 시키는 말이거나, 기다리기 전의 대화로 돌아와 이어 가는 말이면 `예`.\n- 혼잣말, 무언가를 소리 내어 읽는 것, 다른 사람에게 하는 말, 비서와 상관없는 말이면 `아니오`.\n애매하면 `아니오`. `예` 또는 `아니오` 한 단어만 출력한다.";
    let user = format!("<기다리기 전 대화>\n{context}</기다리기 전 대화>\n<기다리는 동안 앞서 들린 말>{asides}</기다리는 동안 앞서 들린 말>\n<이번에 들린 말>{text}</이번에 들린 말>");
    let model = std::env::var("LYBOARD_WAKE_MODEL").unwrap_or_else(|_| "claude-haiku-5-5".into());
    let t0 = std::time::Instant::now();
    let answer = crate::agent::ask(key, &model, SYSTEM, &user, 10);
    crate::sys::log("stt.log", &format!("부름 판단 {}ms: {:?} <- {text}", t0.elapsed().as_millis(), answer));
    matches!(answer, Ok(a) if a.trim().starts_with('예'))
}
