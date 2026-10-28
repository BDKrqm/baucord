# Changelog

What changed from one release to the next, for the people who run a
baucord server and the people who use the apps. Releases before this
file are summed up in SESSION-GUIDE.md §3 ("Shipped").

## 1.4.0 (unreleased)

Numbered for the bot and plugin framework planned on it (the spec is
in the 2026-10-27 review document), on top of the 2026-10 review fixed
whole: what costs while nothing happens, pictures and memory, Windows
audio, Settings that save only when asked, and the release pipeline.
The 1.3.0 that was next became this.

### Bots

- **`baucord-gateway`: bots in any language.** A new binary
  (`crates/gateway`) connects to a server as a user with a key of its
  own and serves a WebSocket on the loopback where a bot reads JSON
  events (`READY`, `MESSAGE_CREATE`, `MEMBER_ADD`, `VOICE_STATE`, …),
  sends JSON actions (`CREATE_MESSAGE`, `CREATE_DM`, `JOIN_CHANNEL`,
  `SET_NICKNAME`, `FETCH_HISTORY`, …) and streams PCM that the gateway
  encodes as Opus and plays into the bot's voice channel, one 20 ms
  frame every 20 ms whatever the bot's pace. Admission is an invite
  code on the first run and the key alone after; the server's identity
  is pinned on first use like the app does. A Python echo bot and a
  `yt-dlp`/`ffmpeg` music bot are in `crates/gateway/examples`, the
  wire is `crates/gateway/README.md`. Tested against a real server on
  the in-process network (admission, chat both ways, voice reaching a
  listener, a refusal) and live on Linux. Slash commands and the `bot`
  role come with the server's side of the bot work; until then a bot
  reacts to chat lines and is an ordinary member.
- **A bot in Rust is one binary.** The gateway is also a library,
  `gateway::bot::Bot` (`connect`, `events`, `act`, `play`,
  `stop_playing`), and `crates/gateway/examples/music.rs` is a music
  bot built on it with nothing to install: `!play <file or url>`,
  `!skip`, `!stop`, `!queue`, files from a `--library` folder and
  never outside it, URLs and internet radio fetched as they play,
  decoded in-process (symphonia, rubato — the audio engine's crates).
  Run live against a real server: a 6 s file lights the bot's speaking
  light for 6.2 s, a queued track follows by itself, `!skip` starts
  the next within 30 ms, `!stop` goes quiet within 0.2 s.
  A link to a YouTube, Spotify, SoundCloud or Bandcamp page — the
  page about a track, not the track — gets a line saying so and what
  the bot does play, instead of a decoder error after a megabyte of
  HTML (the first live test tried all four).
- **A bots guide on the website**, `baucord.com/bots.html`, in the
  hosting guide's shape: the music bot and its commands, a bot of your
  own in thirty lines of Python, what a bot sees and does, playing
  audio, keeping it running, the rules, and what is coming. Linked
  from the front page in all three languages.

### Before you update a server

- **The database moves to schema 7, one way.** Migration
  `0007_drop_unused` drops seven columns nothing read:
  `users.profile_locked`, `user_keys.label`, `channels.is_afk`,
  `channels.position`, `blobs.declared_mime`, `telemetry_samples.sample`
  and `telemetry_events.event`. A server older than this release
  refuses the upgraded database. To go back, restore a backup taken
  before the update (`server backup FILE.tar` now, `server restore
  FILE.tar` then).
- **The 1.0 `authentication:` section is gone from the code.** It has
  been ignored since 1.1 and still is, with the same warning at start.
  Any other top-level section the server does not know (a misspelt
  `chanels:`, say) is now named in a warning too, instead of being
  skipped without a word.
- **Docker:** the example config writes the database path out,
  `persistence.path: /app/data/baucord/baucord.db`. That is where the
  relative default already put it, so nothing moves; it just no longer
  depends on the container's working directory. The official server's
  own `config/baucord.yaml` says port 51000, like the compose file.
- **The image is built on Debian 12 (bookworm)**, like the distroless
  runtime it runs on, and is started once before it is pushed. `latest`
  only moves for the newest stable release: rebuilding an older tag or
  publishing a backport no longer moves it backwards (which made hosts
  that pulled it refuse their newer database). The same rule now holds
  for the version file the apps check for updates and for the website.

### Server

- A client that vanishes (no goodbye) is noticed after up to about 50
  seconds instead of about 30: the server now sends a keep-alive every
  20 seconds, so a phone in the lobby with its screen off keeps its
  session, and the 30-second idle timeout counts from the last one.
- Upload budgets: pictures and files have separate per-person buckets,
  so one large file no longer blocks every picture on the server for a
  minute. The server-wide budget counts them apart too, and the file
  bucket is never below the largest allowed file.
- Chat pictures larger than 4096 pixels on a side are dropped by the
  relay (no app makes them; a crafted one could exhaust a receiver).
- `limits.max_chat_bytes` is capped at 4096 (one message has to fit a
  history page), with a warning; an oversized newest message is
  truncated instead of giving an empty page.
- A ban made from the admin panel can no longer drop out of memory
  after a run of failed reloads.
- A direct message whose save timed out keeps its file; the server
  recounts the file's references when the store answers.
- The command line applies the panel's limits: at most as many roles,
  and restriction reasons cut to the same length.
- Telemetry: the server stores the figures it reports on (round trip,
  loss, sharing figures, audio device names) and the kind of each
  event, not the whole sample; the full line still goes to the log when
  `logging.telemetry` is on. Idle clients send it once a minute instead
  of every 10 seconds.
- A server that is full now says so: apps show "The server is full
  right now." and retry after 5 to 60 seconds. Every reconnect wait has
  a random spread (±50%), so a restart no longer brings everyone back
  in the same instant.

### Desktop

- **Settings → Stream.** A tab for how your screen share starts:
  resolution and frame rate (the presets, as two pickers), the encoder
  — Automatic, GPU or CPU — visibility (My channel / Everyone, as in
  the share dialog) and the mouse pointer. The share dialog opens on
  these and writes its own picks back, so the two never disagree.
- **Streams no longer starve behind a game (Windows).** A game that
  keeps the GPU at 99 % made the share wait its turn — the stream
  dropped to a few frames a second while the game ran fine, until the
  game's frame rate was capped. That was not a shortage of GPU (the
  share needs a few percent) but the GPU scheduler's queue: the share's
  copies and encodes sat behind the game's work. While sharing, baucord
  now asks Windows for a higher GPU scheduling priority, as OBS does
  (`D3DKMTSetProcessSchedulingPriorityClass`: realtime when started as
  administrator, high otherwise) and sets its capture device's thread
  priority to the maximum; it goes back to normal when the share stops.
  The game gives up a frame or two a second to the stream. The switch
  is Settings → Stream → "Stream encoding priority", on by default,
  Windows only (macOS and Linux have no such knob).
- **GPU or CPU encoding is a choice.** Automatic (as before: NVENC,
  Quick Sync, AMF or VideoToolbox when one opens, x264 otherwise), GPU
  only (the share refuses to start without a hardware encoder instead
  of quietly costing the processor) or CPU only (x264 whatever the GPU
  offers — the way around a driver whose encoder opens but streams
  badly). A mid-share encoder switch stays within the choice.
- Settings' switch rows measure a hint that wraps to two or three
  lines; before, one ran into the row below.
- **Settings say what they are.** Labels name the setting in a few
  words and keep at most one line under them: "Connect on launch",
  "Check for updates on launch", "Message timestamps", "Image upload
  size"; Audio's "Microphone", "Output", "Gain", "Sensitivity",
  "Volume" under their headers; a one-line Debug lead. The share
  dialog's "Visible to" is "Visibility".
- **macOS: a real app.** The Mac download is now `baucord.app` in a
  disk image: open it and drag baucord into Applications. One app for
  Apple silicon and Intel Macs. It opens its window without a Terminal
  window first, has its icon in the Dock, and asks for the microphone
  as baucord. Signed with a Developer ID and notarized by Apple once
  those are set up (macos/README.md).
- An installed `baucord.app` updates itself whole, and keeps its
  settings and identity in `~/Library/Application Support/baucord`,
  never inside the app. Copies still running from the old zip go on
  updating from it. **Moving from the zip to the app:** before the
  app's first start, copy `settings.json` and `identity.key` from the
  zip's folder into `~/Library/Application Support/baucord`; otherwise
  the app starts as a new user with a new identity (macos/README.md §1).

- **Sharper 1080p streams.** 1080p at 60 fps encodes at 10 Mbit/s (was
  8) and at 30 fps at 7.5 (was 6). A frame too large to send no longer
  costs the share minutes of quality: the encoder's quality floor
  (since 1.1.9, the remedy for AMD's oversized keyframes; it binds
  every frame, not only keyframes) now rises only as far as that frame
  needed and comes back down once keyframes fit with room, instead of
  jumping four steps at a time and reopening every 30 s into another
  oversized keyframe. It was most of what AMD and Mac sharers lost.
- **Settings save only on "Save and close".** Close, Escape and Back
  put everything back as it was when the page opened; Reset to
  defaults only changes the values on the page. Quitting with unsaved
  changes (the tray's Quit, `/quit`, closing the window without a tray,
  or restarting for an update) asks first: Save and quit, Quit without
  saving, or Cancel. When the system ends the app, nothing unsaved is
  kept.
- **Windows: other apps are no longer turned down in the lobby.** The
  call path (the communications device, which Windows ducks other apps
  for) opens only in a voice room, and under the Automatic echo
  setting only on speakers; headphones never use it. Stream audio
  in a room is opened by device id rather than as a communications
  stream, aimed at reports that watching a stream on Windows 11 still
  lowered other apps (not yet confirmed on hardware).
- A device you picked stays picked while it is unplugged; the system
  default stands in until it returns. Closing Settings or resetting it
  no longer reopens the microphone and speakers mid-call.
- Windows: with an output or microphone picked, a USB or Bluetooth
  device that becomes Windows' default no longer sends the app's voice
  thread spinning at a full core until a re-pick or a restart (a
  side effect of October's fewer wake-ups).
- Screen sharing on Windows: a portrait (rotated) monitor is shared
  upright through the screenshot path instead of sideways; locking the
  screen no longer demotes the share to screenshots for good (streaming
  is retried every 10 seconds); 1440p and 4K screens are halved on the
  GPU when the stream is at most half their size; moving the pointer
  over a still desktop no longer re-reads the whole screen; an encoder
  that starts failing (a driver reset, sleep) is reopened. (The GPU
  halving and the pointer path are type-checked, not yet run on
  Windows hardware.)
- Screen sharing on Linux (X11) reads only what changed (DAMAGE); the
  screenshot path (macOS, Wayland, Windows' fallback) slows to five
  grabs a second after two still seconds. A share nobody watches runs
  at the still-screen pace on every platform.
- Dropping or picking a huge file no longer freezes the app: files are
  read and hashed off the UI and network threads, and the size is
  checked first.
- A minimized or hidden stream viewer keeps its sound and stops
  decoding video. A viewer minimized once no longer blanks that
  sharer's *next* stream for the rest of the run (it was dropped
  before decoding: black, silent, nothing in the log).
- A received program (`.exe`, a script, an installer) is marked as
  coming from the internet (Windows' Mark of the Web, macOS'
  quarantine) and opens its folder instead of running.
- The log is `baucord.log` in the config folder (beside
  `settings.json`), rolled over to `baucord.log.1` at 5 MB; it was in
  the working directory and grew without limit.
- When settings move to another folder, the identity key and your
  avatar move with them: you no longer turn into a new account.
- Received file names are made safe to save on Windows (reserved
  names, `< > " | ? *`, direction controls, long names).
- A failed update download is cleaned up, and installing no longer
  freezes the window.
- BMP and TIFF pictures can still be pasted and attached on the
  desktop (the phones no longer carry those decoders).

### Phones

- **iPhone: a TestFlight beta.** The release workflow uploads each
  build to TestFlight; testers install it from the TestFlight app, no
  cable or registered device needed.
- **Android: a Play internal-testing track.** The release workflow
  uploads the app bundle to Play's internal track itself once a
  service account is set up (android/README.md §6a); the release
  notes it shows live in `android/play/whatsnew/`. Promotion to closed
  testing and production stays a click in Play Console.
- iPhone: a phone call taken in the lobby no longer leaves the app
  spinning in the background afterwards, where iOS ended it.

- A stream you watch stops when the app goes to the background and
  resumes by itself when you come back: no video or sound is downloaded
  for a screen nobody sees.
- Android: a phone in the lobby with the screen off keeps its session.
  The AFK room no longer counts as a voice room for the foreground
  service and wake lock.
- iPhone: connecting no longer stops other apps' audio (only a voice
  room takes over the audio session).
- A mention plays the mention chime; a burst of direct messages makes
  one sound and one banner, not one each.
- Settings fits an iPhone SE, the emoji palette scrolls with a finger,
  toasts and menus stay clear of the status and navigation bars, and
  dialogs sit above the keyboard. Touch targets are at least 40 px,
  Back cancels an edit, and Clear chat asks first.
- Quitting from Android with unsaved Settings changes asks, as on the
  desktop (the three buttons stack on a narrow screen).
- A stream whose decoder lost its session asks for a fresh picture
  instead of staying frozen.
- iPhone: builds can go to TestFlight, uploaded by the release workflow
  (with an App Store profile and an App Store Connect API key). The app
  carries a privacy manifest, and its icon no longer has transparent
  corners, which iOS showed as black.

### Everyone

- **Spatial voice.** The new voice space (the radar button beside
  Deafen, or *Voice space…* in a person's menu) puts you in the middle
  of a disc with everyone in your voice channel around you: drag a
  person somewhere and their voice comes from there, so two or three
  people talking at once are told apart by direction. *Spread out*
  places everyone as far apart as ears can tell; *All to the middle*
  undoes it; the *Spatial voice* switch turns it off without losing the
  places. Places are kept per person on this device with their volume,
  and nothing of it goes to the server — it works on any server, old
  or new. Best with headphones; a mono output (a Bluetooth headset in
  call mode, a phone's call path) hears everyone in the middle.
  *Spread out automatically* (on by default) spreads everyone out again
  whenever someone joins or leaves your voice channel. The *Distance*
  slider moves everyone nearer or farther at once (farther is quieter
  and duller, as in a room), and *Width* sets how far apart the sides
  sound — from a real head's width to wide, where two voices stay apart
  down to their low end.
- **Share a camera** (Windows). The share dialog has a *Monitor* |
  *Camera* switch: on *Camera* the list shows the cameras connected to
  the PC, and the one picked is streamed instead of the screen, at the
  chosen quality (the camera's nearest format, scaled like a screen).
  A camera's share carries no PC sound. The choice is remembered, and
  the share hotkey starts the camera when that was the last share.
- **Equalizers.** *Equalizer…* in a person's menu gives that person five
  bands of their own (100 Hz, 300 Hz, 1 kHz, 3 kHz, 8 kHz, ±12 dB), kept
  with their volume; *Flat* undoes it. Settings → Audio → *Voice tone*
  colours everyone's voice at once: *Natural* (as before), *Crisp* or
  *Bass boosted*. Like the places, both stay on your device.
- **empecord.** The fork's app is called empecord: its window, tray,
  About tab and `empecord.exe`. It reads and writes the same
  `settings.json`, `identity.key` and `baucord.log` as baucord (the
  same login on every server), and it never offers baucord's updates,
  which would put upstream baucord in its place. The settings it adds
  are ones baucord ignores; its own hotkey is kept apart
  (`more_keybinds`), because baucord refuses an action it does not
  know.
- **Save the last 5 minutes** (desktop). empecord keeps the last five
  minutes of the call in memory, and the button beside the voice space
  (or its hotkey, Settings → Keyboard) writes them to
  `replays/<date time>/` next to the settings: one WAV per person,
  named after them and aligned with the others, and `mix.wav`. Only
  speech is kept (silence costs nothing), at most 192 MB; nothing
  touches the disk before the button, and nobody else is told.
  Settings → Audio → *Replay buffer* turns it off, which forgets what it
  held, and opens the folder.
- **Soundboard** (desktop). The music-note button opens it: every
  sound file in `soundboard/` next to the settings (WAV, MP3, OGG,
  FLAC or M4A, up to 60 s) is a button. A press plays the file into the
  call through your microphone — after the noise gate and the denoiser,
  so they leave it alone — and, with *Hear them myself too*, into your
  ears. It has its own volume. Muted or in push-to-talk with the key
  up, nobody hears it, as with your voice.
- **Colour themes.** Settings → General → *Theme*: Dark (the original,
  unchanged), Midnight, Forest, Sunset, Light, High contrast, or Custom
  — a background, a text colour and an accent as `#RRGGBB`, from which
  the rest of the palette is made. Text too close to its background is
  moved apart until it reads. Seen at once in the main window and the
  stream viewers; Close without saving puts the old one back.
- Pictures you send have their EXIF data removed (camera, GPS
  location) and are turned the right way up first; a portrait photo no
  longer arrives sideways.
- Pictures and avatars decode off the UI thread; an avatar larger than
  512 px is refused and shows the initial. Chat pictures are no longer
  also kept as avatars (they were written to disk for good).
- A person's menu acts on the person you opened it on, even if the
  roster moved meanwhile.
- Tabs in a message become four spaces, so pasted code keeps its
  indentation. Messages keep emoji variation selectors as typed (they
  were stripped for good on sending), and the picker no longer lists
  five glyphs twice.
- A picture in a direct message is downloaded once (it was fetched
  twice), and one whose download failed is tried again.
- "Load earlier" stops, with a line saying so, where the chat would
  start dropping the rows being read.
- The latency line's "loss" is translated.
- The roster sorts names without regard to case ("ana" before "Zoe").
- A stream's mute and volume set before its sound starts are kept.
- Cancelling a download stops the server sending it.
- Invite codes no longer appear whole in the app's log.
- Changing the language updates the tray, the roster and the admin
  channel's name at once.
- The connect card holds only the form. A connection error, a ban or
  kick verdict and a missing device show as toasts at the top of the
  window, the last two until they no longer apply (a click on the
  device one opens Settings), and the card centres below them. A
  window shorter than the card scrolls.
- Settings' and the admin panel's footer is a bar across the whole
  window, with its buttons in the bottom-right corner; on a big window
  they stopped in the middle of it. Both scroll with the bar at the
  window's edge (the admin lists' bar ran over the role pickers), and
  the admin panel's rows may grow to 960 px.
- A stream ending in your channel plays the start's two notes falling
  (the setting is now "starts or stops sharing"). A sharer who quits or
  leaves gets the leave chime alone.
- Editing or making a role opens its own page in the Roles tab, in place
  of the list, with an arrow back; Escape and Back leave the editor
  before the panel.
- A ban says its length and reason with spaces between them ("…from
  this server for 2 h: spam", not "…from this serverfor 2 hspam").

### Under the hood

- Far fewer wake-ups with nothing happening: the voice thread, the
  network watcher, the capture and encoder workers, the key watcher
  (its check on whether key events arrive now backs off to every 4
  seconds and is settled once per run), timers paused while nothing is
  shown, and no Opus encoding when nobody can hear it (a microphone
  test, the Settings meter, a room with nobody else).
- FFmpeg 7.1.2 on every desktop, the Linux and macOS source checked
  against its SHA-256; the unused avfilter and swresample libraries are
  no longer linked, nor built on Linux and macOS (Windows' prebuilt
  FFmpeg still contains them, unused, until it is next rebuilt).
- The app reads its own CPU and memory figures directly on every
  platform (`sysinfo` is gone).
- Release builds take `Cargo.lock` as written on every platform;
  signed Android builds come only from release tags, and their
  workflow artifacts are kept for 7 days.
- Releases wait for the checks: every release workflow runs the
  format, lint and test gate on the tagged commit and attaches, pushes
  or uploads nothing until it passed (a tag on a commit whose check
  the next push had cancelled released untested). The Windows build's
  prebuilt FFmpeg is checked against a pinned SHA-256 before it is
  unpacked. The website names the version whose files are there (the
  `version` file's), not `main`'s `Cargo.toml` — bumping the version
  before its release advertised downloads that did not exist — and
  hides a download whose file is missing.
- CI: one lint pass instead of a check and a lint, a pinned nightly for
  formatting, the chat-scroll harness, and a Windows type-check from
  Linux on every change.
- The third-party notices list our own crates out (a hand-written list
  had missed `keywatch`) and the website's Alpine.js in.
- House style, measured against Google's Rust style guide (the
  "Google's Rust guide vs baucord" doc, SESSION-GUIDE.md §4): every
  `unsafe` block says why it is sound (65 new `// SAFETY:` comments, and
  clippy now warns about a missing one); every public item of the
  `protocol` and `transport` crates is documented (`missing_docs`);
  `VoiceChat::set_presence` takes a `Presence` (lobby, live, muted)
  instead of two booleans; `get_`-prefixed getters renamed
  (`AudioStats::summary`, the server's `username_of`); a few `&String`
  and `&Option<String>` parameters take `&str` and `Option<&str>`; doc
  tests on `encode_client_message`, `Driver::new_client` and
  `Bot::connect`; `cargo bench -p audio` times the resampler and a
  jitter-buffer tick; imports are one `use` per module.
