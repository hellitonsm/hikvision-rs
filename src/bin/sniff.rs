use std::io::{Read, Write};
use std::net::TcpListener;

fn main() {
    let listener = TcpListener::bind("127.0.0.1:9999").unwrap();
    println!("Listening on 127.0.0.1:9999");
    for stream in listener.incoming() {
        let mut stream = stream.unwrap();
        let mut buf = [0; 4096];
        let n = stream.read(&mut buf).unwrap();
        println!("=== REQUEST ===");
        print!("{}", String::from_utf8_lossy(&buf[..n]));
        println!("=== END ===");
        let resp = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nOK";
        stream.write_all(resp).unwrap();
        break;
    }
}
