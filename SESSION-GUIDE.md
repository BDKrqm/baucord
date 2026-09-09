# baucord — session guide

Read this first in a new chat. It says how the assistant works on
this codebase without a Rust toolchain, where things are, the rules
that were learned the hard way, what is shipped, and what is left.
The design record is `PERSISTENCE-SPEC.md` (the 1.1 line's contract,
with §12 holding every decision taken since); `ARCHITECTURE.md`
describes the runtime. Keep all three in the repo root.

## 1. Setting up the assistant's environment

There is no `cargo` in the sandbox. Rust is compiled by the user; the
assistant edits files, compile-checks and renders every `.slint`
change, and hands over a tarball. Do this at the start of a session:

```sh
# the workspace: the user uploads crates.tar.gz (the crates/ folder)
mkdir -p /home/claude/work && cd /home/claude/work && tar -xzf /mnt/user-data/uploads/crates_tar.gz

# slint-viewer 1.17.1 (the app pins Slint 1.17.1 — match it)
mkdir -p /tmp/sv && cd /tmp/sv && curl -sSL -o sv.tar.gz \
  https://github.com/slint-ui/slint/releases/download/v1.17.1/slint-viewer-linux.tar.gz \
  && tar -xzf sv.tar.gz     # → /tmp/sv/slint-viewer/slint-viewer

# the icon library the UI imports as @lucide
mkdir -p /tmp/lucide && cd /tmp && curl -sSL -o lucide-slint.crate \
  https://static.crates.io/crates/lucide-slint/lucide-slint-1.34.0.crate \
  && tar -xzf lucide-slint.crate -C /tmp/lucide   # → /tmp/lucide/lucide-slint-1.34.0/lucide.slint

# Slint's own sources, for checking runtime behaviour instead of guessing
for c in i-slint-core i-slint-compiler i-slint-backend-winit; do
  curl -sSL -o /tmp/$c.crate https://static.crates.io/crates/$c/$c-1.17.1.crate
  mkdir -p /tmp/src-$c && tar -xzf /tmp/$c.crate -C /tmp/src-$c
done
```

Any crate the code depends on can be fetched the same way
(`https://static.crates.io/crates/<name>/<name>-<version>.crate`) to
read its real API before writing against it — done for `interprocess`,
`device_query`, `quinn-proto`. `crates.io/api/v1/crates/<name>` lists
versions (send a `User-Agent`).

### The loop

```sh
cd /home/claude/work/crates/ui/slint
V=/tmp/sv/slint-viewer/slint-viewer; L="-L lucide=/tmp/lucide/lucide-slint-1.34.0/lucide.slint"

$V --check $L main.slint                                   # every .slint change
$V --screenshot out.png --load-data props.json $L main.slint          # render the main window
$V --screenshot out.png --component SettingsWindow --load-data p.json $L settings.slint
$V --screenshot out.png --component AdminWindow --load-data p.json $L admin.slint
SLINT_SCALE_FACTOR=2 $V --screenshot out.png ...          # 2× for site screenshots
```

`--load-data` takes JSON: top-level keys are the window's own
properties (`tab`, `phase` lives in `State`) and globals by name
(`State`, `SettingsState`, `AdminState`, `Tr`). Rows of a model must
carry **every** field of their struct — a missing `color`/`image`
field is filled with unit and panics at render (`binding was of the
wrong type: ()`); a missing string field just warns and the model is
left empty. `Timer`s do not fire in a screenshot (the hover card, the
follow), and `image` properties cannot be set from JSON. The
site-screenshot fixtures (`site/*.json`, delivered next to
`web.tar.gz`; keep them in the site repo under `web/site/`) are a
good starting point for any fixture.

Rendered PNGs are read back with a small pure-Python decoder (no PIL
needed, though PIL is available) and cropped to the region of
interest before `view`ing — a full 1000×640 view costs tokens.

Delivery: `tar -czf /mnt/user-data/outputs/crates.tar.gz crates` from
`/home/claude/work`, plus the docs that changed, then `present_files`.
The user untars over the repo root and runs

```sh
cargo test -p protocol -p app-core -p server -p transport -p client
cargo clippy -p protocol -p transport -p server -p client -p desktop --all-targets
cargo run -p desktop        # and -- --profile bob for a second instance
```

and pastes the first `error:` with its `-->` location. One or two
compiler rounds per step is normal. Edit scripts must assert their
anchors (`assert old in s`): a silent miss once shipped a function
that did not exist, and once left a script's changes unwritten.

Style: edition 2024, Rust 1.95, `clippy::pedantic = warn`,
`clippy::all = deny`, rustfmt at 100 columns (`awk 'length > 100'`
is the check; pre-existing long lines in `store.rs`, `network.rs`
and `e2e.rs` are not ours). Markdown wraps at 80. Every new string
goes into all three Fluent catalogs (en, es-AR, pt-BR) **and** every
`Tr` property in `tr.slint` is filled in `fill_tr` — a checker for
catalog parity and unfilled `Tr` props is a ten-line Python script
worth re-running.

## 2. Map

- `protocol` — the wire. Positional postcard: **never** add a field
  in the middle of a shipped struct or enum, never reorder; append
  variants, and gate every appended variant on a **new** feature bit
  (`features.rs`; a client that negotiated an old bit cannot decode a
  variant added later under it). Permission bits and names in
  `permissions.rs` (`PERMISSION_NAMES`, `ROLE_ICONS`,
  `derived_rank`). Admin commands in `admin.rs`.
- `transport` — quinn-proto driver, lanes, media pacing.
- `server` — the tick in `server.rs`; child modules `moderation`,
  `history`, `channels`, `profiles`, `dms`, `files`, `admin`; the
  persistence thread in `persistence/` (`store.rs` is SQLite, the CLI
  helpers `cli_*` live there too); `main.rs` is the CLI; migrations
  in `persistence/migrations/` (0001–0003).
- `client` — network thread (`network.rs`), config, identity.
- `desktop` — the Slint app: `app.rs` (the host: `AppState`, all
  network handling, the slash commands), `settingsui.rs`,
  `adminui.rs`, `media.rs` (images/files/GIFs), `viewers.rs`,
  `hotkeys.rs` (watched keys), `instance.rs` (one per profile),
  `tray.rs`, `updater.rs`.
- `ui` — the `.slint` files and `convert` glue (`lib.rs`):
  `main.slint` (window, overlays, card, dialogs), `sidebar.slint`,
  `chat.slint`, `settings.slint`, `admin.slint`, `viewer.slint`,
  `widgets.slint`, `theme.slint`, `tr.slint`, `globals.slint`
  (`State`/`Logic` and every struct the host fills).
- `app-core` — pure view models with unit tests: `chat.rs`
  (`ChatHistory`, `ChatItem`, `FileMeta`), `roster.rs`, `menu.rs`,
  `markdown.rs`, `images.rs`, `emoji.rs`.
- `i18n` — Fluent bundles; `audio`, `video` — codecs and devices.

Server config keys the user runs with are in the spec §1; the
CLI is `server user|role|invite|restriction|key|telemetry|backup|
restore|db check|db prune|purge` (`server role --help` etc.).

## 3. Shipped

| version | what |
|---|---|
| 1.1.0 | persistence thread, schema, invites and device keys, four built-in roles, restrictions, history with the ring, channels from the UI, telemetry, progress, admin CLI |
| 1.1.1 | direct messages with offline delivery, unread counts, paging, the conversation list |
| 1.1.2 | profiles (name color/style, avatars, contrast check), the app's own context menus with the volume slider, collapsible Messages, invite dialog, XP bar (`UserProgress`), share sounds, one instance per profile |
| 1.1.3 | custom roles with derived ranks, badges (icon + color) and `RoleList`; `panel`, `gold` and `admin-channel` as permissions; files on the blob path in General and DMs (cards, transfer ring, cancel, Open after save); the admin panel (eight tabs, confirm dialogs, humanized audit); resizable sidebar and Messages divider; hover on chat rows |
| 1.1.4 | blob retention, purge drops files, chat xp, pictures as files wherever the server is persistent, transfer budget (`limits.max_transfer_kb_per_minute[_global]`, old names alias), `server restore` / `db prune`, the admin channel (`AuditLine` feed), keybinds watched not grabbed, command output in the conversation it was typed in, closed DMs stay closed |

Memory-mode servers (persistence off) still use the legacy
`ImageMessage` relay for pictures and refuse files.

## 4. Rules learned the hard way

- `wait_for` closures must not borrow the client being pumped: copy
  `x.id` first. And `wait_for` does not consume (see §1).
- **A new wire variant needs a new feature bit** (spec §12): the
  first cut gated `UserProgress` on `PROGRESS`, which a 1.1.0 client
  negotiates too — it would have choked on the variant. Now
  `PROGRESS_ALL` and `ROLE_LIST` (bits 11 and 12).
- `app-core`'s chat test helper multiplied a placeholder id near
  `u64::MAX` by 1000: fine in release, an overflow panic under
  `cargo test` (debug assertions). `saturating_mul` now.
- Tests about media connect people with `connect_member`; `connect`
  makes guests, who cannot share, upload, or DM.
- A CLI change reaches a running server within 2 s (`data_version`
  poll) and is applied to live sessions (bans close, kicks close,
  roles refresh).
- `/kick` forgets keys and roles (needs a new invite); `/ban` is the
  restriction; a ban never catches the actor or an equal rank, and
  skips an address someone else is on.
- Chat earns 1 xp per message since 1.1.4 (at the minute flush); an
  image counts as a chat message for mutes and
  the rate limit.
- `/help` is built from `Session.permissions`; never advertise a
  command the server would refuse — the roster menu mirrors the rank
  rule per row for the same reason.
- Regenerating an identity makes a new user; guests can repeat a
  member-free name, so ghosts pile up — `server user purge` (or an id
  prefix in any CLI `<who>`) handles them.
- `EditChannel` replaces every field, and the server never sends a
  channel's password back: an edit must resend it, so the client
  keeps passwords it created or joined with in `channel_passwords`
  and `current_spec` reads them from there. A password the client
  never saw is cleared by any edit — the dialog says so.
- Only text chat enters the server's edit/delete ring (200 most
  recent); images cannot be edited or deleted; the server is
  *silent* when it refuses an edit or delete — the client's 3 s
  timer is the only feedback.
- `UserDetails` shipped with four positional fields, so the profile
  became its own variant (`UserProfile`), not a fifth field (§12).
- The server type is `ChatServer`; `Config` nests `avatars` under
  `persistence`; the client re-exports `protocol` as a module.
- `ContextMenuArea.enabled`, `if`/`for` inside `Menu`, the `Timer`
  element, `Window.full-screen`, and `absolute-position` all exist
  in Slint 1.17; `StyledText` cannot be selected (hence the plain
  view toggle); Slint maps ⌘ to `control` and physical Ctrl to
  `meta` on macOS; a layout child's `y` is ignored — wrap it in a
  plain `Rectangle` and center there; a `ListView` wants only `for`
  children (use `ScrollView` + layout for short mixed lists); ids
  inside `if` are not visible outside it.
- Slint globals are per window instance: `Theme.scale`, `Tr`, etc.
  must be set on every window (`apply_text_scale`).
- `invoke_from_event_loop` closures must be `Send`: never capture
  `self` (the windows) — read what you need from the closure's own
  `with_current(|app| …)`.
- Row ids for chat rows come from `AppState::next_row_id`, unique
  across conversations, because images and GIFs are keyed by them and
  conversations swap models.
- **Verified in Slint 1.17.1's sources (crates.io), not guessed:**
  `i-slint-backend-selector` enables `i-slint-backend-winit/muda` on
  macOS and Windows whenever `backend-winit` is on, and
  `show_native_popup_menu` always tries muda first — so
  `ContextMenuArea` is native there and there is no runtime opt-out.
  Mouse dispatch is front-to-back among siblings; the first item to
  accept wins and the parent's own `input_event` only runs when every
  child ignored. `TouchArea` returns `GrabMouse` on *any* button, so a
  row's TouchArea swallowed the right-click before the enclosing
  `ContextMenuArea` saw it — why right-clicking a channel header did
  nothing (user rows only worked because they left-clicked into
  `menu.show()`). `TextInput` ignores every button but the left one
  (a `TouchArea` declared first, under a read-only body, gets the
  right-click). `Flickable` intercepts only the left button.
  `StyledText` accepts every press and move (`shared-parley`, on by
  default), so a right-click on a formatted body opens nothing — the
  header, gutter and margin do — and the double-click-to-select
  TouchArea under it never fires; both pre-existing.
- **"Slint: long callback/instantiation chain detected"** is
  `ensure_tree_instantiated` giving up after 10 rounds of
  instantiate-then-run-change-handlers. A `changed viewport-height`
  handler on the chat ListView stacked into it while scrolling (every
  virtualization refinement is a round, and the `if`-gated delegates
  add a nested one per row). The owed follow is now a bounded 16 ms
  `Timer` (`follow-ticks` ≤ 60) instead. Rule: no `changed` handler
  on a property the ListView rewrites while scrolling.
- **The horizontal scroll, found by bisecting renders:** a
  `ListView` sets its `viewport-width` to the max of its rows'
  *minimum* widths (`listview_layout` returns
  `layout_info(Horizontal).min`), and a row's minimum merges every
  unpositioned child — `visible: false` ones too. The delegate hosted
  all five row components per row, and `DayMarkerRow`'s pill was
  `width: label.preferred-width + …` with the *message body* as its
  label, so every long message demanded a pill's worth of width and
  the list grew a horizontal scrollbar (the 2 px thumb above the
  composer in every screenshot, sideways drift on a trackpad). Fixed
  twice over: the delegate instantiates only the matching kind with
  `if` (a conditional child contributes no constraint — which also
  means a bisect built on `if` variants proves nothing, learned the
  hard way), and the pill is capped at 80 % of the row with an
  elided label. A wrapping `TextInput`/`Text` reports `min: 0`, so
  bodies never did this; word-wrap also breaks a 180-character token
  at character boundaries.
- The DM ✕ does nothing because `dm-t` (the whole-row TouchArea) is
  declared after the layout holding the button and sits on top of it;
  the scrollbar collision is the `ScrollView`'s 14 px bar over a row
  with 4 px right padding.
- The chat scrollbar "sticks at the bottom": `changed viewport-height
  => scroll-to-end` while pinned; dragging the thumb near the bottom
  instantiates virtualized rows, the height refines, the pin snaps
  back. The fix is a host-driven follow (set on append) rather than a
  reaction to every geometry change.
- An XP bar needed a wire change: `UserDetails` carries `level`
  only, and `ProgressUpdate { xp, level }` goes to the user themself
  on level-up. `UserProgress` (spec §12) now carries everyone's xp.

### Memory (the "RAM tripled" pass)
What the app retains is now measurable: **`/mem`** prints (and logs)
the process figure beside every bucket the app holds — chat rows
across conversations, picture previews (texture bytes from
`Image::size()`), compressed image bytes, GIF players, decoded
avatars, roster sizes, open viewers. Ask for that first next time;
the renderer's own caches and allocator slack are whatever is left
over. Already bounded before this pass: Skia runs on its raster
backend (the GPU build cost ~150 MB), chat previews are downscaled,
GIFs stream one frame, evicted rows drop their entries, the video
decoder recycles its RGBA buffer. Trimmed in this pass: the send
preview and the gallery kept their **full-resolution** decode in a
`State` image property after closing (a 4K screenshot = 32 MB until
the next one) — both clear it now; chat preview textures follow the
window's scale factor (`preview_max_edge`: 360² on a 1× display,
720² only at 2×, a quarter of the memory per picture for most
Windows users); avatars from other clients are clamped to 128² at
decode (ours already are at pick time). Not done, and why: pooling
`SharedPixelBuffer`s for the stream viewer needs the buffer back
from a `slint::Image`, which the API does not give — the one 8 MB
allocation per 1080p frame stays. Left to measure: the `StyledText`
layouts of visible rows and Skia's glyph cache with color emoji,
both only visible from a heap profile.


## 5. Remaining work

Everything in the spec except its "later" list is done. That list:
channel topics, reserved names, a manual away toggle shown in the
roster, animated avatars, anything built on progress (a leaderboard,
level-gated invites), `Auth::Account` only if a server ever wants
people rather than devices. Plus:

- **Hands-on passes the sandbox cannot do:** the watched keybinds on
  each OS (Wayland and macOS-without-accessibility fall back to
  grabs), the single-instance guard, the tray, `server restore` on a
  real backup, an image over 10 MiB (stays a card with Save).
- `ServerLimits` on the wire so the `+` can grey at the per-user
  channel cap.
- Scroll anchoring on "load earlier".
- Arrow-key navigation in the context menu.

## 6. Rough edges (known, not urgent)

- No scroll anchoring on "load earlier": the list stays at the top,
  showing the oldest of the new page.
- The chat's horizontal scrollbar is left at `as-needed` on purpose:
  with nothing to scroll it never shows, and if a future row inflates
  the list again the 2 px thumb above the composer is the canary.
- The `+` (new channel) cannot grey at the per-user cap: the cap is
  not on the wire (a `ServerLimits` message would fix it).
- DM unread clears when the DM is open even if the window is in the
  background (no focus tracking).
- Dialog `Field`s have no explicit select-all (no FocusScope); add
  one if ⌘A turns out flaky there.
- `profile_locked` on the user row is unused (the lock is the
  restriction). `chat-dm-received` and `nickname-applies-later`
  strings are unused.
- The context menu has no arrow-key navigation yet (Escape and click
  outside only); the native one had it. The hover cursor over links
  stays blocked under a `StyledText` (the row highlight works: the
  row's TouchArea is the content's parent).
- `Tr` properties nothing reads: `ptt-tooltip`, `audio-devices-header`,
  `audio-system-default`, `keybinds-header`, `keybind-record` (older
  leftovers, left alone).


## 7. Site screenshots

`web.tar.gz` holds the site; its `img/*.png` were rendered from the
real UI at 2× with the fixtures in `site/*.json` (call, popover,
prompt, connect, settings-servers, settings-audio; `share.png` is the
viewer chrome composited over `call.png` with a drawn scene, since the
viewer cannot play a stream headless). After a UI change, re-render
with the commands in §1 and replace the files.
