//! Import a signed-by-YAM, bundled .yap into the separately authorized Luna host.
use std::{
    env, fs,
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddrV4, TcpStream},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const ID: &str = "alice-apple-music";
const LIMIT: u64 = 24 * 1024 * 1024;
fn post(port: u16, path: &str, content_type: &str, body: &[u8]) -> Result<u16> {
    let mut socket = TcpStream::connect_timeout(
        &SocketAddrV4::new(Ipv4Addr::LOCALHOST, port).into(),
        Duration::from_secs(3),
    )?;
    socket.set_read_timeout(Some(Duration::from_secs(90)))?;
    socket.set_write_timeout(Some(Duration::from_secs(90)))?;
    write!(socket,"POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\r\n",body.len())?;
    socket.write_all(body)?;
    socket.flush()?;
    let mut reply = Vec::new();
    socket.take(65536).read_to_end(&mut reply)?;
    let line = reply
        .split(|b| *b == b'\n')
        .next()
        .ok_or("Empty Luna response")?;
    let status = std::str::from_utf8(line)?
        .split_whitespace()
        .nth(1)
        .ok_or("Invalid Luna status")?
        .parse::<u16>()?;
    Ok(status)
}
fn successful(status: u16) -> Result<()> {
    if !(200..300).contains(&status) {
        return Err(format!("Luna HTTP {status}; проверьте локальный журнал Luna").into());
    }
    Ok(())
}
fn enable(port: u16) -> Result<()> {
    successful(post(
        port,
        &format!("/api/plugins/{ID}/service/enable"),
        "application/json",
        b"{}",
    )?)
}
fn upload(port: u16, path: &Path, activate: bool) -> Result<()> {
    let file = fs::File::open(path)?;
    if file.metadata()?.len() > LIMIT {
        return Err("YAP exceeds 24 MiB".into());
    }
    let mut data = Vec::new();
    file.take(LIMIT + 1).read_to_end(&mut data)?;
    if data.len() as u64 > LIMIT || !data.starts_with(&[0x1f, 0x8b]) {
        return Err("Expected the bundled gzip YAP".into());
    }
    let boundary = format!(
        "alice_yam_{}_{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    );
    let mut body=format!("--{boundary}\r\nContent-Disposition: form-data; name=\"package\"; filename=\"apple-music.yap\"\r\nContent-Type: application/octet-stream\r\n\r\n").into_bytes();
    body.extend(data);
    body.extend(format!("\r\n--{boundary}--\r\n").as_bytes());
    // Stop the old service before replacing immutable package files. Do not enable on upload failure.
    let status = post(
        port,
        &format!("/api/plugins/{ID}/stop"),
        "application/json",
        b"{}",
    )?;
    if status != 404 {
        successful(status)?;
    }
    std::thread::sleep(Duration::from_secs(2));
    successful(post(
        port,
        "/api/plugins",
        &format!("multipart/form-data; boundary={boundary}"),
        &body,
    )?)?;
    if activate {
        enable(port)?;
    }
    Ok(())
}
fn package_path() -> Result<PathBuf> {
    let executable = fs::canonicalize(env::current_exe()?)?;
    let root = executable
        .parent()
        .and_then(Path::parent)
        .ok_or("Cannot resolve package directory")?;
    Ok(root.join("share/awtainq.apple-music/apple-music.yap"))
}
fn run() -> Result<()> {
    let mut port = 8098;
    let mut activate = false;
    let mut only = false;
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--port" => port = args.next().ok_or("Missing port")?.parse::<u16>()?,
            "--enable" => activate = true,
            "--enable-only" => only = true,
            "--help" | "-h" => {
                println!("awtainq-apple-music [--port 8098] [--enable | --enable-only]\nЗагружает установленный YAM пакет .yap в локальную Luna.\n--enable: также запросить запуск; --enable-only: только запросить запуск после добавления профиля.\nЛичный профиль: <Luna data-dir>/apple-music-private. Порт 8099 зарезервирован плеером.");
                return Ok(());
            }
            _ => return Err("Unknown option; use --help".into()),
        }
    }
    if port == 0 || port == 8099 {
        return Err(
            "Luna должна использовать отдельный порт, например 8098; плееру нужен 8099".into(),
        );
    }
    if only {
        enable(port)?;
        println!("Luna приняла запрос запуска сервиса.");
    } else {
        upload(port, &package_path()?, activate)?;
        println!("Пакет загружен в Luna. Профиль и очередь сохраняются вне пакета.");
        if activate {
            println!("Luna приняла запрос запуска сервиса.");
        } else {
            println!(
                "Добавьте собственный профиль, затем выполните awtainq-apple-music --enable-only."
            );
        }
    }
    Ok(())
}
fn main() {
    if let Err(e) = run() {
        eprintln!("Не удалось настроить плеер: {e}");
        std::process::exit(1);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::{net::TcpListener, thread};
    fn server(statuses: Vec<u16>) -> (u16, thread::JoinHandle<Vec<Vec<u8>>>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = thread::spawn(move || {
            let mut requests = Vec::new();
            for status in statuses {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut data = Vec::new();
                let mut byte = [0];
                while !data.ends_with(b"\r\n\r\n") {
                    socket.read_exact(&mut byte).unwrap();
                    data.push(byte[0]);
                }
                let header = String::from_utf8_lossy(&data);
                let length = header
                    .lines()
                    .find_map(|l| l.strip_prefix("Content-Length: "))
                    .unwrap()
                    .parse::<usize>()
                    .unwrap();
                let mut body = vec![0; length];
                socket.read_exact(&mut body).unwrap();
                data.extend(body);
                requests.push(data);
                write!(
                    socket,
                    "HTTP/1.1 {status} Test\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                )
                .unwrap();
            }
            requests
        });
        (port, handle)
    }
    #[test]
    fn failed_upload_never_enables_service() {
        let path = env::temp_dir().join(format!("alice-yam-fail-{}.yap", std::process::id()));
        fs::write(&path, [0x1f, 0x8b, 1, 2]).unwrap();
        let (port, server) = server(vec![200, 500]);
        assert!(upload(port, &path, true).is_err());
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(requests[1].starts_with(b"POST /api/plugins HTTP/1.1"));
        fs::remove_file(path).unwrap();
    }
    #[test]
    fn multipart_import_and_activation() {
        let path = env::temp_dir().join(format!("alice-yam-ok-{}.yap", std::process::id()));
        fs::write(&path, [0x1f, 0x8b, 3, 4]).unwrap();
        let (port, server) = server(vec![404, 201, 202]);
        upload(port, &path, true).unwrap();
        let requests = server.join().unwrap();
        let upload = String::from_utf8_lossy(&requests[1]);
        assert!(upload.contains("name=\"package\""));
        assert!(upload.contains("multipart/form-data; boundary="));
        assert!(requests[2].starts_with(b"POST /api/plugins/alice-apple-music/service/enable "));
        fs::remove_file(path).unwrap();
    }
    #[test]
    fn missing_payload_does_not_stop_service() {
        assert!(upload(1, Path::new("/definitely/missing/alice.yap"), true).is_err());
    }
}
