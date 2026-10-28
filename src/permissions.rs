//! Settings that are decided per origin by the permission prompt (each edition maps its
//! engine's permission kinds onto these names).

/// (setting, words for the prompt: "<host> wants to …").
pub const SETTINGS: &[(&str, &str)] = &[
    ("content.javascript.clipboard", "read your clipboard"),
    ("content.persistent_storage", "store data persistently"),
    ("content.geolocation", "see your location"),
    ("content.notifications.enabled", "show notifications"),
    ("content.mouse_lock", "lock your mouse pointer"),
    ("content.register_protocol_handler", "open links of a protocol"),
    ("content.media.audio_capture", "use your microphone"),
    ("content.media.video_capture", "use your camera"),
    ("content.desktop_capture", "capture your screen"),
];

pub fn is_setting(name: &str) -> bool {
    SETTINGS.iter().any(|s| s.0 == name)
}
