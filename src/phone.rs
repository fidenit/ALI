//! 문자와 전화 (Mac). 메시지 앱으로 문자를 보내고, 아이폰을 통해 전화를 건다. 연락처 앱에서 사람을 찾는다.
//!
//! 문자와 전화는 다른 사람에게 바로 닿고 되돌릴 수 없다. 그래서 AI 는 "보낼 준비"만 할 수 있다:
//! 준비하면 확인 카드가 화면에 뜨고, 사용자가 그 카드에서 "보내기"나 "걸기"를 눌러야 이 파일이 실제로 보낸다.
//! AI 가 내는 어떤 명령으로도 확인 없이 보내지지 않는다.

use std::collections::HashMap;
use std::process::Command;
use std::sync::Mutex;

/// 확인을 기다리는 일. 카드 ID 로 찾는다.
#[derive(Clone)]
enum Pending {
    Message { name: String, number: String, text: String },
    Call { name: String, number: String },
}

static OUTBOX: Mutex<Option<HashMap<String, Pending>>> = Mutex::new(None);
static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1);

fn mac_only() -> Result<(), String> {
    if cfg!(target_os = "macos") { Ok(()) } else { Err("문자와 전화는 지금 Mac 에서만 된다".into()) }
}

/// AppleScript 를 실행한다. 값은 argv 로 넘겨서 따옴표 문제를 피한다.
fn osascript(script: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new("osascript").arg("-e").arg(script).args(args).output().map_err(|e| format!("osascript 실행 실패: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if err.contains("-1743") || err.contains("not allowed") || err.contains("권한") {
            Err(format!("Mac 이 접근을 막음. 시스템 설정 → 개인정보 보호 및 보안 → 자동화(또는 연락처)에서 ALI 를 허용해야 함 ({err})"))
        } else {
            Err(err)
        }
    }
}

/// 전화번호를 메시지 앱이 받는 꼴로 바꾼다. 한국 번호(010-…)는 +82 로 시작하게 한다.
pub fn normalize(number: &str) -> Option<String> {
    let plus = number.trim().starts_with('+');
    let digits: String = number.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() < 7 {
        return None;
    }
    Some(if plus {
        format!("+{digits}")
    } else if digits.starts_with("82") && digits.len() >= 11 {
        format!("+{digits}")
    } else if digits.starts_with('0') && (9..=11).contains(&digits.len()) {
        format!("+82{}", &digits[1..])
    } else {
        digits
    })
}

/// 연락처 앱에서 이름(일부)으로 사람을 찾는다: (이름, 전화번호들).
fn contacts(query: &str) -> Result<Vec<(String, Vec<String>)>, String> {
    mac_only()?;
    let script = r#"on run argv
    set q to item 1 of argv
    set out to ""
    tell application "Contacts"
        repeat with p in (every person whose name contains q)
            set nums to ""
            repeat with ph in phones of p
                set nums to nums & (value of ph) & ";"
            end repeat
            set out to out & (name of p) & tab & nums & linefeed
        end repeat
    end tell
    return out
end run"#;
    let raw = osascript(script, &[query])?;
    Ok(raw
        .lines()
        .filter_map(|l| {
            let (name, nums) = l.split_once('\t')?;
            let nums: Vec<String> = nums.split(';').map(str::trim).filter(|n| !n.is_empty()).map(str::to_string).collect();
            Some((name.trim().to_string(), nums))
        })
        .collect())
}

/// contact_find 도구.
pub fn find(query: &str) -> Result<String, String> {
    let found = contacts(query)?;
    if found.is_empty() {
        return Ok(format!("연락처에 '{query}' 가 들어간 사람이 없음"));
    }
    Ok(found.iter().take(20).map(|(n, nums)| format!("{n}: {}", if nums.is_empty() { "전화번호 없음".to_string() } else { nums.join(", ") })).collect::<Vec<_>>().join("\n"))
}

/// 받는 사람(이름이나 번호)을 (보일 이름, 번호)로 정한다. 애매하면 후보를 담은 오류를 돌려준다.
fn resolve(to: &str) -> Result<(String, String), String> {
    let to = to.trim();
    if to.chars().filter(|c| c.is_ascii_digit()).count() >= 7 && !to.chars().any(|c| c.is_alphabetic()) {
        let n = normalize(to).ok_or("전화번호 꼴이 아님")?;
        return Ok((to.to_string(), n));
    }
    let found = contacts(to)?;
    let with_num: Vec<&(String, Vec<String>)> = found.iter().filter(|(_, nums)| !nums.is_empty()).collect();
    match with_num.as_slice() {
        [] => Err(format!("연락처에서 '{to}' 의 전화번호를 찾지 못함. 사용자에게 번호를 물어볼 것")),
        [(name, nums)] if nums.len() == 1 => Ok((name.clone(), normalize(&nums[0]).unwrap_or_else(|| nums[0].clone()))),
        many => Err(format!(
            "'{to}' 에 해당하는 번호가 여러 개라 정하지 못함. 사용자에게 decision 카드로 골라 달라고 한 뒤 번호를 to 에 넣어 다시 부를 것:\n{}",
            many.iter().flat_map(|(n, nums)| nums.iter().map(move |x| format!("{n}: {x}"))).collect::<Vec<_>>().join("\n")
        )),
    }
}

fn hold(p: Pending) -> String {
    let id = format!("out{}", SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    OUTBOX.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(id.clone(), p);
    id
}

fn one_line(s: &str) -> String {
    s.replace(['\r', '\n'], " ")
}

/// message_send 도구: 보낼 준비를 하고, 화면에 낼 확인 카드 명령을 돌려준다.
pub fn prepare_message(to: &str, text: &str, topic: &str) -> Result<String, String> {
    mac_only()?;
    if text.trim().is_empty() {
        return Err("보낼 내용이 비어 있음".into());
    }
    if text.chars().count() > 240 {
        return Err(format!("내용이 {}자라 길다. 240자 안으로 줄이거나 나눠서 따로 준비할 것", text.chars().count()));
    }
    let (name, number) = resolve(to)?;
    let id = hold(Pending::Message { name: name.clone(), number: number.clone(), text: text.to_string() });
    let preview = one_line(text);
    Ok(format!(
        "아직 보내지 않았다. 사용자가 확인 카드에서 '보내기'를 눌러야 보내진다. 아래 명령을 화면에 그대로 내라 (주제 ID 만 바꿀 수 있다). 사용자가 누르면 프로그램이 보내고 카드에 결과를 적는다. 너는 shell 등 다른 방법으로 직접 보내지 않는다.\n\
topic {topic} 문자·전화\n\
card {id}t note {topic} 4x2 보낼 문자\n\
set {id}t text {preview}\n\
card {id} decision {topic} 4x2 문자 보내기 확인\n\
set {id} text {name} ({number}) 에게 옆의 내용을 보낼까요?\n\
add {id} send 보내기 | 이대로 보냅니다\n\
add {id} cancel 취소 | 보내지 않습니다\n\
focus {id}"
    ))
}

/// phone_call 도구: 걸 준비를 하고, 확인 카드 명령을 돌려준다.
pub fn prepare_call(to: &str, topic: &str) -> Result<String, String> {
    mac_only()?;
    let (name, number) = resolve(to)?;
    let id = hold(Pending::Call { name: name.clone(), number: number.clone() });
    Ok(format!(
        "아직 걸지 않았다. 사용자가 확인 카드에서 '걸기'를 눌러야 아이폰을 통해 전화가 걸린다 (통화는 사용자가 한다). 아래 명령을 화면에 그대로 내라.\n\
topic {topic} 문자·전화\n\
card {id} decision {topic} 3x2 전화 걸기 확인\n\
set {id} text {name} ({number}) 에게 전화를 겁니다\n\
add {id} call 걸기 | 아이폰으로 겁니다\n\
add {id} cancel 취소 | 걸지 않습니다\n\
focus {id}"
    ))
}

fn send_message(number: &str, text: &str) -> Result<(), String> {
    // 문자(SMS) 계정이 있으면(아이폰의 문자 전달이 켜져 있으면) 그것으로, 없으면 iMessage 로 보낸다.
    let script = r#"on run argv
    set num to item 1 of argv
    set msg to item 2 of argv
    tell application "Messages"
        set svc to missing value
        try
            set svc to first account whose service type is SMS
        end try
        if svc is missing value then set svc to first account whose service type is iMessage
        send msg to participant num of svc
    end tell
    return "ok"
end run"#;
    osascript(script, &[number, text]).map(|_| ())
}

fn place_call(number: &str) -> Result<(), String> {
    let digits: String = number.chars().filter(|c| c.is_ascii_digit() || *c == '+').collect();
    let st = Command::new("open").arg(format!("tel:{digits}")).status().map_err(|e| e.to_string())?;
    if st.success() { Ok(()) } else { Err("전화 앱을 열지 못함".into()) }
}

/// 사용자가 확인 카드에서 고른 것을 처리한다. 확인 카드가 아니면 None.
/// 돌려주는 값은 그 카드에 반영할 화면 명령들.
pub fn confirm(card: &str, option: &str) -> Option<Vec<String>> {
    let pending = OUTBOX.lock().unwrap_or_else(|e| e.into_inner()).as_mut()?.remove(card)?;
    let (date, time) = crate::sys::now_local();
    let line = match (&pending, option) {
        (_, "cancel") => format!("set {card} text 취소했어요. 보내지 않았습니다."),
        (Pending::Message { name, number, text }, "send") => match send_message(number, text) {
            Ok(()) => {
                crate::sys::log("phone.log", &format!("문자 보냄 → {name} ({number}): {}", one_line(text)));
                format!("set {card} text ✓ {time}에 {name}에게 보냈어요.")
            }
            Err(e) => {
                crate::sys::log("phone.log", &format!("문자 실패 → {name} ({number}): {e}"));
                format!("set {card} text 보내지 못했어요: {}", one_line(&e).chars().take(100).collect::<String>())
            }
        },
        (Pending::Call { name, number }, "call") => match place_call(number) {
            Ok(()) => {
                crate::sys::log("phone.log", &format!("전화 걺 → {name} ({number})"));
                format!("set {card} text ☎ {time}에 {name}에게 전화를 걸었어요.")
            }
            Err(e) => format!("set {card} text 걸지 못했어요: {e}"),
        },
        _ => {
            // 모르는 선택지: 다시 기다린다.
            OUTBOX.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(card.to_string(), pending);
            return None;
        }
    };
    let _ = date;
    Some(vec![line])
}

/// typedstream 으로 저장된 메시지 본문(attributedBody)에서 글을 꺼낸다 (요즘 macOS 는 text 칸이 비어 있는 경우가 많다).
fn decode_body(hex: &str) -> Option<String> {
    let bytes: Vec<u8> = (0..hex.len() / 2).filter_map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()).collect();
    let at = bytes.windows(8).position(|w| w == b"NSString")? + 8;
    let plus = bytes[at..].iter().position(|b| *b == b'+')? + at + 1;
    let (len, start) = match *bytes.get(plus)? {
        0x81 => (u16::from_le_bytes([*bytes.get(plus + 1)?, *bytes.get(plus + 2)?]) as usize, plus + 3),
        0x82 => (u32::from_le_bytes([*bytes.get(plus + 1)?, *bytes.get(plus + 2)?, *bytes.get(plus + 3)?, 0]) as usize, plus + 4),
        n => (n as usize, plus + 1),
    };
    let end = (start + len).min(bytes.len());
    Some(String::from_utf8_lossy(&bytes[start..end]).to_string())
}

/// message_read 도구: 최근 문자를 읽는다. `who` 가 있으면 그 사람(이름이나 번호)과 주고받은 것만.
pub fn read(who: &str, limit: usize) -> Result<String, String> {
    mac_only()?;
    let home = std::env::var("HOME").unwrap_or_default();
    let db = format!("{home}/Library/Messages/chat.db");
    let mut filter = String::new();
    if !who.trim().is_empty() {
        let numbers: Vec<String> = match resolve(who) {
            Ok((_, n)) => vec![n],
            Err(_) => contacts(who).unwrap_or_default().into_iter().flat_map(|(_, nums)| nums).filter_map(|n| normalize(&n)).collect(),
        };
        if numbers.is_empty() {
            return Err(format!("'{who}' 의 번호를 몰라서 고를 수 없음"));
        }
        let ors: Vec<String> = numbers.iter().map(|n| format!("h.id LIKE '%{}'", n.trim_start_matches('+').chars().filter(|c| c.is_ascii_digit()).collect::<String>().chars().rev().take(8).collect::<Vec<_>>().into_iter().rev().collect::<String>())).collect();
        filter = format!("WHERE {}", ors.join(" OR "));
    }
    let sql = format!(
        "SELECT datetime(m.date/1000000000 + 978307200,'unixepoch','localtime'), IFNULL(h.id,''), m.is_from_me, IFNULL(m.text,''), hex(m.attributedBody) \
         FROM message m LEFT JOIN handle h ON m.handle_id = h.ROWID {filter} ORDER BY m.date DESC LIMIT {};",
        limit.clamp(1, 100)
    );
    let out = Command::new("sqlite3").args(["-readonly", "-separator", "\u{1f}", &db, &sql]).output().map_err(|e| format!("sqlite3 실행 실패: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!(
            "문자 기록을 열지 못함. 시스템 설정 → 개인정보 보호 및 보안 → 전체 디스크 접근 권한에서 ALI 를 켜야 함 ({})",
            err.trim()
        ));
    }
    let mut rows = Vec::new();
    for l in String::from_utf8_lossy(&out.stdout).lines() {
        let c: Vec<&str> = l.split('\u{1f}').collect();
        if c.len() < 5 {
            continue;
        }
        let text = if c[3].trim().is_empty() { decode_body(c[4]).unwrap_or_default() } else { c[3].to_string() };
        if text.trim().is_empty() {
            continue;
        }
        rows.push(format!("{}  {} {}: {}", c[0], if c[2] == "1" { "나 →" } else { "←" }, c[1], one_line(&text)));
    }
    rows.reverse();
    Ok(if rows.is_empty() { "문자가 없음".into() } else { rows.join("\n") })
}

#[cfg(test)]
mod tests {
    use super::normalize;

    #[test]
    fn numbers() {
        assert_eq!(normalize("010-1234-5678").as_deref(), Some("+821012345678"));
        assert_eq!(normalize("+82 10 1234 5678").as_deref(), Some("+821012345678"));
        assert_eq!(normalize("02-123-4567").as_deref(), Some("+8221234567"));
        assert_eq!(normalize("12"), None);
    }
}
