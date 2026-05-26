use std::env;
use std::io::{Read, Write};
use std::net::TcpStream;

fn main() {
    env_logger::init();

    let host = env::var("HOST").unwrap_or_else(|_| "192.168.5.75".into());
    let port = env::var("PORT").unwrap_or_else(|_| "80".into());
    let user = env::var("USER").unwrap_or_else(|_| "admin".into());
    let pass = env::var("PASS").unwrap_or_else(|_| "".into());

    // Step 1: send GET without auth
    let req1 = format!(
        "GET /ISAPI/System/deviceInfo HTTP/1.0\r\nHost: {}\r\nConnection: close\r\n\r\n",
        host
    );

    let mut stream = TcpStream::connect(format!("{}:{}", host, port)).unwrap();
    stream.write_all(req1.as_bytes()).unwrap();
    stream.flush().unwrap();

    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).unwrap();
    let resp1 = String::from_utf8_lossy(&buf);
    println!("=== RESPONSE 1 ===");
    println!("{}", resp1);

    // Parse WWW-Authenticate
    let challenge = resp1
        .lines()
        .find(|l| l.to_lowercase().starts_with("www-authenticate:"))
        .and_then(|l| l.splitn(2, ':').nth(1))
        .map(|s| s.trim())
        .unwrap_or("");
    println!("Challenge: {}", challenge);

    if challenge.is_empty() {
        return;
    }

    // Parse params
    let body = challenge.strip_prefix("Digest ").unwrap_or("");
    let mut realm = "";
    let mut nonce = "";
    let mut qop = "";
    let mut opaque = "";
    for part in body.split(',') {
        let part = part.trim();
        if let Some(eq) = part.find('=') {
            let key = part[..eq].trim();
            let val = part[eq + 1..].trim().trim_matches('"');
            match key.to_lowercase().as_str() {
                "realm" => realm = val,
                "nonce" => nonce = val,
                "qop" => qop = val,
                "opaque" => opaque = val,
                _ => {}
            }
        }
    }
    println!("realm={} nonce={} qop={} opaque={}", realm, nonce, qop, opaque);

    // Compute Digest
    let ha1 = format!("{:x}", md5::compute(format!("{}:{}:{}", user, realm, pass).as_bytes()));
    let ha2 = format!("{:x}", md5::compute(format!("GET:/ISAPI/System/deviceInfo").as_bytes()));
    let cnonce = format!("{:x}", md5::compute(format!("{}{}", nonce, user).as_bytes()));
    let nc = "00000001";
    let resp_input = format!("{}:{}:{}:{}:{}:{}", ha1, nonce, nc, cnonce, qop, ha2);
    let response = format!("{:x}", md5::compute(resp_input.as_bytes()));

    let auth = format!(
        "Digest username=\"{}\", realm=\"{}\", nonce=\"{}\", uri=\"/ISAPI/System/deviceInfo\", qop={}, nc={}, cnonce=\"{}\", response=\"{}\"",
        user, realm, nonce, qop, nc, cnonce, response,
    );

    let auth = if !opaque.is_empty() {
        format!("{}, opaque=\"{}\"", auth, opaque)
    } else {
        auth
    };

    // Step 2: send with auth
    let req2 = format!(
        "GET /ISAPI/System/deviceInfo HTTP/1.0\r\nHost: {}\r\nAuthorization: {}\r\nConnection: close\r\n\r\n",
        host, auth
    );

    println!("\n=== REQUEST 2 ===");
    println!("{}", req2);

    let mut stream = TcpStream::connect(format!("{}:{}", host, port)).unwrap();
    stream.write_all(req2.as_bytes()).unwrap();
    stream.flush().unwrap();

    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).unwrap();
    let resp2 = String::from_utf8_lossy(&buf);
    println!("=== RESPONSE 2 ===");
    println!("{}", resp2);
}
