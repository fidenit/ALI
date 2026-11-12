//! 사진. 사용자가 붙여 넣거나 끌어다 놓은 그림, 그리고 AI 가 파일에서 직접 열어 보는 그림을
//! 모델에게 보낼 수 있는 크기와 형식으로 맞춘다.

use std::io::Cursor;

use image::{DynamicImage, ImageFormat, RgbaImage};

/// 긴 쪽이 이보다 크면 줄인다 (이보다 커도 모델이 더 잘 보지 않고, 보내는 양만 늘어난다).
const MAX_SIDE: u32 = 1568;
/// PNG 로 이 크기를 넘으면 JPEG 으로 바꾼다 (바이트)
const MAX_PNG: usize = 3_000_000;

pub struct Pic {
    /// "image/png" 또는 "image/jpeg"
    pub media: &'static str,
    /// 보낼 파일의 내용
    pub data: Vec<u8>,
    pub w: u32,
    pub h: u32,
    /// 화면에 작게 보여 줄 그림 (가로, 세로, RGBA)
    pub thumb: (usize, usize, Vec<u8>),
}

impl Pic {
    pub fn b64(&self) -> String {
        crate::tts::base64(&self.data)
    }
    pub fn ext(&self) -> &'static str {
        if self.media == "image/png" { "png" } else { "jpg" }
    }
}

fn prepare(img: DynamicImage) -> Result<Pic, String> {
    let img = if img.width().max(img.height()) > MAX_SIDE { img.resize(MAX_SIDE, MAX_SIDE, image::imageops::FilterType::Triangle) } else { img };
    let mut data = Vec::new();
    img.write_to(&mut Cursor::new(&mut data), ImageFormat::Png).map_err(|e| format!("그림을 저장 형식으로 바꾸지 못함: {e}"))?;
    let mut media = "image/png";
    if data.len() > MAX_PNG {
        data.clear();
        DynamicImage::ImageRgb8(img.to_rgb8()).write_to(&mut Cursor::new(&mut data), ImageFormat::Jpeg).map_err(|e| format!("그림을 저장 형식으로 바꾸지 못함: {e}"))?;
        media = "image/jpeg";
    }
    let small = img.thumbnail(160, 160).to_rgba8();
    Ok(Pic { media, data, w: img.width(), h: img.height(), thumb: (small.width() as usize, small.height() as usize, small.into_raw()) })
}

/// 클립보드에서 온 그림 (RGBA).
pub fn from_rgba(w: usize, h: usize, rgba: Vec<u8>) -> Result<Pic, String> {
    let img = RgbaImage::from_raw(w as u32, h as u32, rgba).ok_or("그림의 크기가 맞지 않음")?;
    prepare(DynamicImage::ImageRgba8(img))
}

/// 그림 파일 (png, jpg, webp, gif, bmp).
pub fn from_file(path: &str) -> Result<Pic, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let img = image::load_from_memory(&bytes).map_err(|e| format!("{path}: 그림으로 읽을 수 없음 ({e})"))?;
    prepare(img)
}

// ---------------------------------------------------------------------------
// 보드의 그림 카드에 보여 줄 그림. 읽고 푸는 일은 뒤에서 하고, 다 되면 화면에 올린다.

enum Slot {
    Loading,
    Decoded(usize, usize, Vec<u8>),
    Ready(eframe::egui::TextureHandle, [f32; 2]),
    Failed(String),
}

/// 그림 카드가 지금 보여 줄 수 있는 것
pub enum Shown {
    Ready(eframe::egui::TextureId, [f32; 2]),
    Loading,
    Failed(String),
}

static SHOWN: std::sync::Mutex<Option<std::collections::HashMap<String, Slot>>> = std::sync::Mutex::new(None);

/// 불러 둔 그림을 잊는다 (초기화했을 때).
pub fn forget() {
    *SHOWN.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

fn fetch(src: &str) -> Result<(usize, usize, Vec<u8>), String> {
    let lower = src.to_lowercase();
    let bytes = if lower.starts_with("http://") || lower.starts_with("https://") {
        let mut resp = crate::agent::http().get(src).header("user-agent", "ALI-desktop/0.1").call().map_err(|e| format!("그림을 받지 못함: {e}"))?;
        if resp.status().as_u16() != 200 {
            return Err(format!("그림을 받지 못함 (HTTP {})", resp.status().as_u16()));
        }
        resp.body_mut().with_config().limit(20_000_000).read_to_vec().map_err(|e| format!("그림을 받지 못함: {e}"))?
    } else {
        let path = src.trim_start_matches("file:///").trim_start_matches("file://");
        std::fs::read(path).map_err(|e| format!("그림 파일을 열지 못함: {e}"))?
    };
    let img = image::load_from_memory(&bytes).map_err(|_| "그림으로 읽을 수 없는 파일".to_string())?;
    let img = if img.width().max(img.height()) > 1400 { img.resize(1400, 1400, image::imageops::FilterType::Triangle) } else { img };
    let rgba = img.to_rgba8();
    Ok((rgba.width() as usize, rgba.height() as usize, rgba.into_raw()))
}

/// 이 그림을 화면에 그릴 수 있게 준비한다. 처음 부르면 뒤에서 불러오기 시작하고, 준비되면 그 뒤로는 바로 돌려준다.
pub fn show(ctx: &eframe::egui::Context, src: &str) -> Shown {
    let mut guard = SHOWN.lock().unwrap_or_else(|e| e.into_inner());
    let map = guard.get_or_insert_with(Default::default);
    if let Some(Slot::Decoded(w, h, rgba)) = map.get(src) {
        let image = eframe::egui::ColorImage::from_rgba_unmultiplied([*w, *h], rgba);
        let size = [*w as f32, *h as f32];
        let tex = ctx.load_texture(format!("img:{src}"), image, Default::default());
        map.insert(src.to_string(), Slot::Ready(tex, size));
    }
    match map.get(src) {
        Some(Slot::Ready(tex, size)) => Shown::Ready(tex.id(), *size),
        Some(Slot::Failed(why)) => Shown::Failed(why.clone()),
        Some(_) => Shown::Loading,
        None => {
            map.insert(src.to_string(), Slot::Loading);
            let (src, ctx) = (src.to_string(), ctx.clone());
            std::thread::spawn(move || {
                let slot = match fetch(&src) {
                    Ok((w, h, rgba)) => Slot::Decoded(w, h, rgba),
                    Err(why) => Slot::Failed(why),
                };
                SHOWN.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Default::default).insert(src, slot);
                ctx.request_repaint();
            });
            Shown::Loading
        }
    }
}
