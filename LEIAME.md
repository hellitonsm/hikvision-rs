# hikvision-rs

[![Licença: MIT](https://img.shields.io/badge/licença-MIT-blue.svg)](LICENSE)

> **🌐 Idioma:** [English](README.md) · Português

Interface gráfica nativa Linux para visualização de streams de vídeo de
DVRs/NVRs Hikvision. Suporta streams criptografados (Verification Code)
via Hikvision Device Network SDK, layouts de múltiplas câmeras e
Canal Zero.

## Funcionalidades

- **Múltiplos métodos de streaming:** RTSP (FFmpeg), Snapshot (JPEG polling),
  PlayCtrl (descriptografia SDK), HCNetSDK (callback + PlayM4), HCNetSDK X11
  (overlay X11 direto)
- **Layouts multi-câmera:** 1×1, 2×2, 3×3, 4×4 com seleção automática de
  sub-stream
- **Canal Zero:** stream único RTSP com mosaico de múltiplas câmeras
- **HTTPS com verificação de impressão digital TLS** (SHA-256)
- **Streams criptografados** via AES-256-CBC + SDK Hikvision
- **Internacionalização:** Inglês e Português
- **Configuração persistente** em `~/.config/hikvision-rs/config.json`

## Requisitos

- Linux com X11
- Rust 1.75+
- Bibliotecas de desenvolvimento FFmpeg:

```bash
sudo apt install libavformat-dev libavcodec-dev libavutil-dev \
                 libswscale-dev build-essential pkg-config libssl-dev
```

## Compilar & Executar

```bash
git clone https://github.com/your-username/hikvision-rs.git
cd hikvision-rs
cargo build --release
cargo run --release
```

Ou usando o Makefile:

```bash
make release   # compilar
make run       # executar (debug)
make run-release
sudo make install  # instalar em /usr/local/bin
```

## Bibliotecas SDK (opcional)

Para streaming criptografado, baixe o SDK Hikvision e coloque os arquivos
`.so` em `hikvision-libs/`:

```bash
./sdkdownload.sh
```

Ou copie manualmente para qualquer um destes diretórios (buscados em ordem):

- `hikvision-libs/` (raiz do projeto)
- `~/.config/hikvision-rs/`
- `/usr/local/lib/`

## Uso

1. Inicie o aplicativo
2. Informe IP do DVR/NVR, porta, usuário e senha
3. Selecione um método de streaming
4. Clique em **Conectar**
5. Clique em um canal na barra lateral para começar a assistir

## Métodos de Streaming

| Método     | Criptografia | FPS  | Dependência            |
|------------|-------------|------|------------------------|
| Snapshot   | Sim         | 2–3  | Nenhuma                |
| RTSP       | Não         | 25+  | FFmpeg                 |
| PlayCtrl   | Sim         | 15+  | libPlayCtrl.so         |
| HCNetSDK   | Sim         | 15+  | libhcnetsdk.so + PlayCtrl |
| HCNetSDK X11 | Sim       | 25+  | libhcnetsdk.so + X11   |

## Arquitetura

```
src/
├── main.rs                 # Interface egui (login, visualizador)
├── api.rs                  # Cliente HTTP ISAPI (Digest auth, canais)
├── i18n.rs                 # Strings em Inglês / Português
├── rtsp.rs                 # FFmpeg H.264/H.265 → RGBA
├── snapshot_stream.rs      # JPEG polling → RGBA
├── playctrl.rs             # Bindings FFI libPlayCtrl.so
├── playctrl_stream.rs      # RTSP + descriptografia PlayCtrl
├── hcnetsdk.rs             # Bindings FFI libhcnetsdk.so
├── hcnetsdk_multi_stream.rs    # SDK callback + PlayM4 por canal
├── hcnetsdk_x11_multi.rs       # SDK overlay X11 por canal
├── hcnetsdk_stream.rs          # SDK X11 monocanal
├── encrypted_stream.rs         # Protocolo WebSocket criptografado
├── netstream.rs                # FFI libnet_stream.so
├── x11_window.rs               # Janela X11 de pré-visualização
└── x11_embed.rs                # Janelas X11 embutidas no egui
```

## Ferramentas Auxiliares

Disponíveis via `cargo run --bin <nome>`:

- `zero_channel_hcnetsdk` — Exploração / teste de Canal Zero
- `playctrl_proxy` / `jpeg_proxy` — Proxies de relay de stream
- `decrypt_proxy` — Relay de descriptografia via NetStream
- `latency_test` — Medição de latência RTSP
- `sniff` — Sniffer HTTP para depuração

## Licença

MIT
