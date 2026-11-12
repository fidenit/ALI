//! 운영체제에 닿는 작은 것들: 프로그램 폴더, 현재 시각, 창 상단바 색.

use std::path::PathBuf;

/// 프로그램의 모든 파일(키, 기록, 보관함)이 놓이는 폴더. 실행 파일이 있는 폴더다.
/// 그래서 폴더를 통째로 다른 곳에 복사해도 그대로 동작한다.
/// (개발 중 cargo 로 실행하면 실행 파일이 target 아래에 있으므로 프로젝트 폴더를 쓴다.)
pub fn base_dir() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_default();
    let dir = exe.parent().map(PathBuf::from).unwrap_or_default();
    let name = |p: &PathBuf| p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    if matches!(name(&dir).as_str(), "release" | "debug") {
        if let Some(target) = dir.parent().map(PathBuf::from) {
            if name(&target) == "target" {
                if let Some(root) = target.parent() {
                    return root.to_path_buf();
                }
            }
        }
    }
    dir
}

pub fn data_dir() -> PathBuf {
    let d = base_dir().join("data");
    let _ = std::fs::create_dir_all(&d);
    d
}

/// 현재 지역 시각: ("2026-11-12", "14:05")
#[cfg(windows)]
pub fn now_local() -> (String, String) {
    #[repr(C)]
    #[derive(Default)]
    struct SystemTime {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        ms: u16,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetLocalTime(t: *mut SystemTime);
    }
    let mut t = SystemTime::default();
    unsafe { GetLocalTime(&mut t) };
    (format!("{:04}-{:02}-{:02}", t.year, t.month, t.day), format!("{:02}:{:02}", t.hour, t.minute))
}

#[cfg(not(windows))]
pub fn now_local() -> (String, String) {
    let out = std::process::Command::new("date").arg("+%Y-%m-%d %H:%M").output();
    let s = out.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
    match s.split_once(' ') {
        Some((d, t)) => (d.to_string(), t.to_string()),
        None => ("0000-00-00".into(), "00:00".into()),
    }
}

/// 창 상단바를 앱 배경과 같은 색으로 맞춘다. 성공하면 true.
/// 상단바 자체는 OS 것을 그대로 쓰므로 끌기, 최대화, 화면 분할 같은 동작은 그대로다.
#[cfg(windows)]
pub fn dark_titlebar(title: &str, rgb: (u8, u8, u8), text_rgb: (u8, u8, u8)) -> bool {
    use std::ffi::c_void;
    #[link(name = "user32")]
    unsafe extern "system" {
        fn FindWindowW(class: *const u16, title: *const u16) -> *mut c_void;
    }
    #[link(name = "dwmapi")]
    unsafe extern "system" {
        fn DwmSetWindowAttribute(hwnd: *mut c_void, attr: u32, value: *const c_void, size: u32) -> i32;
    }
    let wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    let colorref = |c: (u8, u8, u8)| (c.0 as u32) | (c.1 as u32) << 8 | (c.2 as u32) << 16;
    unsafe {
        let hwnd = FindWindowW(std::ptr::null(), wide.as_ptr());
        if hwnd.is_null() {
            return false;
        }
        let set = |attr: u32, v: u32| DwmSetWindowAttribute(hwnd, attr, &v as *const u32 as *const c_void, 4);
        set(20, 1); // 어두운 상단바 (Windows 10 20H1 이상)
        set(35, colorref(rgb)); // 상단바 색 (Windows 11)
        set(36, colorref(text_rgb)); // 상단바 글자 색
        set(34, colorref(rgb)); // 창 테두리 색
    }
    true
}

#[cfg(not(windows))]
pub fn dark_titlebar(_title: &str, _rgb: (u8, u8, u8), _text_rgb: (u8, u8, u8)) -> bool {
    true // Mac 은 시스템 설정(다크 모드)을 따른다
}

fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out += &format!("%{b:02X}");
        }
    }
    out
}

/// 설치된 옵시디언의 실행 파일.
pub fn obsidian_exe() -> Option<PathBuf> {
    #[cfg(windows)]
    let candidates: Vec<PathBuf> = ["LOCALAPPDATA", "ProgramFiles"]
        .iter()
        .filter_map(|v| std::env::var(v).ok())
        .flat_map(|d| [PathBuf::from(&d).join(r"Programs\Obsidian\Obsidian.exe"), PathBuf::from(&d).join(r"Obsidian\Obsidian.exe")])
        .collect();
    #[cfg(not(windows))]
    let candidates = vec![PathBuf::from("/Applications/Obsidian.app/Contents/MacOS/Obsidian")];
    candidates.into_iter().find(|p| p.exists())
}

fn obsidian_config() -> Option<PathBuf> {
    #[cfg(windows)]
    return std::env::var("APPDATA").ok().map(|d| PathBuf::from(d).join("obsidian").join("obsidian.json"));
    #[cfg(not(windows))]
    return std::env::var("HOME").ok().map(|d| PathBuf::from(d).join("Library/Application Support/obsidian/obsidian.json"));
}

/// 이 프로그램의 보관함을 옵시디언의 보관함 목록에 넣는다 (이미 있으면 아무것도 안 한다).
/// 옵시디언은 목록에 있는 보관함만 주소로 열 수 있다. 프로그램 폴더를 다른 PC 로 옮겨도
/// 그 PC 에 옵시디언이 있으면 처음 실행할 때 알아서 등록된다.
pub fn register_obsidian_vault(vault: &std::path::Path) {
    if obsidian_exe().is_none() {
        return;
    }
    let Some(cfg) = obsidian_config() else { return };
    let mut json: serde_json::Value =
        std::fs::read_to_string(&cfg).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_else(|| serde_json::json!({}));
    if !json["vaults"].is_object() {
        json["vaults"] = serde_json::json!({});
    }
    let path = vault.to_string_lossy().to_string();
    // 경로 비교: 구분자와 끝의 구분자, 대소문자 차이는 무시한다.
    let norm = |p: &str| p.replace('/', r"\").trim_end_matches(r"\").to_lowercase();
    let same = |p: &str| norm(p) == norm(&path);
    if json["vaults"].as_object().unwrap().values().any(|v| v["path"].as_str().map(same).unwrap_or(false)) {
        return;
    }
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
    let id = format!("{:016x}", ms.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ path.len() as u64);
    json["vaults"][id] = serde_json::json!({ "path": path, "ts": ms, "open": true });
    if let Some(dir) = cfg.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&cfg, json.to_string());
}

/// 노트를 옵시디언에서 연다. 노트가 아직 없으면 보관함을 열고, 옵시디언이 없으면 폴더를 연다.
pub fn open_note(vault: &std::path::Path, note: &std::path::Path) {
    register_obsidian_vault(vault);
    let target = if note.exists() { note } else { vault };
    let uri = format!("obsidian://open?path={}", percent_encode(&target.to_string_lossy()));
    match obsidian_exe() {
        // 주소를 실행 파일에 직접 넘긴다 (주소 처리기 등록 여부와 상관없이 열린다).
        Some(exe) => {
            let _ = std::process::Command::new(exe).arg(uri).spawn();
        }
        None => {
            let opener = if cfg!(windows) { "explorer.exe" } else { "open" };
            let _ = std::process::Command::new(opener).arg(vault).spawn();
        }
    }
}

/// data 폴더의 로그 파일에 한 줄 덧붙인다 (시각 포함).
pub fn log(name: &str, text: &str) {
    use std::io::Write;
    let (date, time) = now_local();
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(data_dir().join(name)) {
        let _ = writeln!(f, "{date} {time}  {text}");
    }
}
