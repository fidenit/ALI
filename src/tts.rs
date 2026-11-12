//! 음성 출력. 문장 하나를 읽고, 다 읽으면 알려 준다.
//! OS 에 들어 있는 음성을 쓴다 (Windows: SAPI 한국어 음성, Mac: say 명령).
//! 더 좋은 음성 엔진으로 바꿀 때는 이 파일만 바꾸면 된다.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};

pub struct Tts {
    tx: Sender<(u64, String)>,
    speaking: Arc<AtomicBool>,
    /// stop 을 부를 때마다 올라간다. 그 전에 맡긴 문장은 버려진다.
    epoch: Arc<AtomicU64>,
    child: Arc<Mutex<Option<Child>>>,
}

impl Tts {
    /// `wake` 는 문장을 다 읽었을 때 화면을 깨우는 함수.
    pub fn new(wake: impl Fn() + Send + 'static) -> Self {
        let (tx, rx) = channel::<(u64, String)>();
        let speaking = Arc::new(AtomicBool::new(false));
        let epoch = Arc::new(AtomicU64::new(0));
        let child: Arc<Mutex<Option<Child>>> = Arc::new(Mutex::new(None));
        let (sp, ep, ch) = (speaking.clone(), epoch.clone(), child.clone());
        std::thread::spawn(move || {
            // 스피커는 처음 말할 때 연다 (ElevenLabs 목소리를 쓸 때만 필요하다).
            let mut player: Option<crate::audio::Player> = None;
            for (e, text) in rx {
                if e == ep.load(Ordering::SeqCst) {
                    // ElevenLabs 키가 있으면 그 목소리로, 없거나 실패하면 OS 의 기본 음성으로 읽는다.
                    let mut done = false;
                    if crate::eleven::key().is_some() {
                        if player.is_none() {
                            match crate::audio::Player::new() {
                                Ok(p) => player = Some(p),
                                Err(err) => crate::sys::log("tts.log", &format!("스피커를 열지 못함: {err}")),
                            }
                        }
                        if let Some(p) = &player {
                            match crate::eleven::speak(&text, p, &|| ep.load(Ordering::SeqCst) != e) {
                                Ok(()) => done = true,
                                Err(err) => crate::sys::log("tts.log", &format!("ElevenLabs 실패, 기본 음성으로 읽음: {err}")),
                            }
                        }
                    }
                    if !done {
                        speak_blocking(&ch, &text);
                    }
                }
                // 뒤에 맡겨진 문장이 없을 때만 "말이 끝났다"고 알린다.
                if e == ep.load(Ordering::SeqCst) {
                    sp.store(false, Ordering::SeqCst);
                }
                wake();
            }
        });
        Tts { tx, speaking, epoch, child }
    }

    pub fn speak(&self, text: &str) {
        self.speaking.store(true, Ordering::SeqCst);
        let _ = self.tx.send((self.epoch.load(Ordering::SeqCst), text.to_string()));
    }

    /// 읽던 것을 즉시 멈추고 밀린 문장도 버린다.
    pub fn stop(&self) {
        self.epoch.fetch_add(1, Ordering::SeqCst);
        if let Some(mut c) = self.child.lock().unwrap().take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        self.speaking.store(false, Ordering::SeqCst);
    }

    pub fn is_speaking(&self) -> bool {
        self.speaking.load(Ordering::SeqCst)
    }
}

impl Drop for Tts {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(windows)]
fn speak_blocking(child: &Arc<Mutex<Option<Child>>>, text: &str) {
    use std::os::windows::process::CommandExt;
    // 음성 엔진을 한 번만 띄워 두고 문장을 한 줄씩 보낸다. 한글이 깨지지 않도록 base64 로 보낸다.
    const SCRIPT: &str = "Add-Type -AssemblyName System.Speech; \
        $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; \
        try { $v = $s.GetInstalledVoices() | Where-Object { $_.VoiceInfo.Culture.Name -eq 'ko-KR' } | Select-Object -First 1; \
              if ($v) { $s.SelectVoice($v.VoiceInfo.Name) } } catch {}; \
        $s.Rate = 3; if ($env:LYBOARD_MUTE) { $s.Volume = 0 }; \
        while (($l = [Console]::In.ReadLine()) -ne $null) { \
          try { $s.Speak([Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($l))) } catch {}; \
          [Console]::Out.WriteLine('ok') }";
    let (mut stdin, stdout) = {
        let mut guard = child.lock().unwrap();
        if guard.is_none() {
            let spawned = Command::new("powershell.exe")
                .args(["-NoProfile", "-NonInteractive", "-Command", SCRIPT])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .creation_flags(0x0800_0000) // 콘솔 창을 띄우지 않는다
                .spawn();
            match spawned {
                Ok(c) => *guard = Some(c),
                Err(_) => return,
            }
        }
        let c = guard.as_mut().unwrap();
        // 파이프는 이 문장을 읽는 동안만 빌려 쓰고 돌려놓는다.
        match (c.stdin.take(), c.stdout.take()) {
            (Some(i), Some(o)) => (i, o),
            _ => return,
        }
    };
    let ok = writeln!(stdin, "{}", base64(text.as_bytes())).and_then(|_| stdin.flush()).is_ok();
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    let alive = ok && reader.read_line(&mut line).map(|n| n > 0).unwrap_or(false);
    let mut guard = child.lock().unwrap();
    match guard.as_mut() {
        Some(c) if alive => {
            c.stdin = Some(stdin);
            c.stdout = Some(reader.into_inner());
        }
        // 중간에 멈췄거나 엔진이 죽었으면 다음 문장에서 새로 띄운다.
        Some(_) => {
            if let Some(mut c) = guard.take() {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
        None => {}
    }
}

#[cfg(not(windows))]
fn speak_blocking(child: &Arc<Mutex<Option<Child>>>, text: &str) {
    // Mac: 문장마다 say 를 실행한다.
    let spawned = Command::new("say").args(["-r", "210", text]).stdin(Stdio::null()).spawn();
    let Ok(c) = spawned else { return };
    *child.lock().unwrap() = Some(c);
    loop {
        std::thread::sleep(std::time::Duration::from_millis(30));
        let mut guard = child.lock().unwrap();
        match guard.as_mut().map(|c| c.try_wait()) {
            Some(Ok(None)) => continue,
            _ => {
                guard.take();
                return;
            }
        }
    }
}

#[cfg_attr(not(windows), allow(dead_code))]
fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn base64_matches_standard() {
        assert_eq!(super::base64(b""), "");
        assert_eq!(super::base64(b"f"), "Zg==");
        assert_eq!(super::base64(b"fo"), "Zm8=");
        assert_eq!(super::base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(super::base64("가".as_bytes()), "6rCA");
    }
}
