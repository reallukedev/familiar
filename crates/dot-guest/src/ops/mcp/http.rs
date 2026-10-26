//! MCP's Streamable HTTP transport, for servers on the Mac's own loopback address.
//!
//! Loopback only, so plain HTTP/1.1 is enough: one POST per message with
//! `Connection: close`, and an answer that is either JSON or an SSE stream.
use super::redact::{redact_text, redact_url};
use super::rpc::{self, Fail, Incoming};
use crate::error;
use dot_wire::{GuestError, GuestErrorCode};
use serde_json::Value;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::time::Instant;

/// Longest status line plus headers.
const MAX_HEAD: usize = 64 * 1024;
/// How much of the end of an error body is quoted in a message.
const MAX_ERROR_BODY: usize = 300;

/// A parsed loopback URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    /// `host:port` as written, for the `Host` header.
    authority: String,
    /// Where to connect, in order.
    addrs: Vec<SocketAddr>,
    /// Path and query, starting with `/`.
    path: String,
}

impl Endpoint {
    /// `http://` on `127.0.0.1`, `localhost` or `[::1]`, with no user info. Anything
    /// else (HTTPS, other hosts, names that only start with `localhost`) is `None`.
    pub fn parse_loopback(url: &str) -> Option<Self> {
        let scheme = url.get(..7)?;
        if !scheme.eq_ignore_ascii_case("http://") {
            return None;
        }
        let rest = &url[7..];
        let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let (authority, tail) = rest.split_at(end);
        if authority.contains('@') {
            return None;
        }
        let (host, port) = match authority.strip_prefix('[') {
            Some(inner) => {
                let (host, port) = inner.split_once(']')?;
                (host, port)
            }
            None => match authority.find(':') {
                Some(at) => authority.split_at(at),
                None => (authority, ""),
            },
        };
        let port = match port {
            "" => 80,
            port => port.strip_prefix(':')?.parse::<u16>().ok()?,
        };
        if port == 0 {
            return None;
        }
        let v4 = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let v6 = IpAddr::V6(Ipv6Addr::LOCALHOST);
        let ips = match host.to_ascii_lowercase().as_str() {
            "127.0.0.1" => vec![v4],
            "localhost" => vec![v4, v6],
            "::1" if authority.starts_with('[') => vec![v6],
            _ => return None,
        };
        let tail = tail.split('#').next().unwrap_or_default();
        let path = if tail.starts_with('/') {
            tail.to_owned()
        } else {
            format!("/{tail}")
        };
        Some(Self {
            authority: authority.to_owned(),
            addrs: ips
                .into_iter()
                .map(|ip| SocketAddr::new(ip, port))
                .collect(),
            path,
        })
    }

    /// Equal for URLs that reach the same server (`localhost` is `127.0.0.1`, a
    /// trailing `/` doesn't matter).
    pub fn same_server_key(&self) -> String {
        let host = match self.addrs.first() {
            Some(SocketAddr::V6(_)) => "[::1]".to_owned(),
            _ => "127.0.0.1".to_owned(),
        };
        let port = self.addrs.first().map_or(80, SocketAddr::port);
        format!("{host}:{port}{}", self.path.trim_end_matches('/'))
    }
}

/// A server that has finished the `initialize` handshake.
#[derive(Debug)]
pub struct Session {
    name: String,
    app: Option<String>,
    url: String,
    endpoint: Endpoint,
    session_id: Option<String>,
    protocol: String,
    next_id: u64,
    /// A handshake was started and not finished, so the session id may be missing.
    initializing: bool,
}

/// What one POST produced.
enum Posted {
    /// 202/204, or any success for a notification.
    Accepted,
    /// The answer to the request.
    Answer(Result<Value, String>),
    /// 404 for the session id we sent: the server forgot the session.
    Expired,
}

impl Session {
    /// Connect and run the handshake before `deadline`.
    pub async fn start(
        name: &str,
        app: Option<&str>,
        url: &str,
        deadline: Instant,
    ) -> Result<Self, GuestError> {
        let endpoint = Endpoint::parse_loopback(url).ok_or_else(|| {
            error::invalid(format!(
                "{name}'s address {url} is not on this Mac, so it can't be used."
            ))
        })?;
        let mut session = Self {
            name: name.to_owned(),
            app: app.map(ToOwned::to_owned),
            url: url.to_owned(),
            endpoint,
            session_id: None,
            protocol: rpc::PROTOCOL_VERSION.to_owned(),
            next_id: 0,
            initializing: false,
        };
        let outcome = tokio::time::timeout_at(deadline, session.initialize()).await;
        match outcome {
            Ok(Ok(())) => Ok(session),
            Ok(Err(Fail::Rpc(message))) => Err(error::failed(format!(
                "{name} would not start a session: {message}"
            ))),
            Ok(Err(Fail::Broken(e) | Fail::Timeout(e))) => Err(e),
            Err(_) => Err(error::timeout(format!(
                "{name} did not finish starting a session in time."
            ))),
        }
    }

    /// Whether the session is usable after a request was given up on midway. Each POST
    /// has its own connection, so only a cut-off handshake leaves it unusable.
    pub fn settled(&self) -> bool {
        !self.initializing
    }

    /// Send one request and wait for its answer until `deadline`. If the server forgot
    /// the session, starts a new one and tries once more. A timeout just drops that
    /// connection and keeps the session.
    pub async fn request(
        &mut self,
        method: &str,
        params: Value,
        deadline: Instant,
    ) -> Result<Value, Fail> {
        let outcome = tokio::time::timeout_at(deadline, self.request_now(method, params)).await;
        match outcome {
            Ok(result) => result,
            Err(_) => {
                let e = error::timeout(format!("{} did not answer in time.", self.name));
                Err(if self.settled() {
                    Fail::Timeout(e)
                } else {
                    Fail::Broken(e)
                })
            }
        }
    }

    async fn request_now(&mut self, method: &str, params: Value) -> Result<Value, Fail> {
        for attempt in 0..2 {
            self.next_id += 1;
            let sent = self.next_id;
            let msg = rpc::request(sent, method, params.clone());
            match self.post(&msg, Some(sent)).await? {
                Posted::Answer(outcome) => return outcome.map_err(Fail::Rpc),
                Posted::Expired if attempt == 0 => self.initialize().await?,
                Posted::Expired => break,
                Posted::Accepted => {
                    return Err(Fail::Broken(error::failed(format!(
                        "{} accepted the request but sent no answer.",
                        self.name
                    ))));
                }
            }
        }
        Err(Fail::Broken(error::failed(format!(
            "{} keeps forgetting its session.",
            self.name
        ))))
    }

    async fn initialize(&mut self) -> Result<(), Fail> {
        self.initializing = true;
        self.session_id = None;
        self.next_id += 1;
        let sent = self.next_id;
        let msg = rpc::request(sent, "initialize", rpc::initialize_params());
        let result = match self.post(&msg, Some(sent)).await? {
            Posted::Answer(Ok(result)) => result,
            Posted::Answer(Err(message)) => return Err(Fail::Rpc(message)),
            Posted::Accepted | Posted::Expired => {
                return Err(Fail::Broken(error::failed(format!(
                    "{} did not answer the start of a session.",
                    self.name
                ))));
            }
        };
        if let Some(version) = result.get("protocolVersion").and_then(Value::as_str)
            && is_header_safe(version)
        {
            version.clone_into(&mut self.protocol);
        }
        match self
            .post(&rpc::notification("notifications/initialized"), None)
            .await?
        {
            Posted::Expired => Err(Fail::Broken(error::failed(format!(
                "{} forgot its session right after starting it.",
                self.name
            )))),
            Posted::Accepted | Posted::Answer(_) => {
                self.initializing = false;
                Ok(())
            }
        }
    }

    /// Connect and send one POST of `body`.
    async fn send(&self, body: &[u8]) -> Result<BufReader<TcpStream>, Fail> {
        let mut head = format!(
            "POST {} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nMCP-Protocol-Version: {}\r\nConnection: close\r\nContent-Length: {}\r\n",
            self.endpoint.path,
            self.endpoint.authority,
            self.protocol,
            body.len()
        );
        if let Some(id) = &self.session_id {
            head.push_str(&format!("Mcp-Session-Id: {id}\r\n"));
        }
        head.push_str("\r\n");
        let stream = self.connect().await?;
        let mut stream = BufReader::new(stream);
        let written = async {
            let socket = stream.get_mut();
            socket.write_all(head.as_bytes()).await?;
            socket.write_all(body).await?;
            socket.flush().await
        };
        let result = written.await;
        result.map_err(|e| self.lost(&e))?;
        Ok(stream)
    }

    /// POST one message. `sent` is the id to wait for, `None` for a notification.
    async fn post(&mut self, msg: &Value, sent: Option<u64>) -> Result<Posted, Fail> {
        let body = serde_json::to_vec(msg)
            .map_err(|e| Fail::Broken(error::failed(format!("Could not encode a message: {e}"))))?;
        let mut stream = self.send(&body).await?;
        let response = read_head(&mut stream).await.map_err(|e| self.lost(&e))?;

        if response.status == 404 && self.session_id.is_some() {
            return Ok(Posted::Expired);
        }
        if let Some(id) = response.header("mcp-session-id")
            && is_header_safe(id)
        {
            self.session_id = Some(id.to_owned());
        }
        let content_type = response
            .header("content-type")
            .unwrap_or_default()
            .to_ascii_lowercase();
        let sse = content_type.starts_with("text/event-stream");
        let mut body = Body::new(&mut stream, &response, &self.name, !sse)?;
        if !(200..300).contains(&response.status) {
            let snippet = body.snippet().await;
            let mut message = format!("{} answered HTTP {}", self.name, response.status);
            if !snippet.is_empty() {
                message.push_str(&format!(": {snippet}"));
            }
            return Err(Fail::Broken(error::failed(message)));
        }
        let Some(sent) = sent else {
            return Ok(Posted::Accepted);
        };
        if matches!(response.status, 202 | 204) {
            return Ok(Posted::Accepted);
        }
        let answer = if sse {
            body.sse_answer(sent, self).await?
        } else {
            let bytes = body.all().await?;
            serde_json::from_slice::<Value>(&bytes)
                .ok()
                .and_then(|msg| rpc::pick(msg, sent))
        };
        match answer {
            Some(outcome) => Ok(Posted::Answer(outcome)),
            None => Err(Fail::Broken(error::failed(format!(
                "{} answered without a result.",
                self.name
            )))),
        }
    }

    /// Answer a request the server sent on an event stream, in a POST of its own.
    /// Best effort: if the server doesn't take it, it will say so in its answer.
    async fn reply(&self, msg: &Value) {
        let Ok(body) = serde_json::to_vec(msg) else {
            return;
        };
        if let Ok(mut stream) = self.send(&body).await {
            let _ = read_head(&mut stream).await;
        }
    }

    async fn connect(&self) -> Result<TcpStream, Fail> {
        let mut last = None;
        for addr in &self.endpoint.addrs {
            match TcpStream::connect(addr).await {
                Ok(stream) => return Ok(stream),
                Err(e) => last = Some(e),
            }
        }
        let refused = last
            .as_ref()
            .is_none_or(|e| e.kind() == io::ErrorKind::ConnectionRefused);
        if refused {
            let hint = match &self.app {
                Some(app) => format!("Open {app} and turn on its MCP server, then try again."),
                None => format!("Start {}, then try again.", self.name),
            };
            return Err(Fail::Broken(
                error::not_found(format!(
                    "Nothing is answering at {}, so {} isn't running right now.",
                    redact_url(&self.url),
                    self.name
                ))
                .hint(hint),
            ));
        }
        let detail = last.map(|e| e.to_string()).unwrap_or_default();
        Err(Fail::Broken(error::failed(format!(
            "Could not connect to {} at {}: {detail}",
            self.name,
            redact_url(&self.url)
        ))))
    }

    fn lost(&self, e: &io::Error) -> Fail {
        Fail::Broken(error::failed(format!(
            "Lost the connection to {}: {e}",
            self.name
        )))
    }
}

/// Visible ASCII only, so a server can't inject header lines through what we echo.
fn is_header_safe(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|b| (0x21..=0x7e).contains(&b))
}

struct Head {
    status: u16,
    headers: Vec<(String, String)>,
}

impl Head {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

/// Status line and headers, skipping `1xx` interim responses.
async fn read_head(stream: &mut BufReader<TcpStream>) -> io::Result<Head> {
    let mut budget = MAX_HEAD;
    loop {
        let status_line = read_crlf_line(stream, &mut budget).await?;
        let status = status_line
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse::<u16>().ok())
            .filter(|_| status_line.starts_with("HTTP/1."))
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "not an HTTP response"))?;
        let mut headers = Vec::new();
        loop {
            let line = read_crlf_line(stream, &mut budget).await?;
            if line.is_empty() {
                break;
            }
            if let Some((name, value)) = line.split_once(':') {
                headers.push((name.trim().to_ascii_lowercase(), value.trim().to_owned()));
            }
        }
        if !(100..200).contains(&status) {
            return Ok(Head { status, headers });
        }
    }
}

/// One line without its line ending, within what's left of `budget`.
async fn read_crlf_line(
    stream: &mut BufReader<TcpStream>,
    budget: &mut usize,
) -> io::Result<String> {
    let mut line = Vec::new();
    let limit = u64::try_from(*budget).unwrap_or(u64::MAX);
    let n = (&mut *stream)
        .take(limit)
        .read_until(b'\n', &mut line)
        .await?;
    if line.last() != Some(&b'\n') {
        let kind = if n == 0 || line.len() < *budget {
            io::ErrorKind::UnexpectedEof
        } else {
            io::ErrorKind::InvalidData
        };
        return Err(io::Error::new(
            kind,
            "the response headers were cut off or too long",
        ));
    }
    *budget -= line.len();
    line.pop();
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    Ok(String::from_utf8_lossy(&line).into_owned())
}

/// How the body is framed.
#[derive(Clone, Copy)]
enum Framing {
    /// `Content-Length` bytes left.
    Length(usize),
    /// Before a chunk's size line.
    ChunkSize,
    /// Bytes left in the current chunk.
    Chunk(usize),
    /// Until the server closes the connection.
    UntilClose,
    /// Nothing left.
    Done,
}

/// A response body. A JSON body is capped at [`rpc::MAX_MESSAGE`] in all; an event
/// stream is capped per event instead (see [`Sse`]), since a long stream of progress
/// notifications is fine. The call's deadline bounds both.
struct Body<'a> {
    stream: &'a mut BufReader<TcpStream>,
    framing: Framing,
    total: usize,
    capped: bool,
    name: String,
}

impl<'a> Body<'a> {
    fn new(
        stream: &'a mut BufReader<TcpStream>,
        head: &Head,
        name: &str,
        capped: bool,
    ) -> Result<Self, Fail> {
        let chunked = head
            .header("transfer-encoding")
            .is_some_and(|te| te.to_ascii_lowercase().contains("chunked"));
        let framing = if matches!(head.status, 204 | 304) {
            Framing::Done
        } else if chunked {
            Framing::ChunkSize
        } else if let Some(length) = head.header("content-length") {
            let length = length.parse::<u64>().map_err(|_| {
                Fail::Broken(error::failed(format!("{name} sent a malformed answer.")))
            })?;
            match usize::try_from(length) {
                Ok(length) if length <= rpc::MAX_MESSAGE || !capped => Framing::Length(length),
                _ => return Err(Fail::Broken(too_large(name))),
            }
        } else {
            Framing::UntilClose
        };
        Ok(Self {
            stream,
            framing,
            total: 0,
            capped,
            name: name.to_owned(),
        })
    }

    /// The next piece of the body, or `None` at its end.
    async fn next(&mut self) -> Result<Option<Vec<u8>>, Fail> {
        let piece = self.next_raw().await.map_err(|e| {
            Fail::Broken(error::failed(format!(
                "Lost the connection to {} while it answered: {e}",
                self.name
            )))
        })?;
        if let Some(piece) = &piece {
            self.total = self.total.saturating_add(piece.len());
            if self.capped && self.total > rpc::MAX_MESSAGE {
                return Err(Fail::Broken(too_large(&self.name)));
            }
        }
        Ok(piece)
    }

    async fn next_raw(&mut self) -> io::Result<Option<Vec<u8>>> {
        loop {
            match self.framing {
                Framing::Done | Framing::Length(0) => {
                    self.framing = Framing::Done;
                    return Ok(None);
                }
                Framing::Length(left) => {
                    let piece = self.read_some(left).await?;
                    if piece.is_empty() {
                        return Err(io::ErrorKind::UnexpectedEof.into());
                    }
                    self.framing = Framing::Length(left - piece.len());
                    return Ok(Some(piece));
                }
                Framing::UntilClose => {
                    let piece = self.read_some(64 * 1024).await?;
                    if piece.is_empty() {
                        self.framing = Framing::Done;
                        return Ok(None);
                    }
                    return Ok(Some(piece));
                }
                Framing::ChunkSize => {
                    let mut budget = 1024;
                    let line = read_crlf_line(self.stream, &mut budget).await?;
                    let size = line.split(';').next().unwrap_or_default().trim();
                    let size = usize::from_str_radix(size, 16).map_err(|_| {
                        io::Error::new(io::ErrorKind::InvalidData, "bad chunk size")
                    })?;
                    if size == 0 {
                        // Trailers, then the blank line. A server that just closes is fine.
                        let mut budget = MAX_HEAD;
                        while let Ok(line) = read_crlf_line(self.stream, &mut budget).await {
                            if line.is_empty() {
                                break;
                            }
                        }
                        self.framing = Framing::Done;
                        return Ok(None);
                    }
                    self.framing = Framing::Chunk(size);
                }
                Framing::Chunk(left) => {
                    let piece = self.read_some(left).await?;
                    if piece.is_empty() {
                        return Err(io::ErrorKind::UnexpectedEof.into());
                    }
                    let left = left - piece.len();
                    if left == 0 {
                        let mut budget = 16;
                        read_crlf_line(self.stream, &mut budget).await?;
                        self.framing = Framing::ChunkSize;
                    } else {
                        self.framing = Framing::Chunk(left);
                    }
                    return Ok(Some(piece));
                }
            }
        }
    }

    async fn read_some(&mut self, max: usize) -> io::Result<Vec<u8>> {
        let mut buf = vec![0u8; max.min(64 * 1024)];
        let n = self.stream.read(&mut buf).await?;
        buf.truncate(n);
        Ok(buf)
    }

    async fn all(&mut self) -> Result<Vec<u8>, Fail> {
        let mut out = Vec::new();
        while let Some(piece) = self.next().await? {
            out.extend_from_slice(&piece);
        }
        Ok(out)
    }

    /// The end of an error body, credentials hidden, for a message.
    async fn snippet(&mut self) -> String {
        let mut out = Vec::new();
        while out.len() < 4 * 1024 {
            match self.next().await {
                Ok(Some(piece)) => out.extend_from_slice(&piece),
                _ => break,
            }
        }
        redact_text(&String::from_utf8_lossy(&out), &[], MAX_ERROR_BODY)
    }

    /// Read server-sent events until the answer to `sent` arrives. Notifications are
    /// skipped; the server's own requests are answered like on stdio (`ping` with a
    /// result, anything else refused).
    async fn sse_answer(
        &mut self,
        sent: u64,
        session: &Session,
    ) -> Result<Option<Result<Value, String>>, Fail> {
        let mut events = Sse::default();
        let mut ready = Vec::new();
        loop {
            let piece = self.next().await?;
            let within = match &piece {
                Some(piece) => events.feed(piece, &mut ready),
                None => {
                    events.finish(&mut ready);
                    true
                }
            };
            if !within {
                return Err(Fail::Broken(too_large(&self.name)));
            }
            for data in ready.drain(..) {
                let Ok(msg) = serde_json::from_str::<Value>(&data) else {
                    continue;
                };
                let messages = match msg {
                    Value::Array(items) => items,
                    other => vec![other],
                };
                for msg in messages {
                    match rpc::classify(msg) {
                        Incoming::Response { id, outcome } if rpc::same_id(&id, sent) => {
                            return Ok(Some(outcome));
                        }
                        Incoming::Request { id, method } => {
                            session.reply(&rpc::answer(id, &method)).await;
                        }
                        Incoming::Response { .. } | Incoming::Other => {}
                    }
                }
            }
            if piece.is_none() {
                return Ok(None);
            }
        }
    }
}

fn too_large(name: &str) -> GuestError {
    GuestError::new(
        GuestErrorCode::TooLarge,
        format!(
            "{name}'s answer was larger than {} MB.",
            rpc::MAX_MESSAGE / (1024 * 1024)
        ),
    )
}

/// Splits a `text/event-stream` into each event's data.
#[derive(Default)]
struct Sse {
    /// The start of a line whose end hasn't arrived; never holds a `\n`.
    pending: Vec<u8>,
    data: Vec<String>,
    data_len: usize,
}

impl Sse {
    /// Add bytes; complete events go to `out`. False once one event is over the cap.
    fn feed(&mut self, bytes: &[u8], out: &mut Vec<String>) -> bool {
        // Only the new bytes can hold a line end: `pending` never does.
        let mut search = self.pending.len();
        self.pending.extend_from_slice(bytes);
        let mut start = 0;
        while let Some(at) = self.pending[search..].iter().position(|b| *b == b'\n') {
            let end = search + at;
            let mut line = &self.pending[start..end];
            if line.last() == Some(&b'\r') {
                line = &line[..line.len() - 1];
            }
            let line = String::from_utf8_lossy(line).into_owned();
            self.line(&line, out);
            start = end + 1;
            search = start;
        }
        self.pending.drain(..start);
        self.pending.len() + self.data_len <= rpc::MAX_MESSAGE
    }

    fn line(&mut self, line: &str, out: &mut Vec<String>) {
        if line.is_empty() {
            if !self.data.is_empty() {
                out.push(self.data.join("\n"));
                self.data.clear();
                self.data_len = 0;
            }
            return;
        }
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        if field == "data" {
            let value = value.strip_prefix(' ').unwrap_or(value);
            self.data_len += value.len() + 1;
            self.data.push(value.to_owned());
        }
    }

    fn finish(&mut self, out: &mut Vec<String>) {
        if !self.pending.is_empty() {
            let line = String::from_utf8_lossy(&std::mem::take(&mut self.pending)).into_owned();
            self.line(line.trim_end_matches('\r'), out);
        }
        self.line("", out);
    }
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::unwrap_used,
        reason = "Test assertions fail immediately on malformed setup"
    )]
    use super::*;
    use serde_json::json;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tokio::net::TcpListener;

    #[test]
    fn only_plain_http_on_loopback_hosts_is_accepted() {
        let ok = Endpoint::parse_loopback("HTTP://LocalHost:3845/mcp?x=1#frag").unwrap();
        assert_eq!(ok.authority, "LocalHost:3845");
        assert_eq!(ok.path, "/mcp?x=1");
        assert_eq!(ok.addrs.len(), 2);
        assert_eq!(
            Endpoint::parse_loopback("http://[::1]:7000").unwrap().path,
            "/"
        );
        for bad in [
            "https://127.0.0.1/mcp",
            "http://127.0.0.2/mcp",
            "http://localhost.evil.example/mcp",
            "http://127.0.0.1@evil.example/mcp",
            "http://user@localhost/mcp",
            "http://::1/mcp",
            "http://127.0.0.1:0/mcp",
            "http://127.0.0.1:99999/mcp",
            "http://0.0.0.0:3845/mcp",
            "ftp://127.0.0.1/",
        ] {
            assert_eq!(Endpoint::parse_loopback(bad), None, "{bad}");
        }
    }

    /// One request as the fake server saw it.
    struct Seen {
        head: String,
        body: Value,
    }

    type Respond = dyn Fn(&Seen, &[Seen]) -> String + Send + Sync;

    /// A loopback HTTP server that answers each POST with whatever `respond` returns,
    /// given the request and those before it. A reply starting with `SLOW:` is sent
    /// (without that prefix) after two seconds.
    async fn fake(respond: Box<Respond>) -> (String, Arc<Mutex<Vec<Seen>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::<Seen>::new()));
        let log = seen.clone();
        tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let mut stream = BufReader::new(stream);
                let mut head = String::new();
                loop {
                    let mut line = String::new();
                    stream.read_line(&mut line).await.unwrap();
                    if line == "\r\n" {
                        break;
                    }
                    head.push_str(&line.to_ascii_lowercase());
                }
                let length: usize = head
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length: "))
                    .unwrap()
                    .trim()
                    .parse()
                    .unwrap();
                let mut body = vec![0; length];
                stream.read_exact(&mut body).await.unwrap();
                let request = Seen {
                    head,
                    body: serde_json::from_slice(&body).unwrap(),
                };
                let reply = {
                    let mut log = log.lock().unwrap();
                    let reply = respond(&request, &log);
                    log.push(request);
                    reply
                };
                let reply = match reply.strip_prefix("SLOW:") {
                    Some(rest) => {
                        tokio::time::sleep(Duration::from_secs(2)).await;
                        rest.to_owned()
                    }
                    None => reply,
                };
                let socket = stream.get_mut();
                socket.write_all(reply.as_bytes()).await.unwrap();
                socket.shutdown().await.unwrap();
            }
        });
        (url, seen)
    }

    fn json_reply(extra: &str, body: &Value) -> String {
        let body = body.to_string();
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n{extra}Content-Length: {}\r\n\r\n{body}",
            body.len()
        )
    }

    fn method(seen: &Seen) -> &str {
        seen.body["method"].as_str().unwrap_or_default()
    }

    fn soon() -> Instant {
        Instant::now() + Duration::from_secs(10)
    }

    #[tokio::test]
    async fn json_sse_chunked_and_until_close_answers_keep_the_session_id() {
        let (url, seen) = fake(Box::new(|req, _| {
            let id = req.body["id"].clone();
            let known = req.head.contains("mcp-session-id: s-1\r\n");
            match method(req) {
                "initialize" => json_reply(
                    "Mcp-Session-Id: s-1\r\n",
                    &json!({"jsonrpc": "2.0", "id": id, "result": {"protocolVersion": "2025-03-26"}}),
                ),
                _ if !known => "HTTP/1.1 400 Bad Request\r\nContent-Length: 15\r\n\r\nno session here".into(),
                // Notifications, and our answer to the ping below.
                "notifications/initialized" | "" => "HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\n\r\n".into(),
                "tools/list" => {
                    // A progress notification and a ping first, then the answer split
                    // over two data lines, all cut into tiny chunks.
                    let events = format!(
                        ": keepalive\r\nevent: message\r\ndata: {}\r\n\r\ndata: {}\r\n\r\ndata: {{\"jsonrpc\":\"2.0\",\r\ndata: \"id\":{id},\"result\":{{\"tools\":[]}}}}\r\n\r\n",
                        json!({"jsonrpc": "2.0", "method": "notifications/progress", "params": {"progress": 1}}),
                        json!({"jsonrpc": "2.0", "id": "s9", "method": "ping"})
                    );
                    let mut reply = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n".to_owned();
                    for piece in events.as_bytes().chunks(7) {
                        reply.push_str(&format!("{:x}\r\n{}\r\n", piece.len(), String::from_utf8_lossy(piece)));
                    }
                    reply.push_str("0\r\n\r\n");
                    reply
                }
                _ => format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{}",
                    json!({"jsonrpc": "2.0", "id": id, "result": {"content": [{"type": "text", "text": "hi"}]}})
                ),
            }
        }))
        .await;

        let mut session = Session::start("Fake", None, &url, soon()).await.unwrap();
        let listed = session
            .request("tools/list", json!({}), soon())
            .await
            .unwrap();
        assert_eq!(listed, json!({"tools": []}));
        let called = session
            .request("tools/call", json!({"name": "x", "arguments": {}}), soon())
            .await
            .unwrap();
        assert_eq!(called["content"][0]["text"], "hi");

        let seen = seen.lock().unwrap();
        let methods: Vec<_> = seen.iter().map(method).collect();
        assert_eq!(
            methods,
            [
                "initialize",
                "notifications/initialized",
                "tools/list",
                "",
                "tools/call"
            ]
        );
        // The ping was answered in a POST of its own, inside the session.
        assert_eq!(
            seen[3].body,
            json!({"jsonrpc": "2.0", "id": "s9", "result": {}})
        );
        assert!(seen[3].head.contains("mcp-session-id: s-1\r\n"));
        // The version the server chose is sent from then on.
        assert!(
            seen[0]
                .head
                .contains("mcp-protocol-version: 2025-06-18\r\n")
        );
        assert!(
            seen[4]
                .head
                .contains("mcp-protocol-version: 2025-03-26\r\n")
        );
        assert!(
            seen[4]
                .head
                .contains("accept: application/json, text/event-stream\r\n")
        );
    }

    #[tokio::test]
    async fn a_forgotten_session_is_started_again_once() {
        let (url, seen) = fake(Box::new(|req, before| {
            let id = req.body["id"].clone();
            let inits = before.iter().filter(|s| method(s) == "initialize").count();
            let current = format!("mcp-session-id: s-{inits}\r\n");
            match method(req) {
                "initialize" => json_reply(
                    &format!("Mcp-Session-Id: s-{}\r\n", inits + 1),
                    &json!({"jsonrpc": "2.0", "id": id, "result": {}}),
                ),
                "notifications/initialized" => "HTTP/1.1 202 Accepted\r\n\r\n".into(),
                // The server restarts after the first session: s-1 is unknown now.
                _ if inits < 2 || !req.head.contains(&current) => {
                    "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".into()
                }
                _ => json_reply(
                    "",
                    &json!({"jsonrpc": "2.0", "id": id, "result": {"ok": inits}}),
                ),
            }
        }))
        .await;
        let mut session = Session::start("Fake", None, &url, soon()).await.unwrap();
        let result = session
            .request("tools/call", json!({"name": "x"}), soon())
            .await
            .unwrap();
        assert_eq!(result, json!({"ok": 2}));
        let inits = seen
            .lock()
            .unwrap()
            .iter()
            .filter(|s| method(s) == "initialize")
            .count();
        assert_eq!(inits, 2);
    }

    #[tokio::test]
    async fn a_timed_out_request_drops_its_connection_but_keeps_the_session() {
        let (url, seen) = fake(Box::new(|req, _| {
            let id = req.body["id"].clone();
            match (method(req), req.body["params"]["name"].as_str()) {
                ("initialize", _) => json_reply(
                    "Mcp-Session-Id: s-1\r\n",
                    &json!({"jsonrpc": "2.0", "id": id, "result": {}}),
                ),
                ("notifications/initialized", _) => "HTTP/1.1 202 Accepted\r\n\r\n".into(),
                (_, Some("slow")) => format!(
                    "SLOW:{}",
                    json_reply("", &json!({"jsonrpc": "2.0", "id": id, "result": {}}))
                ),
                _ => json_reply(
                    "",
                    &json!({"jsonrpc": "2.0", "id": id, "result": {"ok": true}}),
                ),
            }
        }))
        .await;
        let mut session = Session::start("Fake", None, &url, soon()).await.unwrap();
        let short = Instant::now() + Duration::from_millis(300);
        let err = session
            .request("tools/call", json!({"name": "slow"}), short)
            .await
            .unwrap_err();
        assert!(matches!(err, Fail::Timeout(e) if e.code == GuestErrorCode::Timeout));
        let result = session
            .request("tools/call", json!({"name": "fast"}), soon())
            .await
            .unwrap();
        assert_eq!(result, json!({"ok": true}));
        let seen = seen.lock().unwrap();
        assert_eq!(seen.iter().filter(|s| method(s) == "initialize").count(), 1);
        assert!(seen[3].head.contains("mcp-session-id: s-1\r\n"));
    }

    #[test]
    fn events_are_capped_one_at_a_time_not_in_total() {
        let mut sse = Sse::default();
        let mut out = Vec::new();
        let note = format!("data: {}\n\n", "n".repeat(1000));
        for _ in 0..(rpc::MAX_MESSAGE / 1000 + 10) {
            assert!(sse.feed(note.as_bytes(), &mut out));
        }
        assert!(out.len() > rpc::MAX_MESSAGE / 1000);
        let mut sse = Sse::default();
        let chunk = vec![b'x'; 1024 * 1024];
        assert!(sse.feed(b"data: ", &mut out));
        let fits = (0..17).all(|_| sse.feed(&chunk, &mut out));
        assert!(!fits);
    }

    #[tokio::test]
    async fn failures_are_explained_without_secrets() {
        let free = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", free.local_addr().unwrap());
        drop(free);
        let err = Session::start("Figma", Some("Figma"), &url, soon())
            .await
            .unwrap_err();
        assert_eq!(err.code, GuestErrorCode::NotFound);
        assert_eq!(
            err.hint.as_deref(),
            Some("Open Figma and turn on its MCP server, then try again.")
        );

        let (url, _) = fake(Box::new(|_, _| {
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 20000000\r\n\r\n{"
                .into()
        }))
        .await;
        let err = Session::start("Big", None, &url, soon()).await.unwrap_err();
        assert_eq!(err.code, GuestErrorCode::TooLarge);

        let (url, _) = fake(Box::new(|_, _| {
            let body = "<html>upstream said: invalid token ghp_abcdefghij1234 for me</html>";
            format!(
                "HTTP/1.1 500 Oops\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            )
        }))
        .await;
        let err = Session::start("Oops", None, &url, soon())
            .await
            .unwrap_err();
        assert_eq!(
            err.message,
            format!(
                "Oops answered HTTP 500: <html>upstream said: invalid token {} for me</html>",
                super::super::redact::HIDDEN
            )
        );
    }
}
