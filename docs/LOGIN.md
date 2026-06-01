# Sistema de Login / Autenticação nas Câmeras

## Protocolo: HTTP Digest Access Authentication (RFC 2617)

A comunicação com as câmeras Hikvision usa a **API ISAPI** sobre HTTP com
autenticação **Digest MD5**. O fluxo é:

```
Cliente                                  Servidor (Câmera Hikvision)
   │                                           │
   │  GET /ISAPI/System/deviceInfo             │
   │──────────────────────────────────────────►│
   │                                           │
   │  401 Unauthorized                         │
   │  WWW-Authenticate: Digest                 │
   │    realm="Hikvision",                     │
   │    nonce="abc123...",                     │
   │    qop="auth",                            │
   │    opaque="def456..."                     │
   │◄──────────────────────────────────────────│
   │                                           │
   │  (computa MD5 hashes):                    │
   │    HA1 = MD5(user:realm:password)         │
   │    HA2 = MD5(GET:/ISAPI/System/deviceInfo)│
   │    cnonce = MD5(nonce:user)               │
   │    response = MD5(HA1:nonce:nc:cnonce:qop:HA2) │
   │                                           │
   │  GET /ISAPI/System/deviceInfo             │
   │  Authorization: Digest                    │
   │    username="admin",                      │
   │    realm="Hikvision",                     │
   │    nonce="abc123...",                     │
   │    uri="/ISAPI/System/deviceInfo",        │
   │    qop=auth, nc=00000001,                 │
   │    cnonce="...",                          │
   │    response="...",                        │
   │    opaque="..."                           │
   │──────────────────────────────────────────►│
   │                                           │
   │  200 OK  (XML com DeviceInfo)             │
   │◄──────────────────────────────────────────│
```

## Componentes do código

| Arquivo | Papel |
|---------|-------|
| `src/api.rs` | Cliente HTTP com Digest Auth (`HikvisionAPI`). Contém `compute_digest()`, `parse_digest_params()`, parsers XML. |
| `src/main.rs` | Interface gráfica (egui). Coleta credenciais do usuário, chama `device_info()` e constrói URLs RTSP com senha inline. |
| `src/bin/test_auth.rs` | Binário de exemplo: lê `HOST`/`PORT`/`USER`/`PASS` de env vars e testa autenticação. |
| `src/bin/raw_test.rs` | Teste raw com `TcpStream`: implementa Digest manual para debug. |

## `HikvisionAPI` — Cliente principal

- **`new(host, port, user, password)`** — Cria o cliente. Configura `ureq` para não tratar 4xx como erro.
- **`device_info()`** — GET `/ISAPI/System/deviceInfo`. Dispara o handshake Digest na primeira chamada.
- **`channels()`** — GET `/ISAPI/Streaming/channels`. Lista os canais (IDs no formato `101`, `102`, `201`...).
- **`snapshot(cid)`** — GET `/ISAPI/Streaming/channels/{cid}/picture`. Retorna JPEG.

O cabeçalho `Authorization: Digest ...` é **cacheado** em um `RefCell<Option<String>>`.
Requisições subsequentes reutilizam o header sem passar pelo 401 novamente.
Se o servidor rejeitar, o cache é limpo e o handshake é refeito automaticamente.

## Credenciais

- **GUI** (`main.rs`): campos de texto na tela de login. Valores padrão: host=`192.168.5.75`, porta=`80`, usuário=`admin`.
- **test_auth.rs**: variáveis de ambiente `HOST`, `PORT`, `USER`, `PASS`.
- **raw_test.rs**: variáveis de ambiente `HOST`, `PORT`, `USER`, `PASS`.
- Nenhuma credencial é salva em disco.

## RTSP com credenciais inline

Após autenticar via HTTP, a URL do stream RTSP é montada com as credenciais
embutidas:

```
rtsp://admin:senha@192.168.5.75:554/Streaming/Channels/101
```

A senha é URL-encoded (`%XX`) para caracteres especiais. O streaming é feito
via FFmpeg (`ffmpeg_next`), que lida com a autenticação RTSP básica.

## Configuração do DVR

> ⚠️ Se a **Criptografia de Transmissão** (Verification Code) estiver ativada
> no DVR, o vídeo não carregará. Desative-a no menu de Rede do DVR.
