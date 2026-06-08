//! # Autenticação ISAPI (HTTP Digest)
//!
//! Este módulo implementa o cliente HTTP para a API ISAPI da Hikvision,
//! utilizando **HTTP Digest Access Authentication** (RFC 2617) para
//! autenticação nas câmeras/DVRs.
//!
//! ## Fluxo de Autenticação
//!
//! O handshake Digest ocorre em duas etapas:
//!
//! 1. **Requisição sem credenciais** — O cliente envia um GET para o endpoint
//!    (`/ISAPI/System/deviceInfo`). O servidor responde com `401 Unauthorized`
//!    e um cabeçalho `WWW-Authenticate: Digest ...` contendo os parâmetros do
//!    desafio (`realm`, `nonce`, `qop`, `opaque`, `algorithm`).
//!
//! 2. **Requisição com Digest** — O cliente computa a resposta MD5 com base nos
//!    parâmetros do desafio + credenciais e reenvia a requisição com o cabeçalho
//!    `Authorization: Digest ...`. O servidor valida e retorna `200 OK` com os
//!    dados XML.
//!
//! O cabeçalho `auth_header` é cacheado em [`HikvisionAPI`] via `RefCell`, evitando
//! o round-trip 401 em requisições subsequentes. Se o servidor rejeitar o cabeçalho
//! cacheado (novo 401), a autenticação é refeita automaticamente.
//!
//! ## Endpoints ISAPI utilizados
//!
//! | Endpoint | Método | Descrição |
//! |---|---|---|
//! | `/ISAPI/System/deviceInfo` | GET | Informações do dispositivo (modelo, serial, firmware). Usado como *probe* de autenticação. |
//! | `/ISAPI/Streaming/channels` | GET | Lista de canais de vídeo disponíveis. |
//! | `/ISAPI/Streaming/channels/{id}/picture` | GET | Snapshot JPEG de um canal. |
//!
//! ## Algoritmo Digest (MD5)
//!
//! ```text
//! HA1    = MD5( usuário : realm : senha )
//! HA2    = MD5( método  : uri   )
//! cnonce = MD5( nonce   : usuário )
//! response = MD5( HA1 : nonce : nc : cnonce : qop : HA2 )
//! ```
//!
//! Onde `nc` (nonce count) é fixo em `00000001`, e o `qop` (quality of protection)
//! é `auth`.

use anyhow::{Context, Result};
use quick_xml::events::Event;
use quick_xml::Reader;
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use ureq;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerifier, ServerCertVerified};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};

/// Result of a TLS certificate fingerprint check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FingerprintCheck {
    /// No fingerprint was stored; this is the server's fingerprint to save.
    New(String),
    /// Fingerprint matches the stored one.
    Match,
    /// Fingerprint changed. Contains (stored_hex, actual_hex).
    Mismatch(String, String),
}

/// Connect to the server, retrieve its TLS certificate, compute its SHA-256
/// fingerprint, and compare with an optionally stored fingerprint.
///
/// If `stored_fingerprint` is `None`, returns `FingerprintCheck::New(hex)`.
/// If it matches, returns `FingerprintCheck::Match`.
/// If it differs, returns `FingerprintCheck::Mismatch(old, new)`.
pub fn check_tls_fingerprint(
    host: &str,
    port: u16,
    stored_fingerprint: Option<&str>,
) -> Result<FingerprintCheck> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let captured = Arc::new(Mutex::<Option<Vec<u8>>>::new(None));
    let verifier = CapturingVerifier {
        captured: captured.clone(),
    };

    let config = Arc::new(
        rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|_| anyhow::anyhow!("TLS protocol versions not supported"))?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(verifier))
            .with_no_client_auth(),
    );

    let host_static: &'static str = Box::leak(host.to_string().into_boxed_str());
    let server_name = ServerName::try_from(host_static)
        .map_err(|_| anyhow::anyhow!("invalid hostname: {}", host))?;

    let mut conn = rustls::ClientConnection::new(config, server_name)
        .map_err(|e| anyhow::anyhow!("TLS client init failed: {}", e))?;

    let addr = format!("{}:{}", host, port);
    let mut tcp = TcpStream::connect_timeout(
        &addr
            .parse()
            .map_err(|_| anyhow::anyhow!("invalid address: {}", addr))?,
        Duration::from_secs(10),
    )?;
    tcp.set_read_timeout(Some(Duration::from_secs(5)))?;
    tcp.set_write_timeout(Some(Duration::from_secs(5)))?;

    conn.complete_io(&mut tcp)
        .map_err(|e| anyhow::anyhow!("TLS handshake failed: {}", e))?;

    while conn.is_handshaking() {
        conn.complete_io(&mut tcp)
            .map_err(|e| anyhow::anyhow!("TLS handshake completion failed: {}", e))?;
    }

    let cert_der = captured
        .lock()
        .unwrap()
        .take()
        .ok_or_else(|| anyhow::anyhow!("No certificate received from server"))?;

    let fingerprint = Sha256::digest(&cert_der);
    let hex = fingerprint
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect::<Vec<_>>()
        .join(":");

    match stored_fingerprint {
        Some(stored) if stored == hex => Ok(FingerprintCheck::Match),
        Some(stored) => Ok(FingerprintCheck::Mismatch(stored.to_string(), hex)),
        None => Ok(FingerprintCheck::New(hex)),
    }
}

#[derive(Debug)]
struct CapturingVerifier {
    captured: Arc<Mutex<Option<Vec<u8>>>>,
}

impl ServerCertVerifier for CapturingVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        *self.captured.lock().unwrap() = Some(end_entity.to_vec());
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::RSA_PKCS1_SHA256,
            rustls::SignatureScheme::RSA_PKCS1_SHA384,
            rustls::SignatureScheme::RSA_PKCS1_SHA512,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA512,
            rustls::SignatureScheme::ED25519,
        ]
    }
}

/// Representa um canal de vídeo do DVR.
///
/// Cada canal possui um identificador numérico (ex: `"101"`, `"102"`) e um
/// nome descritivo (ex: `"Camera 1"`). O ID é usado para montar as URLs de
/// streaming RTSP e snapshot.
#[derive(Debug, Clone)]
pub struct Channel {
    /// ID do canal no formato Hikvision (ex: `"101"` para canal 1, stream principal).
    pub id: String,
    /// Nome descritivo do canal configurado no DVR.
    pub name: String,
}

/// Informações do dispositivo retornadas pelo endpoint `/ISAPI/System/deviceInfo`.
///
/// Contém os dados básicos de identificação da câmera/DVR, obtidos após
/// autenticação bem-sucedida. Usado como prova de que a conexão está ativa.
#[derive(Debug)]
pub struct DeviceInfo {
    /// Modelo do dispositivo (ex: `"DS-7608NI-I2/8P"`).
    pub model: String,
    /// Número de série do dispositivo.
    pub serial: String,
    /// Versão do firmware (ex: `"V4.30.110"`).
    pub firmware: String,
    /// Nome do dispositivo configurado pelo usuário.
    pub name: String,
    /// Número de canais zero disponíveis (0 = não suporta Canal Zero).
    /// O Canal Zero permite visualizar múltiplas câmeras em um único stream multiplexado.
    pub zero_chan_num: u8,
}

/// Cliente HTTP para a API ISAPI da Hikvision com autenticação Digest.
///
/// # Funcionamento
///
/// 1. [`HikvisionAPI::new`] cria o cliente com as credenciais da câmera.
/// 2. [`HikvisionAPI::device_info`] faz o primeiro request, que dispara o
///    handshake Digest (401 → computa hash → retry com Authorization).
/// 3. O cabeçalho `Authorization` é cacheado em [`auth_header`](HikvisionAPI::auth_header)
///    para reuso em chamadas seguintes ([`channels`](HikvisionAPI::channels),
///    [`snapshot`](HikvisionAPI::snapshot)).
///
/// # Cache do cabeçalho
///
/// O campo `auth_header` é um `RefCell<Option<String>>` porque o cache precisa
/// ser compartilhado entre chamadas imutáveis (&self). Se o servidor rejeitar
/// o header cacheado, `request_inner` limpa o cache e refaz o handshake.
///
/// # Exemplo
///
/// ```no_run
/// use hikvision_rs::api::HikvisionAPI;
///
/// let api = HikvisionAPI::new("192.168.1.100", 80, "admin", "senha123", false);
/// match api.device_info() {
///     Ok(info) => println!("Conectado: {} ({})", info.name, info.model),
///     Err(e) => eprintln!("Falha na autenticação: {}", e),
/// }
/// ```
#[derive(Clone)]
pub struct HikvisionAPI {
    /// Agente HTTP reutilizável (ureq). Configurado para não tratar códigos
    /// HTTP 4xx/5xx como erro, permitindo capturar o 401 manualmente.
    agent: ureq::Agent,
    /// URL base da câmera, montada como `http://{host}:{port}`.
    base: String,
    /// Nome de usuário para autenticação na câmera.
    user: String,
    /// Senha do usuário para autenticação na câmera.
    password: String,
    /// Cabeçalho `Authorization: Digest ...` cacheado.
    /// `None` = ainda não autenticou; `Some("Digest ...")` = reutilizar.
    auth_header: RefCell<Option<String>>,
}

impl HikvisionAPI {
    /// Cria um novo cliente HTTP para a API ISAPI.
    ///
    /// # Parâmetros
    ///
    /// * `host` — Endereço IP ou hostname da câmera/DVR.
    /// * `port` — Porta HTTP (padrão Hikvision: `80`).
    /// * `user` — Nome de usuário para autenticação.
    /// * `password` — Senha do usuário.
    /// * `use_https` — Se `true`, usa HTTPS e aceita certificado auto-assinado da câmera.
    ///
    /// O `ureq::Agent` é configurado com `http_status_as_error(false)` para que
    /// respostas 401 possam ser inspecionadas manualmente durante o handshake Digest.
    /// Quando `use_https` é ativado, a verificação do certificado TLS é desabilitada
    /// para aceitar certificados auto-assinados.
    pub fn new(host: &str, port: u16, user: &str, password: &str, use_https: bool) -> Self {
        let tls_config = if use_https {
            ureq::tls::TlsConfig::builder()
                .disable_verification(true)
                .build()
        } else {
            ureq::tls::TlsConfig::default()
        };
        let agent = ureq::Agent::new_with_config(
            ureq::config::Config::builder()
                .http_status_as_error(false)
                .tls_config(tls_config)
                .timeout_global(Some(std::time::Duration::from_secs(15)))
                .build(),
        );
        Self {
            agent,
            base: format!(
                "{}://{}:{}",
                if use_https { "https" } else { "http" },
                host,
                port
            ),
            user: user.to_string(),
            password: password.to_string(),
            auth_header: RefCell::new(None),
        }
    }

    /// Executa uma requisição HTTP com suporte a Digest Authentication.
    ///
    /// # Fluxo interno
    ///
    /// 1. Tenta a requisição **sem** `Authorization` (ou com o header cacheado,
    ///    se disponível).
    /// 2. Se o servidor responder `401`:
    ///    - Extrai o cabeçalho `WWW-Authenticate` com os parâmetros do desafio.
    ///    - Computa o Digest via [`compute_digest`].
    ///    - Cacheia o resultado em `self.auth_header`.
    ///    - Descarrega o body da resposta 401.
    ///    - Reenvia a requisição com o cabeçalho `Authorization: Digest ...`.
    /// 3. Se o retry também der `401`, retorna erro `"Digest auth failed"`.
    /// 4. Se a resposta inicial (sem auth) já for `< 400`, retorna direto
    ///    (caso raro em câmeras sem autenticação).
    fn request_inner(&self, method: &str, path: &str) -> Result<ureq::http::Response<ureq::Body>> {
        let url = format!("{}{}", self.base, path);
        log::debug!("{} {}", method, url);

        let maybe_auth = self.auth_header.borrow().clone();
        let mut req = match method {
            "GET" => self.agent.get(&url),
            _ => anyhow::bail!("unsupported method: {}", method),
        };
        if let Some(ref auth) = maybe_auth {
            req = req.header("Authorization", auth);
        }
        let mut resp = req.call().context("HTTP request failed")?;
        let status = resp.status();

        if status == 401 {
            let challenge = resp
                .headers()
                .get_all("www-authenticate")
                .iter()
                .filter_map(|v| v.to_str().ok().map(|s| s.to_string()))
                .next()
                .ok_or_else(|| anyhow::anyhow!("401 without WWW-Authenticate header"))?;

            let auth = compute_digest(&challenge, method, path, &self.user, &self.password)?;
            self.auth_header.borrow_mut().replace(auth.clone());

            let _ = resp.body_mut().read_to_vec();

            let resp = self
                .agent
                .get(&url)
                .header("Authorization", &auth)
                .call()
                .context("HTTP request (with auth) failed")?;
            let status = resp.status();
            if status == 401 {
                anyhow::bail!("Digest auth failed (still 401 after retry)");
            }
        let code = status.as_u16();
        if code >= 400 {
            log::warn!("request_inner {} {} -> HTTP {}", method, path, code);
            anyhow::bail!("HTTP {} for {}", code, path);
        }
        return Ok(resp);
    }

        let code = status.as_u16();
        if code >= 400 {
            log::warn!("request_inner {} {} -> HTTP {}", method, path, code);
            anyhow::bail!("HTTP {} for {}", code, path);
        }
        Ok(resp)
    }

    /// Faz um GET e retorna o body como string.
    ///
    /// Usado internamente por [`device_info`](HikvisionAPI::device_info) e
    /// [`channels`](HikvisionAPI::channels).
    fn get_text(&self, path: &str) -> Result<String> {
        let mut resp = self.request_inner("GET", path)?;
        Ok(resp
            .body_mut()
            .read_to_string()
            .context("body read failed")?)
    }

    /// Faz um GET e retorna o body como bytes.
    ///
    /// Usado por [`snapshot`](HikvisionAPI::snapshot) para obter imagens JPEG.
    fn get_bytes(&self, path: &str) -> Result<Vec<u8>> {
        let mut resp = self.request_inner("GET", path)?;
        Ok(resp
            .body_mut()
            .read_to_vec()
            .context("body read failed")?)
    }

    /// Obtém informações do dispositivo (`/ISAPI/System/deviceInfo`).
    ///
    /// Este é o **primeiro endpoint chamado após a conexão**. Ele serve como
    /// *probe* de autenticação: se as credenciais estiverem corretas, o
    /// handshake Digest ocorre silenciosamente e o XML de resposta é parseado
    /// em [`DeviceInfo`].
    ///
    /// # Erros
    ///
    /// Retorna erro se:
    /// - A câmera não responder (timeout, rede, etc.).
    /// - As credenciais forem inválidas (401 persistente).
    /// - O XML de resposta for mal-formado.
    pub fn device_info(&self) -> Result<DeviceInfo> {
        log::info!("Fetching device info");
        let xml = self.get_text("/ISAPI/System/deviceInfo")?;
        parse_device_info(&xml)
    }

    /// Lista os canais de vídeo disponíveis (`/ISAPI/Streaming/channels`).
    ///
    /// Deve ser chamado **após** [`device_info`](HikvisionAPI::device_info) —
    /// o cabeçalho Digest já estará cacheado e esta chamada não disparará
    /// um novo handshake.
    ///
    /// # Formato dos IDs
    ///
    /// Os IDs dos canais seguem o padrão Hikvision:
    /// - Canal 1, stream principal → `"101"`
    /// - Canal 1, sub-stream → `"102"`
    /// - Canal 2, stream principal → `"201"`
    /// - Canal 2, sub-stream → `"202"`
    pub fn channels(&self) -> Result<Vec<Channel>> {
        log::info!("Fetching channel list");
        let xml = self.get_text("/ISAPI/Streaming/channels")?;
        parse_channels(&xml)
    }

    /// Obtém um snapshot JPEG de um canal (`/ISAPI/Streaming/channels/{cid}/picture`).
    ///
    /// Retorna os bytes brutos da imagem JPEG. Pode ser usado para gerar
    /// thumbnails ou verificação visual sem abrir stream RTSP.
    pub fn snapshot(&self, cid: &str) -> Result<Vec<u8>> {
        let path = format!("/ISAPI/Streaming/channels/{}/picture", cid);
        self.get_bytes(&path)
    }

    /// Verifica se o Canal Zero está **ativado** no dispositivo.
    ///
    /// Consulta `/ISAPI/Streaming/zeroChannels` para determinar se o Canal Zero
    /// está configurado como ativo. Isso é diferente de [`DeviceInfo::zero_chan_num`],
    /// que indica apenas se o dispositivo **suporta** Canal Zero.
    ///
    /// # Retorno
    ///
    /// - `Ok(true)` — Canal Zero está suportado e ativado.
    /// - `Ok(false)` — Canal Zero está suportado mas desativado, ou não suportado.
    /// - `Err(_)` — Falha ao consultar (ISAPI não disponível, erro de rede, etc.).
    pub fn zero_channel_enabled(&self) -> Result<bool> {
        log::info!("Checking if zero channel is enabled");
        let xml = self.get_text("/ISAPI/Streaming/zeroChannels")?;

        // Parse: <ZeroChannelList><ZeroChannel><enabled>true</enabled></ZeroChannel></ZeroChannelList>
        // Some devices use <zeroChannel> or <ZeroChannel> with <enabled> or <enable>
        for line in xml.lines() {
            let line_lower = line.trim().to_lowercase();
            if line_lower.contains("<enabled>") || line_lower.contains("<enable>") {
                if line_lower.contains("true") || line_lower.contains("1") {
                    return Ok(true);
                }
            }
        }

        // Alternative format: check for any <ZeroChannel> node with enabled=true
        if xml.to_lowercase().contains("enabled>true") || xml.to_lowercase().contains("enabled>1") {
            return Ok(true);
        }

        Ok(false)
    }
}

/// Parseia o cabeçalho `WWW-Authenticate: Digest ...` em pares chave/valor.
///
/// # Formato esperado
///
/// ```text
/// Digest realm="Hikvision", nonce="abc123", qop="auth", opaque="def456", algorithm="MD5"
/// ```
///
/// Remove o prefixo `"Digest "` e separa por vírgulas, extraindo `key=value`
/// e removendo as aspas dos valores.
fn parse_digest_params(challenge: &str) -> Vec<(String, String)> {
    let challenge = challenge.trim();
    if !challenge.starts_with("Digest ") {
        return Vec::new();
    }
    let body = &challenge[7..];
    let mut params = Vec::new();
    for part in body.split(',') {
        let part = part.trim();
        if let Some(eq) = part.find('=') {
            let key = part[..eq].trim().to_string();
            let val = part[eq + 1..].trim().trim_matches('"').to_string();
            params.push((key, val));
        }
    }
    params
}

/// Busca um parâmetro pelo nome (case-insensitive) na lista de pares.
fn param<'a>(params: &'a [(String, String)], name: &str) -> Option<&'a str> {
    params
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// Computa o cabeçalho `Authorization: Digest ...` conforme RFC 2617.
///
/// # Algoritmo
///
/// ```text
/// HA1    = MD5( user : realm : password )
/// HA2    = MD5( method : uri )
/// cnonce = MD5( nonce : user )
/// nc     = "00000001"
///
/// IF qop = "auth" ou vazio:
///   response = MD5( HA1 : nonce : nc : cnonce : qop : HA2 )
/// ELSE:
///   response = MD5( HA1 : nonce : HA2 )
/// ```
///
/// O `cnonce` (client nonce) único é derivado de `MD5(nonce + user)` para
/// garantir variabilidade mesmo com o mesmo `nc`.
///
/// # Parâmetros
///
/// * `challenge` — Cabeçalho `WWW-Authenticate: Digest ...` recebido do servidor.
/// * `method` — Método HTTP (ex: `"GET"`).
/// * `uri` — Caminho da requisição (ex: `"/ISAPI/System/deviceInfo"`).
/// * `user` — Nome de usuário.
/// * `password` — Senha do usuário.
fn compute_digest(
    challenge: &str,
    method: &str,
    uri: &str,
    user: &str,
    password: &str,
) -> Result<String> {
    let params = parse_digest_params(challenge);
    let realm = param(&params, "realm")
        .ok_or_else(|| anyhow::anyhow!("Digest challenge missing realm"))?;
    let nonce = param(&params, "nonce")
        .ok_or_else(|| anyhow::anyhow!("Digest challenge missing nonce"))?;
    let qop = param(&params, "qop").unwrap_or("");
    let opaque = param(&params, "opaque").unwrap_or("");
    let algorithm = param(&params, "algorithm").unwrap_or("MD5");

    log::debug!("Digest params: realm={}, nonce={}, qop={}, opaque={}, algorithm={}",
        realm, nonce, qop, opaque, algorithm);

    let ha1_input = format!("{}:{}:{}", user, realm, password);
    let ha1 = format!("{:x}", md5::compute(ha1_input.as_bytes()));

    let ha2_input = format!("{}:{}", method, uri);
    let ha2 = format!("{:x}", md5::compute(ha2_input.as_bytes()));

    log::debug!("Digest HA1={} HA2={}", ha1, ha2);

    let cnonce = format!("{:x}", md5::compute(format!("{}{}", nonce, user).as_bytes()));

    let response = if qop.is_empty() || qop == "auth" {
        let resp_input = if qop.is_empty() {
            format!("{}:{}:{}", ha1, nonce, ha2)
        } else {
            let nc = "00000001";
            format!("{}:{}:{}:{}:{}:{}", ha1, nonce, nc, cnonce, qop, ha2)
        };
        log::debug!("Digest resp_input={}", resp_input);
        format!("{:x}", md5::compute(resp_input.as_bytes()))
    } else {
        anyhow::bail!("unsupported qop: {}", qop)
    };

    let mut auth = format!(
        "Digest username=\"{}\", realm=\"{}\", nonce=\"{}\", uri=\"{}\", qop=auth, nc=00000001, cnonce=\"{}\", response=\"{}\", algorithm={}",
        user, realm, nonce, uri, cnonce, response, algorithm,
    );
    if !opaque.is_empty() {
        auth.push_str(&format!(", opaque=\"{}\"", opaque));
    }

    log::debug!("Digest auth header: {}", auth);
    Ok(auth)
}

// -- XML parsing (unchanged) --

/// Remove namespaces XML do formato Hikvision para simplificar o parsing.
///
/// A API Hikvision retorna XML com namespace `xmlns="http://www.hikvision.com/ver20/XMLSchema"`,
/// que o `quick_xml` não lida bem em modo simples. Esta função remove as declarações
/// de namespace antes do parsing.
fn strip_ns(xml: &str) -> String {
    xml.replace(" xmlns=\"http://www.hikvision.com/ver20/XMLSchema\"", "")
        .replace(" xmlns=\"\"", "")
}

/// Parseia o XML de `/ISAPI/System/deviceInfo` em um [`DeviceInfo`].
///
/// Extrai os campos `model`, `serialNumber`, `firmwareVersion`, `deviceName` e `zeroChanNum`.
fn parse_device_info(xml: &str) -> Result<DeviceInfo> {
    let xml = strip_ns(xml);
    let mut reader = Reader::from_str(&xml);
    let mut buf = Vec::new();
    let mut info = DeviceInfo {
        model: String::new(),
        serial: String::new(),
        firmware: String::new(),
        name: String::new(),
        zero_chan_num: 0,
    };
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                let name = e.name().as_ref().to_vec();
                let tag = std::str::from_utf8(&name).unwrap_or("");
                let text = reader.read_text(e.name()).unwrap_or_default().to_string();
                match tag {
                    "model" => info.model = text,
                    "serialNumber" => info.serial = text,
                    "firmwareVersion" => info.firmware = text,
                    "deviceName" => info.name = text,
                    "zeroChanNum" => info.zero_chan_num = text.parse().unwrap_or(0),
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => anyhow::bail!("XML parse error: {}", e),
            _ => {}
        }
        buf.clear();
    }
    Ok(info)
}

/// Parseia o XML de `/ISAPI/Streaming/channels` em uma lista de [`Channel`].
///
/// Cada `<StreamingChannel>` contém `<id>` (ex: `101`) e `<channelName>` (ex: `"Camera 1"`).
/// Canais sem ID são ignorados.
fn parse_channels(xml: &str) -> Result<Vec<Channel>> {
    let xml = strip_ns(xml);
    let mut reader = Reader::from_str(&xml);
    let mut buf = Vec::new();
    let mut channels = Vec::new();
    let mut in_ch = false;
    let mut ch_id = String::new();
    let mut ch_name = String::new();
    let mut in_id = false;
    let mut in_name = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let name = e.name().as_ref().to_vec();
                let tag = std::str::from_utf8(&name).unwrap_or("");
                match tag {
                    "StreamingChannel" => {
                        in_ch = true;
                        ch_id.clear();
                        ch_name.clear();
                    }
                    "id" if in_ch => in_id = true,
                    "channelName" if in_ch => in_name = true,
                    _ => {}
                }
            }
            Ok(Event::Text(ref e)) => {
                if let Ok(text) = e.unescape() {
                    if in_id {
                        ch_id = text.to_string();
                    }
                    if in_name {
                        ch_name = text.to_string();
                    }
                }
            }
            Ok(Event::End(ref e)) => {
                let name = e.name().as_ref().to_vec();
                let tag = std::str::from_utf8(&name).unwrap_or("");
                match tag {
                    "StreamingChannel" => {
                        if !ch_id.is_empty() {
                            channels.push(Channel {
                                id: ch_id.clone(),
                                name: if ch_name.is_empty() {
                                    format!("Channel {}", ch_id)
                                } else {
                                    ch_name.clone()
                                },
                            });
                        }
                        in_ch = false;
                    }
                    "id" => in_id = false,
                    "channelName" => in_name = false,
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => anyhow::bail!("XML parse error: {}", e),
            _ => {}
        }
        buf.clear();
    }
    Ok(channels)
}
