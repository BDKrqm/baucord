# baucord — state and next steps

## Where things stand

The 1.0 line is in production. What the last sessions established:

- The relay holds RTT at the physical floor under an 8–11 Mbit/s
  fan-out (median 2 ms on a LAN, 37–51 ms on the internet, p95 within
  5 ms of the median) with zero kernel drops at any layer.
- Voice loses what the network loses and nothing more: 47 packets in
  ~122,000 over an hour with eight users, three of them sharing.
- A viewer whose downlink can't carry a share loses video groups and
  keeps its voice; nobody else's stream degrades because of it.
- Sessions survive blackouts under 30 s; longer ones, and server
  restarts, reconnect on their own within seconds.
- Every incident so far traced to the user's own network (flat RTT,
  rising loss). The server has logged zero warnings in production.

Test coverage: 170 tests, of which 40 run the transport and server
through the simulated network. The desktop crate is exercised by hand
only.

## Next steps, by value

### Reliability (do first)

- [ ] **Find the silent crash.** One Windows client died twice under a
      dead uplink with `panic = "abort"` and no stderr. The panic hook
      now writes the location to `baucord.log`; ship it, wait for the
      next occurrence, fix the line. Then decide whether the media
      threads should run under `panic = "unwind"` + `catch_unwind` so a
      capture/encoder fault becomes "share stopped" instead of an exit.
- [ ] **Tune the finish-ack deadline from evidence.** The client now
      logs `video uplink: abandoned N/M (superseded/deadline/unacked)`.
      If a sharer's resets are mostly `unacked`, loosen the
      500 ms + 2·RTT deadline; if `superseded`, make the ladder hold a
      lower rung longer. One sharer log is enough to decide.
- [ ] **AMF oversize frames.** `h264_amf` occasionally emits frames
      over the 275 KB chunk cap (255 × 1080 B); the frame is dropped and
      an IDR forced, which AMF cannot deliver promptly, giving viewers a
      1–2 s freeze. Either a tighter VBV for AMF or a wider chunk index.
- [ ] Windows/macOS CI for the client crate's unit tests (the sim
      suites are platform-independent and already cover the server).

### Product (high leverage, moderate effort)

- [ ] **Chat history for late joiners.** Messages already carry
      server-assigned `id` and `ts`; keep the last N on the server and
      replay on `Init`. Images by hash (already computed) so a replay
      doesn't re-send bytes the client may have.
- [ ] **Identity-changed dialog.** Today the user edits the pin in
      Settings; an accept/reject prompt showing both fingerprints is the
      expected UX.
- [ ] **"Send my log" button.** The client can already upload its log
      (`NetworkControl::SendLog`, server stores under
      `<identity.dir>/logs/`); nothing in the UI triggers it yet.
- [ ] **Connection quality indicator** in the sidebar from the receiver
      reports and `groups behind` — the data exists, the pixel doesn't.
- [ ] **Push-to-talk.** Reserved in settings; the global hotkey
      machinery is in place.
- [ ] **Per-channel speaking indicators** (today they're global).
- [ ] Paste an invite string straight from the connect screen (the
      settings field accepts one; the connect screen should too).

### Users and moderation (needs the user id)

- [ ] The server derives a stable `user_id` from each client's key and
      only logs it. Next: show it in the roster tooltip, remember
      per-user volume/mute on the *server* side too, and add operator
      commands — kick, ban by user id, channel ownership. A `roles`
      section in `server.yaml` is the obvious shape.
- [ ] Per-user passwords or invites as an alternative to the single
      room secret, once user ids are first-class.

### Video (bigger)

- [ ] **Simulcast or per-viewer rungs.** One slow viewer today drops
      groups; a second encoded rung (720p30) that the relay picks per
      viewer from `groups behind` would keep them watching. The group
      model makes switching cheap: it's just which stream the relay
      opens on the viewer's track.
- [ ] **Relay-side keyframe cache**: a new subscriber gets the last
      keyframe group immediately instead of asking the sharer for an
      IDR (spares the sharer's uplink and the other viewers a forced
      keyframe).
- [ ] Hardware decode on macOS (VideoToolbox is compiled into the
      static FFmpeg) and Windows (NVDEC/D3D11VA) — decode is ~3 ms/frame
      in software today, fine at 1080p60 on a modern CPU, not on a
      laptop on battery.
- [ ] Persistent decoder across keyframes on the viewer (a decoder is
      recreated per keyframe today; harmless, wasteful).

### Server and operations

- [ ] Metrics export (Prometheus text endpoint or a `metrics:` log
      line): admitted/refused, per-client RTT/loss, groups behind,
      relay bytes. The counters exist in `LinkStats` and the relay
      stats.
- [ ] Hot-reload more than `channels` (limits, logging level).
- [ ] QUIC address validation (`Retry`) under load — quinn supports it;
      turn it on if handshake floods ever matter.
- [ ] `docker-compose.yml`: `stop_grace_period: 3s`, and a healthcheck
      once there's a cheap probe (a `--check` subcommand that binds and
      exits would do).
- [ ] A load test in the simulator: 200 clients, 5 sharers, mixed link
      specs, assert voice loss and RTT distribution — the harness can do
      it today; nobody has written the scenario.

### Housekeeping

- [ ] Bump `CLIENT_VERSION` on every client release so the server's
      admission line identifies builds.
- [ ] `audio` has a handful of clippy `deny` findings on its Linux
      paths; fix them so `cargo clippy --all-targets` is clean on every
      platform, not just the dev's.
- [ ] Fuzz `protocol::frame::Decoder` and the DER walker in
      `transport::der` (both parse untrusted bytes; both are small).
- [ ] Decide the successor-key retention (2 years today) and document
      the rotation runbook next to `setup.sh`.

## Non-goals (for now)

- End-to-end encryption of media. The relay must read packets to route
  and validate them; E2EE would mean per-channel keys and a different
  trust model. Worth a design doc before any code.
- A web client. QUIC datagrams from a browser are WebTransport, which
  would need a second transport implementation and public certificates.
