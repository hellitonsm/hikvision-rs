use anyhow::{Context, Result};
use quick_xml::events::Event;
use quick_xml::Reader;
use std::cell::RefCell;
use ureq;

#[derive(Debug, Clone)]
pub struct Channel {
    pub id: String,
    pub name: String,
}

#[derive(Debug)]
pub struct DeviceInfo {
    pub model: String,
    pub serial: String,
    pub firmware: String,
    pub name: String,
}

#[derive(Clone)]
pub struct HikvisionAPI {
    agent: ureq::Agent,
    base: String,
    user: String,
    password: String,
    auth_header: RefCell<Option<String>>,
}

impl HikvisionAPI {
    pub fn new(host: &str, port: u16, user: &str, password: &str) -> Self {
        let agent = ureq::Agent::new_with_config(
            ureq::config::Config::builder()
                .http_status_as_error(false)
                .build(),
        );
        Self {
            agent,
            base: format!("http://{}:{}", host, port),
            user: user.to_string(),
            password: password.to_string(),
            auth_header: RefCell::new(None),
        }
    }

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
                anyhow::bail!("HTTP {}", code);
            }
            return Ok(resp);
        }

        let code = status.as_u16();
        if code >= 400 {
            anyhow::bail!("HTTP {}", code);
        }
        Ok(resp)
    }

    fn get_text(&self, path: &str) -> Result<String> {
        let mut resp = self.request_inner("GET", path)?;
        Ok(resp
            .body_mut()
            .read_to_string()
            .context("body read failed")?)
    }

    fn get_bytes(&self, path: &str) -> Result<Vec<u8>> {
        let mut resp = self.request_inner("GET", path)?;
        Ok(resp
            .body_mut()
            .read_to_vec()
            .context("body read failed")?)
    }

    pub fn device_info(&self) -> Result<DeviceInfo> {
        log::info!("Fetching device info");
        let xml = self.get_text("/ISAPI/System/deviceInfo")?;
        parse_device_info(&xml)
    }

    pub fn channels(&self) -> Result<Vec<Channel>> {
        log::info!("Fetching channel list");
        let xml = self.get_text("/ISAPI/Streaming/channels")?;
        parse_channels(&xml)
    }

    pub fn snapshot(&self, cid: &str) -> Result<Vec<u8>> {
        let path = format!("/ISAPI/Streaming/channels/{}/picture", cid);
        self.get_bytes(&path)
    }
}

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

fn param<'a>(params: &'a [(String, String)], name: &str) -> Option<&'a str> {
    params
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

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

fn strip_ns(xml: &str) -> String {
    xml.replace(" xmlns=\"http://www.hikvision.com/ver20/XMLSchema\"", "")
        .replace(" xmlns=\"\"", "")
}

fn parse_device_info(xml: &str) -> Result<DeviceInfo> {
    let xml = strip_ns(xml);
    let mut reader = Reader::from_str(&xml);
    let mut buf = Vec::new();
    let mut info = DeviceInfo {
        model: String::new(),
        serial: String::new(),
        firmware: String::new(),
        name: String::new(),
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
