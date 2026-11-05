//! 테스트용 최소 HTTP 서버 (127.0.0.1에서만 동작, 추가 의존성 없음).

#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Req {
    pub method: String,
    pub path: String,
    pub authorization: Option<String>,
    pub body: String,
}

pub enum Resp {
    Json(u16, String),
    Text(u16, String),
    /// HTTP이 아닌 바이트를 그대로 보낸다.
    Raw(Vec<u8>),
    /// 지정한 시간 동안 응답하지 않는다.
    Hang(Duration),
}

pub struct MockServer {
    pub base_url: String,
    pub requests: Arc<Mutex<Vec<Req>>>,
    stop: Arc<AtomicBool>,
    addr: std::net::SocketAddr,
}

impl MockServer {
    pub fn start<F>(handler: F) -> Self
    where
        F: Fn(&Req) -> Resp + Send + Sync + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let stop = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let handler = Arc::new(handler);

        {
            let stop = Arc::clone(&stop);
            let requests = Arc::clone(&requests);
            thread::spawn(move || {
                for stream in listener.incoming() {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(stream) = stream else { continue };
                    let handler = Arc::clone(&handler);
                    let requests = Arc::clone(&requests);
                    thread::spawn(move || serve(stream, handler.as_ref(), &requests));
                }
            });
        }

        Self {
            base_url: format!("http://{addr}/v1"),
            requests,
            stop,
            addr,
        }
    }

    pub fn recorded(&self) -> Vec<Req> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // accept()를 깨워 스레드를 끝낸다.
        let _ = TcpStream::connect(self.addr);
    }
}

fn serve<F>(stream: TcpStream, handler: &F, requests: &Mutex<Vec<Req>>)
where
    F: Fn(&Req) -> Resp,
{
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut reader = BufReader::new(stream.try_clone().expect("clone"));
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
        return;
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();

    let mut content_length = 0usize;
    let mut authorization = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            match k.trim().to_ascii_lowercase().as_str() {
                "content-length" => content_length = v.trim().parse().unwrap_or(0),
                "authorization" => authorization = Some(v.trim().to_string()),
                _ => {}
            }
        }
    }
    let mut body = vec![0u8; content_length];
    let _ = reader.read_exact(&mut body);

    let req = Req {
        method,
        path,
        authorization,
        body: String::from_utf8_lossy(&body).into_owned(),
    };
    requests.lock().unwrap().push(req.clone());

    let mut out = stream;
    match handler(&req) {
        Resp::Json(status, body) => write_http(&mut out, status, "application/json", &body),
        Resp::Text(status, body) => write_http(&mut out, status, "text/html", &body),
        Resp::Raw(bytes) => {
            let _ = out.write_all(&bytes);
        }
        Resp::Hang(d) => thread::sleep(d),
    }
}

fn write_http(out: &mut TcpStream, status: u16, content_type: &str, body: &str) {
    let msg = format!(
        "HTTP/1.1 {status} X\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = out.write_all(msg.as_bytes());
}

/// 아무도 듣지 않는 로컬 포트의 주소를 돌려준다.
pub fn closed_port_url() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    format!("http://{addr}/v1")
}
