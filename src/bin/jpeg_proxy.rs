use hikvision_rs::api::HikvisionAPI;
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

const BOUNDARY: &str = "hikvision-frame";

fn print_usage() {
    eprintln!("Hikvision JPEG Snapshot Proxy");
    eprintln!();
    eprintln!("Serves MJPEG over HTTP by polling the DVR's JPEG snapshot endpoint.");
    eprintln!("View with: ffplay http://127.0.0.1:9001");
    eprintln!();
    eprintln!("Usage:");
    eprintln!("  jpeg_proxy --host <DVR_IP> --user <USER> --password <PASS>");
    eprintln!("            [--channel 101] [--port 80] [--listen 127.0.0.1:9001]");
    eprintln!("            [--interval 500] [--https]");
    eprintln!();
    eprintln!("Options:");
    eprintln!("  --host        DVR IP address (required)");
    eprintln!("  --user        Username (default: admin)");
    eprintln!("  --password    DVR login password (required)");
    eprintln!("  --channel     Channel ID (default: 101 for ZeroChannel, try 101-164)");
    eprintln!("  --port        HTTP/HTTPS port (default: 80)");
    eprintln!("  --listen      Listen address (default: 127.0.0.1:9001)");
    eprintln!("  --interval    Poll interval in ms (default: 300)");
    eprintln!("  --https       Use HTTPS (accept self-signed camera cert)");
}

struct Args {
    host: String,
    port: u16,
    user: String,
    password: String,
    channel: String,
    listen: String,
    interval_ms: u64,
    https: bool,
}

fn parse_args() -> Option<Args> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2
        || args.contains(&"--help".to_string())
        || args.contains(&"-h".to_string())
    {
        return None;
    }

    let mut host = None;
    let mut port = 80u16;
    let mut user = "admin".to_string();
    let mut password = String::new();
    let mut channel = "101".to_string();
    let mut listen = "127.0.0.1:9001".to_string();
    let mut interval_ms = 300u64;
    let mut https = false;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--host" => {
                i += 1;
                host = args.get(i).cloned();
            }
            "--port" => {
                i += 1;
                port = args.get(i).and_then(|s| s.parse().ok()).unwrap_or(80);
            }
            "--user" => {
                i += 1;
                user = args.get(i).cloned().unwrap_or_else(|| "admin".to_string());
            }
            "--password" => {
                i += 1;
                password = args.get(i).cloned().unwrap_or_default();
            }
            "--channel" => {
                i += 1;
                channel = args.get(i).cloned().unwrap_or_else(|| "101".to_string());
            }
            "--listen" => {
                i += 1;
                listen = args
                    .get(i)
                    .cloned()
                    .unwrap_or_else(|| "127.0.0.1:9001".to_string());
            }
            "--interval" => {
                i += 1;
                interval_ms = args
                    .get(i)
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(300);
            }
            "--https" => {
                https = true;
            }
            _ => {
                eprintln!("Unknown argument: {}", args[i]);
            }
        }
        i += 1;
    }

    Some(Args {
        host: host?,
        port,
        user,
        password,
        channel,
        listen,
        interval_ms,
        https,
    })
}

fn handle_client(
    mut stream: TcpStream,
    api: &HikvisionAPI,
    channel: &str,
    interval: Duration,
    stop: &AtomicBool,
) {
    let header = format!(
        "HTTP/1.0 200 OK\r\n\
         Cache-Control: no-cache\r\n\
         Content-Type: multipart/x-mixed-replace; boundary={}\r\n\
         Connection: close\r\n\
         \r\n",
        BOUNDARY
    );
    if stream.write_all(header.as_bytes()).is_err() {
        return;
    }

    let boundary_end = format!("\r\n--{}--\r\n", BOUNDARY);

    let mut first = true;
    while !stop.load(Ordering::Relaxed) {
        match api.snapshot(channel) {
            Ok(jpeg) => {
                let part = if first {
                    format!(
                        "--{}\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n",
                        BOUNDARY,
                        jpeg.len()
                    )
                } else {
                    format!(
                        "\r\n--{}\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n",
                        BOUNDARY,
                        jpeg.len()
                    )
                };
                if stream.write_all(part.as_bytes()).is_err() {
                    return;
                }
                if stream.write_all(&jpeg).is_err() {
                    return;
                }
                first = false;
            }
            Err(e) => {
                log::warn!("Snapshot error: {}", e);
                return;
            }
        }
        std::thread::sleep(interval);
    }

    let _ = stream.write_all(boundary_end.as_bytes());
    let _ = stream.flush();
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();

    let args = match parse_args() {
        Some(a) => a,
        None => {
            print_usage();
            std::process::exit(1);
        }
    };

    log::info!("Hikvision JPEG Snapshot Proxy");
    log::info!(
        "DVR: {}:{}, Channel: {}, Interval: {}ms",
        args.host,
        args.port,
        args.channel,
        args.interval_ms
    );
    log::info!("Listening on: {}", args.listen);
    log::info!(
        "View with: ffplay http://{}",
        args.listen
    );

    let api = HikvisionAPI::new(&args.host, args.port, &args.user, &args.password, args.https);

    // Quick connectivity test
    match api.device_info() {
        Ok(info) => log::info!("Connected: {} | {} | {}", info.name, info.model, info.firmware),
        Err(e) => {
            log::error!("Failed to connect: {}", e);
            std::process::exit(1);
        }
    }

    // Test snapshot
    match api.snapshot(&args.channel) {
        Ok(jpeg) => log::info!(
            "Snapshot OK: {} bytes (JPEG magic: {:02x}{:02x}{:02x})",
            jpeg.len(),
            jpeg.first().copied().unwrap_or(0),
            jpeg.get(1).copied().unwrap_or(0),
            jpeg.get(2).copied().unwrap_or(0)
        ),
        Err(e) => {
            log::error!("Snapshot test failed for channel {}: {}", args.channel, e);
            std::process::exit(1);
        }
    }

    let listener = match TcpListener::bind(&args.listen) {
        Ok(l) => l,
        Err(e) => {
            log::error!("Failed to bind to {}: {}", args.listen, e);
            std::process::exit(1);
        }
    };

    let stop = Arc::new(AtomicBool::new(false));
    let interval = Duration::from_millis(args.interval_ms);
    let channel = args.channel.clone();

    log::info!("Waiting for client connections...");

    for stream in listener.incoming() {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        match stream {
            Ok(client) => {
                log::info!("Client connected from {:?}", client.peer_addr().ok());
                let api = api.clone();
                let ch = channel.clone();
                let int = interval;
                let s = stop.clone();
                std::thread::spawn(move || {
                    handle_client(client, &api, &ch, int, &s);
                    log::info!("Client disconnected");
                });
            }
            Err(e) => log::error!("Accept error: {}", e),
        }
    }
}
