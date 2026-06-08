# Canal Zero — Implementação HCNetSDK

## Descoberta Principal

O Canal Zero (mosaico multi-câmera) do DVR Hikvision iDS-7216HQHI-M1/S1 **não é acessível via `NET_DVR_RealPlay_V40`** com `lChannel` — mesmo com `byZeroChanNum=1`, nenhum channel number candidato retorna o mosaic. Todos retornam erro 4 (`NET_DVR_CHANNEL_ERROR`) ou abrem câmeras individuais.

## Solução

`NET_DVR_RealPlaySpecial` com URL RTSP customizada.

### URL que funciona

```
rtsp://admin:<PASSWORD>@<DVR_IP>:554/Streaming/channels/001
```

- `link_mode = 4` (RTSP)
- Password **crua** (ex: `#minhaSenha`), **sem URL encoding** — o cliente RTSP interno do SDK não decodifica `%XX` antes de gerar o cabeçalho de autenticação.

### Prioridade de URLs testadas

1. `/Streaming/channels/001` — main stream, Canal Zero virtual
2. `/Streaming/channels/002` — sub stream, Canal Zero virtual
3. `/Streaming/channels/0?zeroChannel=1` — query param, fallback

### Estrutura de Fallback (`zero_channel_hcnetsdk.rs`)

- **[4a]** `RealPlaySpecial` com URLs RTSP (funcionou)
- **[4b]** Canais confirmados por `ZeroMakeKeyFrame`
- **[4c]** V30/V40 RTSP variations
- **[4d]** `RealPlaySpecial` com callback (sem janela)
- **[4e]** Candidatos gerais (standard channel loop)

## Structs FFI Adicionadas

- `NET_DVR_PREVIEWINFO_SPECIAL` — `sURL[1024]`, `dwLinkMode`, `hPlayWnd`, `bBlocked`, `dwDisplayBufNum`, `byRes[64]`
- `NET_DVR_CLIENTINFO` — `lChannel`, `lLinkMode` (bit31=sub), `hPlayWnd`, `sMultiCastIP`, `byProtoType`
- `NET_DVR_PREVIEWCFG_V30` — comando 1104
- `NET_DVR_ZEROCHANCFG` — comando 1102

## Funções SDK Adicionadas

- `realplay_special()` — `NET_DVR_RealPlaySpecial` com janela
- `realplay_special_with_callback()` — sem janela, callback de dados
- `realplay_v30_with_window()` — API V30 antiga, `NET_DVR_CLIENTINFO`
- `realplay_v30_with_window_rtsp()` — V30 com `byProtoType=1`
- `realplay_with_window_ex2()` — V40 com `link_mode` + `preview_mode`
- `realplay_with_window_ex3()` — V40 com `data_type` + `proto_type`
- `get_device_ability()` — `NET_DVR_GetDeviceAbility` via XML
- `get_dvr_config()` / `set_dvr_config()` — genérico `GetDVRConfig`
- `zero_make_key_frame()` — `NET_DVR_ZeroMakeKeyFrame`

## Comandos Úteis

```bash
cargo run --bin zero_channel_hcnetsdk -- \
  --host <DVR_IP> \
  --password 'senha \
  --verification-code 'código de descriptografia'
```

## Notas

- `#` na senha não precisa de URL encoding — o SDK envia cru para o DVR via digest auth.
- `handle=0` é válido no sucesso (sentinel precisa ser `-1`, não `0`).
- `ZeroMakeKeyFrame(1)` confirmou que channel 1 tem zero channel habilitado.
- `ZERO_PREVIEWCFG_V30` retornou mode=0 com canais [1..16] — o grid local do mosaico.
