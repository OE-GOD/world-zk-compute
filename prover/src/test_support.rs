//! Helpers shared by unit tests

use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// A request received by [`serve_http`]
pub struct HttpRequest {
    pub path: String,
    pub body: Vec<u8>,
}

/// Serve HTTP on a free loopback port. `handler` maps each request to a
/// status code and a response body. The server runs until the runtime of the
/// calling test shuts down.
pub async fn serve_http<F>(handler: F) -> SocketAddr
where
    F: Fn(&HttpRequest) -> (u16, Vec<u8>) + Send + Sync + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handler = Arc::new(handler);

    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(serve_connection(stream, handler.clone()));
        }
    });

    addr
}

/// Answer requests on one connection until the client closes it
async fn serve_connection<F>(mut stream: TcpStream, handler: Arc<F>)
where
    F: Fn(&HttpRequest) -> (u16, Vec<u8>) + Send + Sync + 'static,
{
    let mut buf = Vec::new();

    loop {
        // Request line and headers
        let head_end = loop {
            if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break pos + 4;
            }
            if !read_more(&mut stream, &mut buf).await {
                return;
            }
        };
        let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
        let mut lines = head.lines();
        let request_line = lines.next().unwrap_or_default();
        let path = request_line
            .split_whitespace()
            .nth(1)
            .unwrap_or_default()
            .to_string();
        let content_length = lines
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .and_then(|(_, value)| value.trim().parse::<usize>().ok())
            .unwrap_or(0);

        // Body
        while buf.len() < head_end + content_length {
            if !read_more(&mut stream, &mut buf).await {
                return;
            }
        }
        let body = buf[head_end..head_end + content_length].to_vec();
        buf.drain(..head_end + content_length);

        let (status, response_body) = handler(&HttpRequest { path, body });
        let reason = if status == 200 { "OK" } else { "Error" };
        let mut response = format!(
            "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
            status,
            reason,
            response_body.len()
        )
        .into_bytes();
        response.extend_from_slice(&response_body);
        if stream.write_all(&response).await.is_err() {
            return;
        }
    }
}

/// Read more bytes into `buf`; false when the client is gone
async fn read_more(stream: &mut TcpStream, buf: &mut Vec<u8>) -> bool {
    let mut chunk = [0u8; 4096];
    match stream.read(&mut chunk).await {
        Ok(0) | Err(_) => false,
        Ok(n) => {
            buf.extend_from_slice(&chunk[..n]);
            true
        }
    }
}
