//! TypeSafe AI 의 판단 전용 모델 Jev 를 부른다.
//!
//! Jev 는 글을 쓰지 않는다. 상황(state)과 미리 정한 질문들을 주면, 질문마다
//! "정해 준 선택지 중 하나"나 "0~1 사이의 확률"만 돌려준다. 그래서 없는 선택지를 지어낼 수 없고
//! 응답이 0.2초 안팎으로 빠르다. 이 프로그램에서는 메인 모델의 앞뒤에서 작은 결정을 맡는다.
//!
//! 키는 환경 변수 TYPESAFE_API_KEY 또는 프로그램 폴더의 jev.txt 에서 읽는다.
//! 키가 없으면 이 모듈을 쓰는 모든 곳이 기본값으로 동작한다 (프로그램은 그대로 돈다).

use std::collections::HashMap;
use std::time::Duration;

use serde_json::{Value, json};

const URL: &str = "https://api.typesafe.ai/v1/systemone";

pub fn key() -> Option<String> {
    let clean = |s: String| Some(s.trim().to_string()).filter(|s| !s.is_empty());
    std::env::var("TYPESAFE_API_KEY")
        .ok()
        .and_then(clean)
        .or_else(|| std::fs::read_to_string(crate::sys::base_dir().join("jev.txt")).ok().and_then(clean))
}

/// 선택지 중 하나를 고르게 하는 질문. `options` 는 (이름, 설명).
pub fn choice(instructions: &str, options: &[(String, String)]) -> Value {
    let criteria: serde_json::Map<String, Value> = options.iter().map(|(k, v)| (k.clone(), Value::String(v.clone()))).collect();
    json!({ "type": "choice", "instructions": instructions, "criteria": criteria })
}

/// 문장이 참일 확률(0~1)을 묻는 질문.
pub fn noul(statement: &str) -> Value {
    json!({ "type": "noul", "instructions": statement })
}

/// 낮은 쪽부터 높은 쪽으로 늘어놓은 단계 위에서의 위치(0~1)를 묻는 질문.
pub fn score(instructions: &str, steps: &[&str]) -> Value {
    json!({ "type": "score", "instructions": instructions, "criteria": steps })
}

#[derive(Debug, Clone, Default)]
pub struct Answer {
    pub choice: Option<String>,
    /// noul 질문의 확률(0~1), 또는 score 질문에서 단계 번호(0, 1, 2 …) 위의 위치
    pub value: Option<f64>,
    pub confidence: f64,
}

pub type Answers = HashMap<String, Answer>;

/// 응답 본문을 해석한다.
pub fn parse(text: &str) -> Result<Answers, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("Jev 응답 해석 실패: {e}"))?;
    let answers = v["answers"].as_object().ok_or_else(|| {
        format!("Jev 오류: {}", v["error"]["message"].as_str().or(v["message"].as_str()).unwrap_or(&text.chars().take(200).collect::<String>()))
    })?;
    Ok(answers
        .iter()
        .map(|(name, a)| {
            let value = a["score"].as_f64().or(a["noul"].as_f64());
            let answer = Answer {
                choice: a["choice"].as_str().map(str::to_string),
                value,
                // noul 에는 confidence 가 없다: 0.5 에서 멀수록 확신이 큰 것으로 본다.
                confidence: a["confidence"].as_f64().unwrap_or_else(|| value.map(|p| (p - 0.5).abs() * 2.0).unwrap_or(0.0)),
            };
            (name.clone(), answer)
        })
        .collect())
}

/// 질문 묶음을 한 번에 보낸다 (질문들은 같은 상황에 대해 동시에 판단된다).
pub fn decide(state: &str, questions: Value) -> Result<Answers, String> {
    let key = key().ok_or("Jev 키 없음")?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .provider(ureq::tls::TlsProvider::NativeTls)
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .build(),
        )
        .http_status_as_error(false)
        // 빨라야 의미가 있는 결정이다. 늦으면 기다리지 않고 기본값으로 간다.
        .timeout_global(Some(Duration::from_millis(2500)))
        .build()
        .into();
    let body = json!({ "model": "jev-latest", "state": state, "questions": questions }).to_string();
    let mut resp = agent
        .post(URL)
        .header("Authorization", &format!("Bearer {key}"))
        .header("Content-Type", "application/json")
        .send(body)
        .map_err(|e| format!("Jev 연결 실패: {e}"))?;
    let status = resp.status().as_u16();
    let text = resp.body_mut().read_to_string().unwrap_or_default();
    if status != 200 {
        return Err(format!("Jev HTTP {status}: {}", text.chars().take(200).collect::<String>()));
    }
    parse(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_documented_response() {
        // TypeSafe 문서의 응답 예시 그대로
        let text = r#"{"model":"jev-1.13.0","answers":{
            "department":{"type":"choice","choice":"technical","confidence":1.0,"probabilities":{"technical":1.0,"billing":0.0,"sales":0.0}},
            "frustration":{"type":"score","score":0.67,"confidence":0.5,"legend":{"0":"a","1":"b","2":"c"},"probabilities":{"0":0.33,"1":0.67,"2":0.0}},
            "is_urgent":{"type":"noul","noul":0.99}},
            "usage":{"input_tokens":428,"output_tokens":73}}"#;
        let a = parse(text).unwrap();
        assert_eq!(a["department"].choice.as_deref(), Some("technical"));
        assert_eq!(a["frustration"].value, Some(0.67));
        assert_eq!(a["is_urgent"].value, Some(0.99));
        assert!(a["is_urgent"].confidence > 0.9);
    }

    #[test]
    fn builds_documented_request() {
        let q = json!({
            "team": choice("Which team should handle this", &[("billing".into(), "Payment issues".into())]),
            "urgent": noul("The message conveys urgency"),
            "mood": score("How frustrated", &["calm", "angry"]),
        });
        assert_eq!(q["team"]["type"], "choice");
        assert_eq!(q["team"]["criteria"]["billing"], "Payment issues");
        assert_eq!(q["urgent"], json!({"type":"noul","instructions":"The message conveys urgency"}));
        assert_eq!(q["mood"]["criteria"][1], "angry");
    }

    #[test]
    fn error_body_is_reported() {
        assert!(parse(r#"{"error":{"message":"bad key"}}"#).unwrap_err().contains("bad key"));
    }
}
