//! Driving the browser through the DevTools protocol.
//!
//! [`Devtools`] lists, opens and closes targets over HTTP. [`Page`] attaches
//! to one and speaks the protocol to it: navigate to a URL, evaluate script in
//! the page, capture it, send input. None of it needs a display, and none of
//! it depends on coordinates from a screenshot.
//!
//! The WebSocket client is written here rather than taken as a dependency,
//! for one connection to loopback, and negotiates no compression extension.
//! Anything this does not wrap is reachable through [`Page::call`].

use crate::error::{Error, Result};
use crate::{BrowserEndpoint, Point};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// How long any one exchange with the browser may take.
pub const TIMEOUT: Duration = Duration::from_secs(30);

/// How many unread events a page keeps before it starts dropping the oldest.
pub const EVENT_QUEUE: usize = 512;

/// One page, window or worker the browser will let us attach to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub url: String,
    /// Where to attach. The host in it is the browser's own idea of where it
    /// is, which is not where a client out here reaches it.
    pub ws_path: String,
}

impl Target {
    fn from_json(value: &Value) -> Option<Self> {
        let ws = value.get("webSocketDebuggerUrl")?.as_str()?;
        // Keep the path, drop the authority: the browser reports the port it
        // listens on inside the box, and a client out here reaches a different
        // one.
        let path = ws.split_once("://")?.1;
        let path = path.split_once('/').map(|(_, rest)| rest)?;

        Some(Self {
            id: value.get("id")?.as_str()?.to_string(),
            kind: value
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            title: value
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            url: value
                .get("url")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            ws_path: format!("/{path}"),
        })
    }

    /// Whether this is a page rather than a worker or an extension.
    pub fn is_page(&self) -> bool {
        self.kind == "page"
    }
}

/// The browser's DevTools endpoint, as reached from this machine.
#[derive(Debug, Clone)]
pub struct Devtools {
    host: String,
    port: u16,
}

impl Devtools {
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
        }
    }

    /// From what [`crate::Computer::devtools`] reports.
    pub fn from_endpoint(endpoint: &BrowserEndpoint) -> Result<Self> {
        let authority = endpoint
            .http_url
            .split_once("://")
            .map(|(_, rest)| rest)
            .unwrap_or(&endpoint.http_url);
        let (host, port) = authority
            .split_once(':')
            .ok_or_else(|| Error::denied(format!("{} has no port", endpoint.http_url)))?;

        let port = port
            .trim_end_matches('/')
            .parse()
            .map_err(|_| Error::denied(format!("{port} is not a port")))?;

        Ok(Self::new(host, port))
    }

    /// What the browser says it is.
    pub async fn version(&self) -> Result<Value> {
        self.get("/json/version").await
    }

    /// Every target the browser will attach to.
    pub async fn targets(&self) -> Result<Vec<Target>> {
        let listed = self.get("/json/list").await?;
        Ok(listed
            .as_array()
            .map(|targets| targets.iter().filter_map(Target::from_json).collect())
            .unwrap_or_default())
    }

    /// The pages, in the order the browser reports them.
    pub async fn pages(&self) -> Result<Vec<Target>> {
        Ok(self
            .targets()
            .await?
            .into_iter()
            .filter(Target::is_page)
            .collect())
    }

    /// Open a new tab.
    pub async fn open(&self, url: &str) -> Result<Target> {
        // `PUT`, because Chromium stopped accepting `GET` on this endpoint.
        let value = self
            .request("PUT", &format!("/json/new?{}", escape(url)))
            .await?;

        Target::from_json(&value)
            .ok_or_else(|| Error::denied(format!("the browser answered {value}")))
    }

    pub async fn close(&self, target: &str) -> Result<()> {
        self.request("GET", &format!("/json/close/{target}"))
            .await
            .map(|_| ())
    }

    /// Attach to a target and speak the protocol to it.
    pub async fn attach(&self, target: &Target) -> Result<Page> {
        let socket = handshake(&self.host, self.port, &target.ws_path).await?;
        Ok(Page {
            socket,
            next: 1,
            target: target.clone(),
            events: VecDeque::new(),
            dropped: 0,
        })
    }

    /// Open a page and wait until it is really there.
    ///
    /// `/json/new?url=` answers as soon as the tab exists, and that tab shows
    /// `about:blank`, which is already loaded. `Page.navigate` answers once the
    /// navigation has committed.
    pub async fn open_page(&self, url: &str, within: Duration) -> Result<Page> {
        let target = self.open("about:blank").await?;
        let mut page = self.attach(&target).await?;

        page.navigate(url).await?;
        page.wait_for_load(within).await?;
        Ok(page)
    }

    /// Attach to the first page, opening one if the browser has none.
    pub async fn visible_page(&self) -> Result<Option<Page>> {
        for target in self.pages().await? {
            let mut page = self.attach(&target).await?;
            if page.visible().await.unwrap_or(false) {
                return Ok(Some(page));
            }
        }
        Ok(None)
    }

    pub async fn first_page(&self) -> Result<Page> {
        let target = match self.pages().await?.into_iter().next() {
            Some(target) => target,
            None => self.open("about:blank").await?,
        };
        self.attach(&target).await
    }

    async fn get(&self, path: &str) -> Result<Value> {
        self.request("GET", path).await
    }

    async fn request(&self, method: &str, path: &str) -> Result<Value> {
        let mut socket = connect(&self.host, self.port).await?;

        let request = format!(
            "{method} {path} HTTP/1.1\r\nHost: {}:{}\r\nConnection: close\r\n\r\n",
            self.host, self.port
        );
        socket
            .write_all(request.as_bytes())
            .await
            .map_err(|error| Error::transport(error.to_string(), true))?;

        let answer = read_http(&mut socket).await?;
        if answer.status != 200 {
            return Err(Error::denied(format!(
                "the browser answered {} to {path}: {}",
                answer.status, answer.body
            )));
        }

        // `/json/close` answers with a bare word rather than JSON.
        Ok(serde_json::from_str(&answer.body).unwrap_or(Value::String(answer.body)))
    }
}

/// A message the browser sent that nobody asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub method: String,
    pub params: Value,
}

/// One attached target.
pub struct Page {
    socket: TcpStream,
    next: u64,
    target: Target,
    /// Events that arrived while an answer was being waited for.
    events: VecDeque<Event>,
    /// How many were dropped because nobody drained them.
    dropped: usize,
}

impl Page {
    pub fn target(&self) -> &Target {
        &self.target
    }

    /// Any method in the protocol, and whatever it answers.
    ///
    /// Events arriving meanwhile are queued rather than returned.
    pub async fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next;
        self.next += 1;

        let request = json!({ "id": id, "method": method, "params": params });
        send_text(&mut self.socket, &request.to_string()).await?;

        let deadline = SystemTime::now() + TIMEOUT;
        loop {
            let frame = read_text(&mut self.socket).await?;
            let message: Value = serde_json::from_str(&frame)
                .map_err(|error| Error::transport(error.to_string(), false))?;

            if message.get("id").and_then(Value::as_u64) == Some(id) {
                if let Some(error) = message.get("error") {
                    return Err(Error::denied(format!("{method}: {error}")));
                }
                return Ok(message.get("result").cloned().unwrap_or(Value::Null));
            }

            self.remember(&message);

            if SystemTime::now() >= deadline {
                return Err(Error::Timeout {
                    after: TIMEOUT,
                    detail: format!("{method} was never answered"),
                });
            }
        }
    }

    /// How many events were dropped because the queue was full.
    ///
    /// The oldest go first, so a caller can tell nothing happened from not
    /// having listened.
    pub fn dropped_events(&self) -> usize {
        self.dropped
    }

    /// Everything the browser reported that nobody asked for, oldest first.
    pub fn take_events(&mut self) -> Vec<Event> {
        self.dropped = 0;
        self.events.drain(..).collect()
    }

    /// Wait for one event, keeping everything else that arrives meanwhile.
    pub async fn next_event(&mut self, method: &str, within: Duration) -> Result<Event> {
        if let Some(at) = self.events.iter().position(|event| event.method == method) {
            return Ok(self.events.remove(at).expect("just found"));
        }

        let deadline = SystemTime::now() + within;
        loop {
            let frame = read_text(&mut self.socket).await?;
            let message: Value = serde_json::from_str(&frame)
                .map_err(|error| Error::transport(error.to_string(), false))?;

            if message.get("method").and_then(Value::as_str) == Some(method) {
                return Ok(Event {
                    method: method.to_string(),
                    params: message.get("params").cloned().unwrap_or(Value::Null),
                });
            }
            self.remember(&message);

            if SystemTime::now() >= deadline {
                return Err(Error::Timeout {
                    after: within,
                    detail: format!("{method} never arrived"),
                });
            }
        }
    }

    fn remember(&mut self, message: &Value) {
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            return;
        };

        if self.events.len() >= EVENT_QUEUE {
            self.events.pop_front();
            self.dropped += 1;
        }
        self.events.push_back(Event {
            method: method.to_string(),
            params: message.get("params").cloned().unwrap_or(Value::Null),
        });
    }

    /// Go to a URL, whatever the address bar happens to show.
    pub async fn navigate(&mut self, url: &str) -> Result<()> {
        self.call("Page.enable", json!({})).await?;
        let answer = self.call("Page.navigate", json!({ "url": url })).await?;

        // A navigation the browser refused answers with an error text rather
        // than a failure, and treating that as success leaves the caller
        // looking at the previous page believing it is the new one.
        match answer.get("errorText").and_then(Value::as_str) {
            Some(error) => Err(Error::denied(format!("{url}: {error}"))),
            None => Ok(()),
        }
    }

    /// Put this page in front, so screen coordinates address it.
    ///
    /// The desktop API points at pixels rather than pages: a click goes to
    /// whichever tab is frontmost.
    pub async fn bring_to_front(&mut self) -> Result<()> {
        self.call("Page.bringToFront", json!({})).await.map(|_| ())
    }

    /// Whether this page is the one on screen.
    ///
    /// Asked of the page rather than worked out from the order tabs are listed
    /// in.
    pub async fn visible(&mut self) -> Result<bool> {
        Ok(self.evaluate("document.visibilityState").await? == "visible")
    }

    /// Run JavaScript in the page, and bring the value back.
    pub async fn evaluate(&mut self, javascript: &str) -> Result<Value> {
        let answer = self
            .call(
                "Runtime.evaluate",
                json!({
                    "expression": javascript,
                    "returnByValue": true,
                    "awaitPromise": true,
                }),
            )
            .await?;

        if let Some(thrown) = answer.get("exceptionDetails") {
            return Err(Error::denied(format!("the page threw: {thrown}")));
        }
        Ok(answer
            .get("result")
            .and_then(|result| result.get("value"))
            .cloned()
            .unwrap_or(Value::Null))
    }

    /// The current URL, asked of the page rather than read off a screenshot.
    pub async fn url(&mut self) -> Result<String> {
        Ok(self
            .evaluate("location.href")
            .await?
            .as_str()
            .unwrap_or_default()
            .to_string())
    }

    pub async fn title(&mut self) -> Result<String> {
        Ok(self
            .evaluate("document.title")
            .await?
            .as_str()
            .unwrap_or_default()
            .to_string())
    }

    /// Wait until the document has finished loading.
    pub async fn wait_for_load(&mut self, within: Duration) -> Result<()> {
        let deadline = SystemTime::now() + within;

        loop {
            let state = self.evaluate("document.readyState").await?;
            if state.as_str() == Some("complete") {
                return Ok(());
            }
            if SystemTime::now() >= deadline {
                return Err(Error::Timeout {
                    after: within,
                    detail: format!("document.readyState is {state}"),
                });
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    /// A capture from the browser rather than from the screen.
    ///
    /// The page as it renders: no window frame, no address bar, no pointer, and
    /// the same on a box with no display.
    pub async fn screenshot(&mut self) -> Result<Vec<u8>> {
        let answer = self
            .call("Page.captureScreenshot", json!({ "format": "png" }))
            .await?;

        let encoded = answer
            .get("data")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::denied("the capture came back with no image"))?;

        base64_decode(encoded).ok_or_else(|| Error::denied("the capture is not valid base64"))
    }

    /// A click in the page's own coordinates, where `0,0` is the top left of the
    /// viewport.
    pub async fn click(&mut self, at: Point) -> Result<()> {
        for kind in ["mousePressed", "mouseReleased"] {
            self.call(
                "Input.dispatchMouseEvent",
                json!({
                    "type": kind,
                    "x": at.x,
                    "y": at.y,
                    "button": "left",
                    "clickCount": 1,
                }),
            )
            .await?;
        }
        Ok(())
    }

    /// Type text into whatever the page has focused.
    pub async fn type_text(&mut self, text: &str) -> Result<()> {
        for character in text.chars() {
            self.call("Input.insertText", json!({ "text": character.to_string() }))
                .await?;
        }
        Ok(())
    }
}

async fn connect(host: &str, port: u16) -> Result<TcpStream> {
    TcpStream::connect((host, port))
        .await
        .map_err(|error| Error::transport(format!("{host}:{port}: {error}"), true))
}

struct HttpAnswer {
    status: u16,
    body: String,
}

/// Enough HTTP to talk to a debugger on loopback.
async fn read_http(socket: &mut TcpStream) -> Result<HttpAnswer> {
    let mut raw = Vec::new();
    let mut buffer = [0u8; 4096];

    loop {
        match tokio::time::timeout(TIMEOUT, socket.read(&mut buffer)).await {
            Ok(Ok(0)) | Err(_) => break,
            Ok(Ok(read)) => raw.extend_from_slice(&buffer[..read]),
            Ok(Err(error)) => return Err(Error::transport(error.to_string(), true)),
        }

        // `Connection: close` means end of file marks the end of the body, but
        // a body that already arrived whole should not wait for the close.
        if let Some(answer) = parse_http(&raw) {
            if answer.complete {
                return Ok(HttpAnswer {
                    status: answer.status,
                    body: answer.body,
                });
            }
        }
    }

    parse_http(&raw)
        .map(|answer| HttpAnswer {
            status: answer.status,
            body: answer.body,
        })
        .ok_or_else(|| Error::transport("the browser closed without answering", true))
}

struct ParsedHttp {
    status: u16,
    body: String,
    complete: bool,
}

fn parse_http(raw: &[u8]) -> Option<ParsedHttp> {
    let text = String::from_utf8_lossy(raw);
    let (head, body) = text.split_once("\r\n\r\n")?;

    let status = head
        .lines()
        .next()?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()?;

    let length = head.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("content-length")
            .then(|| value.trim().parse::<usize>().ok())?
    });

    Some(ParsedHttp {
        status,
        complete: length.map(|length| body.len() >= length).unwrap_or(false),
        body: body.to_string(),
    })
}

/// The websocket upgrade.
///
/// The accept header is not checked: this is a debugger on loopback that we
/// opened, and a wrong answer shows up as a frame that will not parse.
async fn handshake(host: &str, port: u16, path: &str) -> Result<TcpStream> {
    let mut socket = connect(host, port).await?;

    let key = base64_encode(&nonce());
    let request = format!(
        "GET {path} HTTP/1.1\r\n\
         Host: {host}:{port}\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Key: {key}\r\n\
         Sec-WebSocket-Version: 13\r\n\r\n"
    );
    socket
        .write_all(request.as_bytes())
        .await
        .map_err(|error| Error::transport(error.to_string(), true))?;

    // Read exactly the head, and not a byte more: whatever follows the blank
    // line is the first frame, and swallowing it here would lose it.
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        match tokio::time::timeout(TIMEOUT, socket.read_exact(&mut byte)).await {
            Ok(Ok(_)) => head.push(byte[0]),
            Ok(Err(error)) => return Err(Error::transport(error.to_string(), true)),
            Err(_) => return Err(Error::transport("the upgrade was never answered", true)),
        }
    }

    let head = String::from_utf8_lossy(&head);
    if !head.starts_with("HTTP/1.1 101") {
        let status = head.lines().next().unwrap_or_default();
        return Err(Error::denied(format!(
            "the browser refused to upgrade: {status}"
        )));
    }

    Ok(socket)
}

/// A client frame, which the protocol requires to be masked.
async fn send_text(socket: &mut TcpStream, text: &str) -> Result<()> {
    let payload = text.as_bytes();
    let mut frame = vec![0x81u8]; // FIN, text

    let mask_bit = 0x80;
    match payload.len() {
        length if length < 126 => frame.push(mask_bit | length as u8),
        length if length <= u16::MAX as usize => {
            frame.push(mask_bit | 126);
            frame.extend_from_slice(&(length as u16).to_be_bytes());
        }
        length => {
            frame.push(mask_bit | 127);
            frame.extend_from_slice(&(length as u64).to_be_bytes());
        }
    }

    let mask = nonce()[..4].to_vec();
    frame.extend_from_slice(&mask);
    frame.extend(
        payload
            .iter()
            .zip(mask.iter().cycle())
            .map(|(byte, key)| byte ^ key),
    );

    socket
        .write_all(&frame)
        .await
        .map_err(|error| Error::transport(error.to_string(), true))
}

/// The next text message, answering pings and skipping anything else.
async fn read_text(socket: &mut TcpStream) -> Result<String> {
    let mut assembled = Vec::new();

    loop {
        let mut header = [0u8; 2];
        read_exact(socket, &mut header).await?;

        let final_frame = header[0] & 0x80 != 0;
        let opcode = header[0] & 0x0f;
        let masked = header[1] & 0x80 != 0;

        let length = match header[1] & 0x7f {
            126 => {
                let mut extended = [0u8; 2];
                read_exact(socket, &mut extended).await?;
                u16::from_be_bytes(extended) as usize
            }
            127 => {
                let mut extended = [0u8; 8];
                read_exact(socket, &mut extended).await?;
                u64::from_be_bytes(extended) as usize
            }
            short => short as usize,
        };

        let mut mask = [0u8; 4];
        if masked {
            read_exact(socket, &mut mask).await?;
        }

        let mut payload = vec![0u8; length];
        if length > 0 {
            read_exact(socket, &mut payload).await?;
        }
        if masked {
            for (at, byte) in payload.iter_mut().enumerate() {
                *byte ^= mask[at % 4];
            }
        }

        match opcode {
            // Continuation, text, binary: all part of a message.
            0x0..=0x2 => {
                assembled.extend_from_slice(&payload);
                if final_frame {
                    return String::from_utf8(assembled)
                        .map_err(|error| Error::transport(error.to_string(), false));
                }
            }
            // A ping, which has to be answered to keep the connection open.
            0x9 => {
                let mut pong = vec![0x8au8, 0x80 | payload.len() as u8];
                let mask = nonce()[..4].to_vec();
                pong.extend_from_slice(&mask);
                pong.extend(
                    payload
                        .iter()
                        .zip(mask.iter().cycle())
                        .map(|(byte, key)| byte ^ key),
                );
                socket
                    .write_all(&pong)
                    .await
                    .map_err(|error| Error::transport(error.to_string(), true))?;
            }
            0xa => {}
            0x8 => return Err(Error::transport("the browser closed the connection", true)),
            other => {
                return Err(Error::transport(format!("unknown frame {other}"), false));
            }
        }
    }
}

async fn read_exact(socket: &mut TcpStream, into: &mut [u8]) -> Result<()> {
    match tokio::time::timeout(TIMEOUT, socket.read_exact(into)).await {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(error)) => Err(Error::transport(error.to_string(), true)),
        Err(_) => Err(Error::Timeout {
            after: TIMEOUT,
            detail: "the browser stopped mid-frame".to_string(),
        }),
    }
}

/// Sixteen bytes nobody can predict from the last sixteen.
///
/// Not a cryptographic nonce: the mask exists so a proxy cannot be tricked
/// into caching a frame, and this connection goes to loopback.
fn nonce() -> [u8; 16] {
    // A counter as well as the clock: two calls in the same tick would
    // otherwise mask two frames with the same key.
    static COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    let mut seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_nanos() as u64)
        .unwrap_or(0x2545_f491_4f6c_dd1d)
        ^ COUNT
            .fetch_add(0x9e37_79b9_7f4a_7c15, std::sync::atomic::Ordering::Relaxed)
            .rotate_left(17);

    let mut bytes = [0u8; 16];
    for byte in bytes.iter_mut() {
        // xorshift64. Small, deterministic from the clock, and enough.
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        *byte = (seed & 0xff) as u8;
    }
    bytes
}

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);

    for chunk in bytes.chunks(3) {
        let block = ((chunk[0] as u32) << 16)
            | ((*chunk.get(1).unwrap_or(&0) as u32) << 8)
            | (*chunk.get(2).unwrap_or(&0) as u32);

        for at in 0..4 {
            if at <= chunk.len() {
                let index = ((block >> (18 - at * 6)) & 0x3f) as usize;
                out.push(ALPHABET[index] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

pub fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let mut block = 0u32;
    let mut held = 0;

    for byte in text.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' | b'\n' | b'\r' => continue,
            _ => return None,
        };

        block = (block << 6) | u32::from(value);
        held += 1;

        if held == 4 {
            out.push((block >> 16) as u8);
            out.push((block >> 8) as u8);
            out.push(block as u8);
            block = 0;
            held = 0;
        }
    }

    match held {
        0 => Some(out),
        2 => {
            out.push((block >> 4) as u8);
            Some(out)
        }
        3 => {
            out.push((block >> 10) as u8);
            out.push((block >> 2) as u8);
            Some(out)
        }
        _ => None,
    }
}

/// A URL as a query string value.
fn escape(url: &str) -> String {
    let mut out = String::with_capacity(url.len());
    for byte in url.bytes() {
        match byte {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'~'
            | b':'
            | b'/'
            | b'?'
            | b'='
            | b'&'
            | b'#' => out.push(byte as char),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_base64_survives_a_round_trip() {
        for original in [
            b"".to_vec(),
            b"f".to_vec(),
            b"fo".to_vec(),
            b"foo".to_vec(),
            b"foob".to_vec(),
            b"fooba".to_vec(),
            b"foobar".to_vec(),
            vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a],
        ] {
            let encoded = base64_encode(&original);
            assert_eq!(
                base64_decode(&encoded).as_deref(),
                Some(original.as_slice()),
                "{encoded}"
            );
        }
    }

    #[test]
    fn test_base64_matches_the_known_answers() {
        assert_eq!(base64_encode(b"Man"), "TWFu");
        assert_eq!(base64_encode(b"Ma"), "TWE=");
        assert_eq!(base64_encode(b"M"), "TQ==");
        assert_eq!(base64_decode("TWFu").as_deref(), Some(&b"Man"[..]));
    }

    #[test]
    fn test_a_target_keeps_the_path_and_drops_the_port_the_browser_reported() {
        let target = Target::from_json(&json!({
            "id": "ABC",
            "type": "page",
            "title": "Example",
            "url": "https://example.com/",
            "webSocketDebuggerUrl": "ws://127.0.0.1:9222/devtools/page/ABC",
        }))
        .expect("a target");

        assert_eq!(
            target.ws_path, "/devtools/page/ABC",
            "9222 is where the browser listens inside the box, and a client \
             out here reaches a different port"
        );
        assert!(target.is_page());
    }

    #[test]
    fn test_a_target_without_a_socket_is_not_a_target() {
        assert_eq!(
            Target::from_json(&json!({ "id": "ABC", "type": "page" })),
            None,
            "there is nothing to attach to"
        );
    }

    #[test]
    fn test_an_endpoint_is_split_into_a_host_and_a_port() {
        let endpoint = BrowserEndpoint {
            http_url: "http://127.0.0.1:49632".to_string(),
            ws_url: "ws://127.0.0.1:49632/devtools/browser".to_string(),
        };
        let devtools = Devtools::from_endpoint(&endpoint).expect("an endpoint");

        assert_eq!(devtools.host, "127.0.0.1");
        assert_eq!(devtools.port, 49632);
    }

    #[test]
    fn test_a_url_is_escaped_before_it_goes_in_a_query() {
        assert_eq!(escape("https://a.dev/x?y=1"), "https://a.dev/x?y=1");
        assert_eq!(escape("https://a.dev/a b"), "https://a.dev/a%20b");
    }

    #[test]
    fn test_a_nonce_is_not_the_same_twice() {
        assert_ne!(nonce(), nonce());
    }

    #[test]
    fn test_a_body_that_already_arrived_is_not_waited_on() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}";
        let parsed = parse_http(raw).expect("an answer");

        assert_eq!(parsed.status, 200);
        assert!(parsed.complete);
        assert_eq!(parsed.body, "{}");
    }

    #[test]
    fn test_a_body_still_arriving_is_not_complete() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 20\r\n\r\n{}";
        assert!(!parse_http(raw).expect("an answer").complete);
    }
}
