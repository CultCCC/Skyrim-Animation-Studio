//! Sound output: XAudio2 2.9 (part of Windows 10/11) on its own thread.
//! It plays what Skyrim ships — PCM and ADPCM `.wav`, xWMA `.xwm`, and the
//! xWMA inside a voice `.fuz` — without any decoder of ours.

use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::Duration;

/// A sound file ready for a source voice.
pub struct Clip {
    /// WAVEFORMATEX bytes (at least 18).
    fmt: Vec<u8>,
    data: Vec<u8>,
    /// xWMA decoded-packet cumulative bytes (`dpds`); empty for PCM/ADPCM.
    dpds: Vec<u32>,
    pub seconds: f32,
    pub format: &'static str,
}

/// RIFF WAVE / XWMA, or a FUZE wrapper around one.
pub fn parse(bytes: &[u8]) -> Result<Clip, String> {
    let mut b = bytes;
    if b.len() >= 12 && &b[..4] == b"FUZE" {
        let lip = u32::from_le_bytes(b[8..12].try_into().unwrap()) as usize;
        b = b.get(12 + lip..).ok_or(sds_i18n::t!("FUZE короче заявленного", "FUZE shorter than declared"))?;
    }
    if b.len() < 12 || &b[..4] != b"RIFF" {
        return Err(sds_i18n::t!("не RIFF", "not RIFF").into());
    }
    let kind = &b[8..12];
    let (mut fmt, mut data, mut dpds) = (None, None, Vec::new());
    let mut o = 12;
    while o + 8 <= b.len() {
        let id = &b[o..o + 4];
        let len = u32::from_le_bytes(b[o + 4..o + 8].try_into().unwrap()) as usize;
        let body = &b[o + 8..(o + 8 + len).min(b.len())];
        match id {
            b"fmt " => fmt = Some(body.to_vec()),
            b"data" => data = Some(body.to_vec()),
            b"dpds" => dpds = body.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect(),
            _ => {}
        }
        o += 8 + len + (len & 1);
    }
    let mut fmt = fmt.ok_or(sds_i18n::t!("нет fmt", "no fmt"))?;
    let data = data.ok_or(sds_i18n::t!("нет data", "no data"))?;
    if fmt.len() < 16 {
        return Err(sds_i18n::t!("короткий fmt", "short fmt").into());
    }
    if fmt.len() < 18 {
        fmt.resize(18, 0);
    }
    let tag = u16::from_le_bytes([fmt[0], fmt[1]]);
    let rate = u32::from_le_bytes(fmt[4..8].try_into().unwrap()).max(1);
    let avg = u32::from_le_bytes(fmt[8..12].try_into().unwrap()).max(1);
    let format = match (kind, tag) {
        (b"XWMA", _) | (_, 0x161) | (_, 0x162) => "xWMA",
        (_, 1) => "PCM",
        (_, 2) => "MS ADPCM",
        (_, 0xFFFE) => "PCM (ext)",
        _ => "?",
    };
    let seconds = if format == "xWMA" {
        // The last cumulative count is the decoded PCM size.
        let ch = u16::from_le_bytes([fmt[2], fmt[3]]).max(1) as f32;
        let bits = u16::from_le_bytes([fmt[14], fmt[15]]).max(16) as f32;
        dpds.last().map(|&n| n as f32 / (rate as f32 * ch * bits / 8.0)).unwrap_or(data.len() as f32 / avg as f32)
    } else {
        data.len() as f32 / avg as f32
    };
    Ok(Clip { fmt, data, dpds, seconds, format })
}

enum Cmd {
    Play { clip: Arc<Clip>, gain: f32, ratio: f32, looping: bool },
    StopAll,
    StopLoops,
    Master(f32),
    Speed(f32),
    /// Can XAudio2 take this clip? (voice + buffer, then dropped).
    Probe(Arc<Clip>, Sender<Result<(), String>>),
}

pub struct Audio {
    tx: Sender<Cmd>,
}

impl Audio {
    /// Starts the audio thread; `Err` when there is no XAudio2 or no device.
    pub fn start(volume: f32) -> Result<Audio, String> {
        let (tx, rx) = channel();
        let (ready_tx, ready_rx) = channel();
        std::thread::Builder::new()
            .name("sds-audio".into())
            .spawn(move || run(rx, ready_tx, volume))
            .map_err(|e| e.to_string())?;
        ready_rx.recv().map_err(|e| e.to_string())??;
        Ok(Audio { tx })
    }

    /// `gain` linear; `ratio` playback rate (1 = as recorded).
    pub fn play(&self, clip: Arc<Clip>, gain: f32, ratio: f32, looping: bool) {
        let _ = self.tx.send(Cmd::Play { clip, gain, ratio, looping });
    }
    pub fn stop_all(&self) {
        let _ = self.tx.send(Cmd::StopAll);
    }
    pub fn stop_loops(&self) {
        let _ = self.tx.send(Cmd::StopLoops);
    }
    pub fn set_volume(&self, v: f32) {
        let _ = self.tx.send(Cmd::Master(v));
    }
    /// Whether the clip's format is accepted (the `--sound-test` check).
    pub fn probe(&self, clip: Arc<Clip>) -> Result<(), String> {
        let (tx, rx) = channel();
        let _ = self.tx.send(Cmd::Probe(clip, tx));
        rx.recv().map_err(|e| e.to_string())?
    }

    /// Playback speed of the timeline: every voice follows it.
    pub fn set_speed(&self, s: f32) {
        let _ = self.tx.send(Cmd::Speed(s));
    }
}

#[cfg(windows)]
fn run(rx: Receiver<Cmd>, ready: Sender<Result<(), String>>, volume: f32) {
    use windows::core::{s, w, Interface};
    use windows::Win32::Media::Audio::XAudio2::*;
    use windows::Win32::Media::Audio::{AudioCategory_GameEffects, WAVEFORMATEX};
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

    const MAX_RATIO: f32 = 4.0;
    const MAX_VOICES: usize = 48;

    struct Voice {
        voice: IXAudio2SourceVoice,
        _clip: Arc<Clip>,
        ratio: f32,
        looping: bool,
    }

    let engine = unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        // 2.9 by name: the 2.8 the windows crate links has no xWMA.
        let lib = match LoadLibraryW(w!("xaudio2_9.dll")) {
            Ok(h) => h,
            Err(e) => {
                let _ = ready.send(Err(format!("xaudio2_9.dll: {e}")));
                return;
            }
        };
        type Create = unsafe extern "system" fn(*mut *mut core::ffi::c_void, u32, u32) -> windows::core::HRESULT;
        let Some(f) = GetProcAddress(lib, s!("XAudio2Create")) else {
            let _ = ready.send(Err(sds_i18n::t!("в xaudio2_9.dll нет XAudio2Create", "xaudio2_9.dll has no XAudio2Create").into()));
            return;
        };
        let create: Create = std::mem::transmute(f);
        let mut raw = std::ptr::null_mut();
        if let Err(e) = create(&mut raw, 0, XAUDIO2_DEFAULT_PROCESSOR).ok() {
            let _ = ready.send(Err(format!("XAudio2Create: {e}")));
            return;
        }
        IXAudio2::from_raw(raw)
    };
    let mut master: Option<IXAudio2MasteringVoice> = None;
    if let Err(e) = unsafe { engine.CreateMasteringVoice(&mut master, 0, 0, 0, windows::core::PCWSTR::null(), None, AudioCategory_GameEffects) } {
        let _ = ready.send(Err(sds_i18n::tf!("нет устройства вывода: {e}", "no output device: {e}")));
        return;
    }
    let Some(master) = master else {
        let _ = ready.send(Err(sds_i18n::t!("нет устройства вывода", "no output device").into()));
        return;
    };
    unsafe {
        let _ = master.SetVolume(volume, 0);
    }
    let _ = ready.send(Ok(()));

    let mut voices: Vec<Voice> = Vec::new();
    let mut speed = 1.0f32;
    let destroy = |v: &Voice| unsafe {
        let _ = v.voice.Stop(0, 0);
        v.voice.DestroyVoice();
    };
    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Cmd::Play { clip, gain, ratio, looping }) => unsafe {
                if voices.len() >= MAX_VOICES {
                    destroy(&voices.remove(0));
                }
                let mut sv: Option<IXAudio2SourceVoice> = None;
                let fmt = clip.fmt.as_ptr() as *const WAVEFORMATEX;
                if engine.CreateSourceVoice(&mut sv, fmt, 0, MAX_RATIO, None::<&IXAudio2VoiceCallback>, None, None).is_err() {
                    continue;
                }
                let Some(voice) = sv else { continue };
                let buf = XAUDIO2_BUFFER {
                    Flags: XAUDIO2_END_OF_STREAM,
                    AudioBytes: clip.data.len() as u32,
                    pAudioData: clip.data.as_ptr(),
                    LoopCount: if looping { XAUDIO2_LOOP_INFINITE } else { 0 },
                    ..Default::default()
                };
                let wma = XAUDIO2_BUFFER_WMA { pDecodedPacketCumulativeBytes: clip.dpds.as_ptr(), PacketCount: clip.dpds.len() as u32 };
                let submitted = if clip.dpds.is_empty() { voice.SubmitSourceBuffer(&buf, None) } else { voice.SubmitSourceBuffer(&buf, Some(&wma)) };
                if submitted.is_err() {
                    voice.DestroyVoice();
                    continue;
                }
                let _ = voice.SetVolume(gain, 0);
                let _ = voice.SetFrequencyRatio((ratio * speed).clamp(0.01, MAX_RATIO), 0);
                let _ = voice.Start(0, 0);
                voices.push(Voice { voice, _clip: clip, ratio, looping });
            },
            Ok(Cmd::Probe(clip, reply)) => unsafe {
                let mut sv: Option<IXAudio2SourceVoice> = None;
                let r = engine
                    .CreateSourceVoice(&mut sv, clip.fmt.as_ptr() as *const WAVEFORMATEX, 0, MAX_RATIO, None::<&IXAudio2VoiceCallback>, None, None)
                    .map_err(|e| format!("CreateSourceVoice: {e}"))
                    .and_then(|_| {
                        let voice = sv.as_ref().ok_or(sds_i18n::t!("нет голоса", "no voice"))?;
                        let buf = XAUDIO2_BUFFER { Flags: XAUDIO2_END_OF_STREAM, AudioBytes: clip.data.len() as u32, pAudioData: clip.data.as_ptr(), ..Default::default() };
                        let wma = XAUDIO2_BUFFER_WMA { pDecodedPacketCumulativeBytes: clip.dpds.as_ptr(), PacketCount: clip.dpds.len() as u32 };
                        let r = if clip.dpds.is_empty() { voice.SubmitSourceBuffer(&buf, None) } else { voice.SubmitSourceBuffer(&buf, Some(&wma)) };
                        r.map_err(|e| format!("SubmitSourceBuffer: {e}"))
                    });
                if let Some(v) = sv {
                    v.DestroyVoice();
                }
                let _ = reply.send(r);
            },
            Ok(Cmd::StopAll) => {
                for v in voices.drain(..) {
                    destroy(&v);
                }
            }
            Ok(Cmd::StopLoops) => voices.retain(|v| {
                if v.looping {
                    destroy(v);
                }
                !v.looping
            }),
            Ok(Cmd::Master(v)) => unsafe {
                let _ = master.SetVolume(v, 0);
            },
            Ok(Cmd::Speed(s)) => {
                speed = s;
                for v in &voices {
                    unsafe {
                        let _ = v.voice.SetFrequencyRatio((v.ratio * speed).clamp(0.01, MAX_RATIO), 0);
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        // Finished voices give their buffers back.
        voices.retain(|v| {
            let mut st = XAUDIO2_VOICE_STATE::default();
            unsafe { v.voice.GetState(&mut st, XAUDIO2_VOICE_NOSAMPLESPLAYED) };
            let done = st.BuffersQueued == 0;
            if done {
                destroy(v);
            }
            !done
        });
    }
    for v in voices.drain(..) {
        destroy(&v);
    }
    unsafe { master.DestroyVoice() };
}

#[cfg(not(windows))]
fn run(_rx: Receiver<Cmd>, ready: Sender<Result<(), String>>, _volume: f32) {
    let _ = ready.send(Err(sds_i18n::t!("звук есть только в Windows (XAudio2)", "sound is Windows-only (XAudio2)").into()));
}
