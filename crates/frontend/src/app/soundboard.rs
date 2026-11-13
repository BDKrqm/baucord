//! The soundboard: files in the `soundboard/` folder, played into the
//! call through the microphone path and, if wanted, into our own ears.
//! It works with any server.
//!
//! A child of `app`: everything private to `AppState` is in reach
//! through `super::*`, so these are the same methods, only filed.

use std::path::{Path, PathBuf};

use ui::SoundboardClip;

// A child module of `app`: its parent's private items and imports are
// its own, which is the whole point of the file split.
#[allow(clippy::wildcard_imports)]
use super::*;

/// What the soundboard plays (what symphonia reads here).
const CLIP_EXTENSIONS: [&str; 8] = ["wav", "mp3", "ogg", "oga", "flac", "m4a", "aac", "mp4"];

impl AppState {
    // ── The soundboard ─────────────────────────────────────────────

    pub(super) fn open_soundboard(&mut self) {
        self.soundboard_refresh();
        let s = self.main.global::<State>();
        s.set_soundboard_volume(self.settings.audio.soundboard_volume);
        s.set_soundboard_hear(self.settings.audio.soundboard_hear);
        s.set_soundboard_folder(Settings::soundboard_dir().display().to_string().into());
        s.set_soundboard_open(true);
    }

    pub(super) fn close_soundboard(&mut self) {
        self.main.global::<State>().set_soundboard_open(false);
    }

    /// Read the folder again: the buttons are its files, by name.
    pub(super) fn soundboard_refresh(&mut self) {
        let dir = Settings::soundboard_dir();
        // Made on the first look, so "Open folder" has somewhere to go.
        let _ = std::fs::create_dir_all(&dir);
        self.soundboard_files = clip_files(&dir);
        let rows: Vec<SoundboardClip> = self
            .soundboard_files
            .iter()
            .map(|path| SoundboardClip {
                name: path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default()
                    .into(),
            })
            .collect();
        self.main
            .global::<State>()
            .set_soundboard_clips(ModelRc::new(VecModel::from(rows)));
    }

    /// Button `index` was pressed: play it, if anyone can hear it.
    pub(super) fn soundboard_play(&mut self, index: i32) {
        let Some(path) = usize::try_from(index)
            .ok()
            .and_then(|i| self.soundboard_files.get(i))
            .cloned()
        else {
            return;
        };
        if !self.in_voice_room() {
            self.show_toast(ToastKind::Info, &t!("soundboard-not-in-room"));
            return;
        }
        if self.session.is_muted {
            // Nobody will hear it: the microphone path is closed.
            self.show_toast(ToastKind::Warning, &t!("soundboard-muted"));
        }
        self.voice
            .tx
            .send(VoiceControl::PlayClip {
                path,
                gain: self.settings.audio.soundboard_volume,
                hear_it: self.settings.audio.soundboard_hear,
            })
            .ok();
    }

    pub(super) fn soundboard_stop(&mut self) {
        self.voice.tx.send(VoiceControl::StopClips).ok();
    }

    /// The volume slider let go: kept (the next clip plays at it).
    pub(super) fn soundboard_volume(&mut self, volume: f32) {
        self.settings.audio.soundboard_volume = volume.clamp(0.0, 1.0);
        self.save_settings();
    }

    pub(super) fn soundboard_hear(&mut self, on: bool) {
        self.settings.audio.soundboard_hear = on;
        self.save_settings();
    }

    pub(super) fn open_soundboard_folder() {
        open_folder(&Settings::soundboard_dir());
    }

    /// A clip would not play (`VoiceEvent::ClipFailed`).
    pub(super) fn clip_failed(&mut self, path: &Path, error: &audio::ClipError) {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let reason = match error {
            audio::ClipError::TooBig => t!("soundboard-too-big"),
            audio::ClipError::TooLong => t!("soundboard-too-long"),
            audio::ClipError::Unreadable(_) => t!("soundboard-unreadable"),
        };
        if let audio::ClipError::Unreadable(e) = error {
            warn!("soundboard: {name}: {e}");
        }
        self.show_error(
            &t!("soundboard-failed", "file" => name.as_str(), "reason" => reason.as_str()),
        );
    }
}

/// The playable files in `dir`, sorted by name (case-insensitively).
fn clip_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file() && is_clip(p))
        .collect();
    files.sort_by_key(|p| {
        p.file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default()
    });
    files
}

fn is_clip(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| CLIP_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Open `dir` in the system's file manager, making it first.
fn open_folder(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
    if let Err(e) = crate::host::open_target(dir) {
        warn!("Failed to open {}: {e}", dir.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_audio_files_are_buttons_in_name_order() {
        let dir = std::env::temp_dir().join(format!("empecord-sb-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub.mp3")).expect("dir");
        for name in ["b.MP3", "a.wav", "notes.txt", "c.flac", "noext"] {
            std::fs::write(dir.join(name), b"x").expect("write");
        }
        let names: Vec<String> = clip_files(&dir)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(names, ["a.wav", "b.MP3", "c.flac"]);
    }
}
