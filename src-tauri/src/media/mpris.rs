//! Linux MPRIS backend (org.mpris.MediaPlayer2.spotify over D-Bus, via zbus).
//!
//! Not implemented in this Windows-only Phase 1 pass -- see docs/phase0-plan.md §2.1 for
//! the intended design (subscribe to `PropertiesChanged` on the Player interface, read
//! `xesam:artist` / `xesam:title` / `xesam:album` from the `Metadata` property). The
//! `MediaBackend` trait it implements is exactly what `smtc.rs` implements, so this can be
//! filled in later without touching `media::mod` or anything above it.

use super::{MediaBackend, RawEventSender};

pub struct MprisBackend;

impl MprisBackend {
    pub fn new() -> Self {
        Self
    }
}

impl MediaBackend for MprisBackend {
    fn spawn(self: Box<Self>, _tx: RawEventSender) {
        log::error!("MPRIS media backend is not yet implemented (Windows-only Phase 1 pass)");
    }
}
