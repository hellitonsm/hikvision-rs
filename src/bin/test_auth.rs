use hikvision_rs::api::HikvisionAPI;
use std::env;

fn main() {
    env_logger::init();

    let host = env::var("HOST").unwrap_or_else(|_| "192.168.5.75".into());
    let port: u16 = env::var("PORT")
        .unwrap_or_else(|_| "80".into())
        .parse()
        .unwrap_or(80);
    let user = env::var("USER").unwrap_or_else(|_| "admin".into());
    let pass = env::var("PASS").unwrap_or_else(|_| "".into());

    let api = HikvisionAPI::new(&host, port, &user, &pass);
    match api.device_info() {
        Ok(info) => println!("OK: {} | {} | {}", info.name, info.model, info.firmware),
        Err(e) => eprintln!("ERR: {}", e),
    }
}
