## Description

A lightweight voice and text chat application built with Rust, using `fltk` for the UI `renet` for network communication.

## Features

- **Voice Chat** with Voice Activity Detection (VAD)
- **Per-user audio controls** (mute/volume adjustment via right-click)
- **Ultra lightweight** - ~3.5MB RAM usage
- **Low latency** - 10ms audio frames for responsive voice chat
- **Cross-platform** - Works on Windows, Linux, and macOS

## Screenshots

![Client Interface 2](web/img/client2.png)

*Connect to server and nickname setup*

![Client Interface 1](web/img/client1.png)

*Main chat interface with user list,voice indicators and settings page*

## Building and Running

### Prerequisites

- Rust 1.87 or later
- CMake (for FLTK)

### Server

```bash
cargo run --bin server --release
```

The server starts on `127.0.0.1:5000` by default. Configuration can be adjusted in [config.yaml](config.yaml).

### Client
```bash
cargo run --bin client --release
```

Enter your nickname and server address (e.g., `127.0.0.1:5000`) to connect.

--
### What works

- Voice transmission starts automatically when you speak (VAD enabled)
- "Mute" to toggle your microphone
- Right-click on users to mute or adjust their volume individually
- Very basic resampling to work with different audio interfaces (its bad)
- Messages support up to 400 characters
- Audio settings via the ⚙ Settings button
- Configuration for VAD sensitivity, audio loopback, and default mute state
- Settings persist between sessions (stored in `~/.config/baucord/`, or `%APPDATA%/Roaming` in windows)