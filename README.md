# hikvision-rs

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

> **🌐 Language:** English · [Português](LEIAME.md)

Native Linux GUI for viewing Hikvision DVR/NVR video streams. Supports
encrypted streams (Verification Code) via the Hikvision Device Network SDK,
multi-camera grid layouts, and Canal Zero.

## Features

- **Multiple streaming methods:** RTSP (FFmpeg), Snapshot (JPEG polling),
  PlayCtrl (SDK decryption), HCNetSDK (callback + PlayM4), HCNetSDK X11
  (direct X11 overlay)
- **Multi-camera layouts:** 1×1, 2×2, 3×3, 4×4 with auto sub-stream
  switching
- **Canal Zero (Channel Zero):** single RTSP mosaic stream from the DVR
- **HTTPS with TLS fingerprint pinning** (SHA-256 certificate verification)
- **Encrypted stream support** via AES-256-CBC + Hikvision SDK
- **Internationalization:** English and Portuguese
- **Persistent config** in `~/.config/hikvision-rs/config.json`

## Requirements

- Linux with X11
- Rust 1.75+
- FFmpeg development libraries:

```bash
sudo apt install libavformat-dev libavcodec-dev libavutil-dev \
                 libswscale-dev build-essential pkg-config libssl-dev
```

## Build & Run

```bash
git clone https://github.com/your-username/hikvision-rs.git
cd hikvision-rs
cargo build --release
cargo run --release
```

Or use the Makefile:

```bash
make release   # build
make run       # run (debug)
make run-release
sudo make install  # install to /usr/local/bin
```

## SDK Libraries (optional)

For encrypted streaming, download the Hikvision SDK and place the `.so`
files in `hikvision-libs/`:

```bash
./sdkdownload.sh
```

Or copy manually to any of these paths (searched in order):

- `hikvision-libs/` (project root)
- `~/.config/hikvision-rs/`
- `/usr/local/lib/`

## Usage

1. Launch the app
2. Enter DVR/NVR IP, port, username, password
3. Select a streaming method
4. Click **Connect**
5. Click a channel in the sidebar to start viewing

## Streaming Methods

| Method     | Encryption | FPS  | Dependency            |
|------------|------------|------|-----------------------|
| Snapshot   | Yes        | 2–3  | None                  |
| RTSP       | No         | 25+  | FFmpeg                |
| PlayCtrl   | Yes        | 15+  | libPlayCtrl.so        |
| HCNetSDK   | Yes        | 15+  | libhcnetsdk.so + PlayCtrl |
| HCNetSDK X11 | Yes      | 25+  | libhcnetsdk.so + X11  |

## Architecture

```
src/
├── main.rs                 # egui UI (login, viewer, multi-stream)
├── api.rs                  # ISAPI HTTP client (Digest auth, channels)
├── i18n.rs                 # English / Portuguese strings
├── rtsp.rs                 # FFmpeg H.264/H.265 → RGBA
├── snapshot_stream.rs      # JPEG polling → RGBA
├── playctrl.rs             # libPlayCtrl.so FFI bindings
├── playctrl_stream.rs      # RTSP + PlayCtrl decryption + decode
├── hcnetsdk.rs             # libhcnetsdk.so FFI bindings
├── hcnetsdk_multi_stream.rs    # SDK callback + PlayM4 per channel
├── hcnetsdk_x11_multi.rs       # SDK X11 overlay per channel
├── hcnetsdk_stream.rs          # Single-channel SDK X11
├── encrypted_stream.rs         # WebSocket encrypted stream protocol
├── netstream.rs                # libnet_stream.so FFI
├── x11_window.rs               # Standalone X11 preview window
└── x11_embed.rs                # X11 child windows embedded in egui
```

## Auxiliary Tools

Available under `cargo run --bin <name>`:

- `zero_channel_hcnetsdk` — Canal Zero exploration / testing
- `playctrl_proxy` / `jpeg_proxy` — stream relay proxies
- `decrypt_proxy` — NetStream-based decryption relay
- `latency_test` — RTSP latency measurement
- `sniff` — HTTP debug sniffer

## License

MIT
