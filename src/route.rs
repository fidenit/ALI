//! 경로. 지도 서비스 화면을 띄우는 대신, 필요한 것만 직접 그린다: 들르는 곳의 위치, 길의 모양, 구간별 거리와 시간.
//!
//!   좌표 찾기   오픈스트리트맵(Nominatim)
//!   길 찾기     OSRM 공개 서버 (자동차, 교통 상황은 반영되지 않는다)
//!   길의 모양   카드마다 `data/routes/<카드ID>.json` 에 둔다 (보드에는 싣지 않는다: 점이 수백 개라 모델에게 보낼 까닭이 없다)

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use crate::sys;

/// (위도, 경도)
pub type Point = (f64, f64);

static CACHE: Mutex<Option<HashMap<String, Option<Arc<Vec<Point>>>>>> = Mutex::new(None);

fn file(card: &str) -> std::path::PathBuf {
    sys::data_dir().join("routes").join(format!("{card}.json"))
}

/// 이 카드의 길 모양 (없으면 None: 그때는 들르는 곳을 곧은 선으로 잇는다).
pub fn path(card: &str) -> Option<Arc<Vec<Point>>> {
    let mut guard = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let cache = guard.get_or_insert_with(HashMap::new);
    if let Some(hit) = cache.get(card) {
        return hit.clone();
    }
    let loaded = std::fs::read_to_string(file(card)).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).and_then(|v| {
        let pts: Vec<Point> = v["path"].as_array()?.iter().filter_map(|p| Some((p[0].as_f64()?, p[1].as_f64()?))).collect();
        (pts.len() >= 2).then(|| Arc::new(pts))
    });
    cache.insert(card.to_string(), loaded.clone());
    loaded
}

/// 기억해 둔 길 모양을 잊는다 (초기화했을 때).
pub fn forget() {
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

fn get(url: &str) -> Result<Value, String> {
    let mut resp = crate::agent::http().get(url).header("user-agent", "ALI-desktop/0.1").call().map_err(|e| format!("연결 실패: {e}"))?;
    let text = resp.body_mut().read_to_string().map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|_| format!("응답을 읽지 못함: {}", text.chars().take(160).collect::<String>()))
}

fn encode(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

/// 이름이나 주소로 좌표를 찾는다. (찾은 곳의 이름, 좌표)
pub fn geocode(query: &str) -> Result<(String, Point), String> {
    let v = get(&format!("https://nominatim.openstreetmap.org/search?q={}&format=jsonv2&limit=1&accept-language=ko", encode(query)))?;
    let p = v.get(0).ok_or_else(|| format!("'{query}' 의 위치를 찾지 못함"))?;
    let num = |k: &str| p[k].as_str().and_then(|x| x.parse::<f64>().ok());
    match (num("lat"), num("lon")) {
        (Some(lat), Some(lng)) => Ok((p["display_name"].as_str().unwrap_or("").to_string(), (lat, lng))),
        _ => Err(format!("'{query}' 의 위치를 찾지 못함")),
    }
}

/// 구글이 쓰는 방식으로 줄여 적은 선을 푼다 (정밀도 5자리).
pub fn decode_polyline(s: &str) -> Vec<Point> {
    let (mut lat, mut lng) = (0i64, 0i64);
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    let next = |i: &mut usize| -> Option<i64> {
        let (mut shift, mut result) = (0u32, 0i64);
        loop {
            let b = *bytes.get(*i)? as i64 - 63;
            *i += 1;
            result |= (b & 0x1f) << shift;
            shift += 5;
            if b < 0x20 {
                break;
            }
        }
        Some(if result & 1 != 0 { !(result >> 1) } else { result >> 1 })
    };
    while i < bytes.len() {
        let (Some(dlat), Some(dlng)) = (next(&mut i), next(&mut i)) else { break };
        lat += dlat;
        lng += dlng;
        out.push((lat as f64 / 1e5, lng as f64 / 1e5));
    }
    out
}

pub fn km_text(meters: f64) -> String {
    let km = meters / 1000.0;
    if km >= 10.0 { format!("{km:.0}km") } else if km >= 1.0 { format!("{km:.1}km") } else { format!("{:.0}m", meters) }
}

pub fn time_text(seconds: f64) -> String {
    let min = (seconds / 60.0).round() as i64;
    if min >= 60 { if min % 60 == 0 { format!("{}시간", min / 60) } else { format!("{}시간 {}분", min / 60, min % 60) } } else { format!("{}분", min.max(1)) }
}

/// 들르는 곳들을 순서대로 잇는 자동차 길을 찾고, 길의 모양을 `card` 의 것으로 저장한다.
/// 돌려주는 글은 모델에게 주는 결과: 구간별 거리·시간과, 화면에 그릴 명령.
pub fn find(input: &Value) -> Result<String, String> {
    let card = input["card"].as_str().map(str::trim).filter(|c| !c.is_empty()).ok_or("'card' 가 필요함 (이 경로를 그릴 route 카드의 ID)")?;
    let stops = input["stops"].as_array().filter(|a| a.len() >= 2).ok_or("'stops' 에 들르는 곳이 둘 이상 있어야 함")?;
    if stops.len() > 12 {
        return Err("들르는 곳은 12곳까지".into());
    }
    let mut found: Vec<(String, Point, String)> = Vec::new();
    for (n, s) in stops.iter().enumerate() {
        let name = s["name"].as_str().unwrap_or("").trim().to_string();
        if name.is_empty() {
            return Err(format!("{}번째 곳에 name 이 없음", n + 1));
        }
        match (s["lat"].as_f64(), s["lng"].as_f64()) {
            (Some(lat), Some(lng)) => found.push((name, (lat, lng), String::new())),
            _ => {
                let query = s["find"].as_str().map(str::trim).filter(|q| !q.is_empty()).unwrap_or(&name);
                if n > 0 {
                    std::thread::sleep(std::time::Duration::from_millis(1100)); // 좌표 찾기 서비스의 사용 규칙: 1초에 한 번
                }
                let (matched, at) = geocode(query).map_err(|e| format!("{e}. 이 곳은 find 에 동네 이름이나 도로명 주소를 주거나, lat·lng 을 직접 준다"))?;
                found.push((name, at, matched));
            }
        }
    }
    let coords: Vec<String> = found.iter().map(|(_, (lat, lng), _)| format!("{lng:.6},{lat:.6}")).collect();
    let v = get(&format!("https://router.project-osrm.org/route/v1/driving/{}?overview=full&geometries=polyline&steps=false", coords.join(";")))?;
    if v["code"] != "Ok" {
        return Err(format!("길을 찾지 못함: {}", v["message"].as_str().or(v["code"].as_str()).unwrap_or("알 수 없음")));
    }
    let route = &v["routes"][0];
    let mut shape = decode_polyline(route["geometry"].as_str().unwrap_or(""));
    // 그리는 데는 수백 점이면 충분하다.
    if shape.len() > 400 {
        let step = shape.len() as f64 / 400.0;
        let last = *shape.last().unwrap();
        shape = (0..400).map(|i| shape[(i as f64 * step) as usize]).collect();
        shape.push(last);
    }
    let dir = sys::data_dir().join("routes");
    let _ = std::fs::create_dir_all(&dir);
    let saved = json!({ "stops": found.iter().map(|f| json!([f.1.0, f.1.1])).collect::<Vec<_>>(), "path": shape.iter().map(|p| json!([p.0, p.1])).collect::<Vec<_>>() });
    std::fs::write(file(card), saved.to_string()).map_err(|e| e.to_string())?;
    CACHE.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(card.to_string(), Some(Arc::new(shape)));

    let mut legs: Vec<(f64, f64)> = route["legs"].as_array().map(|a| a.iter().map(|l| (l["distance"].as_f64().unwrap_or(0.0), l["duration"].as_f64().unwrap_or(0.0))).collect()).unwrap_or_default();
    let (rough_m, rough_s) = (route["distance"].as_f64().unwrap_or(0.0), route["duration"].as_f64().unwrap_or(0.0));
    // 거리와 시간은 카카오맵에서 확인한다 (지금의 교통 상황이 반영된다). 위에서 찾은 길은 그리는 데만 쓴다.
    // 카카오맵은 도로에서 떨어진 좌표를 받지 않으므로, 도로 위로 옮겨진 좌표를 준다.
    let on_road: Vec<(String, Point)> = found
        .iter()
        .enumerate()
        .map(|(n, f)| {
            let loc = &v["waypoints"][n]["location"];
            (f.0.clone(), match (loc[1].as_f64(), loc[0].as_f64()) { (Some(lat), Some(lng)) => (lat, lng), _ => f.1 })
        })
        .collect();
    let (total, mut out, note) = match ask_kakao(&on_road) {
        Ok((m, secs, toll)) => {
            // 구간별 값은 전체가 카카오맵의 값과 맞도록 같은 비율로 맞춘다.
            let (km, ks) = (if rough_m > 0.0 { m / rough_m } else { 1.0 }, if rough_s > 0.0 { secs / rough_s } else { 1.0 });
            for l in &mut legs {
                *l = (l.0 * km, l.1 * ks);
            }
            let total = format!("{} · {}", km_text(m), time_text(secs));
            let note = format!("카카오맵 기준 (지금 교통 상황){}", toll.map(|t| format!(" · {t}")).unwrap_or_default());
            (total.clone(), format!("길을 찾았다. 거리와 시간은 카카오맵에서 읽은 값이다 (자동차, 지금의 교통 상황 반영).\n전체: {total}\n"), note)
        }
        Err(why) => {
            sys::log("api.log", &format!("카카오맵 확인 실패: {why}"));
            let total = format!("{} · 약 {}", km_text(rough_m), time_text(rough_s));
            (total.clone(), format!("길을 찾았다. 카카오맵에서 확인하지 못해서({why}) 교통 상황이 반영되지 않은 대략값이다.\n전체: {total}\n"), "교통 상황이 반영되지 않은 대략값".to_string())
        }
    };
    let mut lines = format!("card {card} route <주제ID> 6x5 <제목>\nset {card} total {total}\nset {card} text {note}\n");
    for (n, (name, (lat, lng), matched)) in found.iter().enumerate() {
        let leg = legs.get(n).map(|(d, t)| format!("{} · {}", km_text(*d), time_text(*t))).unwrap_or_default();
        out += &format!("{}. {name} ({lat:.5}, {lng:.5}){}{}\n", n + 1, if matched.is_empty() { String::new() } else { format!(" — 찾은 곳: {matched}") }, if leg.is_empty() { String::new() } else { format!(" → 다음 곳까지 {leg}") });
        lines += &format!("add {card} s{} {name} | {lat:.5},{lng:.5}{}\n", n + 1, if leg.is_empty() { String::new() } else { format!(" | {leg}") });
    }
    out += "\n\"찾은 곳\"이 뜻한 곳과 다르면 그 곳의 find 를 바꿔 다시 찾는다. 맞으면 아래 명령으로 화면에 그린다 (주제 ID 와 제목만 정해서, 나머지는 그대로):\n";
    Ok(out + &lines)
}

/// 지도 서비스 화면의 글에서 경로의 (거리 m, 시간 초)를 읽는다. 아직 경로가 안 나왔으면 None.
/// 카카오맵의 길찾기 결과에는 "2시간 27분" 한 줄과 "182.8km" 한 줄이 잇따라 나온다.
pub fn read_summary(text: &str) -> Option<(f64, f64)> {
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    for pair in lines.windows(2) {
        if let (Some(secs), Some(m)) = (read_time(pair[0]), read_distance(pair[1])) {
            return Some((m, secs));
        }
    }
    None
}

/// "2시간 27분", "48분", "1시간" → 초
fn read_time(s: &str) -> Option<f64> {
    let (mut hours, mut mins, mut num, mut seen) = (0.0, 0.0, String::new(), false);
    for ch in s.chars() {
        match ch {
            '0'..='9' => num.push(ch),
            ' ' => {}
            '시' if !num.is_empty() => {
                hours = num.parse().ok()?;
                num.clear();
                seen = true;
            }
            '간' if seen => {}
            '분' if !num.is_empty() => {
                mins = num.parse().ok()?;
                num.clear();
                seen = true;
            }
            _ => return None,
        }
    }
    (seen && num.is_empty()).then_some(hours * 3600.0 + mins * 60.0)
}

/// "182.8km", "850m" → m
fn read_distance(s: &str) -> Option<f64> {
    let s = s.replace(',', "");
    if let Some(n) = s.strip_suffix("km") {
        return n.trim().parse::<f64>().ok().map(|k| k * 1000.0);
    }
    s.strip_suffix('m').and_then(|n| n.trim().parse::<f64>().ok())
}

/// 화면 글에서 "통행료 약 9,100원" 같은 조각을 찾는다.
fn read_toll(text: &str) -> Option<String> {
    let at = text.find("통행료")?;
    let rest = &text[at..];
    let end = rest.find('원')?;
    Some(rest[..end + '원'.len_utf8()].trim().to_string()).filter(|t| t.chars().count() <= 20 && !t.contains("0원") || t.contains(','))
}

/// 카카오맵에서 이 길의 실제 거리와 시간을 읽어 온다 (지금의 교통 상황이 반영된 값). 보이지 않는 창으로 길찾기 화면을 열어 글을 읽는다.
/// `points` 는 (이름, 도로 위로 옮긴 좌표). 돌려주는 값은 (거리 m, 시간 초, 통행료 글).
fn ask_kakao(points: &[(String, Point)]) -> Result<(f64, f64, Option<String>), String> {
    if points.len() > 7 {
        return Err("카카오맵은 경유지를 5곳까지만 받음".into());
    }
    let path: Vec<String> = points.iter().map(|(name, (lat, lng))| format!("{},{lat:.6},{lng:.6}", encode(&name.replace([',', '/'], " ")))).collect();
    let url = format!("https://map.kakao.com/link/by/car/{}", path.join("/"));
    let (tx, rx) = std::sync::mpsc::channel();
    if !crate::jobs::to_ui(crate::llm::Msg::Probe { url, reply: tx }) {
        return Err("화면 쪽에 요청하지 못함".into());
    }
    let text = rx.recv_timeout(std::time::Duration::from_secs(26)).map_err(|_| "카카오맵에서 답이 오지 않음".to_string())?;
    match read_summary(&text) {
        Some((m, secs)) => Ok((m, secs, read_toll(&text))),
        None if text.contains("길이 없어요") => Err("카카오맵이 그 위치 근처에서 길을 찾지 못함".into()),
        None => Err("카카오맵 화면에서 거리와 시간을 읽지 못함".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_the_documented_polyline() {
        // 구글 문서의 예: (38.5, -120.2), (40.7, -120.95), (43.252, -126.453)
        let pts = decode_polyline("_p~iF~ps|U_ulLnnqC_mqNvxq`@");
        assert_eq!(pts.len(), 3);
        assert!((pts[0].0 - 38.5).abs() < 1e-6 && (pts[0].1 + 120.2).abs() < 1e-6);
        assert!((pts[2].0 - 43.252).abs() < 1e-6 && (pts[2].1 + 126.453).abs() < 1e-6);
    }

    #[test]
    fn reads_kakao_map_result() {
        let page = "경로 옵션\n1종, 휘발유 기준\n\n2시간 27분\n182.8km\n나중에 출발\n\n통행료 약 9,100원|택시비 약 315,500원|주유비 약 27,824원\n출발";
        assert_eq!(read_summary(page), Some((182_800.0, 2.0 * 3600.0 + 27.0 * 60.0)));
        assert_eq!(read_toll(page).as_deref(), Some("통행료 약 9,100원"));
        assert_eq!(read_summary("48분\n32.5km"), Some((32_500.0, 48.0 * 60.0)));
        // 아직 결과가 안 나온 화면
        assert_eq!(read_summary("오늘은\n어디로 안내할까요?\n50m\n측정"), None);
    }

    #[test]
    fn writes_distance_and_time_plainly() {
        assert_eq!(km_text(122_635.9), "123km");
        assert_eq!(km_text(4_230.0), "4.2km");
        assert_eq!(time_text(6406.8), "1시간 47분");
        assert_eq!(time_text(2880.0), "48분");
    }
}
