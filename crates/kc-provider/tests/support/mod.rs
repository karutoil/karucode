//! Minimal HTTP/1.1 mock server built directly on `tokio::net` so provider tests
//! never touch the network. Each test supplies a handler that owns the socket and
//! decides how/when to write the response.

#![allow(dead_code)]

use std::future::Future;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::io::AsyncReadExt;
pub use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};

#[derive(Debug, Clone)]
pub struct HttpRequest {
    head: String,
    pub body: Vec<u8>,
}

impl HttpRequest {
    pub fn method(&self) -> &str {
        self.head.split_whitespace().next().unwrap_or("")
    }

    pub fn path(&self) -> &str {
        self.head.split_whitespace().nth(1).unwrap_or("")
    }

    /// Case-insensitive header lookup.
    pub fn header(&self, name: &str) -> Option<String> {
        self.head
            .lines()
            .skip(1)
            .take_while(|line| !line.trim().is_empty())
            .filter_map(|line| line.split_once(':'))
            .find(|(key, _)| key.trim().eq_ignore_ascii_case(name))
            .map(|(_, value)| value.trim().to_string())
    }

    pub fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|e| panic!("request body is not JSON: {e}\n{}", self.text_body()))
    }

    pub fn text_body(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

pub struct MockServer {
    pub addr: SocketAddr,
    pub requests: Arc<Mutex<Vec<HttpRequest>>>,
    accept: tokio::task::JoinHandle<()>,
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.accept.abort();
    }
}

impl MockServer {
    /// First captured request; panics if the provider never called us.
    pub fn request(&self) -> HttpRequest {
        self.requests
            .lock()
            .expect("request log poisoned")
            .first()
            .cloned()
            .expect("no request reached the mock server")
    }

    pub async fn wait_for_request(&self) -> HttpRequest {
        for _ in 0..200 {
            if let Some(req) = self.requests.lock().expect("request log poisoned").first() {
                return req.clone();
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("no request reached the mock server");
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }
}

/// Spawn a listener whose handler receives each request plus the open socket.
/// The handler is responsible for writing the response and closing the socket.
pub async fn spawn<F, Fut>(handler: F) -> MockServer
where
    F: Fn(HttpRequest, TcpStream) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock server");
    let addr = listener.local_addr().expect("mock server addr");
    let requests: Arc<Mutex<Vec<HttpRequest>>> = Arc::new(Mutex::new(Vec::new()));
    let handler = Arc::new(handler);
    let logged = Arc::clone(&requests);
    let accept = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let handler = Arc::clone(&handler);
            let logged = Arc::clone(&logged);
            tokio::spawn(async move {
                if let Some(request) = read_request(&mut socket).await {
                    logged
                        .lock()
                        .expect("request log poisoned")
                        .push(request.clone());
                    handler(request, socket).await;
                }
            });
        }
    });
    MockServer {
        addr,
        requests,
        accept,
    }
}

async fn read_request(socket: &mut TcpStream) -> Option<HttpRequest> {
    let mut buf: Vec<u8> = Vec::new();
    let mut tmp = [0u8; 2048];
    let header_end = loop {
        let read = socket.read(&mut tmp).await.ok()?;
        if read == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..read]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
        if buf.len() > 128 * 1024 {
            return None;
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let content_length = head
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(key, _)| key.trim().eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = buf[header_end..].to_vec();
    while body.len() < content_length {
        let read = socket.read(&mut tmp).await.ok()?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&tmp[..read]);
    }
    Some(HttpRequest { head, body })
}

/// Response head for an SSE stream; the body ends when the connection closes.
pub const SSE_HEAD: &str = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncache-control: no-cache\r\nconnection: close\r\n\r\n";

pub fn response(status_line: &str, content_type: &str, body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 {status_line}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

pub async fn write_sse(socket: &mut TcpStream, body: &str) {
    socket.write_all(SSE_HEAD.as_bytes()).await.unwrap();
    socket.write_all(body.as_bytes()).await.unwrap();
    socket.flush().await.unwrap();
    // Closing the socket ends the response (no content-length was sent).
    let _ = socket.shutdown().await;
}
