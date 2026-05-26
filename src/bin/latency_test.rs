use std::env;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

fn recv_all(stream: &mut TcpStream) -> Vec<u8> {
    stream.set_read_timeout(Some(Duration::from_secs(10))).ok();
    let mut buf = Vec::new();
    let mut tmp = [0u8; 32768];
    loop {
        match stream.read(&mut tmp) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
            Err(_) => break,
        }
        if let Some(hdr_end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&buf[..hdr_end]);
            if let Some(cl) = headers
                .lines()
                .find(|l| l.to_lowercase().starts_with("content-length:"))
                .and_then(|l| l.splitn(2, ':').nth(1))
                .and_then(|s| s.trim().parse::<usize>().ok())
            {
                let body_start = hdr_end + 4;
                if buf.len() >= body_start + cl {
                    break;
                }
            }
        }
    }
    buf
}

fn extract_challenge(resp: &[u8]) -> (String, String, String, String) {
    let text = String::from_utf8_lossy(resp);
    let challenge = text
        .lines()
        .find(|l| l.to_lowercase().starts_with("www-authenticate:"))
        .and_then(|l| l.splitn(2, ':').nth(1))
        .map(|s| s.trim())
        .unwrap_or("");
    let body = challenge.strip_prefix("Digest ").unwrap_or("");
    let mut realm = String::new();
    let mut nonce = String::new();
    let mut qop = String::new();
    let mut opaque = String::new();
    for part in body.split(',') {
        let part = part.trim();
        if let Some(eq) = part.find('=') {
            let key = part[..eq].trim();
            let val = part[eq + 1..].trim().trim_matches('"');
            match key.to_lowercase().as_str() {
                "realm" => realm = val.to_string(),
                "nonce" => nonce = val.to_string(),
                "qop" => qop = val.to_string(),
                "opaque" => opaque = val.to_string(),
                _ => {}
            }
        }
    }
    (realm, nonce, qop, opaque)
}

fn compute_auth(
    user: &str, pass: &str, realm: &str, nonce: &str, qop: &str, opaque: &str, uri: &str,
) -> String {
    let ha1 = format!("{:x}", md5::compute(format!("{}:{}:{}", user, realm, pass).as_bytes()));
    let ha2 = format!("{:x}", md5::compute(format!("GET:{}", uri).as_bytes()));
    let cnonce = format!("{:x}", md5::compute(format!("{}{}", nonce, user).as_bytes()));
    let resp_input = format!("{}:{}:00000001:{}:{}:{}", ha1, nonce, cnonce, qop, ha2);
    let response = format!("{:x}", md5::compute(resp_input.as_bytes()));
    format!(
        "Digest username=\"{}\", realm=\"{}\", nonce=\"{}\", uri=\"{}\", qop=auth, nc=00000001, cnonce=\"{}\", response=\"{}\", opaque=\"{}\"",
        user, realm, nonce, uri, cnonce, response, opaque
    )
}

fn main() {
    let host = env::var("HOST").unwrap_or_else(|_| "192.168.5.75".into());
    let port = env::var("PORT").unwrap_or_else(|_| "80".into());
    let user = env::var("USER").unwrap_or_else(|_| "admin".into());
    let pass = env::var("PASS").expect("PASS env var required");

    let uri = "/ISAPI/Streaming/channels/101/picture";

    for i in 0..5 {
        let start = Instant::now();

        // Use mesma conexão TCP para ambos os requests
        let mut stream = TcpStream::connect(format!("{}:{}", host, port)).unwrap();

        // Step 1: send without auth
        let req1 = format!(
            "GET {} HTTP/1.1\r\nHost: {}\r\n\r\n",
            uri, host
        );
        stream.write_all(req1.as_bytes()).unwrap();
        stream.flush().unwrap();
        let resp1 = recv_all(&mut stream);

        let (realm, nonce, qop, opaque) = extract_challenge(&resp1);
        eprintln!("DEBUG: realm={} nonce={}", realm, nonce);
        let auth = compute_auth(&user, &pass, &realm, &nonce, &qop, &opaque, uri);

        // Step 2: retry with auth na MESMA conexão
        let req2 = format!(
            "GET {} HTTP/1.1\r\nHost: {}\r\nAuthorization: {}\r\nConnection: close\r\n\r\n",
            uri, host, auth
        );
        stream.write_all(req2.as_bytes()).unwrap();
        stream.flush().unwrap();
        let resp2 = recv_all(&mut stream);

        let elapsed = start.elapsed();
        let text = String::from_utf8_lossy(&resp2);
        let status = text.lines().next().unwrap_or("unknown").to_string();
        let jpeg_start = resp2.windows(4).position(|w| w == b"\xff\xd8\xff\xe0");
        let body_start = resp2.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4).unwrap_or(0);

        println!(
            "[{}] {} | body={}B | jpeg={:?} | time={:.1}ms",
            i + 1,
            status,
            resp2.len(),
            jpeg_start,
            elapsed.as_secs_f64() * 1000.0,
        );
        if jpeg_start.is_some() {
            let jpg_len = resp2.len() - jpeg_start.unwrap();
            eprintln!("DEBUG: JPEG {} bytes", jpg_len);
        } else {
            // Print body for debugging
            let body = String::from_utf8_lossy(&resp2[body_start..]);
            eprintln!("DEBUG: Resp body (first 200): {}", &body[..body.len().min(200)]);
        }
    }
}
