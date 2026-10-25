//! A person's own five-band equalizer (a person's menu → Equalizer…):
//! the dialog in `ui/slint/equalizer.slint`, the bands in `audio`'s
//! `processor/eq.rs`. Kept per person with their volume, mute and place
//! (`Settings::set_peer_audio`), on this device only.
//!
//! A child of `app`: everything private to `AppState` is in reach
//! through `super::*`, so these are the same methods, only filed.

// A child module of `app`: its parent's private items and imports are
// its own, which is the whole point of the file split.
#[allow(clippy::wildcard_imports)]
use super::*;

impl AppState {
    /// Open `client_id`'s equalizer at their bands as kept.
    pub(super) fn open_peer_eq(&mut self, client_id: u64) {
        let Some(user) = self.session.users.get(&client_id) else {
            return;
        };
        let title = t!("eq-title", "name" => user.username.as_str());
        let bands = user.audio.eq;
        self.session.eq_peer = Some(client_id);
        let s = self.main.global::<State>();
        s.set_eq_title(title.into());
        show_bands(&s, bands);
        s.set_eq_open(true);
    }

    pub(super) fn close_peer_eq(&mut self) {
        self.session.eq_peer = None;
        self.main.global::<State>().set_eq_open(false);
    }

    /// A band moved to `db`: heard at once; `released`, also kept.
    pub(super) fn peer_eq_changed(&mut self, band: i32, db: f32, released: bool) {
        let Ok(band) = usize::try_from(band) else {
            return;
        };
        if band >= audio::EQ_BANDS {
            return;
        }
        // Half-dB steps, as the dialog shows them.
        let db = (db.clamp(-audio::EQ_MAX_DB, audio::EQ_MAX_DB) * 2.0).round() / 2.0;
        self.set_peer_eq(|bands| bands[band] = db, released);
    }

    /// "Flat": every band back to 0 dB, kept.
    pub(super) fn peer_eq_reset(&mut self) {
        self.set_peer_eq(|bands| *bands = [0.0; audio::EQ_BANDS], true);
        show_bands(&self.main.global::<State>(), [0.0; audio::EQ_BANDS]);
    }

    /// Change the open dialog's person's bands with `edit`, tell the
    /// engine, and — `keep` — save them.
    fn set_peer_eq(&mut self, edit: impl FnOnce(&mut [f32; audio::EQ_BANDS]), keep: bool) {
        let Some(client_id) = self.session.eq_peer else {
            return;
        };
        // Gone since the dialog opened: nothing to equalize.
        let Some(user) = self.session.users.get_mut(&client_id) else {
            self.close_peer_eq();
            return;
        };
        edit(&mut user.audio.eq);
        let audio = peer_audio_to(user.audio);
        let username = user.username.clone();
        if keep {
            self.save_peer_audio(client_id, &username, Some(audio.clone()));
            self.save_settings();
        }
        self.voice
            .tx
            .send(VoiceControl::SetPeerAudio(client_id, audio))
            .ok();
    }
}

/// The dialog's five sliders at `bands`.
fn show_bands(s: &State<'_>, [b0, b1, b2, b3, b4]: [f32; audio::EQ_BANDS]) {
    s.set_eq_b0(b0);
    s.set_eq_b1(b1);
    s.set_eq_b2(b2);
    s.set_eq_b3(b3);
    s.set_eq_b4(b4);
}
