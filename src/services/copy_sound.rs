//! Copy sound feedback — plays a subtle audible confirmation when
//! new clipboard content is detected.
//!
//! Uses rodio for cross-platform WAV playback. The current default output
//! device is checked before every playback. Its stream is reused briefly for
//! bursts, then released after an idle timeout; changing the default device
//! rebuilds the stream immediately. A short debounce window prevents
//! overlapping sounds from rapid-fire clipboard changes (e.g. delayed
//! rendering when a clipboard owner exits).
//!
//! Multiple sound variants are embedded at compile time. The active
//! sound is selected via [`play_copy_sound`] by passing the filename
//! stored in settings (`copy_sound_file`).

use std::io::Cursor;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use rodio::cpal::traits::{DeviceTrait, HostTrait};

/// Embedded WAV sound effects (short, subtle click/pop sounds).
const SOUND_PENCLICK: &[u8] = include_bytes!("../../assets/copy_penclick.wav");
const SOUND_KACHA: &[u8] = include_bytes!("../../assets/copy_kacha.wav");
const SOUND_BLIP: &[u8] = include_bytes!("../../assets/copy_blip.wav");
const SOUND_BUBBLE: &[u8] = include_bytes!("../../assets/copy_bubble.wav");
const SOUND_CLACK: &[u8] = include_bytes!("../../assets/copy_clack.wav");
const SOUND_MECHKB: &[u8] = include_bytes!("../../assets/copy_mechkb.wav");

/// Ordered list of available sound files (filename, display key).
pub const SOUND_LIST: &[(&str, &str)] = &[
    ("copy_penclick.wav", "按动笔"),
    ("copy_kacha.wav", "咔嚓"),
    ("copy_clack.wav", "清脆键盘"),
    ("copy_mechkb.wav", "机械键盘"),
    ("copy_blip.wav", "小啾啾"),
    ("copy_bubble.wav", "泡泡"),
];

/// Default sound file.
pub const DEFAULT_SOUND: &str = "copy_penclick.wav";

/// Look up embedded sound data by filename.
fn get_sound_data(filename: &str) -> Option<&'static [u8]> {
    match filename {
        "copy_penclick.wav" => Some(SOUND_PENCLICK),
        "copy_kacha.wav" => Some(SOUND_KACHA),
        "copy_blip.wav" => Some(SOUND_BLIP),
        "copy_bubble.wav" => Some(SOUND_BUBBLE),
        "copy_clack.wav" => Some(SOUND_CLACK),
        "copy_mechkb.wav" => Some(SOUND_MECHKB),
        _ => None,
    }
}

/// Timestamp of the last sound playback. Used for debouncing —
/// rapid-fire clipboard changes within the debounce window
/// (e.g. delayed rendering when Explorer exits) are suppressed.
static LAST_PLAY: Mutex<Option<Instant>> = Mutex::new(None);

/// Minimum interval between consecutive sound plays, in milliseconds.
const PLAY_DEBOUNCE_MS: u64 = 200;

/// Keep the current output stream alive briefly so bursts of copy actions do
/// not repeatedly initialise the same audio device. The device is still
/// checked before every playback and a changed default device is reopened
/// immediately.
const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(3);

/// A single playback worker owns the non-Send audio stream. Requests queued
/// while a short sound is playing are coalesced so stale feedback cannot build
/// up into audible latency.
static PLAYBACK_TX: OnceLock<Sender<&'static [u8]>> = OnceLock::new();

struct CachedAudio {
    device_id: String,
    _stream: rodio::OutputStream,
    handle: rodio::OutputStreamHandle,
    last_used: Instant,
}

fn playback_sender() -> &'static Sender<&'static [u8]> {
    PLAYBACK_TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("clippi-audio".to_string())
            .spawn(move || playback_worker(rx))
            .expect("failed to spawn copy sound worker");
        tx
    })
}

fn queue_sound(data: &'static [u8]) {
    if playback_sender().send(data).is_err() {
        log::warn!("copy_sound: playback worker stopped unexpectedly");
    }
}

fn playback_worker(rx: Receiver<&'static [u8]>) {
    let mut cached: Option<CachedAudio> = None;

    loop {
        let received = if let Some(audio) = cached.as_ref() {
            let remaining = STREAM_IDLE_TIMEOUT.saturating_sub(audio.last_used.elapsed());
            rx.recv_timeout(remaining)
        } else {
            match rx.recv() {
                Ok(data) => Ok(data),
                Err(_) => break,
            }
        };

        let mut data = match received {
            Ok(data) => data,
            Err(RecvTimeoutError::Timeout) => {
                cached = None;
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => break,
        };

        // Only the newest pending feedback is useful. Playing every stale
        // request would make the audible response lag behind rapid copies.
        for newer in rx.try_iter() {
            data = newer;
        }

        let Some((device, device_id)) = current_output_device() else {
            continue;
        };

        let device_changed = cached
            .as_ref()
            .is_none_or(|audio| audio.device_id != device_id);
        if device_changed {
            cached = match rodio::OutputStream::try_from_device(&device) {
                Ok((stream, handle)) => Some(CachedAudio {
                    device_id,
                    _stream: stream,
                    handle,
                    last_used: Instant::now(),
                }),
                Err(e) => {
                    log::warn!("copy_sound: failed to open audio device: {e}");
                    None
                }
            };
        }

        let Some(audio) = cached.as_mut() else {
            continue;
        };
        let source = match rodio::Decoder::new(Cursor::new(data)) {
            Ok(source) => source,
            Err(e) => {
                log::warn!("copy_sound: failed to decode sound: {e}");
                continue;
            }
        };
        let sink = match rodio::Sink::try_new(&audio.handle) {
            Ok(sink) => sink,
            Err(e) => {
                log::warn!("copy_sound: failed to create sink: {e}");
                cached = None;
                continue;
            }
        };

        sink.append(source);
        sink.sleep_until_end();
        audio.last_used = Instant::now();
    }
}

fn current_output_device() -> Option<(rodio::Device, String)> {
    let device = rodio::cpal::default_host().default_output_device()?;
    let name = device.name().unwrap_or_else(|_| "<unknown>".to_string());
    let config = device.default_output_config().ok();
    let device_id = match config {
        Some(config) => format!(
            "{name}|{}|{}|{:?}",
            config.channels(),
            config.sample_rate().0,
            config.sample_format()
        ),
        None => name,
    };
    Some((device, device_id))
}

/// Play the copy confirmation sound asynchronously.
///
/// `sound_file` is the filename (e.g. `"copy_kacha.wav"`) from settings.
/// Falls back to [`DEFAULT_SOUND`] if the given file is not found.
///
/// Checks the current default output device before playback and reuses its
/// stream only for a short idle window. A 200 ms debounce prevents overlapping
/// sounds from rapid-fire clipboard changes.
///
/// A dedicated worker keeps audio initialisation and playback off the UI
/// thread. Failures are logged and swallowed because sound is non-critical.
pub fn play_copy_sound(sound_file: &str) {
    // Debounce: suppress calls within the window to avoid overlapping
    // sounds (e.g. delayed rendering when a clipboard owner exits).
    {
        let mut last = LAST_PLAY.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(t) = *last {
            let elapsed = t.elapsed().as_millis();
            if elapsed < PLAY_DEBOUNCE_MS as u128 {
                return;
            }
        }
        *last = Some(Instant::now());
    }

    let data = get_sound_data(sound_file)
        .unwrap_or_else(|| get_sound_data(DEFAULT_SOUND).expect("default sound must exist"));

    queue_sound(data);
}

/// Preview a specific sound file (for the settings UI).
/// Same as [`play_copy_sound`] but always plays the exact file given,
/// without fallback.
pub fn preview_sound(sound_file: &str) {
    let Some(data) = get_sound_data(sound_file) else {
        log::warn!("copy_sound: unknown sound file for preview: {sound_file}");
        return;
    };

    queue_sound(data);
}
