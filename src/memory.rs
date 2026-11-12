//! 기억. 세 겹으로 되어 있다.
//!
//!   1. 핵심 기억 (보관함 `기억/핵심.md`)  매 턴 통째로 모델에게 보인다. 대화가 끝날 때마다 작은 모델이
//!      "앞으로도 알아야 할 사실"만 골라 여기에 적는다. 길어지면 스스로 간추린다. 사용자가 직접 고쳐도 된다.
//!   2. 턴 기록 (`data/turns.jsonl`)        모든 턴이 남는다. 프로그램을 껐다 켜도 최근 대화가 이어지고,
//!      새 입력과 관련 있는 옛 턴은 프로그램이 찾아서 `<recall>` 로 함께 보낸다 (모델이 찾으러 가지 않아도 된다).
//!   3. 노트와 일일 기록 (보관함)            지금까지와 같다.

use std::io::Write;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::{sys, vault};

pub const CORE: &str = "기억/핵심.md";
const ARCHIVE: &str = "기억/지난 핵심 기억.md";
const CORE_HEAD: &str = "---\ndescription: 늘 기억하고 있는 핵심 사실. 프로그램이 대화에서 자동으로 추려 적는다 (직접 고쳐도 된다)\ntags:\n  - ali/기억\n---\n# 핵심 기억\n";
/// 핵심 기억이 이보다 길어지면 간추린다 (글자 수).
const CORE_MAX: usize = 6000;
const CORE_TARGET: usize = 3500;

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Turn {
    pub at: String,
    pub user: String,
    pub say: String,
    /// 한 일 (도구 사용)
    #[serde(default)]
    pub did: Vec<String>,
    /// 화면에 낸 명령 원문의 앞부분 (그때 화면에 무엇을 올렸는지)
    #[serde(default)]
    pub ops: String,
    /// 이 턴에서 다룬 주제의 이름들
    #[serde(default)]
    pub topics: String,
}

fn path() -> std::path::PathBuf {
    sys::data_dir().join("turns.jsonl")
}

/// 지금까지의 모든 턴의 찾아보기. 턴이 수만 개가 되어도 가볍도록, 메모리에는 턴마다 "파일에서의 위치"와
/// "글자 묶음의 해시"만 두고, 내용은 찾아낸 몇 개만 파일에서 다시 읽는다.
#[derive(Default)]
pub struct Past {
    entries: Vec<(u64, Vec<u32>)>,
    end: u64,
}

impl Past {
    /// 턴 기록을 읽어 찾아보기를 만든다. 가장 최근 `tail` 개의 턴은 내용도 함께 돌려준다.
    pub fn load(tail: usize) -> (Past, Vec<Turn>) {
        let bytes = std::fs::read(path()).unwrap_or_default();
        let mut past = Past::default();
        let mut last: std::collections::VecDeque<Turn> = Default::default();
        let mut at = 0usize;
        for line in bytes.split_inclusive(|b| *b == b'\n') {
            if let Ok(t) = serde_json::from_slice::<Turn>(line) {
                past.entries.push((at as u64, grams(&doc(&t))));
                last.push_back(t);
                if last.len() > tail {
                    last.pop_front();
                }
            }
            at += line.len();
        }
        past.end = at as u64;
        // 턴마다 나중에 덧붙여진 "찾을 때 쓸 낱말"을 합친다.
        for line in std::fs::read_to_string(keys_path()).unwrap_or_default().lines() {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                past.merge(v["i"].as_u64().unwrap_or(u64::MAX) as usize, v["keys"].as_str().unwrap_or(""));
            }
        }
        (past, last.into())
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 한 턴을 파일에 덧붙이고 찾아보기에도 넣는다.
    pub fn push(&mut self, t: &Turn) {
        let line = format!("{}\n", serde_json::to_string(t).unwrap_or_default());
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path()) {
            if f.write_all(line.as_bytes()).is_ok() {
                self.entries.push((self.end, grams(&doc(t))));
                self.end += line.len() as u64;
            }
        }
    }

    fn merge(&mut self, i: usize, keys: &str) {
        if let Some(e) = self.entries.get_mut(i) {
            e.1.extend(grams(keys));
            e.1.sort_unstable();
            e.1.dedup();
        }
    }

    /// 턴 `i` 에 "찾을 때 쓸 낱말"(비슷한 말, 다른 표기)을 덧붙인다. 다른 낱말로 물어도 이 턴이 찾아지게 된다.
    pub fn add_keys(&mut self, i: usize, keys: &str) {
        if i >= self.entries.len() || keys.trim().is_empty() {
            return;
        }
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(keys_path()) {
            let _ = writeln!(f, "{}", serde_json::json!({ "i": i, "keys": keys }));
        }
        self.merge(i, keys);
    }

    /// 앞에서부터 `upto` 개의 턴 가운데 `query` 와 글자가 겹치는 후보를 고른다. (점수, 턴 번호, 모델에게 보일 글).
    /// 여기서는 넉넉히 고르고, 정말 관련 있는지는 판단 모델이 다시 거른다.
    pub fn candidates(&self, query: &str, upto: usize, max: usize, min: f32) -> Vec<(f32, usize, String)> {
        let upto = upto.min(self.entries.len());
        let sets: Vec<&[u32]> = self.entries[..upto].iter().map(|e| e.1.as_slice()).collect();
        let picked = rank(&sets, query, max, min);
        if picked.is_empty() {
            return Vec::new();
        }
        let Ok(mut f) = std::fs::File::open(path()) else { return Vec::new() };
        let mut found = Vec::new();
        for (score, i) in picked {
            let mut out = String::new();
            use std::io::{BufRead, Seek};
            if f.seek(std::io::SeekFrom::Start(self.entries[i].0)).is_err() {
                continue;
            }
            let mut line = String::new();
            if std::io::BufReader::new(&mut f).read_line(&mut line).is_err() {
                continue;
            }
            let Ok(t) = serde_json::from_str::<Turn>(&line) else { continue };
            out += &format!("[{}]{} 사용자: {}\nsay: {}\n", t.at, if t.topics.is_empty() { String::new() } else { format!(" ({})", t.topics) }, t.user, t.say);
            if !t.did.is_empty() {
                let did: Vec<&str> = t.did.iter().take(6).map(String::as_str).collect();
                out += &format!("한 일: {}\n", did.join(" / "));
            }
            if !t.ops.trim().is_empty() {
                let ops: String = t.ops.chars().take(600).collect();
                out += &format!("그때 화면에 낸 것:\n{}\n", ops.trim());
            }
            found.push((score, i, out));
        }
        found
    }
}

fn keys_path() -> std::path::PathBuf {
    sys::data_dir().join("turn-keys.jsonl")
}

pub fn forget_turns() {
    let _ = std::fs::remove_file(path());
    let _ = std::fs::remove_file(keys_path());
}

// ---------------------------------------------------------------------------
// 회상: 새 입력과 닮은 옛 턴을 찾는다.

/// 낱말 안의 두 글자 묶음 (해시, 정렬됨). 한국어는 조사가 붙어 낱말이 달라지므로 낱말 대신 이것으로 견준다.
pub fn grams(text: &str) -> Vec<u32> {
    let mut out = Vec::new();
    for word in text.to_lowercase().split(|c: char| !c.is_alphanumeric()) {
        let chars: Vec<char> = word.chars().collect();
        for pair in chars.windows(2) {
            out.push((pair[0] as u32).wrapping_mul(0x9E37_79B1) ^ (pair[1] as u32).wrapping_mul(0x85EB_CA6B));
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

fn doc(t: &Turn) -> String {
    format!("{} {} {} {}", t.user, t.say, t.topics, t.ops)
}

/// 관련 있는 턴을 (점수, 번호) 로, 점수가 높은 것부터 돌려준다.
fn rank(docs: &[&[u32]], query: &str, max: usize, min: f32) -> Vec<(f32, usize)> {
    let q = grams(query);
    if q.is_empty() || docs.is_empty() {
        return Vec::new();
    }
    let has = |d: &[u32], g: u32| d.binary_search(&g).is_ok();
    // 흔한 묶음일수록 가볍게 친다.
    let df: Vec<f32> = q.iter().map(|g| docs.iter().filter(|d| has(d, *g)).count() as f32).collect();
    let n = docs.len() as f32;
    // 어느 턴에도 없는 묶음("했었지" 같은 말투)은 견줄 근거가 못 되므로 절반만 친다.
    let weight: Vec<f32> = df.iter().map(|d| if *d > 0.0 { (1.0 + n / (1.0 + d)).ln() } else { (1.0 + n).ln() * 0.5 }).collect();
    let total: f32 = weight.iter().sum();
    let mut scored: Vec<(f32, usize)> = Vec::new();
    for (i, d) in docs.iter().enumerate() {
        let (mut shared, mut sum) = (0, 0.0);
        for (k, g) in q.iter().enumerate() {
            if has(d, *g) {
                shared += 1;
                sum += weight[k];
            }
        }
        // 넉넉히 뽑는 단계(min 이 낮을 때)에서는 한 군데만 겹쳐도 후보로 올린다. 관련 없는 것은 판단 모델이 걸러 낸다.
        let enough = shared >= 2 || (shared >= 1 && (q.len() <= 2 || min < 0.3));
        if enough && sum / total >= min {
            // 같은 이야기가 여러 번 나왔으면 나중 것이 지금의 사실에 가깝다. 최근 것을 조금 더 친다.
            scored.push((sum / total * (0.85 + 0.15 * (i as f32 + 1.0) / n), i));
        }
    }
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then(b.1.cmp(&a.1)));
    scored.truncate(max);
    scored
}

// ---------------------------------------------------------------------------
// 핵심 기억

/// 핵심 기억의 본문 (속성 부분 제외).
pub fn core() -> String {
    let text = vault::read(CORE).unwrap_or_default();
    let body = match text.strip_prefix("---") {
        Some(rest) => rest.split_once("\n---").map(|(_, b)| b).unwrap_or(rest),
        None => &text,
    };
    body.lines().filter(|l| !l.trim().is_empty() && l.trim() != "# 핵심 기억").collect::<Vec<_>>().join("\n")
}

fn save_core(lines: &[String]) {
    let _ = vault::write(CORE, &format!("{CORE_HEAD}{}\n", lines.join("\n")));
}

/// 줄 앞의 "- (날짜)" 를 뗀 사실 부분.
fn fact_of(line: &str) -> &str {
    let t = line.trim().trim_start_matches(['-', '+']).trim();
    match t.strip_prefix('(').and_then(|r| r.split_once(')')) {
        Some((date, rest)) if date.len() <= 12 => rest.trim(),
        _ => t,
    }
}

const KEEPER: &str = "너는 개인 비서 프로그램의 장기 기억을 관리한다. 지금까지의 핵심 기억과 방금 끝난 대화 한 턴을 받는다. 이 턴에서 앞으로의 대화에도 알아야 할 사실만 골라 낸다. 비서는 대화가 끝나면 전부 잊으므로, 여기에 적히지 않은 것은 다음에 모른다.

남길 것
- 사용자에 대한 사실: 이름, 하는 일, 소속, 주변 사람과 반려동물, 쓰는 도구, 파일과 폴더의 위치
- 선호와 일하는 방식, 사용자가 비서의 방식을 고쳐 준 것
- 사용자가 내린 결정
- 진행 중인 일의 목표, 상태, 기한
- 사용자가 기억해 달라고 한 것은 무엇이든

남기지 않을 것
- 한 번 묻고 끝난 것, 누구나 아는 일반 지식, 화면을 어떻게 고쳤는지
- 비서가 추측한 것 (사용자가 말했거나 도구로 확인된 것만 적는다)
- 핵심 기억에 이미 있는 것

출력 형식 (한 줄에 하나, 다른 글은 쓰지 않는다)
+ <새 사실. 이 한 문장만 읽어도 뜻이 통하게 쓴다. \"내일\" 같은 말은 날짜로 바꾼다>
- <지울 기존 사실을 그대로 옮겨 적는다. 틀렸거나, 바뀌었거나, 끝난 일일 때>
바뀐 사실은 옛 줄을 - 로 지우고 새 줄을 + 로 적는다. 남길 사실이 없으면 그 자리에 `없음` 한 마디를 적는다. 대부분의 턴은 `없음`이다.

그리고 맨 마지막 줄에는 언제나 `? ` 로 시작하는 줄을 하나 적는다. 나중에 사용자가 다른 낱말로 물어도 이 대화가 찾아지도록, 이 턴의 내용을 가리키는 낱말을 10~20개 띄어쓰기로 늘어놓는다: 핵심 낱말, 그 비슷한 말과 더 넓은 말, 한국어·영어의 다른 표기. 사용자는 한국어로 물으므로 비슷한 말은 한글로 먼저 적는다(펜션이면 호텔, 숙박, 숙소도). 예: `? 숙소 호텔 숙박 펜션 잠잘곳 제주 제주도 여행 휴가 예약 hotel`";

const TRIMMER: &str = "너는 개인 비서 프로그램의 장기 기억을 간추린다. 핵심 기억 목록을 받아서 같은 내용을 더 짧게 다시 쓴다.
- 겹치는 줄은 하나로 합친다. 끝난 일, 지난 기한, 더 새 줄로 바뀐 옛 사실은 뺀다.
- 사용자에 대한 사실, 선호, 내려진 결정은 빠뜨리지 않는다.
- 한 줄에 사실 하나, `- ` 로 시작한다. 날짜가 붙어 있던 줄은 `- (날짜) 사실` 형태를 지킨다.
- 제목이나 설명 없이 목록만 출력한다.";

static LOCK: Mutex<()> = Mutex::new(());

fn model() -> String {
    std::env::var("LYBOARD_MEMORY_MODEL").unwrap_or_else(|_| "claude-haiku-5-5".into())
}

/// 한 턴이 끝난 뒤 (뒤에서) 핵심 기억을 고친다.
/// 돌려주는 값은 이 턴을 찾을 때 쓸 낱말들.
pub fn keep(key: &str, turn: &Turn, events: &str) -> Option<String> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let before = core();
    let ops: String = turn.ops.chars().take(1500).collect();
    let user = format!(
        "<core>\n{before}\n</core>\n<turn at=\"{}\">\n사용자: {}\n사용자가 화면에서 한 조작: {}\n비서의 한마디: {}\n비서가 한 일: {}\n비서가 화면에 낸 명령:\n{ops}\n</turn>",
        turn.at,
        turn.user,
        if events.is_empty() { "없음" } else { events },
        turn.say,
        if turn.did.is_empty() { "없음".to_string() } else { turn.did.join(" / ") },
    );
    let answer = match crate::agent::ask(key, &model(), KEEPER, &user, 800) {
        Ok(a) => a,
        Err(e) => {
            sys::log("memory.log", &format!("기억 정리 실패: {e}"));
            return None;
        }
    };
    let keys = answer.lines().rev().find_map(|l| l.trim().strip_prefix("? ")).map(|k| k.trim().to_string()).filter(|k| !k.is_empty());
    let mut lines: Vec<String> = before.lines().map(str::to_string).collect();
    let date = turn.at.split(' ').next().unwrap_or("").to_string();
    let mut changed = Vec::new();
    for l in answer.lines().map(str::trim) {
        if let Some(fact) = l.strip_prefix("+ ").map(str::trim).filter(|f| !f.is_empty()) {
            if !lines.iter().any(|x| fact_of(x) == fact) {
                lines.push(format!("- ({date}) {fact}"));
                changed.push(format!("+ {fact}"));
            }
        } else if let Some(old) = l.strip_prefix("- ").map(fact_of).filter(|f| !f.is_empty()) {
            let n = lines.len();
            lines.retain(|x| fact_of(x) != old);
            if lines.len() < n {
                changed.push(format!("- {old}"));
            }
        }
    }
    if changed.is_empty() {
        return keys;
    }
    sys::log("memory.log", &format!("{}: {}", turn.at, changed.join(" | ")));
    save_core(&lines);
    // 길어졌으면 간추린다. 간추리기 전 것은 data 폴더에 남겨 둔다.
    let body = lines.join("\n");
    if body.chars().count() > CORE_MAX {
        let ask = format!("{body}\n\n위 목록을 {CORE_TARGET}자 안쪽으로 간추려라.");
        match crate::agent::ask(key, &model(), TRIMMER, &ask, 4000) {
            Ok(short) if short.lines().filter(|l| l.trim().starts_with("- ")).count() >= 3 => {
                // 간추리면서 빠지는 줄도 잃지 않는다: 간추리기 전의 목록을 보관함에 쌓아 두면 note_search 로 찾아진다.
                let _ = vault::append(
                    ARCHIVE,
                    &if vault::read(ARCHIVE).is_ok() {
                        format!("\n## {} 까지\n{body}", turn.at)
                    } else {
                        format!("---\ndescription: 핵심 기억에서 간추려져 빠진 옛 사실들 (찾을 때만 본다)\ntags:\n  - ali/기억\n---\n\n## {} 까지\n{body}", turn.at)
                    },
                );
                let kept: Vec<String> = short.lines().map(str::trim).filter(|l| l.starts_with("- ")).map(str::to_string).collect();
                sys::log("memory.log", &format!("{}: 간추림 {}줄 → {}줄", turn.at, lines.len(), kept.len()));
                save_core(&kept);
            }
            Ok(_) => sys::log("memory.log", "간추리기 결과가 이상해서 버림"),
            Err(e) => sys::log("memory.log", &format!("간추리기 실패: {e}")),
        }
    }
    keys
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(user: &str, say: &str) -> Turn {
        Turn { at: "2026-11-01 10:00".into(), user: user.into(), say: say.into(), ..Default::default() }
    }

    #[test]
    fn recall_finds_related_turns_only() {
        let turns = [
            turn("제주 여행 숙소는 애월로 정했어", "숙소를 애월로 반영했어요."),
            turn("3분기 매출 보고서 목차 잡아줘", "목차를 잡았어요."),
            turn("강아지 예방접종 일정 알려줘", "접종 일정을 정리했어요."),
        ];
        let sets: Vec<Vec<u32>> = turns.iter().map(|t| grams(&doc(t))).collect();
        let docs: Vec<&[u32]> = sets.iter().map(Vec::as_slice).collect();
        let found: Vec<usize> = rank(&docs, "제주 숙소 어디로 했었지?", 5, 0.3).into_iter().map(|r| r.1).collect();
        assert_eq!(found, vec![0]);
        assert!(rank(&docs, "오늘 날씨 어때", 5, 0.3).is_empty());
        // 다른 낱말("호텔")로 물으면 글자만으로는 못 찾지만, 찾을 때 쓸 낱말을 붙여 두면 찾아진다.
        assert!(rank(&docs, "호텔 예약한 데가 어디였지", 5, 0.15).is_empty());
        let mut with_keys = sets[0].clone();
        with_keys.extend(grams("숙소 호텔 숙박 예약 제주도"));
        with_keys.sort_unstable();
        with_keys.dedup();
        let docs2: Vec<&[u32]> = vec![&with_keys, &sets[1], &sets[2]];
        let found: Vec<usize> = rank(&docs2, "호텔 예약한 데가 어디였지", 5, 0.15).into_iter().map(|r| r.1).collect();
        assert_eq!(found, vec![0]);
    }

    #[test]
    fn recall_stays_fast_with_many_turns() {
        // 하루 20턴씩 5년치(약 36,000턴)가 쌓여도 한 번 찾는 데 걸리는 시간이 짧아야 한다.
        let words = ["제주", "숙소", "보고서", "매출", "강아지", "접종", "운동", "예산", "회식", "스토어", "광고", "면접", "일정", "견적", "서버", "배포"];
        let sets: Vec<Vec<u32>> = (0..36_000usize)
            .map(|i| {
                let text: Vec<String> = (0..40).map(|k| format!("{}{}", words[(i * 7 + k * 3) % words.len()], (i + k) % 97)).collect();
                grams(&text.join(" "))
            })
            .collect();
        let docs: Vec<&[u32]> = sets.iter().map(Vec::as_slice).collect();
        let started = std::time::Instant::now();
        let picked = rank(&docs, "제주 숙소 예산 어떻게 됐지", 5, 0.3);
        let ms = started.elapsed().as_millis();
        assert!(picked.len() <= 5);
        assert!(ms < 400, "36,000턴에서 찾는 데 {ms}ms");
    }

    #[test]
    fn fact_strips_bullet_and_date() {
        assert_eq!(fact_of("- (2026-11-12) 강아지 이름은 콩이다"), "강아지 이름은 콩이다");
        assert_eq!(fact_of("- 강아지 이름은 콩이다"), "강아지 이름은 콩이다");
    }
}
