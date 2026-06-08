# Análise: Por que o Qt demo funciona com criptografia e nosso código não

**Data:** 2026-06-01

## Problema

Nosso código Rust falha ao decodificar streams criptografados com erro:
```
PlayM4_SetSecretKey failed (error 32: PLAYM4_NOT_SUPPORT_DECODE)
```

O Qt demo oficial da Hikvision funciona perfeitamente com os mesmos dispositivos e mesma chave.

## Causa Raiz

### Qt Demo (funciona)
```cpp
// 1. Login no SDK
LONG userID = NET_DVR_Login_V40(&struLoginInfo, &struDeviceInfoV40);

// 2. Configura chave de descriptografia NO SDK (antes de qualquer preview)
char secretKey[16] = {0};
strncpy(secretKey, encryptionKey.c_str(), 16);
NET_DVR_SetSDKSecretKey(userID, secretKey);  // ← CHAVE AQUI

// 3. Inicia preview - SDK já descriptografa automaticamente
LONG handle = NET_DVR_RealPlay_V40(userID, &struPlayInfo, callback, NULL);
```

**Arquitetura:**
```
DVR → [stream criptografado] → HCNetSDK (descriptografa) → callback com dados limpos → PlayM4 (decodifica)
```

### Nosso código Rust (não funciona)
```rust
// 1. Conecta RTSP direto (sem SDK)
let stream = TcpStream::connect("192.168.5.75:554")?;

// 2. Recebe RTP criptografado
let encrypted_payload = receive_rtp();

// 3. Tenta descriptografar manualmente
let decrypted = aes_decrypt(encrypted_payload, key);

// 4. Tenta configurar chave no PlayM4 (tarde demais!)
playctrl.set_secret_key(port, verification_code)?;  // ← ERRO 32

// 5. Envia dados para PlayM4
playctrl.input_data(port, &decrypted)?;
```

**Arquitetura:**
```
DVR → [stream criptografado] → nosso código RTSP → descriptografia manual → PlayM4 → ERRO 32
```

## Diferenças Críticas

| Aspecto | Qt Demo | Nosso Código |
|---------|---------|--------------|
| **SDK usado** | HCNetSDK (libhcnetsdk.so) | Nenhum (RTSP direto) |
| **Função de chave** | `NET_DVR_SetSDKSecretKey(userID, key)` | `PlayM4_SetSecretKey(port, key)` |
| **Quando configura** | APÓS login, ANTES de preview | APÓS abrir stream |
| **Quem descriptografa** | SDK (automático) | Código manual (AES-CBC) |
| **Entrada do PlayM4** | Dados já descriptografados | Dados criptografados ou mal descriptografados |

## Por que `PlayM4_SetSecretKey` não funciona

`PlayM4_SetSecretKey` é para **arquivos de vídeo criptografados salvos localmente**, não para streams RTSP ao vivo.

O erro 32 (`PLAYM4_NOT_SUPPORT_DECODE`) indica que o PlayM4 recebeu dados que não consegue decodificar:
- Pode ser porque ainda estão criptografados
- Pode ser porque nossa descriptografia manual está incorreta
- Pode ser porque falta metadados que o SDK adiciona

## Soluções

### Opção 1: Implementar bindings para HCNetSDK (recomendado)
```rust
// Criar bindings FFI para:
// - NET_DVR_Init()
// - NET_DVR_Login_V40()
// - NET_DVR_SetSDKSecretKey()
// - NET_DVR_RealPlay_V40() com callback
// - NET_DVR_StopRealPlay()
// - NET_DVR_Logout()
// - NET_DVR_Cleanup()
```

**Vantagens:**
- Funciona exatamente como o Qt demo
- SDK cuida de toda a descriptografia
- Suporte oficial da Hikvision

**Desvantagens:**
- Precisa de libhcnetsdk.so (mais uma dependência)
- Mais complexo que RTSP direto

### Opção 2: Melhorar descriptografia manual
Investigar exatamente como o SDK descriptografa:
- IV (Initialization Vector) correto
- Padding correto
- Possível header/metadata adicional

**Desvantagens:**
- Engenharia reversa
- Pode quebrar em futuras versões do firmware

### Opção 3: Usar apenas Snapshot mode (atual)
Já funciona perfeitamente com criptografia ativada.

**Desvantagens:**
- Baixo FPS (~2-3)
- Não é vídeo contínuo

## Arquivos Relevantes

### Qt Demo
- `newqtdemo/src/qtclientdemo.cpp:908-909` - Configura chave após login
- `newqtdemo/src/RealPlay/realplay.cpp:655` - Inicia preview
- `newqtdemo/includeCn/HCNetSDK.h` - Header do SDK

### Nosso Código
- `src/playctrl_stream.rs` - Tentativa de usar PlayM4_SetSecretKey
- `src/playctrl.rs` - Wrapper da libPlayCtrl.so
- `src/encrypted_stream.rs` - Descriptografia manual AES

## Conclusão

Para ter streaming fluido com criptografia ativada, precisamos usar o **HCNetSDK** completo, não apenas o PlayCtrl. O PlayCtrl sozinho não é suficiente porque ele espera receber dados já descriptografados pelo SDK.

A abordagem correta é:
```
HCNetSDK (login + chave + preview) → callback com dados limpos → processar frames
```

Não:
```
RTSP direto → descriptografia manual → PlayM4 → erro
```
