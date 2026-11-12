//! ElevenLabs: 자연스러운 목소리(말하기)와 음성 인식(받아쓰기).
//!
//! 키는 환경 변수 ELEVENLABS_API_KEY 또는 프로그램 폴더의 eleven.txt 에서 읽는다.
//! 키가 없으면 말하기는 OS 의 기본 음성으로 돌아가고, 받아쓰기(마이크 입력)는 쓸 수 없다.
//!
//!   voice.txt       (선택) 쓸 목소리의 voice_id
//!   말하기 모델      환경 변수 LYBOARD_TTS_MODEL (기본 eleven_v4_turbo: 실시간 대화용, 한국어 지원)
//!   받아쓰기 모델    scribe_v3 (안 되면 scribe_v2)

use std::io::Read;
use std::time::{Duration, Instant};

use serde_json::json;

use crate::audio::Player;

const API: &str = "https://api.elevenlabs.io";
/// 기본 목소리 (George). 다른 목소리를 쓰려면 voice.txt 에 voice_id 를 적는다.
const DEFAULT_VOICE: &str = "JBFqnCBsd6RMkjVDRZzb";
const TTS_RATE: u32 = 24000;

fn file(name: &str) -> Option<String> {
    std::fs::read_to_string(crate::sys::base_dir().join(name)).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

pub fn key() -> Option<String> {
    std::env::var("ELEVENLABS_API_KEY").ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).or_else(|| file("eleven.txt"))
}

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .provider(ureq::tls::TlsProvider::NativeTls)
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .build(),
        )
        .http_status_as_error(false)
        .timeout_global(Some(timeout))
        .build()
        .into()
}

/// 문장 하나를 소리 내어 읽는다. 소리가 도착하는 대로 틀기 시작하고, 다 틀 때까지 기다린다.
/// `stopped` 가 참을 돌려주면 즉시 멈춘다.
pub fn speak(text: &str, player: &Player, stopped: &dyn Fn() -> bool) -> Result<(), String> {
    let key = key().ok_or("ElevenLabs 키 없음")?;
    let voice = file("voice.txt").unwrap_or_else(|| DEFAULT_VOICE.to_string());
    let model = std::env::var("LYBOARD_TTS_MODEL").unwrap_or_else(|_| "eleven_v4_turbo".to_string());
    let started = Instant::now();
    let mut resp = agent(Duration::from_secs(60))
        .post(&format!("{API}/v1/text-to-speech/{voice}/stream?output_format=pcm_{TTS_RATE}"))
        .header("xi-api-key", &key)
        .header("Content-Type", "application/json")
        .send(json!({ "text": text, "model_id": model }).to_string())
        .map_err(|e| format!("연결 실패: {e}"))?;
    let status = resp.status().as_u16();
    if status != 200 {
        let body = resp.body_mut().read_to_string().unwrap_or_default();
        return Err(format!("HTTP {status}: {}", body.chars().take(200).collect::<String>()));
    }
    let mut reader = resp.into_body().into_reader();
    let mut buf = [0u8; 4800];
    let mut carry: Option<u8> = None; // 샘플은 2바이트인데 조각이 홀수 바이트로 끊길 수 있다
    let mut first_ms = 0;
    let mut secs = 0.0_f32;
    loop {
        if stopped() {
            player.clear();
            return Ok(());
        }
        let n = reader.read(&mut buf).map_err(|e| format!("소리 수신 끊김: {e}"))?;
        if n == 0 {
            break;
        }
        if first_ms == 0 {
            first_ms = started.elapsed().as_millis();
        }
        let mut bytes: Vec<u8> = carry.take().into_iter().collect();
        bytes.extend_from_slice(&buf[..n]);
        if bytes.len() % 2 == 1 {
            carry = bytes.pop();
        }
        let pcm: Vec<i16> = bytes.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
        secs += pcm.len() as f32 / TTS_RATE as f32;
        player.push(&pcm, TTS_RATE);
    }
    crate::sys::log("tts.log", &format!("{model}  첫 소리 {first_ms}ms  길이 {secs:.1}초  \"{}\"", text.chars().take(30).collect::<String>()));
    // 다 틀 때까지 기다린다.
    while player.pending_secs() > 0.02 {
        if stopped() {
            player.clear();
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}

/// 녹음한 소리(16kHz, 16비트, 한 채널)를 글로 바꾼다.
pub fn transcribe(pcm: &[i16]) -> Result<String, String> {
    let key = key().ok_or("ElevenLabs 키 없음")?;
    let audio: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
    let started = Instant::now();
    let mut last = String::new();
    for model in ["scribe_v3", "scribe_v2"] {
        let boundary = "----lyboard7d81b3c9f2a44e6f";
        let mut body = Vec::new();
        let mut field = |name: &str, value: &str| {
            body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n").as_bytes());
        };
        field("model_id", model);
        field("file_format", "pcm_s16le_16");
        field("tag_audio_events", "false");
        // 고른 대화 언어로 받아쓴다 (짧은 말에서 언어를 잘못 짐작하는 일을 막는다).
        field("language_code", if crate::sys::english() { "eng" } else { "kor" });
        body.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"speech.pcm\"\r\nContent-Type: application/octet-stream\r\n\r\n").as_bytes(),
        );
        body.extend_from_slice(&audio);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        let mut resp = agent(Duration::from_secs(60))
            .post(&format!("{API}/v1/speech-to-text"))
            .header("xi-api-key", &key)
            .header("Content-Type", &format!("multipart/form-data; boundary={boundary}"))
            .send(&body[..])
            .map_err(|e| format!("연결 실패: {e}"))?;
        let status = resp.status().as_u16();
        let text = resp.body_mut().read_to_string().unwrap_or_default();
        if status == 200 {
            let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
            let heard = v["text"].as_str().unwrap_or("").trim().to_string();
            crate::sys::log(
                "stt.log",
                &format!("{model}  {}ms  소리 {:.1}초  언어 {}  \"{heard}\"", started.elapsed().as_millis(), pcm.len() as f32 / 16000.0, v["language_code"].as_str().unwrap_or("?")),
            );
            return Ok(heard);
        }
        last = format!("HTTP {status}: {}", text.chars().take(200).collect::<String>());
        crate::sys::log("stt.log", &format!("{model} 실패: {last}"));
    }
    Err(last)
}
