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

/// Stable identity for the current output device.
///
/// On macOS this is the CoreAudio Device UID, which uniquely identifies a
/// physical or aggregate device and stays stable across renames. The
/// name/format-based fallback (used on other platforms) can collide when two
/// devices share the same name and format — e.g. two identical USB audio
/// devices — which would keep a stale output stream open when the system
/// default switches between them.
fn output_device_id(device: &rodio::Device) -> String {
    #[cfg(target_os = "macos")]
    {
        if let Some(uid) = coreaudio_uid::default_output_device_uid() {
            return uid;
        }
        log::warn!(
            "copy_sound: failed to resolve CoreAudio device UID; falling back to name-based identity"
        );
    }
    let name = device.name().unwrap_or_else(|_| "<unknown>".to_string());
    match device.default_output_config() {
        Ok(config) => format!(
            "{name}|{}|{}|{:?}",
            config.channels(),
            config.sample_rate().0,
            config.sample_format()
        ),
        Err(_) => name,
    }
}

fn current_output_device() -> Option<(rodio::Device, String)> {
    let device = rodio::cpal::default_host().default_output_device()?;
    let device_id = output_device_id(&device);
    Some((device, device_id))
}

/// CoreAudio lookup for the default output device UID.
///
/// Only two read-only properties are queried (`kAudioHardwarePropertyDefaultOutputDevice`
/// on the system object, then `kAudioDevicePropertyDeviceUID` on the device),
/// so no audio stream or device handle is kept alive here.
#[cfg(target_os = "macos")]
mod coreaudio_uid {
    use core_foundation::base::{CFTypeRef, TCFType};
    use core_foundation::string::{CFString, CFStringRef};
    use coreaudio::sys::{
        kAudioDevicePropertyDeviceUID, kAudioHardwarePropertyDefaultOutputDevice,
        kAudioObjectPropertyElementMaster, kAudioObjectPropertyScopeGlobal,
        kAudioObjectSystemObject, AudioObjectGetPropertyData, AudioObjectGetPropertyDataSize,
        AudioObjectID, AudioObjectPropertyAddress,
    };
    use std::ffi::c_void;

    /// UID of the current default output device, or `None` when CoreAudio is
    /// unavailable or the property cannot be read.
    pub(super) fn default_output_device_uid() -> Option<String> {
        let mut device_id: AudioObjectID = 0;
        let address = AudioObjectPropertyAddress {
            mSelector: kAudioHardwarePropertyDefaultOutputDevice,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMaster,
        };
        let mut size = std::mem::size_of::<AudioObjectID>() as u32;
        // Safety: `out_data` points to a writable `AudioObjectID` sized buffer.
        let status = unsafe {
            AudioObjectGetPropertyData(
                kAudioObjectSystemObject,
                &address,
                0,
                std::ptr::null(),
                &mut size,
                &mut device_id as *mut AudioObjectID as *mut c_void,
            )
        };
        if status != 0 || device_id == 0 {
            return None;
        }

        // The UID property holds a CFStringRef; query its size first (the
        // standard two-call CoreAudio pattern), then read the value.
        let uid_address = AudioObjectPropertyAddress {
            mSelector: kAudioDevicePropertyDeviceUID,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMaster,
        };
        let mut uid_size = 0u32;
        // Safety: `uid_size` points to writable storage for the property size.
        let status = unsafe {
            AudioObjectGetPropertyDataSize(
                device_id,
                &uid_address,
                0,
                std::ptr::null(),
                &mut uid_size,
            )
        };
        if status != 0 || uid_size as usize != std::mem::size_of::<CFTypeRef>() {
            return None;
        }

        let mut string_ref: CFTypeRef = std::ptr::null();
        let mut size = std::mem::size_of::<CFTypeRef>() as u32;
        // Safety: `out_data` points to a writable `CFTypeRef` sized buffer.
        let status = unsafe {
            AudioObjectGetPropertyData(
                device_id,
                &uid_address,
                0,
                std::ptr::null(),
                &mut size,
                &mut string_ref as *mut CFTypeRef as *mut c_void,
            )
        };
        if status != 0 || string_ref.is_null() {
            return None;
        }

        // `kAudioDevicePropertyDeviceUID` returns a new reference (create
        // rule); wrapping it lets `CFString` release it on drop.
        let uid = unsafe { CFString::wrap_under_create_rule(string_ref as CFStringRef) };
        let uid = uid.to_string();
        if uid.is_empty() {
            None
        } else {
            Some(uid)
        }
    }
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
