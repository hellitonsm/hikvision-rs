# HCNetSDK Rust Demo — Documentação do Programa

## Visão Geral

Demonstração de integração com câmeras Hikvision usando o SDK HCNetSDK em Rust. O programa lista câmeras de um DVR/NVR, faz login no dispositivo, abre preview de vídeo em janela X11 externa e suporta alarmes em tempo real.

### Fluxo Principal

```
Carregar device_tree.txt → Login no dispositivo → Listar canais → Preview de vídeo
```

---

## Arquitetura

```
src/
├── main.rs                  # Entry point, UI wiring, callbacks
├── realplay/
│   ├── mod.rs               # RealPlay: ciclo de vida do preview (start/stop)
│   └── x11wnd.rs            # PreviewWindow: janela X11 para renderização SDK
├── sdk/
│   ├── mod.rs               # Façade SDK: init, login, logout, cleanup
│   ├── hcnetsdk.rs          # FFI bindings (libhcnetsdk + libPlayCtrl)
│   ├── loader.rs            # Carregamento dinâmico das .so via libloading
│   └── callbacks.rs         # Bridge C callbacks → Rust channels (alarmes/exceptions)
├── mainwindow/
│   ├── devicetree/
│   │   ├── data.rs          # DeviceData, ChannelData, ProtocolType, StreamType
│   │   ├── storage.rs       # Leitura/escrita do device_tree.txt
│   │   └── mod.rs           # TreeNode enum (Root/Device/Channel)
│   └── logalarm/            # Estado de logs e alarmes
└── public/module.rs         # Trait Module para agrupamento de submódulos

ui/
├── main.slint               # Janela principal (tabs, device tree, log panel)
├── realplay.slint           # Página de preview (canal, stream, botões start/stop)
├── app_state.slint          # Estado compartilhado UI (AppState global)
└── ...                      # Outras páginas (playback, configure, manage)
```

---

## Dependências

| Crate | Uso |
|-------|-----|
| `x11rb` | Criação e gerenciamento de janela X11 para o SDK renderizar vídeo |
| `libloading` | Carregamento dinâmico de `libhcnetsdk.so` e `libPlayCtrl.so` |
| `slint` | Framework UI (main window, dialogs, realplay page) |
| `libc` | Tipos C para FFI |
| `once_cell` | Globais estáticos lazy (handles SDK, event channels) |
| `parking_lot` | Mutex leve para receiver de eventos |
| `chrono` | Timestamps no status bar |
| `serde` | Serialização do device tree |

### Bibliotecas Nativas (SDK Hikvision)

O SDK deve estar em `Linux64/lib/` relativo à raiz do workspace:

```
newqtdemo/
├── Linux64/lib/
│   ├── libhcnetsdk.so       # SDK principal (login, preview, alarmes)
│   ├── libPlayCtrl.so       # Decoder/renderer de vídeo
│   ├── libAudioRender.so    # Renderização de áudio
│   └── libSuperRender.so    # Renderização de vídeo (overlay)
└── rust/
    ├── src/
    └── ui/
```

Alternativamente, defina `HCNETSDK_LIB_DIR` como variável de ambiente.

---

## Ciclo de Vida do Preview

### 1. Início (Start)

```
Usuário clica "Start preview"
  → RealPlayPage.start(channel, stream, protocol)     [ui/realplay.slint]
    → MainWindow.realplay_start(ch, stream, proto)     [ui/main.slint]
      → ui.on_realplay_start callback                  [src/main.rs]
        → RealPlay::start(uid, ch, stream_type, link_mode)  [src/realplay/mod.rs]
```

Dentro de `RealPlay::start()`:

1. Cria `PreviewWindow` (janela X11 via x11rb)
2. Configura `NET_DVR_PREVIEWINFO`:
   - `lChannel` — número do canal
   - `dwStreamType` — 0=Main, 1=Sub, 2=Third, 3=Trans, 4=Fourth
   - `dwLinkMode` — 0=TCP, 1=UDP, 2=MULTICAST, 3=RTP, 4=RTSP, 5=HTTPS
   - `hPlayWnd` — ID da janela X11
   - `bBlocked` — 1 (bloqueante)
   - `dwDisplayBufNum` — 1
3. Chama `NET_DVR_RealPlay_V40()` — SDK renderiza vídeo diretamente na janela X11
4. Armazena `PreviewWindow` em `self.preview_wnd`

### 2. Execução

O timer principal (100ms) chama `RealPlay::poll_window_events()` que processa eventos X11:
- `WM_DELETE_WINDOW` — usuário clicou no X da janela → para o preview
- `DestroyNotify` — janela destruída externamente → para o preview
- `UnmapNotify` / `MapNotify` — controle de estado mapped/unmapped

### 3. Parada (Stop)

```
Usuário clica "Stop" ou fecha a janela X11
  → RealPlay::stop()
    → NET_DVR_StopRealPlay(real_handle)   # Para o stream SDK
    → self.preview_wnd = None              # Drop destrói a janela X11
```

---

## Autenticação e Criptografia

### Login

```rust
sdk::login(ip, port, username, password, secret_key) -> Result<(user_id, DeviceInfo), String>
```

1. Preenche `NET_DVR_USER_LOGIN_INFO` com IP, porta, usuário, senha
2. Chama `NET_DVR_Login_V40()` — retorna `user_id` e `DeviceInfo`
3. Se `secret_key` não está vazio, chama `NET_DVR_SetSDKSecretKey(user_id, key)` — **chave de descriptografia para câmeras com criptografia habilitada**
4. `DeviceInfo` contém: serial, número de canais analógicos, IP, canal inicial, zero channels

### Chave de Criptografia (Secret Key)

Câmeras Hikvision com criptografia habilitada exigem uma chave de 16 caracteres (AES-128). O fluxo é:

```
Login com senha correta
  → Se a câmera tem criptografia:
    → NET_DVR_SetSDKSecretKey(user_id, "16-char-secret-key")
    → O SDK usa essa chave para descriptografar o stream de vídeo
  → Se a chave estiver errada ou ausente:
    → O preview pode abrir mas exibe vídeo preto/criptografado
    → Ou NET_DVR_RealPlay_V40 falha com erro
```

O campo `secret_key` é armazenado por dispositivo no `device_tree.txt`.

### Logout

```rust
sdk::logout(user_id)  // Chama NET_DVR_Logout_V30
```

---

## Formato do Device Tree (device_tree.txt)

```
<device>
Nome do Dispositivo
192.168.1.100
8000
admin
senha12345
<channel>
Camera1
1
0
0
</channel>
<channel>
Camera2
2
0
0
</channel>
</device>
```

### Campos do Dispositivo

| Campo | Descrição |
|-------|-----------|
| name | Nome exibido na árvore |
| ip | Endereço IP do DVR/NVR |
| porta | Porta (padrão 8000) |
| user | Usuário (padrão "admin") |
| password | Senha de acesso |
| secret_key | Chave de descriptografia (16 chars, vazio se não usar) |

### Campos do Canal

| Campo | Descrição |
|-------|-----------|
| name | Nome da câmera |
| number | Número do canal (1-255) |
| protocol | 0=TCP, 1=UDP, 2=MULTICAST, 3=RTP, 4=RTSP, 5=HTTPS |
| stream | 0=Main, 1=Sub, 2=Third, 3=Trans, 4=Fourth |

---

## Mapeamento Stream/Protocol (UI → SDK)

### Stream Type (dwStreamType)

| UI | Valor SDK | Descrição |
|----|-----------|-----------|
| Main stream | 0 | Stream principal (alta resolução) |
| Sub stream | 1 | Stream secundário (baixa resolução) |
| Third stream | 2 | Terceiro stream |
| Trans code | 3 | Stream transcodificado |
| Fourth stream | 4 | Quarto stream |

### Link Mode (dwLinkMode)

| UI | Valor SDK | Descrião |
|----|-----------|----------|
| TCP | 0 | Conexão TCP (padrão, confiável) |
| UDP | 1 | Conexão UDP (menor latência) |
| MULTICAST | 2 | Multicast (rede local) |
| RTP | 3 | RTP sobre UDP |
| RTSP | 4 | RTSP (Real Time Streaming Protocol) |
| HTTPS | 5 | HTTPS (criptografado) |

---

## Callbacks e Alarmes

O SDK usa callbacks C para notificar eventos. Estes são convertidos para Rust channels em `src/sdk/callbacks.rs`.

### Tipos de Evento

```rust
enum SdkEvent {
    Alarm { kind: AlarmKind, device_ip: String, device_name: String },
    Exception { kind: ExceptionKind, device_ip: String },
}
```

### Alarmes Suportados

| Tipo | Descrição |
|------|-----------|
| SignalInput | Entrada de alarme |
| DiskFull | Disco cheio |
| SignalLost | Sinal perdido |
| MotionDetect | Detecção de movimento |
| DiskFormat | Formatação de disco |
| DiskReadWriteErro | Erro de leitura/escrita |
| NetDisconnect | Desconexão de rede |
| IpConflict | Conflito de IP |
| IllegalAccess | Acesso ilegal |
| VideoSignalAbnormal | Sinal de vídeo anormal |
| RecordAbnormal | Gravação anormal |

### Exceções

| Tipo | Descrição |
|------|-----------|
| Network | Exceção de rede |
| Preview | Exceção no preview |
| PreviewReconnect | Reconectando preview |
| PreviewReconnectSuccess | Reconexão bem sucedida |
| Alarm | Exceção de alarme |
| Serial | Exceção na porta serial |
| VoiceTalk | Exceção no voice talk |

---

## Janela X11 (PreviewWindow)

O SDK renderiza vídeo diretamente em uma janela X11 nativa (não dentro da UI Slint). Isso é necessário porque o `libPlayCtrl` usa X11/DRI3 para aceleração de hardware.

### Criação

```rust
PreviewWindow::new() -> Option<PreviewWindow>
```

1. Conecta ao servidor X11 via `x11rb::connect(None)`
2. Cria janela 704x576 centralizada
3. Registra `WM_DELETE_WINDOW` para interceptar fechamento pelo WM
4. Mapeia (exibe) a janela

### Eventos Processados

| Evento | Ação |
|--------|------|
| `ClientMessage(WM_DELETE_WINDOW)` | Unmap + sinaliza fechamento |
| `DestroyNotify` | Sinaliza fechamento |
| `UnmapNotify` | Atualiza estado mapped=false |
| `MapNotify` | Atualiza estado mapped=true |

### Destruição

O `Drop` do `PreviewWindow` chama `destroy_window()` + `flush()`, liberando todos os recursos X11/DRI3.

---

## Erros Comuns

### `dri3_alloc_render_buffer` / `xcb_dri3_pixmap_from_buffer failed`

**Causa**: Bug do driver Mesa/DRI3 ao reutilizar buffers de janela X11 com conteúdo de vídeo.

**Solução**: Cada preview cria uma janela X11 nova. Ao parar, a janela é completamente destruída (não apenas escondida).

### Preview abre mas não mostra vídeo

**Possíveis causas**:
1. `secret_key` incorreta ou ausente para câmera criptografada
2. Canal inativo ou offline
3. Protocolo incorreto (ex: UDP quando câmera só aceita TCP)
4. Firewall bloqueando a porta

### Crash ao abrir segunda câmera

**Causa antiga**: Janela X11 era reusada entre previews com buffers DRI3 stale.

**Solução atual**: Cada `RealPlay::start()` cria uma `PreviewWindow` nova. Cada `stop()` destrói a janela completamente.

---

## Guia de Implementação

### Para criar um novo programa que lista câmeras e exibe vídeo:

#### 1. Inicialização do SDK

```rust
// Carregar bibliotecas nativas
let handles = sdk::loader::handles()?;

// Inicializar SDK
sdk::init()?;  // NET_DVR_Init + configurações

// Opcional: iniciar listener de alarmes
sdk::start_alarm_listener(callbacks::new_queue())?;
```

#### 2. Login no Dispositivo

```rust
let (user_id, device_info) = sdk::login(
    "192.168.1.100",  // IP
    8000,              // Porta
    "admin",           // Usuário
    "senha12345",      // Senha
    "16-char-secret",  // Chave de criptografia (vazio se não usar)
)?;
```

#### 3. Listar Canais

```rust
// Canais analógicos: 1..device_info.by_chan_num
for i in 0..device_info.by_chan_num {
    let channel_number = device_info.by_start_chan as i32 + i as i32;
    println!("Canal {}: {}", channel_number, format!("Camera{}", channel_number));
}

// Canais IP: 33..(33 + device_info.by_ip_chan_num)
for i in 0..device_info.by_ip_chan_num {
    let channel_number = i as i32 + 33;
    println!("Canal IP {}: {}", channel_number, format!("IPCamera{}", i + 1));
}
```

#### 4. Abrir Preview

```rust
// Criar janela X11
let wnd = x11wnd::PreviewWindow::new().ok_or("Falha ao criar janela")?;
let hwnd = wnd.window_id();

// Configurar preview
let mut preview_info: NET_DVR_PREVIEWINFO = Default::default();
preview_info.lChannel = channel;        // Número do canal
preview_info.dwStreamType = 0;          // 0=Main, 1=Sub
preview_info.dwLinkMode = 0;            // 0=TCP, 1=UDP
preview_info.hPlayWnd = hwnd;           // Janela X11
preview_info.bBlocked = 1;              // Modo bloqueante
preview_info.dwDisplayBufNum = 1;       // Buffer count

// Iniciar preview
let real_handle = unsafe {
    NET_DVR_RealPlay_V40(user_id, &preview_info, null_mut(), null_mut())
};
if real_handle < 0 {
    return Err(format!("Erro: {}", unsafe { NET_DVR_GetLastError() }));
}
```

#### 5. Tratar Criptografia

Se a câmera usa criptografia, a `secret_key` deve ser fornecida no login:

```rust
// A chave é enviada após login bem sucedido
let key = std::ffi::CString::new(secret_key).unwrap();
unsafe { NET_DVR_SetSDKSecretKey(user_id, key.as_ptr()) };
```

Sem a chave correta, o preview pode:
- Falhar com erro
- Abrir mas mostrar vídeo preto
- Mostrar vídeo com artefatos

#### 6. Parar Preview

```rust
unsafe { NET_DVR_StopRealPlay(real_handle) };
// A janela X11 é destruída pelo Drop do PreviewWindow
```

#### 7. Cleanup

```rust
sdk::logout(user_id);
sdk::cleanup();  // NET_DVR_Cleanup
```

---

## Referência de Funções SDK Utilizadas

### libhcnetsdk

| Função | Descrição |
|--------|-----------|
| `NET_DVR_Init` | Inicializa o SDK |
| `NET_DVR_Cleanup` | Libera recursos do SDK |
| `NET_DVR_Login_V40` | Login no dispositivo |
| `NET_DVR_Logout_V30` | Logout |
| `NET_DVR_RealPlay_V40` | Inicia preview de vídeo |
| `NET_DVR_StopRealPlay` | Para preview |
| `NET_DVR_SetSDKSecretKey` | Define chave de criptografia |
| `NET_DVR_GetLastError` | Retorna código do último erro |
| `NET_DVR_SetExceptionCallBack_V30` | Registra callback de exceções |
| `NET_DVR_StartListen_V30` | Inicia listener de alarmes |
| `NET_DVR_SetLogToFile` | Configura log em arquivo |

### libPlayCtrl

| Função | Descrição |
|--------|-----------|
| `PlayM4_GetPort` | Obtém porta de decoder |
| `PlayM4_OpenStream` | Abre stream para decoding |
| `PlayM4_Play` | Inicia reprodução |
| `PlayM4_Stop` | Para reprodução |
| `PlayM4_SetDecCallBack` | Registra callback de frames decodificados |
| `PlayM4_InputData` | Envia dados brutos para decoding |

---

## Estrutura de Dados Principal

```rust
// Dispositivo
struct DeviceData {
    name: String,
    ip: String,
    port: u16,
    user: String,
    password: String,
    secret_key: String,      // Chave de criptografia
    user_id: i32,            // -1 = não logado
    channels: Vec<ChannelData>,
}

// Canal
struct ChannelData {
    name: String,
    number: i32,             // Número do canal
    protocol: ProtocolType,  // TCP/UDP/MULTICAST/RTP/RTSP/HTTPS
    stream: StreamType,      // Main/Sub/Third/Trans/Fourth
    real_handle: i32,        // Handle do preview (-1 = parado)
    online: bool,
}

// Informações do dispositivo (retornadas no login)
struct DeviceInfo {
    serial: String,
    by_chan_num: u8,         // Número de canais analógicos
    by_ip_chan_num: u8,      // Número de canais IP
    by_start_chan: u8,       // Número do primeiro canal
    by_zero_chan_num: u8,    // Número de zero channels
}
```
