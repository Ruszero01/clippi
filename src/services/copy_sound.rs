//! Copy sound feedback — plays a subtle audible confirmation when
//! new clipboard content is detected.
//!
//! Uses rodio for cross-platform WAV playback. The audio device is
//! opened lazily on first use and does not block the UI thread.
//!
//! Multiple sound variants are embedded at compile time. The active
//! sound is selected via [`play_copy_sound`] by passing the filename
//! stored in settings (`copy_sound_file`).

use std::io::Cursor;
use std::sync::OnceLock;

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

/// Lazily-initialized audio output handle.
/// On first call, rodio opens the platform audio device. This is a non-trivial
/// operation (~50-200ms on some platforms), so we defer it until actually needed.
static AUDIO_HANDLE: OnceLock<Option<rodio::OutputStreamHandle>> = OnceLock::new();

/// Initialize the audio output stream and cache the handle.
/// Returns None if the platform audio device could not be opened
/// (e.g., no speakers, driver issues).
fn ensure_audio() -> Option<&'static rodio::OutputStreamHandle> {
    AUDIO_HANDLE
        .get_or_init(|| match rodio::OutputStream::try_default() {
            Ok((stream, handle)) => {
                // The stream must live as long as we want to play sounds.
                // We leak it into a static to keep it alive for the
                // process lifetime.
                std::mem::forget(stream);
                Some(handle)
            }
            Err(e) => {
                log::warn!("copy_sound: failed to open audio device: {e}");
                None
            }
        })
        .as_ref()
}

/// Play the copy confirmation sound asynchronously.
///
/// `sound_file` is the filename (e.g. `"copy_kacha_low.wav"`) from settings.
/// Falls back to [`DEFAULT_SOUND`] if the given file is not found.
///
/// Spawns a short-lived thread so audio initialization and playback
/// never block the UI thread. Failures are logged and swallowed --
/// sound is a non-critical feature.
pub fn play_copy_sound(sound_file: &str) {
    let data = get_sound_data(sound_file)
        .unwrap_or_else(|| get_sound_data(DEFAULT_SOUND).expect("default sound must exist"));

    std::thread::spawn(move || {
        let handle = match ensure_audio() {
            Some(h) => h,
            None => return,
        };

        let cursor = Cursor::new(data);
        let source = match rodio::Decoder::new(cursor) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("copy_sound: failed to decode sound: {e}");
                return;
            }
        };

        let sink = match rodio::Sink::try_new(handle) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("copy_sound: failed to create sink: {e}");
                return;
            }
        };

        sink.append(source);
        sink.sleep_until_end();
    });
}

/// Preview a specific sound file (for the settings UI).
/// Same as [`play_copy_sound`] but always plays the exact file given,
/// without fallback.
pub fn preview_sound(sound_file: &str) {
    let Some(data) = get_sound_data(sound_file) else {
        log::warn!("copy_sound: unknown sound file for preview: {sound_file}");
        return;
    };

    std::thread::spawn(move || {
        let handle = match ensure_audio() {
            Some(h) => h,
            None => return,
        };

        let cursor = Cursor::new(data);
        let source = match rodio::Decoder::new(cursor) {
            Ok(s) => s,
            Err(_) => return,
        };

        let sink = match rodio::Sink::try_new(handle) {
            Ok(s) => s,
            Err(_) => return,
        };

        sink.append(source);
        sink.sleep_until_end();
    });
}
