# hikvision-rs

Visualizador RTSP para DVRs Hikvision com interface gráfica nativa (egui/eframe).

## Funcionalidades

- Conexão via API ISAPI (HTTP Digest Auth)
- Listagem automática dos canais do DVR
- **Múltiplos modos de streaming**:
  - RTSP direto (H.264/H.265 via FFmpeg)
  - Snapshot (polling JPEG via ISAPI - funciona com criptografia)
  - PlayCtrl (descriptografia com libPlayCtrl.so)
  - Canal Zero (stream multiplexado de múltiplas câmeras)
- Seleção entre stream principal e sub-stream
- **Visualização múltipla**: 1x1, 2x2, 3x3 e 4x4 (até 16 câmeras simultâneas)
- Sub-stream automático em modo multi-view
- Reconexão automática em caso de falha
- Interface gráfica com exibição em tempo real e contador de FPS

## Dependências

- [FFmpeg](https://ffmpeg.org/) (libavformat, libavcodec, libavutil, libswscale)
- Rust 2021 edition
- **Para descriptografia**: bibliotecas do SDK Hikvision (libPlayCtrl.so)

### Instalação do FFmpeg (Debian/Ubuntu)

```bash
sudo apt install libavformat-dev libavcodec-dev libavutil-dev libswscale-dev
```

### Bibliotecas do SDK Hikvision (para streams criptografados)

Para usar os modos **PlayCtrl** ou **Canal Zero** com criptografia ativada, você precisa da biblioteca proprietária `libPlayCtrl.so` do SDK Hikvision.

#### Onde obter

1. **Device Network SDK** (recomendado): Baixe o [SDK para Linux 64-bit](https://www.hikvision.com/en/support/download/sdk/) no site oficial da Hikvision
2. **LocalComponent**: Se você já usa o plugin web Hikvision, a biblioteca pode estar em:
   ```
   ~/.local/share/hikvision/weblocalserver/files/bin/libPlayCtrl.so
   ```

#### Instalação

Copie `libPlayCtrl.so` (e suas dependências Qt5) para um dos caminhos de busca:

```bash
# Opção 1: Diretório do projeto
mkdir -p hikvision-libs
cp libPlayCtrl.so hikvision-libs/

# Opção 2: Diretório de configuração do usuário
mkdir -p ~/.config/hikvision-rs
cp libPlayCtrl.so ~/.config/hikvision-rs/

# Opção 3: Sistema
sudo cp libPlayCtrl.so /usr/local/lib/
sudo ldconfig
```

#### Verificação

O aplicativo busca automaticamente a biblioteca nos seguintes locais (em ordem):

1. `./hikvision-libs/libPlayCtrl.so`
2. `~/.config/hikvision-rs/libPlayCtrl.so`
3. `~/.local/share/hikvision/weblocalserver/files/bin/libPlayCtrl.so`
4. `/usr/local/lib/libPlayCtrl.so`
5. `/usr/lib/libPlayCtrl.so`

Se não encontrar, você verá o erro: `libPlayCtrl.so not found in any search path.`

## Compilação

```bash
cargo build --release
```

## Uso

```bash
cargo run --release
```

1. Preencha os dados de conexão (host, porta HTTP, porta RTSP, usuário, senha)
2. **Escolha o modo de streaming**:
   - **RTSP direto**: streaming contínuo (requer criptografia desativada)
   - **Snapshot**: polling de JPEG (~2-3 FPS, funciona com criptografia)
   - **PlayCtrl**: descriptografia com libPlayCtrl.so (requer Verification Code)
   - **Canal Zero**: stream multiplexado (requer ativação no DVR + Verification Code)
3. Clique em **Connect**
4. **Modo 1x1**: clique em um canal na barra lateral para exibir em tela cheia
5. **Modo multi-view** (2x2, 3x3, 4x4): marque os canais desejados com checkbox para exibir em grade

## Modos de Streaming

### RTSP Direto
- **Protocolo**: RTSP/RTP com FFmpeg
- **FPS**: 25-30
- **Requisitos**: Criptografia de Transmissão **desativada** no DVR
- **Uso**: Melhor qualidade e fluidez quando criptografia não é necessária

### Snapshot (JPEG Polling)
- **Protocolo**: HTTP GET `/ISAPI/Streaming/channels/{cid}/picture`
- **FPS**: ~2-3 (configurável 100-2000ms)
- **Requisitos**: Nenhum
- **Vantagens**: ✅ Funciona com criptografia ativada, sem dependências extras
- **Desvantagens**: Baixo FPS, não é vídeo contínuo
- **Uso**: Monitoramento não crítico com criptografia ativada

### PlayCtrl (Descriptografia)
- **Protocolo**: RTP + descriptografia AES-256-CBC
- **FPS**: 25-30
- **Requisitos**: libPlayCtrl.so + Verification Code do DVR
- **Uso**: Streaming fluido com criptografia ativada

### Canal Zero (Channel Zero)
- **Protocolo**: RTSP multiplexado com descriptografia manual
- **FPS**: 25-30
- **Requisitos**: 
  - DVR com suporte a Canal Zero (verificado via `zeroChanNum` no deviceInfo)
  - Canal Zero ativado no DVR: Configurações > Visualização > Canal Zero
  - Verification Code
- **Vantagens**: Visualiza múltiplas câmeras em um único stream (economia de banda)
- **Uso**: Visualização em grid de múltiplas câmeras com menor consumo de banda

## Perfis de compilação

```bash
# Debug com dependências otimizadas (recomendado para desenvolvimento)
cargo build

# Release com LTO
cargo build --release
```

O perfil debug otimiza dependências (`opt-level = 2`) para melhor performance de decodificação sem sacrificar a experiência de desenvolvimento.
