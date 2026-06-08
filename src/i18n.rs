use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    En,
    Pt,
}

impl Default for Lang {
    fn default() -> Self {
        Lang::En
    }
}

impl Lang {
    pub fn variants() -> &'static [(Lang, &'static str, &'static str)] {
        &[
            (Lang::En, "English", "🇬🇧 English"),
            (Lang::Pt, "Português", "🇧🇷 Português"),
        ]
    }
}

pub struct Strings {
    pub lang: Lang,
}

impl Strings {
    pub fn new(lang: Lang) -> Self {
        Self { lang }
    }

    // ── Login Screen ──────────────────────────────────────────────────────

    pub fn heading(&self) -> &'static str {
        match self.lang {
            Lang::En => "hikvision-rs",
            Lang::Pt => "hikvision-rs",
        }
    }

    pub fn subtitle(&self) -> &'static str {
        match self.lang {
            Lang::En => "RTSP Viewer for Hikvision DVRs",
            Lang::Pt => "Visualizador RTSP para DVRs Hikvision",
        }
    }

    pub fn host_label(&self) -> &'static str {
        "Host:"
    }

    pub fn http_port_label(&self) -> &'static str {
        match self.lang {
            Lang::En => "HTTP Port:",
            Lang::Pt => "Porta HTTP:",
        }
    }

    pub fn https_port_label(&self) -> &'static str {
        match self.lang {
            Lang::En => "HTTPS Port:",
            Lang::Pt => "Porta HTTPS:",
        }
    }

    pub fn https_label(&self) -> &'static str {
        "HTTPS:"
    }

    pub fn https_checkbox(&self) -> &'static str {
        match self.lang {
            Lang::En => "Use HTTPS (self-signed certificate)",
            Lang::Pt => "Usar HTTPS (certificado auto-assinado)",
        }
    }

    pub fn rtsp_port_label(&self) -> &'static str {
        match self.lang {
            Lang::En => "RTSP Port:",
            Lang::Pt => "Porta RTSP:",
        }
    }

    pub fn sdk_port_label(&self) -> &'static str {
        match self.lang {
            Lang::En => "SDK Port:",
            Lang::Pt => "Porta SDK:",
        }
    }

    pub fn username_label(&self) -> &'static str {
        match self.lang {
            Lang::En => "Username:",
            Lang::Pt => "Usuário:",
        }
    }

    pub fn password_label(&self) -> &'static str {
        match self.lang {
            Lang::En => "Password:",
            Lang::Pt => "Senha:",
        }
    }

    pub fn method_label(&self) -> &'static str {
        match self.lang {
            Lang::En => "Method:",
            Lang::Pt => "Método:",
        }
    }

    pub fn substream_checkbox(&self) -> &'static str {
        match self.lang {
            Lang::En => "Sub-stream (lower resolution, lighter)",
            Lang::Pt => "Sub-stream (menor resolução, mais leve)",
        }
    }

    pub fn interval_label(&self) -> &'static str {
        match self.lang {
            Lang::En => "Interval (ms):",
            Lang::Pt => "Intervalo (ms):",
        }
    }

    pub fn verification_code_label(&self) -> &'static str {
        match self.lang {
            Lang::En => "Verification Code:",
            Lang::Pt => "Código de Verificação:",
        }
    }

    pub fn library_path_label(&self) -> &'static str {
        match self.lang {
            Lang::En => "Library Path:",
            Lang::Pt => "Caminho da Biblioteca:",
        }
    }

    pub fn library_path_hint(&self) -> &'static str {
        match self.lang {
            Lang::En => "libhcnetsdk.so (empty = auto)",
            Lang::Pt => "libhcnetsdk.so (vazio = auto)",
        }
    }

    pub fn library_path_auto_hint(&self) -> &'static str {
        match self.lang {
            Lang::En => "Leave empty for auto-detect",
            Lang::Pt => "Deixe vazio para buscar automático",
        }
    }

    pub fn connect_button(&self) -> &'static str {
        match self.lang {
            Lang::En => "Connect",
            Lang::Pt => "Conectar",
        }
    }

    pub fn language_selector_label(&self) -> &'static str {
        match self.lang {
            Lang::En => "Language:",
            Lang::Pt => "Idioma:",
        }
    }

    // ── Method Labels ─────────────────────────────────────────────────────

    pub fn method_rtsp(&self) -> &'static str {
        match self.lang {
            Lang::En => "RTSP (direct)",
            Lang::Pt => "RTSP (direto)",
        }
    }

    pub fn method_snapshot(&self) -> &'static str {
        "Snapshot (JPEG polling)"
    }

    pub fn method_playctrl(&self) -> &'static str {
        match self.lang {
            Lang::En => "PlayCtrl (decryption)",
            Lang::Pt => "PlayCtrl (descriptografia)",
        }
    }

    pub fn method_hcnetsdk(&self) -> &'static str {
        "HCNetSDK (callback + PlayM4)"
    }

    pub fn method_hcnetsdk_x11(&self) -> &'static str {
        match self.lang {
            Lang::En => "HCNetSDK X11 (direct overlay)",
            Lang::Pt => "HCNetSDK X11 (overlay direto)",
        }
    }

    // ── Method Short Labels (sidebar) ──────────────────────────────────────

    pub fn method_short_hcnetsdk(&self) -> &'static str {
        match self.lang {
            Lang::En => "🔐 HCNetSDK",
            Lang::Pt => "🔐 HCNetSDK",
        }
    }

    pub fn method_short_hcnetsdk_x11(&self) -> &'static str {
        match self.lang {
            Lang::En => "🖥️ HCNetSDK X11",
            Lang::Pt => "🖥️ HCNetSDK X11",
        }
    }

    pub fn method_short_playctrl(&self) -> &'static str {
        match self.lang {
            Lang::En => "🔐 PlayCtrl",
            Lang::Pt => "🔐 PlayCtrl",
        }
    }

    pub fn method_short_snapshot(&self) -> &'static str {
        "📷 Snapshot JPEG"
    }

    pub fn method_short_rtsp(&self) -> &'static str {
        match self.lang {
            Lang::En => "🎥 RTSP direct",
            Lang::Pt => "🎥 RTSP direto",
        }
    }

    // ── Method Descriptions ────────────────────────────────────────────────

    pub fn status_hcnetsdk(&self) -> &'static str {
        match self.lang {
            Lang::En => "🔐 HCNetSDK (callback + PlayM4). Automatic decryption via NET_DVR_SetSDKSecretKey.",
            Lang::Pt => "🔐 HCNetSDK (callback + PlayM4). Descriptografia automática via NET_DVR_SetSDKSecretKey.",
        }
    }

    pub fn status_hcnetsdk_x11(&self) -> &'static str {
        match self.lang {
            Lang::En => "🖥️ HCNetSDK X11 (direct overlay). SDK renders via X11 — no manual decoding. Requires libhcnetsdk.so.",
            Lang::Pt => "🖥️ HCNetSDK X11 (overlay direto). SDK renderiza via X11 — sem decodificação manual. Requer libhcnetsdk.so.",
        }
    }

    pub fn status_playctrl(&self) -> &'static str {
        match self.lang {
            Lang::En => "🔐 PlayCtrl with decryption. Requires libPlayCtrl.so and DVR Verification Code.",
            Lang::Pt => "🔐 PlayCtrl com descriptografia. Requer libPlayCtrl.so e Verification Code do DVR.",
        }
    }

    pub fn status_snapshot(&self) -> &'static str {
        match self.lang {
            Lang::En => "ℹ️ Snapshot JPEG polling. ~2-3 FPS. Does not require disabling encryption.",
            Lang::Pt => "ℹ️ Snapshot JPEG polling. ~2-3 FPS. Não requer desativar criptografia.",
        }
    }

    pub fn status_rtsp(&self) -> &'static str {
        match self.lang {
            Lang::En => "⚠️ RTSP direct. If 'Transmission Encryption' is enabled on the DVR, video will not load.",
            Lang::Pt => "⚠️ RTSP direto. Se a 'Criptografia de Transmissão' estiver ativada no DVR, o vídeo não carregará.",
        }
    }

    // ── Viewer Screen ──────────────────────────────────────────────────────

    pub fn disconnect_button(&self) -> &'static str {
        match self.lang {
            Lang::En => "Disconnect",
            Lang::Pt => "Desconectar",
        }
    }

    pub fn channels_heading(&self) -> &'static str {
        match self.lang {
            Lang::En => "Channels",
            Lang::Pt => "Canais",
        }
    }

    pub fn layout_label(&self) -> &'static str {
        match self.lang {
            Lang::En => "Layout",
            Lang::Pt => "Layout",
        }
    }

    pub fn substream_info(&self) -> &'static str {
        match self.lang {
            Lang::En => "Sub-stream (auto in multi-view)",
            Lang::Pt => "Sub-stream (auto em multi-view)",
        }
    }

    pub fn start_all(&self) -> &'static str {
        "Start All"
    }

    pub fn stop_all(&self) -> &'static str {
        "Stop All"
    }

    pub fn no_signal(&self) -> &'static str {
        match self.lang {
            Lang::En => "No Signal",
            Lang::Pt => "Sem Sinal",
        }
    }

    pub fn live_text(&self) -> &'static str {
        match self.lang {
            Lang::En => "● Live",
            Lang::Pt => "● Ao Vivo",
        }
    }

    pub fn waiting_text(&self) -> &'static str {
        match self.lang {
            Lang::En => "Waiting...",
            Lang::Pt => "Aguardando...",
        }
    }

    pub fn select_channel(&self) -> &'static str {
        "Select a channel to view"
    }

    pub fn streams_count(&self, active: usize, total: usize) -> String {
        match self.lang {
            Lang::En => format!("{}/{} streams", active, total),
            Lang::Pt => format!("{}/{} streams", active, total),
        }
    }

    pub fn video_info(&self, name: &str, w: usize, h: usize, fps: f32) -> String {
        format!("{} | {}x{} | {:.1} fps", name, w, h, fps)
    }

    pub fn channel_label(&self, id: &str, name: &str) -> String {
        format!("[{}] {}", id, name)
    }

    // ── Zero Channel ──────────────────────────────────────────────────────

    pub fn zero_channel_label(&self, num: u8) -> String {
        match self.lang {
            Lang::En => {
                if num > 0 {
                    format!("Zero Channel ({})", num)
                } else {
                    "Zero Channel".to_string()
                }
            }
            Lang::Pt => {
                if num > 0 {
                    format!("Canal Zero ({})", num)
                } else {
                    "Canal Zero".to_string()
                }
            }
        }
    }

    pub fn zero_channel_desc(&self) -> &'static str {
        match self.lang {
            Lang::En => "● Zero Channel — Multi-camera mosaic (X11 overlay)",
            Lang::Pt => "● Canal Zero — Mosaico multi-câmera (X11 overlay)",
        }
    }

    pub fn x11_waiting(&self) -> &'static str {
        match self.lang {
            Lang::En => "X11 overlay — waiting for stream",
            Lang::Pt => "X11 overlay — aguardando stream",
        }
    }

    // ── Loading Labels ─────────────────────────────────────────────────────

    pub fn loading_hcnetsdk(&self) -> &'static str {
        "HCNetSDK (callback mode)"
    }

    pub fn loading_hcnetsdk_x11(&self) -> &'static str {
        "HCNetSDK X11 (overlay mode)"
    }

    pub fn loading_playctrl(&self) -> &'static str {
        match self.lang {
            Lang::En => "PlayCtrl decrypting...",
            Lang::Pt => "PlayCtrl descriptografando...",
        }
    }

    pub fn loading_snapshot(&self) -> &'static str {
        "Polling snapshot..."
    }

    pub fn loading_rtsp(&self) -> &'static str {
        "Connecting to RTSP stream..."
    }

    pub fn fps_label(&self, name: &str, fps: f32) -> String {
        format!("{} {:.0}fps", name, fps)
    }

    // ── Live Label ─────────────────────────────────────────────────────────

    pub fn live_label(&self, name: &str) -> String {
        match self.lang {
            Lang::En => format!("● {} — Live (X11 overlay)", name),
            Lang::Pt => format!("● {} — Ao vivo (X11 overlay)", name),
        }
    }

    // ── Certificate Dialog ─────────────────────────────────────────────────

    pub fn cert_confirm_title(&self) -> &'static str {
        match self.lang {
            Lang::En => "Confirm Certificate",
            Lang::Pt => "Confirmar certificado",
        }
    }

    pub fn cert_changed_title(&self) -> &'static str {
        match self.lang {
            Lang::En => "Fingerprint Changed",
            Lang::Pt => "Fingerprint alterado",
        }
    }

    pub fn cert_first_connection(&self) -> &'static str {
        match self.lang {
            Lang::En => "First HTTPS connection to this server.",
            Lang::Pt => "Primeira conexão HTTPS com este servidor.",
        }
    }

    pub fn cert_verify_fingerprint(&self) -> &'static str {
        match self.lang {
            Lang::En => "Verify the certificate fingerprint:",
            Lang::Pt => "Verifique o fingerprint do certificado:",
        }
    }

    pub fn cert_changed_msg(&self) -> &'static str {
        match self.lang {
            Lang::En => "The server SSL certificate has changed!",
            Lang::Pt => "O certificado SSL do servidor mudou!",
        }
    }

    pub fn cert_meanings_prompt(&self) -> &'static str {
        match self.lang {
            Lang::En => "This could mean:",
            Lang::Pt => "Isso pode significar:",
        }
    }

    pub fn cert_meanings_1(&self) -> &'static str {
        match self.lang {
            Lang::En => "  • The DVR was factory reset",
            Lang::Pt => "  • O DVR foi reiniciado de fábrica",
        }
    }

    pub fn cert_meanings_2(&self) -> &'static str {
        match self.lang {
            Lang::En => "  • The certificate was regenerated",
            Lang::Pt => "  • O certificado foi regenerado",
        }
    }

    pub fn cert_old_fingerprint(&self) -> String {
        match self.lang {
            Lang::En => "Old fingerprint: ".to_string(),
            Lang::Pt => "Fingerprint antigo: ".to_string(),
        }
    }

    pub fn cert_fingerprint_label(&self) -> &'static str {
        match self.lang {
            Lang::En => "Fingerprint:",
            Lang::Pt => "Fingerprint:",
        }
    }

    pub fn cert_accept(&self) -> &'static str {
        match self.lang {
            Lang::En => "✓ Accept (save fingerprint)",
            Lang::Pt => "✓ Aceitar (salvar fingerprint)",
        }
    }

    pub fn cert_reject(&self) -> &'static str {
        match self.lang {
            Lang::En => "✗ Reject",
            Lang::Pt => "✗ Rejeitar",
        }
    }

    pub fn cert_first_https(&self, fp: &str) -> String {
        match self.lang {
            Lang::En => format!(
                "First HTTPS connection.\nCertificate fingerprint:\n{}\n\nTrust this certificate?",
                fp
            ),
            Lang::Pt => format!(
                "Primeira conexão HTTPS.\nFingerprint do certificado:\n{}\n\nConfie neste certificado?",
                fp
            ),
        }
    }

    pub fn cert_changed_fingerprint(&self, old: &str, new: &str) -> String {
        match self.lang {
            Lang::En => format!(
                "Certificate fingerprint changed!\nOld: {}\nNew: {}\nAccept the new fingerprint to proceed.",
                old, new
            ),
            Lang::Pt => format!(
                "O fingerprint do certificado mudou!\nAntigo: {}\nNovo: {}\nAceite o novo fingerprint para continuar.",
                old, new
            ),
        }
    }

    // ── Errors ─────────────────────────────────────────────────────────────

    pub fn error_invalid_port(&self) -> &'static str {
        match self.lang {
            Lang::En => "Invalid port",
            Lang::Pt => "Porta inválida",
        }
    }

    pub fn error_host_required(&self) -> &'static str {
        match self.lang {
            Lang::En => "Host is required",
            Lang::Pt => "Host é obrigatório",
        }
    }

    pub fn error_verification_code_required(&self) -> &'static str {
        match self.lang {
            Lang::En => "Verification Code is required for this streaming method",
            Lang::Pt => "Verification Code é obrigatório para este método de streaming",
        }
    }

    pub fn error_connection_rejected(&self) -> &'static str {
        match self.lang {
            Lang::En => "Connection rejected",
            Lang::Pt => "Conexão rejeitada",
        }
    }
}
