//! The voice space (spatial voice): us in the middle of a disc, facing
//! up; everyone else in our voice room placed around us by dragging,
//! and heard from there (`audio`'s `spatial.rs`). A place is kept per
//! person with their volume and mute (`Settings::set_peer_audio`), on
//! this device only: nothing of it goes to the server, so it works on
//! any server.
//!
//! A child of `app`: everything private to `AppState` is in reach
//! through `super::*`, so these are the same methods, only filed.

// A child module of `app`: its parent's private items and imports are
// its own, which is the whole point of the file split.
use ui::SpacePerson;

#[allow(clippy::wildcard_imports)]
use super::*;

/// Nearer the middle than this is no place (as `spatial.rs` has it):
/// kept as the middle itself.
const CENTRE: f32 = 0.05;
/// The Distance slider's ends (of the radius): from where a voice is as
/// loud as sent (`spatial::NEAR`) out to the rim.
const DISTANCE_MIN: f32 = 0.3;
const DISTANCE_MAX: f32 = 1.0;

impl AppState {
    /// The voice space opens (the sidebar's button, a person's menu).
    pub(super) fn open_voice_space(&mut self) {
        let s = self.main.global::<State>();
        s.set_space_spatial(self.settings.audio.spatial_voice);
        s.set_space_auto(self.settings.audio.space_auto_spread);
        s.set_space_distance(self.space_distance());
        s.set_space_width(self.settings.audio.space_width.clamp(0.0, 1.0));
        s.set_space_open(true);
        self.sync_voice_space();
    }

    pub(super) fn close_voice_space(&mut self) {
        self.main.global::<State>().set_space_open(false);
    }

    /// Who is in the space, while it is open: everyone in our voice
    /// room but us, as the roster shows them (look, speaking), at
    /// their places. Written in place when the same people are still
    /// there, so a dot being dragged keeps its drag.
    pub(super) fn sync_voice_space(&self) {
        let s = self.main.global::<State>();
        if !s.get_space_open() {
            return;
        }
        let our_room = self
            .session
            .our_client_id
            .and_then(|us| self.session.users.get(&us))
            .map(|u| i32::from(u.channel_id));
        let people: Vec<SpacePerson> = self
            .users_model
            .iter()
            .filter(|row| !row.is_channel && !row.is_us && Some(row.channel_id) == our_room)
            .map(|row| {
                let client_id = roster::join_id(row.client_id_lo, row.client_id_hi);
                let [x, y] = self
                    .session
                    .users
                    .get(&client_id)
                    .map_or([0.0, 0.0], |u| u.audio.position);
                SpacePerson {
                    client_id_lo: row.client_id_lo,
                    client_id_hi: row.client_id_hi,
                    name: row.name,
                    avatar: row.avatar,
                    has_avatar: row.has_avatar,
                    initial: row.initial,
                    tile_tint: row.tile_tint,
                    speaking: row.speaking,
                    local_muted: row.local_muted,
                    x,
                    y,
                }
            })
            .collect();
        let model = s.get_space_people();
        let same = model.row_count() == people.len()
            && people.iter().enumerate().all(|(i, p)| {
                model.row_data(i).is_some_and(|row| {
                    (row.client_id_lo, row.client_id_hi) == (p.client_id_lo, p.client_id_hi)
                })
            });
        if same {
            for (i, person) in people.into_iter().enumerate() {
                if model.row_data(i).as_ref() != Some(&person) {
                    model.set_row_data(i, person);
                }
            }
        } else {
            s.set_space_people(ModelRc::new(VecModel::from(people)));
        }
    }

    /// A dot was dragged to `[x, y]` (the disc's own units: x right, y
    /// ahead). Every move is heard at once; the release is kept.
    pub(super) fn voice_space_moved(&mut self, index: i32, x: f32, y: f32, released: bool) {
        let model = self.main.global::<State>().get_space_people();
        let Ok(i) = usize::try_from(index) else {
            return;
        };
        let Some(mut row) = model.row_data(i) else {
            return;
        };
        let client_id = roster::join_id(row.client_id_lo, row.client_id_hi);
        let [x, y] = on_disc([x, y]);
        self.place_peer(client_id, [x, y], released);
        if released {
            self.save_settings();
            row.x = x;
            row.y = y;
            model.set_row_data(i, row);
        }
    }

    /// "Spread out": everyone in our voice room as far apart as ears
    /// can tell them (`spread`).
    pub(super) fn voice_space_spread(&mut self) {
        let members = self.room_members();
        self.spread_people(&members);
    }

    /// Once per pass of the event loop (`flush_roster`): if who is in
    /// our voice room changed since the last look — someone joined or
    /// left, or we moved — and the voice space spreads by itself
    /// (`audio.space_auto_spread`), everyone is spread out again.
    pub(crate) fn auto_spread_voice_space(&mut self) {
        let members = self.room_members();
        if members == self.session.space_room {
            return;
        }
        self.session.space_room.clone_from(&members);
        if self.settings.audio.space_auto_spread && !members.is_empty() {
            self.spread_people(&members);
        }
    }

    /// The voice space's "Spread out automatically": on, everyone is
    /// spread out at once.
    pub(super) fn set_space_auto_spread(&mut self, on: bool) {
        self.settings.audio.space_auto_spread = on;
        self.main.global::<State>().set_space_auto(on);
        self.save_settings();
        if on {
            self.session.space_room.clear();
            self.auto_spread_voice_space();
        }
    }

    /// Everyone in our voice room but us, in a steady order: by name,
    /// then by session (so a join or a leave moves as few as it can).
    fn room_members(&self) -> Vec<u64> {
        let Some(us) = self.session.our_client_id else {
            return Vec::new();
        };
        let Some(room) = self.session.users.get(&us).map(|u| u.channel_id) else {
            return Vec::new();
        };
        let mut members: Vec<(String, u64)> = self
            .session
            .users
            .iter()
            .filter(|&(&id, user)| id != us && user.channel_id == room)
            .map(|(&id, user)| (user.username.to_lowercase(), id))
            .collect();
        members.sort();
        members.into_iter().map(|(_, id)| id).collect()
    }

    /// `members` spread out (`spread`), kept, and shown.
    fn spread_people(&mut self, members: &[u64]) {
        let n = members.len();
        let distance = self.space_distance();
        for (i, &client_id) in members.iter().enumerate() {
            self.place_peer(client_id, spread(i, n, distance), true);
        }
        self.save_settings();
        self.sync_voice_space();
    }

    /// The Distance slider as kept, within its ends.
    fn space_distance(&self) -> f32 {
        let d = self.settings.audio.space_distance;
        if d.is_finite() {
            d.clamp(DISTANCE_MIN, DISTANCE_MAX)
        } else {
            client::config::DEFAULT_SPACE_DISTANCE
        }
    }

    /// The Distance slider moved: everyone placed in our room moves to
    /// that distance, each in their own direction (whoever sits in the
    /// middle stays there), heard at once; `released`, also kept.
    pub(super) fn voice_space_distance(&mut self, distance: f32, released: bool) {
        self.settings.audio.space_distance = distance;
        let distance = self.space_distance();
        for client_id in self.room_members() {
            let Some([x, y]) = self.session.users.get(&client_id).map(|u| u.audio.position) else {
                continue;
            };
            let r = x.hypot(y);
            if r < CENTRE {
                continue;
            }
            self.place_peer(client_id, [x / r * distance, y / r * distance], released);
        }
        if released {
            self.save_settings();
        }
        self.sync_voice_space();
    }

    /// The Width slider moved: heard at once; `released`, also kept.
    pub(super) fn voice_space_width(&mut self, width: f32, released: bool) {
        let width = if width.is_finite() {
            width.clamp(0.0, 1.0)
        } else {
            audio::DEFAULT_SPATIAL_WIDTH
        };
        self.settings.audio.space_width = width;
        self.voice
            .tx
            .send(VoiceControl::SetSpatialWidth(width))
            .ok();
        if released {
            self.save_settings();
        }
    }

    /// "All to the middle": nobody placed, as before spatial voice.
    pub(super) fn voice_space_centre(&mut self) {
        let model = self.main.global::<State>().get_space_people();
        for i in 0..model.row_count() {
            if let Some(row) = model.row_data(i) {
                let client_id = roster::join_id(row.client_id_lo, row.client_id_hi);
                self.place_peer(client_id, [0.0, 0.0], true);
            }
        }
        self.save_settings();
        self.sync_voice_space();
    }

    /// Spatial voice on or off for everyone; the places are kept.
    pub(super) fn set_spatial_voice(&mut self, on: bool) {
        self.settings.audio.spatial_voice = on;
        self.main.global::<State>().set_space_spatial(on);
        self.voice.tx.send(VoiceControl::SetSpatialVoice(on)).ok();
        self.save_settings();
    }

    /// Put `client_id` at `position` for the engine, and — `keep` — in
    /// the settings with the rest of their audio (the caller saves the
    /// file).
    fn place_peer(&mut self, client_id: u64, position: [f32; 2], keep: bool) {
        let Some(user) = self.session.users.get_mut(&client_id) else {
            return;
        };
        user.audio.position = position;
        let audio = peer_audio_to(user.audio);
        let username = user.username.clone();
        if keep {
            self.save_peer_audio(client_id, &username, Some(audio.clone()));
        }
        self.voice
            .tx
            .send(VoiceControl::SetPeerAudio(client_id, audio))
            .ok();
    }
}

/// `position` inside the unit disc, and exactly the middle when it is
/// nearly there.
fn on_disc([x, y]: [f32; 2]) -> [f32; 2] {
    if !x.is_finite() || !y.is_finite() {
        return [0.0, 0.0];
    }
    let r = x.hypot(y);
    if r < CENTRE {
        [0.0, 0.0]
    } else if r > 1.0 {
        [x / r, y / r]
    } else {
        [x, y]
    }
}

/// Where the `i`th of `n` people goes when the space is spread out, at
/// `distance` from us: up to five across the half in front of us — two
/// are hard left and right, as far apart as ears can tell — and more
/// round the whole circle, evenly.
fn spread(i: usize, n: usize, distance: f32) -> [f32; 2] {
    let degrees = match n {
        0 | 1 => 0.0,
        2..=5 => -90.0 + 180.0 * i as f32 / (n - 1) as f32,
        _ => -180.0 + 360.0 * (i as f32 + 0.5) / n as f32,
    };
    let a = degrees.to_radians();
    [distance * a.sin(), distance * a.cos()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn degrees([x, y]: [f32; 2]) -> f32 {
        x.atan2(y).to_degrees()
    }

    #[test]
    fn two_people_go_hard_left_and_right() {
        assert!((degrees(spread(0, 2, 0.6)) + 90.0).abs() < 1e-3);
        assert!((degrees(spread(1, 2, 0.6)) - 90.0).abs() < 1e-3);
    }

    #[test]
    fn up_to_five_stay_in_front_and_more_go_round() {
        for n in 1..=5 {
            for i in 0..n {
                let [_, y] = spread(i, n, 0.6);
                assert!(y >= -1e-6, "{i} of {n} is behind");
            }
        }
        let angles: Vec<f32> = (0..8).map(|i| degrees(spread(i, 8, 0.6))).collect();
        for pair in angles.windows(2) {
            assert!((pair[1] - pair[0] - 45.0).abs() < 1e-3, "{angles:?}");
        }
    }

    #[test]
    fn spreading_puts_everyone_at_the_distance() {
        for n in 1..=8 {
            for i in 0..n {
                let [x, y] = spread(i, n, 0.45);
                assert!((x.hypot(y) - 0.45).abs() < 1e-5, "{i} of {n}");
            }
        }
    }

    fn near([x, y]: [f32; 2], [ex, ey]: [f32; 2]) -> bool {
        (x - ex).abs() < 1e-6 && (y - ey).abs() < 1e-6
    }

    #[test]
    fn places_stay_on_the_disc() {
        assert!(near(on_disc([0.01, 0.02]), [0.0, 0.0]));
        assert!(near(on_disc([3.0, 4.0]), [0.6, 0.8]));
        assert!(near(on_disc([0.5, -0.5]), [0.5, -0.5]));
        assert!(near(on_disc([f32::NAN, 0.5]), [0.0, 0.0]));
    }
}
