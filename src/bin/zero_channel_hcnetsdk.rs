//! Teste do Canal Zero via HCNetSDK com janela X11.
//!
//! Abre uma janela X11 e usa o modo overlay do SDK (realplay_with_window)
//! para renderizar o vídeo do Canal Zero diretamente — sem PlayM4.
//!
//! Tenta automaticamente múltiplos channel numbers candidatos baseados
//! no device info. Se todos falharem, tenta com LINK_RTSP.
//!
//! Uso:
//!   zero_channel_hcnetsdk \
//!     --host 192.168.1.100 \
//!     --password senha \
//!     --verification-code ABC123 \
//!     [--user admin] \
//!     [--sdk-port 8000] \
//!     [--zero-channel-number 65] \
//!     [--link-mode 0]

use anyhow::{Context, Result};
use hikvision_rs::hcnetsdk;
use hikvision_rs::hcnetsdk::{
    HCNetSDK, NET_DVR_DEVICEINFO_V30, STREAM_MAIN, STREAM_SUB,
    LINK_TCP, LINK_RTSP, DEVICE_ABILITY_INFO, DEVICE_DYNCHAN_ABILITY,
    NET_DVR_GET_ZEROCHANCFG, NET_DVR_GET_ZERO_PREVIEWCFG_V30,
    NET_DVR_ZEROCHANCFG, NET_DVR_PREVIEWCFG_V30,
};
use hikvision_rs::x11_window::PreviewWindow;
use std::time::{Duration, Instant};

struct Args {
    host: String,
    sdk_port: u16,
    user: String,
    password: String,
    verification_code: Option<String>,
    library_path: Option<String>,
    zero_channel_number: Option<i32>,
    link_mode: u32,
}

fn print_usage() {
    eprintln!("Teste do Canal Zero via HCNetSDK com janela X11");
    eprintln!();
    eprintln!("Uso:");
    eprintln!("  zero_channel_hcnetsdk --host <DVR_IP> --password <PASS> [opcoes]");
    eprintln!();
    eprintln!("Obrigatorio:");
    eprintln!("  --host              IP do DVR/NVR");
    eprintln!("  --password          Senha de login do DVR");
    eprintln!("  --verification-code Verification Code (para descriptografia)");
    eprintln!();
    eprintln!("Opcional:");
    eprintln!("  --user              Usuario (default: admin)");
    eprintln!("  --sdk-port          Porta SDK (default: 8000)");
    eprintln!("  --library-path      Caminho customizado para libhcnetsdk.so");
    eprintln!("  --zero-channel-number Canal zero manual (ex: 65)");
    eprintln!("  --link-mode         0=TCP(default), 4=RTSP");
}

fn parse_args() -> Option<Args> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 || args.contains(&"--help".to_string()) || args.contains(&"-h".to_string()) {
        return None;
    }

    let mut host = None;
    let mut sdk_port = 8000u16;
    let mut user = "admin".to_string();
    let mut password = String::new();
    let mut verification_code = None;
    let mut library_path = None;
    let mut zero_channel_number = None;
    let mut link_mode = LINK_TCP;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--host" => { i += 1; host = args.get(i).cloned(); }
            "--sdk-port" => { i += 1; sdk_port = args.get(i).and_then(|s| s.parse().ok()).unwrap_or(8000); }
            "--user" => { i += 1; user = args.get(i).cloned().unwrap_or_else(|| "admin".to_string()); }
            "--password" => { i += 1; password = args.get(i).cloned().unwrap_or_default(); }
            "--verification-code" => { i += 1; verification_code = args.get(i).cloned(); }
            "--library-path" => { i += 1; library_path = args.get(i).cloned(); }
            "--zero-channel-number" => { i += 1; zero_channel_number = args.get(i).and_then(|s| s.parse().ok()); }
            "--link-mode" => { i += 1; link_mode = args.get(i).and_then(|s| s.parse().ok()).unwrap_or(LINK_TCP); }
            _ => { eprintln!("Argumento desconhecido: {}", args[i]); }
        }
        i += 1;
    }

    Some(Args {
        host: host?,
        sdk_port,
        user,
        password,
        verification_code,
        library_path,
        zero_channel_number,
        link_mode,
    })
}

/// Calcula candidatos para o número do canal zero, do mais provável para o menos.
fn zero_channel_candidates(v30: &NET_DVR_DEVICEINFO_V30) -> Vec<i32> {
    let mut cand = Vec::new();

    // 1. Canal virtual 129 (padrão histórico para Canal Zero em muitos DVRs/NVRs)
    //    Também adiciona range 129..129+byZeroChanNum para devices com múltiplos
    //    canais zero (ex: diferentes áreas do NVR).
    let base129 = 129i32;
    for i in 0..v30.byZeroChanNum as i32 {
        let ch = base129 + i;
        if !cand.contains(&ch) {
            cand.push(ch);
        }
    }

    // 2. Após todos os canais digitais: byStartDChan + byIPChanNum
    let c1 = v30.byStartDChan as i32 + v30.byIPChanNum as i32;
    if !cand.contains(&c1) {
        cand.push(c1);
    }

    // 3. Após analógicos + IPs: byStartChan + byChanNum + byIPChanNum
    let c2 = v30.byStartChan as i32 + v30.byChanNum as i32 + v30.byIPChanNum as i32;
    if !cand.contains(&c2) {
        cand.push(c2);
    }

    // 4. Apenas após analógicos: byStartChan + byChanNum
    let c3 = v30.byStartChan as i32 + v30.byChanNum as i32;
    if !cand.contains(&c3) {
        cand.push(c3);
    }

    // 5. Primeiro canal digital: byStartDChan
    let c4 = v30.byStartDChan as i32;
    if !cand.contains(&c4) {
        cand.push(c4);
    }

    // 6. Canal 1 (fallback — mostra o canal analógico 1)
    if !cand.contains(&1) {
        cand.push(1);
    }

    cand
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();

    let args = match parse_args() {
        Some(a) => a,
        None => { print_usage(); std::process::exit(1); }
    };

    log::info!("=== Canal Zero — Teste HCNetSDK X11 ===");

    // -----------------------------------------------------------------------
    // 1. Carregar HCNetSDK
    // -----------------------------------------------------------------------
    log::info!("[1/4] Carregando HCNetSDK...");

    let sdk = if let Some(lib_path) = &args.library_path {
        HCNetSDK::load_from(std::path::Path::new(lib_path))?
    } else {
        hcnetsdk::search_and_load()?
    };

    sdk.init().context("NET_DVR_Init")?;
    sdk.set_connect_time(10_000, 1)
        .context("NET_DVR_SetConnectTime")?;
    log::info!("HCNetSDK inicializado");

    // -----------------------------------------------------------------------
    // 2. Login
    // -----------------------------------------------------------------------
    log::info!("[2/4] Logando...");

    let (user_id, device_info) = sdk
        .login(&args.host, args.sdk_port, &args.user, &args.password)
        .context("Login")?;

    let v30 = &device_info.struDeviceV30;
    log::info!("Login OK. user_id={}", user_id);

    // Log detalhado das informacoes de canais
    log::info!("--- Device Info (canais) ---");
    log::info!("  byChanNum={} byStartChan={}", v30.byChanNum, v30.byStartChan);
    log::info!("  byIPChanNum={} byStartDChan={}", v30.byIPChanNum, v30.byStartDChan);
    log::info!("  byZeroChanNum={}", v30.byZeroChanNum);
    log::info!("  byDVRType={} byHighDChanNum={}", v30.byDVRType, v30.byHighDChanNum);

    let serial_bytes: Vec<u8> = v30.sSerialNumber.iter()
        .take_while(|&&c| c != 0).map(|&c| c as u8).collect();
    let serial = std::str::from_utf8(&serial_bytes).unwrap_or("(invalid utf8)");
    log::info!("  Serial: {}", serial);

    if v30.byZeroChanNum == 0 {
        log::error!("byZeroChanNum=0 — dispositivo nao suporta Canal Zero!");
        sdk.logout(user_id)?;
        std::process::exit(1);
    }

    // Calcular e logar candidatos
    let candidates = zero_channel_candidates(v30);
    log::info!("Candidatos a channel number do Canal Zero: {:?}", candidates);

    // -----------------------------------------------------------------------
    // 2.5 Query device ability XML (GetDeviceAbility)
    // -----------------------------------------------------------------------
    log::info!("[2.5] Querying device ability XML...");

    // Try DEVICE_DYNCHAN_ABILITY with XML-formatted channel number
    for ch in [1, 33, 34, 65, 129, 257] {
        let xml = format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<DynChannelAbility>\n<channelNo>{}</channelNo>\n</DynChannelAbility>", ch);
        match sdk.get_device_ability(user_id, DEVICE_DYNCHAN_ABILITY, &xml) {
            Ok(xml) => {
                log::info!("--- DYNCHAN_ABILITY XML ch={} ---", ch);
                for line in xml.lines().take(40) {
                    log::info!("{}", line);
                }
            }
            Err(e) => log::warn!("DEVICE_DYNCHAN_ABILITY XML ch={}: {}", ch, e),
        }
    }

    // DEVICE_ABILITY_INFO: try different XML nodes and empty
    let xml_nodes = ["", "<PreviewSwitchAbility/>", "<ZeroChannel/>", "<DynChannelAbility/>"];
    for node in &xml_nodes {
        match sdk.get_device_ability(user_id, DEVICE_ABILITY_INFO, node) {
            Ok(xml) => {
                log::info!("--- DEVICE_ABILITY_INFO({:?}) XML ---", node);
                for line in xml.lines().take(40) {
                    log::info!("{}", line);
                }
                if xml.len() > 2000 {
                    log::info!("... (truncado, total {} chars)", xml.len());
                }
            }
            Err(e) => log::warn!("DEVICE_ABILITY_INFO({:?}): {}", node, e),
        }
    }

    // Query zero channel preview config (which channels are mapped to it)
    log::info!("[2.6] Querying zero channel preview config...");
    {
        let mut zpc: NET_DVR_PREVIEWCFG_V30 = unsafe { std::mem::zeroed() };
        zpc.dwSize = std::mem::size_of::<NET_DVR_PREVIEWCFG_V30>() as u32;
        match sdk.get_dvr_config(user_id, NET_DVR_GET_ZERO_PREVIEWCFG_V30, 0, &mut zpc) {
            Ok(()) => {
                log::info!("--- ZERO_PREVIEWCFG_V30 ---");
                log::info!("  dwSize={} byPreviewNumber={} byEnableAudio={} wSwitchTime={}",
                    zpc.dwSize, zpc.byPreviewNumber, zpc.byEnableAudio, zpc.wSwitchTime);
                // Dump the channel mapping for each preview mode
                for mode in 0..8 {
                    let seq = &zpc.bySwitchSeq[mode];
                    let used: Vec<u8> = seq.iter().copied().take_while(|&c| c != 0xFF).collect();
                    if !used.is_empty() {
                        log::info!("  mode={}: channels={:?}", mode, used);
                    }
                }
            }
            Err(e) => log::warn!("NET_DVR_GET_ZERO_PREVIEWCFG_V30: {}", e),
        }
    }

    // Query zero channel encoding config (already done in ensure_zero_channel_enabled)
    {
        let mut zcfg: NET_DVR_ZEROCHANCFG = unsafe { std::mem::zeroed() };
        zcfg.dwSize = std::mem::size_of::<NET_DVR_ZEROCHANCFG>() as u32;
        // Try with different channel values — some devices need the zero channel index
        for ch in [0, 1, 33, 129] {
            match sdk.get_dvr_config(user_id, NET_DVR_GET_ZEROCHANCFG, ch, &mut zcfg) {
                Ok(()) => {
                    log::info!("--- ZEROCHANCFG(ch={}) ---", ch);
                    log::info!("  byEnable={} dwVideoBitrate={} dwVideoFrameRate={}",
                        zcfg.byEnable, zcfg.dwVideoBitrate, zcfg.dwVideoFrameRate);
                }
                Err(e) => log::info!("NET_DVR_GET_ZEROCHANCFG(ch={}): {}", ch, e),
            }
        }
    }

    // Probe ZeroMakeKeyFrame to find the correct zero channel number
    log::info!("[2.7] Probing NET_DVR_ZeroMakeKeyFrame...");
    let mut zero_key_ok_channels: Vec<i32> = Vec::new();
    for ch in [1, 33, 34, 35, 51, 65, 129, 257] {
        match sdk.zero_make_key_frame(user_id, ch) {
            Ok(true) => {
                log::info!("  >>> ZeroMakeKeyFrame({}) = SUCESSO (canal zero confirmado)", ch);
                zero_key_ok_channels.push(ch);
            }
            Ok(false) => log::debug!("  ZeroMakeKeyFrame({}) = falha", ch),
            Err(e) => log::warn!("  ZeroMakeKeyFrame({}) = erro: {}", ch, e),
        }
    }
    if !zero_key_ok_channels.is_empty() {
        log::info!("  Canais confirmados por ZeroMakeKeyFrame: {:?}", zero_key_ok_channels);
    } else {
        log::info!("  Nenhum canal confirmado por ZeroMakeKeyFrame");
    }

    // Try DEVICE_ABILITY_INFO with other common XML nodes
    let other_nodes = ["<PreviewAbility/>", "<ChannelInputAbility/>", "<EncodeAbility/>",
        "<ZeroChannelAbility/>", "<ITSEAbility/>", "<DigitalChannelAbility/>"];
    for node in &other_nodes {
        match sdk.get_device_ability(user_id, DEVICE_ABILITY_INFO, node) {
            Ok(xml) => {
                log::info!("--- DEVICE_ABILITY_INFO({}) XML ---", node);
                for line in xml.lines().take(30) {
                    log::info!("{}", line);
                }
            }
            Err(e) => log::warn!("DEVICE_ABILITY_INFO({}): {}", node, e),
        }
    }

    // -----------------------------------------------------------------------
    // 3. Ativar Canal Zero
    // -----------------------------------------------------------------------
    log::info!("[3/4] Ativando Canal Zero...");

    match sdk.ensure_zero_channel_enabled(user_id, true) {
        Ok((enabled, was_us)) => {
            log::info!("Canal Zero ativado={}, ativado_por_nos={}", enabled, was_us);
        }
        Err(e) => log::warn!("Falha ao ativar Canal Zero: {}", e),
    }

    // -----------------------------------------------------------------------
    // 4. Janela X11 + RealPlay (estratégia em cascata)
    //    Prioridade: RealPlaySpecial > candidatos ZeroMakeKeyFrame >
    //    V30/V40 RTSP > RealPlaySpecial callback > candidatos gerais
    // -----------------------------------------------------------------------
    log::info!("[4/4] Abrindo janela X11 e iniciando RealPlay...");

    if let Some(ref vc) = args.verification_code {
        if !vc.trim().is_empty() {
            match sdk.set_sdk_secret_key(user_id, vc) {
                Ok(()) => log::info!("NET_DVR_SetSDKSecretKey OK"),
                Err(e) => log::warn!("NET_DVR_SetSDKSecretKey: {}", e),
            }
        }
    }

    // Criar janela X11
    let mut window = match PreviewWindow::new() {
        Some(w) => w,
        None => {
            log::error!("Falha ao criar janela X11 (sem DISPLAY?)");
            sdk.logout(user_id)?;
            std::process::exit(1);
        }
    };

    let hwnd = window.window_id();
    log::info!("Janela X11 criada: 0x{:x}", hwnd);

    // play_handle comeca em -1 (invalido). SDK retorna handle >= 0 no sucesso.
    // IMPORTANTE: o primeiro handle bem-sucedido pode ser 0, entao usamos -1
    // como sinal de "nenhum handle aberto ainda".
    let mut play_handle: i32 = -1;

    // [4a] RealPlaySpecial com URLs RTSP exclusivas do Canal Zero.
    //      Tentada primeiro por ser a abordagem mais confiavel.
    log::info!("[4a] Trying RealPlaySpecial with RTSP URLs (zero channel)...");
    if let Ok(h) = try_zero_special_rtsp(&sdk, user_id, &args.host, &args.user, &args.password, hwnd) {
        log::info!(">>> SUCESSO [4a] RealPlaySpecial handle={}", h);
        play_handle = h;
    }

    // [4b] Canais confirmados pelo probe ZeroMakeKeyFrame
    if play_handle < 0 && !zero_key_ok_channels.is_empty() {
        log::info!("[4b] Trying channels confirmed by ZeroMakeKeyFrame: {:?}...", zero_key_ok_channels);
        for ch in &zero_key_ok_channels {
            match try_variations_for(&sdk, user_id, *ch, hwnd) {
                Ok(h) => {
                    log::info!(">>> SUCESSO [4b] ZeroMakeKeyFrame channel {} handle={}", ch, h);
                    play_handle = h;
                    break;
                }
                Err(e) => log::warn!("[4b] ZeroMakeKeyFrame channel {} falhou: {}", ch, e),
            }
        }
    }

    // [4c] V30/V40 RTSP variations on ch=1 and ch=0
    if play_handle < 0 {
        log::info!("[4c] Trying V30/V40 RTSP on ch=1 and ch=0...");
        for ch in [1, 0] {
            match try_zero_rtsp_variations(&sdk, user_id, ch, hwnd) {
                Ok(h) => {
                    log::info!(">>> SUCESSO [4c] RTSP ch={} handle={}", ch, h);
                    play_handle = h;
                    break;
                }
                Err(e) => log::warn!("[4c] RTSP ch={}: {}", ch, e),
            }
        }
    }

    // [4d] RealPlaySpecial com callback (fallback sem janela)
    if play_handle < 0 {
        log::info!("[4d] Trying RealPlaySpecial with callback (no window)...");
        if let Ok(h) = try_zero_special_rtsp_callback(&sdk, user_id, &args.host, &args.user, &args.password) {
            log::info!(">>> SUCESSO [4d] RealPlaySpecial callback handle={}", h);
            play_handle = h;
        }
    }

    // [4e] Candidatos gerais (standard candidate loop)
    if play_handle < 0 {
        log::info!("[4e] Trying standard candidate loop...");
        play_handle = try_candidates(&sdk, user_id, &args, &candidates, hwnd)
            .context("Nenhum candidato funcionou para Canal Zero")?;
    }

    log::info!("=== Video ao vivo [handle={}] — feche a janela X11 ou Ctrl+C para sair ===", play_handle);

    // Loop de eventos X11
    let mut last_stats = Instant::now();
    let mut frame_count = 0u64;

    loop {
        if !window.poll_events() {
            log::info!("Janela fechada pelo usuario");
            break;
        }

        frame_count += 1;
        if last_stats.elapsed() >= Duration::from_secs(5) {
            log::info!(
                "Vivo: {:.0} poll/s, {}s decorridos",
                frame_count as f64 / last_stats.elapsed().as_secs_f64(),
                last_stats.elapsed().as_secs(),
            );
            frame_count = 0;
            last_stats = Instant::now();
        }

        std::thread::sleep(Duration::from_millis(10));
    }

    log::info!("Parando RealPlay...");
    if play_handle >= 0 {
        let _ = sdk.stop_realplay(play_handle);
    }
    drop(window);
    let _ = sdk.logout(user_id);

    log::info!("=== Teste encerrado ===");
    Ok(())
}

/// Tenta zero channel via RTSP específico: V30 RTSP e V40 RTSP.
/// Essas variações podem acessar o zero channel quando TCP mostra analógico.
fn try_zero_rtsp_variations(
    sdk: &HCNetSDK, user_id: i32, ch: i32, hwnd: u32,
) -> Result<i32> {
    // --- V30: RTSP main ---
    log::info!("  V30 RTSP main ch={}", ch);
    if let Ok(h) = sdk.realplay_v30_with_window_rtsp(user_id, ch, false, hwnd) {
        log::info!(">>> SUCESSO V30 RTSP main ch={} handle={}", ch, h);
        return Ok(h);
    }
    log::info!("  erro={}", sdk.get_last_error());

    // --- V30: RTSP sub ---
    log::info!("  V30 RTSP sub  ch={}", ch);
    if let Ok(h) = sdk.realplay_v30_with_window_rtsp(user_id, ch, true, hwnd) {
        log::info!(">>> SUCESSO V30 RTSP sub ch={} handle={}", ch, h);
        return Ok(h);
    }
    log::info!("  erro={}", sdk.get_last_error());

    // --- V40: RTSP MAIN pm=0 ---
    log::info!("  V40 RTSP MAIN pm=0 ch={}", ch);
    if let Ok(h) = sdk.realplay_with_window_ex2(user_id, ch, STREAM_MAIN, LINK_RTSP, 0, hwnd) {
        log::info!(">>> SUCESSO V40 RTSP MAIN ch={} handle={}", ch, h);
        return Ok(h);
    }
    log::info!("  erro={}", sdk.get_last_error());

    // --- V40: RTSP SUB pm=0 ---
    log::info!("  V40 RTSP SUB  pm=0 ch={}", ch);
    if let Ok(h) = sdk.realplay_with_window_ex2(user_id, ch, STREAM_SUB, LINK_RTSP, 0, hwnd) {
        log::info!(">>> SUCESSO V40 RTSP SUB ch={} handle={}", ch, h);
        return Ok(h);
    }
    log::info!("  erro={}", sdk.get_last_error());

    // --- V40: TCP MAIN pm=1 (zero channel preview mode) ---
    log::info!("  V40 TCP  MAIN pm=1 ch={}", ch);
    if let Ok(h) = sdk.realplay_with_window_ex2(user_id, ch, STREAM_MAIN, LINK_TCP, 1, hwnd) {
        log::info!(">>> SUCESSO V40 TCP MAIN pm=1 ch={} handle={}", ch, h);
        return Ok(h);
    }
    log::info!("  erro={}", sdk.get_last_error());

    // --- V40: TCP MAIN pm=2 (another preview mode variant) ---
    log::info!("  V40 TCP  MAIN pm=2 ch={}", ch);
    if let Ok(h) = sdk.realplay_with_window_ex2(user_id, ch, STREAM_MAIN, LINK_TCP, 2, hwnd) {
        log::info!(">>> SUCESSO V40 TCP MAIN pm=2 ch={} handle={}", ch, h);
        return Ok(h);
    }
    log::info!("  erro={}", sdk.get_last_error());

    anyhow::bail!("ch={} falhou em todas as variacoes zero RTSP", ch)
}

/// Tenta zero channel via sub-stream, que em alguns devices mostra o composite.
#[allow(dead_code)]
fn try_zero_sub_variations(
    sdk: &HCNetSDK, user_id: i32, ch: i32, hwnd: u32,
) -> Result<i32> {
    // --- V40: TCP SUB pm=0 ---
    log::info!("  V40 TCP  SUB  pm=0 ch={}", ch);
    if let Ok(h) = sdk.realplay_with_window_ex2(user_id, ch, STREAM_SUB, LINK_TCP, 0, hwnd) {
        log::info!(">>> SUCESSO V40 TCP SUB pm=0 ch={} handle={}", ch, h);
        return Ok(h);
    }
    log::info!("  erro={}", sdk.get_last_error());

    // --- V40: TCP SUB pm=1 ---
    log::info!("  V40 TCP  SUB  pm=1 ch={}", ch);
    if let Ok(h) = sdk.realplay_with_window_ex2(user_id, ch, STREAM_SUB, LINK_TCP, 1, hwnd) {
        log::info!(">>> SUCESSO V40 TCP SUB pm=1 ch={} handle={}", ch, h);
        return Ok(h);
    }
    log::info!("  erro={}", sdk.get_last_error());

    // --- V40: TCP SUB pm=2 ---
    log::info!("  V40 TCP  SUB  pm=2 ch={}", ch);
    if let Ok(h) = sdk.realplay_with_window_ex2(user_id, ch, STREAM_SUB, LINK_TCP, 2, hwnd) {
        log::info!(">>> SUCESSO V40 TCP SUB pm=2 ch={} handle={}", ch, h);
        return Ok(h);
    }
    log::info!("  erro={}", sdk.get_last_error());

    anyhow::bail!("ch={} falhou em todas as variacoes sub", ch)
}

/// Tenta cada candidato com múltiplas estratégias:
///   V40: TCP/RSTP × MAIN/SUB × preview_mode=0/1/2
///   V30: main/sub (API antiga, mais permissiva)
fn try_candidates(
    sdk: &HCNetSDK,
    user_id: i32,
    args: &Args,
    candidates: &[i32],
    hwnd: u32,
) -> Result<i32> {
    if let Some(ch) = args.zero_channel_number {
        log::info!("Usando --zero-channel-number={}", ch);
        return try_variations_for(sdk, user_id, ch, hwnd);
    }

    for ch in candidates {
        match try_variations_for(sdk, user_id, *ch, hwnd) {
            Ok(h) => return Ok(h),
            Err(_) => continue,
        }
    }

    anyhow::bail!(
        "Nenhum candidato funcionou. Candidatos testados: {:?}\n\
         Tente especificar manualmente com --zero-channel-number <NUM>",
        candidates
    )
}

/// Gera URLs RTSP exclusivas para Canal Zero.
/// Apenas URLs com ?zeroChannel=1 — sem canais de câmera (101, 102, etc)
/// para evitar que o SDK abra uma câmera real em vez do mosaico do Canal Zero.
fn generate_zero_channel_rtsp_urls(user: &str, password: &str, host: &str) -> Vec<String> {
    let hosts = vec![
        format!("{}:554", host),    // porta explícita primeiro
        host.to_string(),
    ];

    // Paths em ordem de prioridade: nativo Canal Zero, depois query param
    // NOTA: senha NÃO pode ser URL-encoded — o cliente RTSP do SDK não
    // decodifica %XX antes de gerar o cabeçalho de autenticação e envia
    // literalmente o %XX para o DVR, causando Unauthorized.
    let paths = vec![
        // Native Canal Zero paths (iDS/AcuSense): 001=Canal 0 main, 002=Canal 0 sub
        ("/Streaming/channels/001", vec![""]),
        ("/Streaming/channels/002", vec![""]),
        // Canal 0 com zeroChannel=1 (padrão ISAPI para Canal Zero)
        ("/Streaming/channels/0", vec!["?zeroChannel=1", "?zeroChannel=1&transportmode=unicast"]),
        // ISAPI path
        ("/ISAPI/Streaming/channels/0", vec!["?zeroChannel=1"]),
        // Caminho curto (alguns devices aceitam)
        ("/zeroChannel=1", vec![""]),
    ];

    let mut urls = Vec::new();
    for host_str in &hosts {
        for (path, param_list) in &paths {
            for param in param_list {
                urls.push(format!("rtsp://{}:{}@{}{}{}", user, password, host_str, path, param));
            }
        }
    }
    urls
}

/// Tenta Canal Zero via NET_DVR_RealPlaySpecial com todas as URLs RTSP.
/// Testa primeiro com LINK_RTSP, depois LINK_TCP.
fn try_zero_special_rtsp(
    sdk: &HCNetSDK, user_id: i32, host: &str, user: &str, password: &str, hwnd: u32,
) -> Result<i32> {
    let urls = generate_zero_channel_rtsp_urls(user, password, host);
    let total_urls = urls.len();
    log::info!("  Testando {} URLs RTSP com RealPlaySpecial...", total_urls);

    for (mode_name, link_mode) in [("RTSP", LINK_RTSP), ("TCP", LINK_TCP)] {
        for url in &urls {
            log::info!("  RealPlaySpecial {}: {}", mode_name, url);
            match sdk.realplay_special(user_id, url, link_mode, hwnd) {
                Ok(h) => {
                    log::info!(">>> SUCESSO RealPlaySpecial {} handle={}", mode_name, h);
                    return Ok(h);
                }
                Err(e) => log::debug!("  RealPlaySpecial {} falhou: {}", mode_name, e),
            }
        }
    }

    anyhow::bail!("RealPlaySpecial falhou em todas as {} URLs x 2 modos", total_urls)
}

/// Tenta Canal Zero via NET_DVR_RealPlaySpecial com callback (sem janela).
/// Fallback quando o modo com janela falha.
fn try_zero_special_rtsp_callback(
    sdk: &HCNetSDK, user_id: i32, host: &str, user: &str, password: &str,
) -> Result<i32> {
    let urls = generate_zero_channel_rtsp_urls(user, password, host);
    log::info!("  Testando {} URLs com RealPlaySpecial callback...", urls.len());

    // Callback dummy que apenas loga os pacotes recebidos
    extern "C" fn dummy_callback(_handle: i32, _data_type: u32, _buffer: *mut u8, _size: u32, _user: *mut std::ffi::c_void) {
        log::debug!("RealPlaySpecial callback: type={}, size={}", _data_type, _size);
    }

    for url in &urls {
        log::info!("  RealPlaySpecial callback: {}", url);
        match sdk.realplay_special_with_callback(user_id, url, LINK_RTSP, dummy_callback, std::ptr::null_mut()) {
            Ok(h) => {
                log::info!(">>> SUCESSO RealPlaySpecial callback handle={}", h);
                return Ok(h);
            }
            Err(e) => log::debug!("  RealPlaySpecial callback falhou: {}", e),
        }
    }

    anyhow::bail!("RealPlaySpecial callback falhou em todas as URLs")
}

fn try_variations_for(
    sdk: &HCNetSDK, user_id: i32, ch: i32, hwnd: u32,
) -> Result<i32> {
    // --- V40: TCP / MAIN / preview_mode=0 ---
    log::info!("  V40 TCP  MAIN pm=0  ch={}", ch);
    if let Ok(h) = sdk.realplay_with_window_ex2(user_id, ch, STREAM_MAIN, LINK_TCP, 0, hwnd) {
        log::info!(">>> SUCESSO V40 TCP MAIN pm=0 ch={} handle={}", ch, h);
        return Ok(h);
    }
    log::info!("  erro={}", sdk.get_last_error());

    // --- V40: TCP / MAIN / preview_mode=1 ---
    log::info!("  V40 TCP  MAIN pm=1  ch={}", ch);
    if let Ok(h) = sdk.realplay_with_window_ex2(user_id, ch, STREAM_MAIN, LINK_TCP, 1, hwnd) {
        log::info!(">>> SUCESSO V40 TCP MAIN pm=1 ch={} handle={}", ch, h);
        return Ok(h);
    }
    log::info!("  erro={}", sdk.get_last_error());

    // --- V40: TCP / MAIN / preview_mode=2 ---
    log::info!("  V40 TCP  MAIN pm=2  ch={}", ch);
    if let Ok(h) = sdk.realplay_with_window_ex2(user_id, ch, STREAM_MAIN, LINK_TCP, 2, hwnd) {
        log::info!(">>> SUCESSO V40 TCP MAIN pm=2 ch={} handle={}", ch, h);
        return Ok(h);
    }
    log::info!("  erro={}", sdk.get_last_error());

    // --- V40: TCP / MAIN / pm=0 / data_type=1 (standard stream) ---
    log::info!("  V40 TCP  MAIN dt=1 pm=0  ch={}", ch);
    if let Ok(h) = sdk.realplay_with_window_ex3(user_id, ch, STREAM_MAIN, LINK_TCP, 0, 1, 0, hwnd) {
        log::info!(">>> SUCESSO V40 TCP MAIN dt=1 pm=0 ch={} handle={}", ch, h);
        return Ok(h);
    }
    log::info!("  erro={}", sdk.get_last_error());

    // --- V40: RTSP / MAIN / pm=0 / proto_type=1 ---
    log::info!("  V40 RTSP MAIN dt=0 pt=1 pm=0  ch={}", ch);
    if let Ok(h) = sdk.realplay_with_window_ex3(user_id, ch, STREAM_MAIN, LINK_RTSP, 0, 0, 1, hwnd) {
        log::info!(">>> SUCESSO V40 RTSP MAIN dt=0 pt=1 ch={} handle={}", ch, h);
        return Ok(h);
    }
    log::info!("  erro={}", sdk.get_last_error());

    // --- V40: RTSP / MAIN / pm=0 / data_type=1 ---
    log::info!("  V40 RTSP MAIN dt=1 pt=1 pm=0  ch={}", ch);
    if let Ok(h) = sdk.realplay_with_window_ex3(user_id, ch, STREAM_MAIN, LINK_RTSP, 0, 1, 1, hwnd) {
        log::info!(">>> SUCESSO V40 RTSP MAIN dt=1 pt=1 ch={} handle={}", ch, h);
        return Ok(h);
    }
    log::info!("  erro={}", sdk.get_last_error());

    // --- V40: RTSP / SUB / pm=0 ---
    log::info!("  V40 RTSP SUB  pm=0  ch={}", ch);
    if let Ok(h) = sdk.realplay_with_window_ex2(user_id, ch, STREAM_SUB, LINK_RTSP, 0, hwnd) {
        log::info!(">>> SUCESSO V40 RTSP SUB ch={} handle={}", ch, h);
        return Ok(h);
    }
    log::info!("  erro={}", sdk.get_last_error());

    // --- V40: TCP / SUB / pm=0 ---
    log::info!("  V40 TCP  SUB  pm=0  ch={}", ch);
    if let Ok(h) = sdk.realplay_with_window_ex2(user_id, ch, STREAM_SUB, LINK_TCP, 0, hwnd) {
        log::info!(">>> SUCESSO V40 TCP SUB ch={} handle={}", ch, h);
        return Ok(h);
    }
    log::info!("  erro={}", sdk.get_last_error());

    // --- V40: TCP / SUB / pm=0 / data_type=1 ---
    log::info!("  V40 TCP  SUB dt=1 pm=0  ch={}", ch);
    if let Ok(h) = sdk.realplay_with_window_ex3(user_id, ch, STREAM_SUB, LINK_TCP, 0, 1, 0, hwnd) {
        log::info!(">>> SUCESSO V40 TCP SUB dt=1 ch={} handle={}", ch, h);
        return Ok(h);
    }
    log::info!("  erro={}", sdk.get_last_error());

    // --- V40: RTSP / SUB / pm=0 / data_type=1 ---
    log::info!("  V40 RTSP SUB dt=1 pt=1 pm=0  ch={}", ch);
    if let Ok(h) = sdk.realplay_with_window_ex3(user_id, ch, STREAM_SUB, LINK_RTSP, 0, 1, 1, hwnd) {
        log::info!(">>> SUCESSO V40 RTSP SUB dt=1 pt=1 ch={} handle={}", ch, h);
        return Ok(h);
    }
    log::info!("  erro={}", sdk.get_last_error());

    // --- V30: main stream ---
    log::info!("  V30 main ch={}", ch);
    if let Ok(h) = sdk.realplay_v30_with_window(user_id, ch, false, hwnd) {
        log::info!(">>> SUCESSO V30 main ch={} handle={}", ch, h);
        return Ok(h);
    }
    log::info!("  erro={}", sdk.get_last_error());

    // --- V30: sub stream ---
    log::info!("  V30 sub  ch={}", ch);
    if let Ok(h) = sdk.realplay_v30_with_window(user_id, ch, true, hwnd) {
        log::info!(">>> SUCESSO V30 sub ch={} handle={}", ch, h);
        return Ok(h);
    }
    log::info!("  erro={}", sdk.get_last_error());

    // --- V30: main RTSP ---
    log::info!("  V30 RTSP main ch={}", ch);
    if let Ok(h) = sdk.realplay_v30_with_window_rtsp(user_id, ch, false, hwnd) {
        log::info!(">>> SUCESSO V30 RTSP main ch={} handle={}", ch, h);
        return Ok(h);
    }
    log::info!("  erro={}", sdk.get_last_error());

    // --- V30: sub RTSP ---
    log::info!("  V30 RTSP sub  ch={}", ch);
    if let Ok(h) = sdk.realplay_v30_with_window_rtsp(user_id, ch, true, hwnd) {
        log::info!(">>> SUCESSO V30 RTSP sub ch={} handle={}", ch, h);
        return Ok(h);
    }
    log::info!("  erro={}", sdk.get_last_error());

    anyhow::bail!("ch={} falhou em todas as variacoes", ch)
}
