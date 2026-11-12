//! 소리 내보내기(스피커)와 받기(마이크). OS 의 기본 장치를 쓴다.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// 한 채널짜리 소리를 원하는 샘플 속도로 바꾼다 (직선 보간).
pub fn resample(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || input.is_empty() {
        return input.to_vec();
    }
    let n = (input.len() as u64 * to as u64 / from as u64) as usize;
    let step = from as f64 / to as f64;
    (0..n)
        .map(|i| {
            let pos = i as f64 * step;
            let a = pos.floor() as usize;
            let b = (a + 1).min(input.len() - 1);
            let t = (pos - a as f64) as f32;
            input[a.min(input.len() - 1)] * (1.0 - t) + input[b] * t
        })
        .collect()
}

/// 스피커. 받은 소리를 줄에 세워 두고 장치가 가져가는 대로 내보낸다.
/// 소리가 도착하는 대로 바로 틀 수 있어서, 문장이 다 만들어지기 전에 재생이 시작된다.
pub struct Player {
    queue: Arc<Mutex<VecDeque<f32>>>,
    rate: u32,
    _stream: cpal::Stream,
}

impl Player {
    pub fn new() -> Result<Player, String> {
        let device = cpal::default_host().default_output_device().ok_or("소리를 낼 장치가 없음")?;
        let supported = device.default_output_config().map_err(|e| e.to_string())?;
        let config = supported.config();
        let (rate, channels) = (config.sample_rate, config.channels as usize);
        let queue: Arc<Mutex<VecDeque<f32>>> = Default::default();
        let q = queue.clone();
        let err = |e| crate::sys::log("tts.log", &format!("스피커 오류: {e}"));
        let stream = match supported.sample_format() {
            cpal::SampleFormat::F32 => device.build_output_stream(
                config,
                move |out: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    let mut q = q.lock().unwrap();
                    for frame in out.chunks_mut(channels) {
                        let s = q.pop_front().unwrap_or(0.0);
                        frame.fill(s);
                    }
                },
                err,
                None,
            ),
            cpal::SampleFormat::I16 => device.build_output_stream(
                config,
                move |out: &mut [i16], _: &cpal::OutputCallbackInfo| {
                    let mut q = q.lock().unwrap();
                    for frame in out.chunks_mut(channels) {
                        let s = (q.pop_front().unwrap_or(0.0) * 32767.0) as i16;
                        frame.fill(s);
                    }
                },
                err,
                None,
            ),
            other => return Err(format!("지원하지 않는 스피커 형식: {other:?}")),
        }
        .map_err(|e| e.to_string())?;
        stream.play().map_err(|e| e.to_string())?;
        Ok(Player { queue, rate, _stream: stream })
    }

    /// 16비트 한 채널 소리를 줄 끝에 붙인다.
    pub fn push(&self, pcm: &[i16], rate: u32) {
        // LYBOARD_MUTE: 확인용. 소리는 내지 않되 재생 시간은 그대로 흐르게 한다.
        let gain = if std::env::var("LYBOARD_MUTE").is_ok() { 0.0 } else { 1.0 };
        let mono: Vec<f32> = pcm.iter().map(|s| *s as f32 / 32768.0 * gain).collect();
        let out = resample(&mono, rate, self.rate);
        self.queue.lock().unwrap().extend(out);
    }

    /// 아직 내보내지 않은 소리의 길이(초).
    pub fn pending_secs(&self) -> f32 {
        self.queue.lock().unwrap().len() as f32 / self.rate as f32
    }

    pub fn clear(&self) {
        self.queue.lock().unwrap().clear();
    }
}

/// 마이크. 켜 두는 동안 들어온 소리를 모은다.
pub struct Mic {
    samples: Arc<Mutex<Vec<f32>>>,
    rate: Arc<AtomicU32>,
    level: Arc<AtomicU32>,
    stop: Arc<AtomicBool>,
    failed: Arc<Mutex<Option<String>>>,
}

impl Mic {
    pub fn start() -> Mic {
        let mic = Mic {
            samples: Default::default(),
            rate: Arc::new(AtomicU32::new(16000)),
            level: Default::default(),
            stop: Default::default(),
            failed: Default::default(),
        };
        let (samples, rate, level, stop, failed) = (mic.samples.clone(), mic.rate.clone(), mic.level.clone(), mic.stop.clone(), mic.failed.clone());
        // 입력 장치는 만든 스레드에서만 다룰 수 있어서 전용 스레드에 둔다.
        std::thread::spawn(move || {
            let opened: Result<cpal::Stream, String> = (|| {
                let device = cpal::default_host().default_input_device().ok_or("마이크가 없음")?;
                let supported = device.default_input_config().map_err(|e| e.to_string())?;
                let config = supported.config();
                let channels = config.channels as usize;
                rate.store(config.sample_rate, Ordering::SeqCst);
                // 여러 채널이면 평균을 내어 한 채널로 만들고, 방금 들어온 소리의 크기도 같이 잰다.
                let take = move |mono: Vec<f32>| {
                    if !mono.is_empty() {
                        let rms = (mono.iter().map(|s| s * s).sum::<f32>() / mono.len() as f32).sqrt();
                        level.store(rms.to_bits(), Ordering::SeqCst);
                        samples.lock().unwrap().extend(mono);
                    }
                };
                let err = |e| crate::sys::log("stt.log", &format!("마이크 오류: {e}"));
                let stream = match supported.sample_format() {
                    cpal::SampleFormat::F32 => device.build_input_stream(
                        config,
                        move |data: &[f32], _: &cpal::InputCallbackInfo| take(data.chunks(channels).map(|f| f.iter().sum::<f32>() / channels as f32).collect()),
                        err,
                        None,
                    ),
                    cpal::SampleFormat::I16 => device.build_input_stream(
                        config,
                        move |data: &[i16], _: &cpal::InputCallbackInfo| {
                            take(data.chunks(channels).map(|f| f.iter().map(|s| *s as f32 / 32768.0).sum::<f32>() / channels as f32).collect())
                        },
                        err,
                        None,
                    ),
                    other => return Err(format!("지원하지 않는 마이크 형식: {other:?}")),
                }
                .map_err(|e| e.to_string())?;
                stream.play().map_err(|e| e.to_string())?;
                Ok(stream)
            })();
            match opened {
                Ok(_stream) => {
                    while !stop.load(Ordering::SeqCst) {
                        std::thread::sleep(std::time::Duration::from_millis(20));
                    }
                }
                Err(e) => {
                    crate::sys::log("stt.log", &format!("마이크를 열지 못함: {e}"));
                    *failed.lock().unwrap() = Some(e);
                }
            }
        });
        mic
    }

    /// 방금 들어온 소리의 크기 (0 = 조용함).
    pub fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::SeqCst))
    }

    pub fn error(&self) -> Option<String> {
        self.failed.lock().unwrap().clone()
    }

    /// 마이크를 끄고, 모은 소리를 음성 인식이 받는 형식(16kHz, 16비트, 한 채널)으로 돌려준다.
    pub fn finish(self) -> Vec<i16> {
        self.stop.store(true, Ordering::SeqCst);
        let samples = std::mem::take(&mut *self.samples.lock().unwrap());
        resample(&samples, self.rate.load(Ordering::SeqCst), 16000).iter().map(|s| (s.clamp(-1.0, 1.0) * 32767.0) as i16).collect()
    }
}

impl Drop for Mic {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn resample_keeps_duration() {
        let one_sec: Vec<f32> = (0..24000).map(|i| (i as f32 * 0.01).sin()).collect();
        assert_eq!(super::resample(&one_sec, 24000, 48000).len(), 48000);
        assert_eq!(super::resample(&one_sec, 24000, 16000).len(), 16000);
        assert_eq!(super::resample(&one_sec, 24000, 24000).len(), 24000);
    }
}
