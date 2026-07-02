//! Copy sound feedback — plays a subtle audible confirmation when
//! new clipboard content is detected.
//!
//! Uses rodio for cross-platform WAV playback. A fresh audio stream is
//! created for each playback so the current default output device is
//! always used (e.g. when the user switches between speakers and
//! headphones). A short debounce window prevents overlapping sounds
//! from rapid-fire clipboard changes (e.g. delayed rendering when a
//! clipboard owner exits).
//!
//! Multiple sound variants are embedded at compile time. The active
//! sound is selected via [`play_copy_sound`] by passing the filename
//! stored in settings (`copy_sound_file`).

use std::io::Cursor;
use std::sync::Mutex;
use std::time::Instant;

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

/// Play the copy confirmation sound asynchronously.
///
/// `sound_file` is the filename (e.g. `"copy_kacha.wav"`) from settings.
/// Falls back to [`DEFAULT_SOUND`] if the given file is not found.
///
/// Creates a fresh audio stream for every playback so the current
/// system default output device is always used. A 200 ms debounce
/// prevents overlapping sounds from rapid-fire clipboard changes.
///
/// Spawns a short-lived thread so audio initialisation and playback
/// never block the UI thread. Failures are logged and swallowed —
/// sound is a non-critical feature.
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

    std::thread::spawn(move || {
        // Create a fresh stream each time so the current default
        // audio device is always used (handles device switching).
        let (_stream, handle) = match rodio::OutputStream::try_default() {
            Ok(s) => s,
            Err(e) => {
                log::warn!("copy_sound: failed to open audio device: {e}");
                return;
            }
        };

        let cursor = Cursor::new(data);
        let source = match rodio::Decoder::new(cursor) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("copy_sound: failed to decode sound: {e}");
                return;
            }
        };

        let sink = match rodio::Sink::try_new(&handle) {
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
        let (_stream, handle) = match rodio::OutputStream::try_default() {
            Ok(s) => s,
            Err(e) => {
                log::warn!("copy_sound: failed to open audio device: {e}");
                return;
            }
        };

        let cursor = Cursor::new(data);
        let source = match rodio::Decoder::new(cursor) {
            Ok(s) => s,
            Err(_) => return,
        };

        let sink = match rodio::Sink::try_new(&handle) {
            Ok(s) => s,
            Err(_) => return,
        };

        sink.append(source);
        sink.sleep_until_end();
    });
}
