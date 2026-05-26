# hikvision-rs

Visualizador RTSP para DVRs Hikvision com interface gráfica nativa (egui/eframe).

## Funcionalidades

- Conexão via API ISAPI (HTTP Digest Auth)
- Listagem automática dos canais do DVR
- Streaming RTSP com decodificação H.264/H.265 via FFmpeg
- Seleção entre stream principal e sub-stream
- Reconexão automática em caso de falha
- Interface gráfica com exibição em tempo real e contador de FPS

## Dependências

- [FFmpeg](https://ffmpeg.org/) (libavformat, libavcodec, libavutil, libswscale)
- Rust 2021 edition

### Instalação do FFmpeg (Debian/Ubuntu)

```bash
sudo apt install libavformat-dev libavcodec-dev libavutil-dev libswscale-dev
```

## Compilação

```bash
cargo build --release
```

## Uso

```bash
cargo run --release
```

1. Preencha os dados de conexão (host, porta HTTP, porta RTSP, usuário, senha)
2. Clique em **Connect**
3. Selecione um canal na barra lateral para iniciar o streaming

> ⚠️ Se a **Criptografia de Transmissão** (Verification Code) estiver ativada no DVR, o vídeo não carregará. Desative-a no menu de Rede do DVR.

## Perfis de compilação

```bash
# Debug com dependências otimizadas (recomendado para desenvolvimento)
cargo build

# Release com LTO
cargo build --release
```

O perfil debug otimiza dependências (`opt-level = 2`) para melhor performance de decodificação sem sacrificar a experiência de desenvolvimento.
