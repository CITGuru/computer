use crate::error::{Error, Result};
use crate::motion::{Step, path};
use crate::{BrowserEndpoint, Button, Point};
use computer_types::Motion;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub const TIMEOUT: Duration = Duration::from_secs(30);

pub const EVENT_QUEUE: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub url: String,
    pub ws_path: String,
}

/// `/json/list` HTML-escapes titles.
fn unescaped(title: &str) -> String {
    const NAMED: [(&str, char); 5] = [
        ("amp;", '&'),
        ("lt;", '<'),
        ("gt;", '>'),
        ("quot;", '"'),
        ("#39;", '\''),
    ];

    let mut pieces = title.split('&');
    let mut out = String::with_capacity(title.len());
    out.push_str(pieces.next().unwrap_or_default());

    for piece in pieces {
        let named = NAMED
            .iter()
            .find_map(|(entity, plain)| piece.strip_prefix(entity).map(|tail| (*plain, tail)));

        match named {
            Some((plain, tail)) => {
                out.push(plain);
                out.push_str(tail);
            }
            None => {
                out.push('&');
                out.push_str(piece);
            }
        }
    }

    out
}

impl Target {
    fn from_json(value: &Value) -> Option<Self> {
        let ws = value.get("webSocketDebuggerUrl")?.as_str()?;

        Some(Self {
            id: value.get("id")?.as_str()?.to_string(),
            kind: value
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            title: unescaped(
                value
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            ),
            url: value
                .get("url")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            ws_path: websocket_path(ws)?,
        })
    }

    pub fn is_page(&self) -> bool {
        self.kind == "page"
    }
}

fn websocket_path(url: &str) -> Option<String> {
    let with_authority = url.split_once("://")?.1;
    let (_, path) = with_authority.split_once('/')?;
    Some(format!("/{path}"))
}

#[derive(Debug, Clone)]
pub struct Devtools {
    host: String,
    port: u16,
    browser_path: Arc<OnceLock<String>>,
}

impl Devtools {
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
            browser_path: Arc::new(OnceLock::new()),
        }
    }

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

    pub async fn version(&self) -> Result<Value> {
        self.get("/json/version").await
    }

    pub async fn http(&self, method: &str, path: &str) -> Result<Value> {
        self.request(method, path).await
    }

    pub fn socket_url(&self, path: &str) -> String {
        format!("ws://{}:{}{path}", self.host, self.port)
    }

    pub async fn targets(&self) -> Result<Vec<Target>> {
        let listed = self.get("/json/list").await?;
        Ok(listed
            .as_array()
            .map(|targets| targets.iter().filter_map(Target::from_json).collect())
            .unwrap_or_default())
    }

    pub async fn pages(&self) -> Result<Vec<Target>> {
        Ok(self
            .targets()
            .await?
            .into_iter()
            .filter(Target::is_page)
            .collect())
    }

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

    pub async fn attach(&self, target: &Target) -> Result<Page> {
        Ok(Page {
            connection: Connection::open(&self.host, self.port, &target.ws_path).await?,
            target: target.clone(),
        })
    }

    /// `/json/new?url=` answers before the page loads, so this navigates and waits.
    pub async fn open_page(&self, url: &str, within: Duration) -> Result<Page> {
        let target = self.open("about:blank").await?;
        let mut page = self.attach(&target).await?;

        page.navigate(url).await?;
        page.wait_for_load(within).await?;
        Ok(page)
    }

    pub async fn export_session(&self, origins: &[String], carry: Carry) -> Result<Session> {
        let mut session = Session {
            origins: origins.to_vec(),
            cookies: Vec::new(),
            storage: BTreeMap::new(),
            session_storage: BTreeMap::new(),
            databases: BTreeMap::new(),
            carried: carry,
            incomplete: Vec::new(),
        };

        if carry.cookies {
            let all = self.browser_call("Storage.getCookies", json!({})).await?;

            session.cookies = all
                .get("cookies")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or_default()
                .iter()
                .filter_map(cookie_in)
                .filter(|cookie| origins.iter().any(|origin| covers(origin, &cookie.domain)))
                .collect();
        }

        if !carry.needs_a_page() {
            return Ok(session);
        }

        for origin in origins {
            // Session storage belongs to a tab, so reuse one already on this origin.
            let (mut page, ours) = match self.page_on(origin).await {
                Some(page) => (page, false),
                None => {
                    let Ok(page) = self.open_page(origin, TIMEOUT).await else {
                        session.incomplete.push(format!(
                            "{origin}: would not load, so nothing was taken from it"
                        ));
                        continue;
                    };

                    if carry.session_storage {
                        session.incomplete.push(format!(
                            "{origin}: no tab was open on it, and session storage lives in a tab"
                        ));
                    }

                    (page, true)
                }
            };

            if carry.local_storage
                && let Some(items) = read_items(&mut page, "localStorage").await
            {
                session.storage.insert(origin.clone(), items);
            }

            if carry.session_storage
                && let Some(items) = read_items(&mut page, "sessionStorage").await
            {
                session.session_storage.insert(origin.clone(), items);
            }

            if carry.indexed_db {
                match page.evaluate(IDB_EXPORT).await {
                    Ok(read) => {
                        match serde_json::from_str::<IdbRead>(read.as_str().unwrap_or("")) {
                            Ok(read) => {
                                if !read.databases.is_empty() {
                                    session.databases.insert(origin.clone(), read.databases);
                                }
                                session.incomplete.extend(
                                    read.skipped
                                        .into_iter()
                                        .map(|why| format!("{origin}: {why}")),
                                );
                            }
                            Err(error) => session
                                .incomplete
                                .push(format!("{origin}: its databases would not parse: {error}")),
                        }
                    }
                    Err(error) => session.incomplete.push(format!(
                        "{origin}: its databases could not be read: {error}"
                    )),
                }
            }

            if ours {
                page.close().await.ok();
            }
        }

        Ok(session)
    }

    /// Returns the tabs left open: session storage lives only as long as its tab.
    pub async fn import_session(&self, session: &Session) -> Result<Vec<Page>> {
        let cookies: Vec<Value> = session
            .cookies
            .iter()
            .filter(|cookie| {
                session
                    .origins
                    .iter()
                    .any(|origin| covers(origin, &cookie.domain))
            })
            .map(cookie_out)
            .collect();

        if !cookies.is_empty() {
            self.browser_call("Storage.setCookies", json!({ "cookies": cookies }))
                .await?;
        }

        let mut held = Vec::new();
        for origin in &session.origins {
            let local = session.storage.get(origin);
            let temporary = session.session_storage.get(origin);
            let databases = session.databases.get(origin);

            if local.is_none() && temporary.is_none() && databases.is_none() {
                continue;
            }

            let (mut page, ours) = match self.page_on(origin).await {
                Some(page) => (page, false),
                None => match self.open_page(origin, TIMEOUT).await {
                    // Closing it would discard the session storage just written.
                    Ok(page) => (page, temporary.is_none()),
                    Err(_) => continue,
                },
            };

            if let Some(items) = local {
                write_items(&mut page, "localStorage", items).await;
            }
            if let Some(items) = temporary {
                write_items(&mut page, "sessionStorage", items).await;
            }

            if let Some(databases) = databases
                && let Ok(written) = serde_json::to_string(databases)
            {
                page.evaluate(&format!("({IDB_IMPORT})({written})"))
                    .await
                    .ok();
            }

            match ours {
                true => {
                    page.close().await.ok();
                }
                false => held.push(page),
            }
        }

        Ok(held)
    }

    async fn page_on(&self, origin: &str) -> Option<Page> {
        for target in self.pages().await.ok()? {
            if target.url.starts_with(origin) {
                return self.attach(&target).await.ok();
            }
        }

        None
    }

    pub async fn search(
        &self,
        query: &str,
        provider: SearchProvider,
        within: Duration,
    ) -> Result<Page> {
        self.open_page(&provider.url_for(query), within).await
    }

    /// Never closes the visible page.
    pub async fn tidy(&self, keep: usize) -> Result<usize> {
        let pages = self.pages().await?;
        if pages.len() <= keep {
            return Ok(0);
        }

        // By id: several tabs can share a URL.
        let showing = match self.visible_page().await {
            Ok(Some(page)) => Some(page.target().id.clone()),
            _ => None,
        };

        let mut closed = 0;
        // The debugger lists newest first.
        for target in pages.into_iter().skip(keep) {
            if showing.as_deref() == Some(target.id.as_str()) {
                continue;
            }

            if self.close(&target.id).await.is_ok() {
                closed += 1;
            }
        }

        Ok(closed)
    }

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

    pub async fn create_group(&self) -> Result<BrowserGroup> {
        let result = self
            .browser_call("Target.createBrowserContext", json!({}))
            .await?;
        let id = required_string(&result, "browserContextId", "Target.createBrowserContext")?;
        Ok(BrowserGroup {
            devtools: self.clone(),
            id,
        })
    }

    pub async fn groups(&self) -> Result<Vec<BrowserGroup>> {
        let result = self
            .browser_call("Target.getBrowserContexts", json!({}))
            .await?;
        Ok(browser_context_ids(&result)?
            .into_iter()
            .map(|id| BrowserGroup {
                devtools: self.clone(),
                id,
            })
            .collect())
    }

    async fn browser_connection(&self) -> Result<Connection> {
        let path = self.browser_path().await?;
        Connection::open(&self.host, self.port, &path).await
    }

    async fn browser_path(&self) -> Result<String> {
        if let Some(path) = self.browser_path.get() {
            return Ok(path.clone());
        }

        let version = self.version().await?;
        let url = version
            .get("webSocketDebuggerUrl")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::denied("the browser reported no WebSocket debugger URL"))?;
        let discovered = websocket_path(url)
            .ok_or_else(|| Error::denied("the browser WebSocket debugger URL has no path"))?;
        let _ = self.browser_path.set(discovered);
        Ok(self
            .browser_path
            .get()
            .expect("this call or another just set the path")
            .clone())
    }

    async fn browser_call(&self, method: &str, params: Value) -> Result<Value> {
        self.browser_call_within(method, params, TIMEOUT).await
    }

    async fn browser_call_within(
        &self,
        method: &str,
        params: Value,
        within: Duration,
    ) -> Result<Value> {
        let deadline = Instant::now() + within;
        let mut connection = tokio::time::timeout(within, self.browser_connection())
            .await
            .map_err(|_| Error::Timeout {
                after: within,
                detail: format!("{method} could not connect to the browser"),
            })??;
        let left = deadline.saturating_duration_since(Instant::now());
        let result = tokio::time::timeout(left, connection.call_within(method, params, left))
            .await
            .map_err(|_| Error::Timeout {
                after: within,
                detail: format!("{method} was never answered"),
            })?;
        let _ = connection.close().await;
        result
    }

    async fn wait_for_target(&self, id: &str, within: Duration) -> Result<Target> {
        let deadline = Instant::now() + within;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(Error::Timeout {
                    after: within,
                    detail: format!("target {id} never appeared"),
                });
            }
            let targets = tokio::time::timeout(left, self.targets())
                .await
                .map_err(|_| Error::Timeout {
                    after: within,
                    detail: format!("target {id} never appeared"),
                })??;
            if let Some(target) = targets.into_iter().find(|target| target.id == id) {
                return Ok(target);
            }
            tokio::time::sleep(
                Duration::from_millis(50).min(deadline.saturating_duration_since(Instant::now())),
            )
            .await;
        }
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

#[derive(Debug)]
pub struct BrowserGroup {
    devtools: Devtools,
    id: String,
}

impl BrowserGroup {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub async fn targets(&self) -> Result<Vec<Target>> {
        let result = self
            .devtools
            .browser_call("Target.getTargets", json!({}))
            .await?;
        let ids = target_ids_in_context(&result, &self.id)?;

        Ok(self
            .devtools
            .targets()
            .await?
            .into_iter()
            .filter(|target| ids.contains(target.id.as_str()))
            .collect())
    }

    pub async fn pages(&self) -> Result<Vec<Target>> {
        Ok(self
            .targets()
            .await?
            .into_iter()
            .filter(Target::is_page)
            .collect())
    }

    pub async fn open(&self, url: &str) -> Result<Target> {
        self.open_within(url, TIMEOUT).await
    }

    async fn open_within(&self, url: &str, within: Duration) -> Result<Target> {
        let deadline = Instant::now() + within;
        let result = self
            .devtools
            .browser_call_within(
                "Target.createTarget",
                create_target_params(&self.id, url),
                within,
            )
            .await?;
        let id = required_string(&result, "targetId", "Target.createTarget")?;
        let left = deadline.saturating_duration_since(Instant::now());
        self.devtools.wait_for_target(&id, left).await
    }

    pub async fn open_page(&self, url: &str, within: Duration) -> Result<Page> {
        tokio::time::timeout(within, async {
            let target = self.open_within("about:blank", within).await?;
            let mut page = self.devtools.attach(&target).await?;
            page.navigate(url).await?;
            page.wait_for_load(within).await?;
            Ok(page)
        })
        .await
        .map_err(|_| Error::Timeout {
            after: within,
            detail: format!("{url} did not open"),
        })?
    }

    pub async fn close(self) -> Result<()> {
        let result = self
            .devtools
            .browser_call(
                "Target.disposeBrowserContext",
                browser_context_params(&self.id),
            )
            .await;
        match result {
            Ok(_) => Ok(()),
            Err(error) => match self.devtools.groups().await {
                Ok(groups) if groups.iter().all(|group| group.id() != self.id.as_str()) => Ok(()),
                _ => Err(error),
            },
        }
    }
}

fn create_target_params(context: &str, url: &str) -> Value {
    json!({
        "url": url,
        "browserContextId": context,
    })
}

fn browser_context_params(context: &str) -> Value {
    json!({ "browserContextId": context })
}

fn browser_context_ids(result: &Value) -> Result<Vec<String>> {
    result
        .get("browserContextIds")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::denied("Target.getBrowserContexts returned no browserContextIds"))?
        .iter()
        .map(|id| {
            id.as_str()
                .map(str::to_string)
                .ok_or_else(|| Error::denied("a browserContextId is not a string"))
        })
        .collect()
}

fn target_ids_in_context(result: &Value, context: &str) -> Result<HashSet<String>> {
    Ok(result
        .get("targetInfos")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::denied("Target.getTargets returned no targetInfos"))?
        .iter()
        .filter(|info| info.get("browserContextId").and_then(Value::as_str) == Some(context))
        .filter_map(|info| {
            info.get("targetId")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect())
}

fn required_string(result: &Value, field: &str, method: &str) -> Result<String> {
    result
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| Error::denied(format!("{method} returned no {field}")))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub method: String,
    pub params: Value,
}

struct Connection {
    socket: TcpStream,
    next: u64,
    events: VecDeque<Event>,
    dropped: usize,
    closed: bool,
}

impl Connection {
    async fn open(host: &str, port: u16, path: &str) -> Result<Self> {
        Ok(Self {
            socket: handshake(host, port, path).await?,
            next: 1,
            events: VecDeque::new(),
            dropped: 0,
            closed: false,
        })
    }

    async fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        self.call_within(method, params, TIMEOUT).await
    }

    async fn call_within(
        &mut self,
        method: &str,
        params: Value,
        within: Duration,
    ) -> Result<Value> {
        let id = self.next;
        self.next += 1;

        let request = json!({ "id": id, "method": method, "params": params });
        send_text(&mut self.socket, &request.to_string()).await?;

        let deadline = SystemTime::now() + within;
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
                    after: within,
                    detail: format!("{method} was never answered"),
                });
            }
        }
    }

    async fn close(&mut self) -> Result<()> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        send_close(&mut self.socket).await
    }

    async fn next_event(&mut self, method: &str, within: Duration) -> Result<Event> {
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
}

impl Drop for Connection {
    fn drop(&mut self) {
        if !self.closed {
            let _ = self.socket.try_write(&close_frame());
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Carry {
    pub cookies: bool,
    pub local_storage: bool,
    pub session_storage: bool,
    pub indexed_db: bool,
}

impl Default for Carry {
    fn default() -> Self {
        Self {
            cookies: true,
            local_storage: true,
            session_storage: false,
            indexed_db: false,
        }
    }
}

impl Carry {
    pub fn none() -> Self {
        Self {
            cookies: false,
            local_storage: false,
            session_storage: false,
            indexed_db: false,
        }
    }

    pub fn all() -> Self {
        Self {
            cookies: true,
            local_storage: true,
            session_storage: true,
            indexed_db: true,
        }
    }

    fn needs_a_page(&self) -> bool {
        self.local_storage || self.session_storage || self.indexed_db
    }
}

/// Holds live credentials, so `Debug` prints counts only.
#[derive(Clone, Serialize, Deserialize)]
pub struct Session {
    pub origins: Vec<String>,
    pub cookies: Vec<Cookie>,
    pub storage: BTreeMap<String, BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub session_storage: BTreeMap<String, BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub databases: BTreeMap<String, Vec<Database>>,
    #[serde(default)]
    pub carried: Carry,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub incomplete: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Database {
    pub name: String,
    pub version: u64,
    pub stores: Vec<BrowserStore>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserStore {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_path: Option<Value>,
    #[serde(default)]
    pub auto_increment: bool,
    /// `[key, value]`: a store without a key path keeps no key in its value.
    pub records: Vec<Value>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Session")
            .field("origins", &self.origins)
            .field("cookies", &self.cookies.len())
            .field("storage", &self.storage.len())
            .field("session_storage", &self.session_storage.len())
            .field("databases", &self.databases.len())
            .field("incomplete", &self.incomplete)
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires: Option<f64>,
    #[serde(default)]
    pub http_only: bool,
    #[serde(default)]
    pub secure: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub same_site: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SearchProvider {
    #[default]
    DuckDuckGo,
    Google,
    Bing,
}

impl SearchProvider {
    pub fn url_for(&self, query: &str) -> String {
        let query = encode(query);

        match self {
            Self::DuckDuckGo => format!("https://duckduckgo.com/html/?q={query}"),
            Self::Google => format!("https://www.google.com/search?q={query}"),
            Self::Bing => format!("https://www.bing.com/search?q={query}"),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::DuckDuckGo => "duckduckgo",
            Self::Google => "google",
            Self::Bing => "bing",
        }
    }
}

fn encode(query: &str) -> String {
    let mut out = String::with_capacity(query.len());

    for byte in query.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(char::from(*byte))
            }
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }

    out
}

pub struct Page {
    connection: Connection,
    target: Target,
}

const TEXT_DEFAULT: usize = 100_000;
const LINKS_DEFAULT: usize = 100;

const POLL: Duration = Duration::from_millis(120);

/// Lets the protocol's own timeout answer first.
const GRACE: Duration = Duration::from_millis(250);

pub const JPEG_QUALITY: u32 = 70;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageShot {
    pub full: bool,
    pub format: Picture,
    pub quality: u32,
}

impl Default for PageShot {
    fn default() -> Self {
        Self {
            full: false,
            format: Picture::Png,
            quality: JPEG_QUALITY,
        }
    }
}

impl PageShot {
    pub fn whole() -> Self {
        Self {
            full: true,
            format: Picture::Jpeg,
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Picture {
    #[default]
    Png,
    Jpeg,
}

impl Picture {
    pub fn name(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpeg",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scroll {
    By { x: i32, y: i32 },
    To { x: i32, y: i32 },
    Bottom,
    Top,
}

impl Scroll {
    fn as_js(self) -> String {
        match self {
            Self::By { x, y } => format!("{{ x: null, dx: {x}, dy: {y} }}"),
            Self::To { x, y } => format!("{{ x: {x}, y: {y} }}"),
            // The browser clamps it; `scrollHeight` of the wrong element would fall short.
            Self::Bottom => "{ x: 0, y: 1e9 }".to_string(),
            Self::Top => "{ x: 0, y: 0 }".to_string(),
        }
    }
}

const FOUND_DEFAULT: usize = 20;
const SNAPSHOT_DEFAULT: usize = 200;
/// A dialog fades in a few hundred milliseconds; longer, and it is staying.
const COVERED: Duration = Duration::from_millis(600);
const COVERED_POLL: Duration = Duration::from_millis(50);
/// A redirect leaves no context for a moment; a slow load is not a redirect.
const CONTEXT_WAIT: Duration = Duration::from_secs(2);
const CONTEXT_POLL: Duration = Duration::from_millis(100);

fn button_parts(button: Button) -> (&'static str, u8) {
    match button {
        Button::Left => ("left", 1),
        Button::Right => ("right", 2),
        Button::Middle => ("middle", 4),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Element {
    pub text: String,
    pub tag: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub states: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default)]
    pub visible: bool,
    /// Viewport coordinates, not screen ones; `None` when the middle is off the viewport.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Point>,
    pub width: u32,
    pub height: u32,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// Given by the last `snapshot`; `@e12` names it in a query.
    #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
    pub r#ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
}

const MATCH: &str = r#"(q, exact) => {
  const seen = new Set();
  const out = [];
  const add = el => {
    if (!el || seen.has(el)) return;
    const r = el.getBoundingClientRect();
    if (r.width < 1 || r.height < 1) return;
    const style = getComputedStyle(el);
    if (style.visibility === 'hidden' || style.display === 'none') return;
    seen.add(el); out.push(el);
  };

  const ref = /^@e(\d+)$/.exec(q.trim());
  if (ref) {
    const numbered = (window.__computerRefs || [])[+ref[1] - 1];
    if (numbered && numbered.isConnected) add(numbered);
    return out;
  }

  try { document.querySelectorAll(q).forEach(add); } catch (e) {}

  for (const by of ['[name=', '[id=', '[placeholder=', '[aria-label=']) {
    try { document.querySelectorAll(by + JSON.stringify(q) + ']').forEach(add); } catch (e) {}
  }

  const want = q.trim().toLowerCase();
  const icon = el => {
    const parts = [];
    for (const img of el.querySelectorAll('img[alt], area[alt], input[alt]')) parts.push(img.alt);
    for (const named of el.querySelectorAll('svg > title, svg > desc')) parts.push(named.textContent);
    return parts.join(' ');
  };
  const words = el => (el.innerText || el.value || el.getAttribute('aria-label') ||
                       el.getAttribute('placeholder') || el.getAttribute('title') ||
                       icon(el) || '')
                        .replace(/\s+/g, ' ').trim().toLowerCase();
  const clickable = 'a,button,input,select,textarea,[role=button],[role=link],[onclick]';

  const passes = exact
    ? [el => words(el) === want]
    : [el => words(el) === want, el => words(el).includes(want)];

  for (const pass of passes) {
    for (const el of document.querySelectorAll(clickable)) if (pass(el)) add(el);

    // Innermost only: an ancestor's innerText contains its descendants'.
    const hits = [];
    for (const el of document.querySelectorAll('*')) if (pass(el)) hits.push(el);
    for (const el of hits) {
      if (hits.some(other => other !== el && el.contains(other))) continue;
      add(el);
    }
  }

  // Not sorted by position: a form sits above its own button.
  return out;
}"#;

pub fn selector_for(role: &str) -> Option<&'static str> {
    let selector = match role {
        "button" => {
            "button, [role=button], input[type=button], input[type=submit], \
                     input[type=reset], input[type=image], summary"
        }
        "link" => "a[href], area[href], [role=link]",
        "textbox" => {
            "input[type=text], input[type=search], input[type=email], \
                      input[type=url], input[type=tel], input[type=password], \
                      input:not([type]), textarea, [role=textbox], [contenteditable=true]"
        }
        "checkbox" => "input[type=checkbox], [role=checkbox], [role=switch]",
        "radio" => "input[type=radio], [role=radio]",
        "combobox" => "select, [role=combobox], [role=listbox]",
        "option" => "option, [role=option]",
        "heading" => "h1, h2, h3, h4, h5, h6, [role=heading]",
        "image" => "img, svg, [role=img], [role=image]",
        "tab" => "[role=tab]",
        "dialog" => "dialog, [role=dialog], [role=alertdialog]",
        _ => return None,
    };

    Some(selector)
}

pub const ROLES: &str = "button, link, textbox, checkbox, radio, combobox, option, heading, \
                         image, tab, dialog";

const SELECTOR: &str = r#"(el) => {
  const alone = q => {
    try { return document.querySelectorAll(q).length === 1; } catch (e) { return false; }
  };

  if (el.id && alone('#' + CSS.escape(el.id))) return '#' + CSS.escape(el.id);

  for (const by of ['data-testid', 'data-test', 'data-qa', 'name', 'aria-label',
                    'placeholder', 'title']) {
    const value = el.getAttribute(by);
    if (!value) continue;
    const q = '[' + by + '=' + JSON.stringify(value) + ']';
    if (alone(q)) return q;
  }

  const step = node => {
    const tag = node.tagName.toLowerCase();
    const parent = node.parentElement;
    if (!parent) return tag;
    const same = Array.from(parent.children).filter(one => one.tagName === node.tagName);
    return same.length === 1 ? tag : tag + ':nth-of-type(' + (same.indexOf(node) + 1) + ')';
  };

  const path = [];
  for (let node = el; node && node.nodeType === 1 && path.length < 8; node = node.parentElement) {
    path.unshift(step(node));
    const q = path.join(' > ');
    if (alone(q)) return q;
  }

  return undefined;
}"#;

fn describe() -> String {
    // Only the name: the parens around it in DESCRIBE make it callable.
    DESCRIBE.replace("SELECTOR_FN", SELECTOR)
}

fn checkbox() -> String {
    BOX.replace("MATCH_FN", MATCH)
}

fn fillable() -> String {
    FILLABLE.replace("MATCH_FN", MATCH)
}

fn named() -> String {
    NAMED
        .replace("MATCH_FN", MATCH)
        .replace("SELECTOR_FN", SELECTOR)
}

fn nearby() -> String {
    NEARBY
        .replace("MATCH_FN", MATCH)
        .replace("SELECTOR_FN", SELECTOR)
}

fn assign() -> String {
    ASSIGN.replace("MATCH_FN", MATCH)
}

fn rich() -> String {
    RICH.replace("MATCH_FN", MATCH)
}

fn dropdown() -> String {
    PICK.replace("MATCH_FN", MATCH)
}

/// "I agree" names a checkbox but matches the `label`. A click lands on the
/// control either way; ticking has to find it.
const BOX: &str = r#"(query) => {
  const el = (MATCH_FN)(query, false).find(Boolean);
  if (!el) return null;
  if ('checked' in el) return el;
  return el.control || el.querySelector('input[type=checkbox], input[type=radio]') || el;
}"#;

const NAMED: &str = r#"(query, kind) => {
  const fits = el =>
    kind === 'select' ? el.tagName === 'SELECT'
    : kind === 'file' ? el.tagName === 'INPUT' && el.type === 'file'
    : el.tagName !== 'LABEL';

  for (const el of (MATCH_FN)(query, false).filter(Boolean)) {
    if (fits(el)) return null;
    const control = el.tagName === 'LABEL' ? el.control : null;
    if (control && fits(control)) return (SELECTOR_FN)(control) || null;
  }
  return null;
}"#;

const NEARBY: &str = r#"(query, kind) => {
  const el = (MATCH_FN)(query, false).find(Boolean);
  if (!el) return null;

  const wanted = kind === 'select' ? 'select'
    : kind === 'file' ? 'input[type=file]'
    : 'input:not([type=hidden]):not([type=submit]):not([type=button]):not([type=reset])' +
      ':not([type=image]), textarea, select, [contenteditable=""], [contenteditable="true"]';
  const reachable = field => kind === 'file' || field.getClientRects().length > 0;
  const fields = node => Array.from(node.querySelectorAll(wanted)).filter(reachable);
  const refs = window.__computerRefs || [];
  const named = field => {
    const at = refs.indexOf(field);
    return at >= 0 ? '@e' + (at + 1) : (SELECTOR_FN)(field);
  };

  let where = 'in';
  let found = fields(el);
  if (!found.length && el.parentElement) {
    where = 'beside';
    found = fields(el.parentElement);
  }
  const count = found.length;
  const crowd = count > 3;
  if (crowd) found = [];

  return { tag: el.tagName.toLowerCase(), where, count, crowd, fields: found.map(named).filter(Boolean) };
}"#;

/// Which of three ways a control takes a value, or the verb that reaches it.
const FILLABLE: &str = r#"(query) => {
  const el = (MATCH_FN)(query, false).find(Boolean);
  if (!el) return 'gone';

  const tag = el.tagName;
  const kind = tag === 'INPUT' ? (el.type || 'text').toLowerCase() : '';

  if (tag === 'SELECT') return 'select';
  if (kind === 'file') return 'upload';
  if (kind === 'checkbox' || kind === 'radio') return 'check';
  if (tag === 'BUTTON' || ['submit', 'button', 'reset', 'image'].includes(kind)) return 'click';
  if (tag !== 'INPUT' && tag !== 'TEXTAREA') return el.isContentEditable ? 'rich' : 'no';

  if (el.disabled) return 'disabled';
  if (el.readOnly) return 'read-only';
  return ['range', 'color', 'date', 'time', 'datetime-local', 'month', 'week'].includes(kind)
    ? 'assign'
    : 'type';
}"#;

/// React shadows `value` with a setter that remembers what it last saw, so a
/// write through it leaves React believing nothing changed. The prototype's
/// setter goes under the shadow.
///
/// A control that will not hold the value empties instead of refusing, and a
/// colour turns black, so both are read back rather than trusted.
const ASSIGN: &str = r#"(query, value) => {
  const el = (MATCH_FN)(query, false).find(Boolean);
  if (!el) return 'gone';
  if ((el.type || '').toLowerCase() === 'color' && !/^#[0-9a-f]{6}$/i.test(value))
    return 'rejected';

  const native = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(el), 'value');
  if (native && native.set) native.set.call(el, value); else el.value = value;
  if (el.value === '' && value !== '') return 'rejected';

  el.dispatchEvent(new Event('input', { bubbles: true }));
  el.dispatchEvent(new Event('change', { bubbles: true }));
  return el.value;
}"#;

/// A contenteditable has no `value` to clear, so what replaces the old text is
/// the selection the typing lands on.
const RICH: &str = r#"(query, empty) => {
  const el = (MATCH_FN)(query, false).find(Boolean);
  if (!el) return 'gone';
  el.focus({ preventScroll: true });

  const range = document.createRange();
  range.selectNodeContents(el);
  const picked = getSelection();
  picked.removeAllRanges();
  picked.addRange(range);

  if (empty) document.execCommand('delete');
  return 'ok';
}"#;

const NO_DROPDOWN: &str = "no dropdown matched";

const FILE_INPUT: &str = r#"(query) => {
  const takes = el => el && el.tagName === 'INPUT' && el.type === 'file';
  const seen = (MATCH_FN)(query, false).find(takes);
  if (seen) return seen;

  try {
    const hidden = document.querySelector(query);
    return takes(hidden) ? hidden : undefined;
  } catch (_) {
    return undefined;
  }
}"#;

fn file_input() -> String {
    FILE_INPUT.replace("MATCH_FN", MATCH)
}

const PICK: &str = r#"(query, wanted, drop) => {
  const el = (MATCH_FN)(query, false).find(e => e.tagName === 'SELECT');
  if (!el) return { no: 'no dropdown matched' };
  if (el.disabled) return { no: 'the dropdown is disabled' };
  if (!el.multiple && (drop || wanted.length > 1))
    return { no: 'it takes one option, and always holds one' };

  const want = [];
  for (const name of wanted) {
    const looking = name.trim().toLowerCase();
    const at = Array.from(el.options).findIndex(
      o => o.text.trim().toLowerCase() === looking || String(o.value).toLowerCase() === looking);
    if (at < 0) return { no: 'no option named ' + name };
    if (el.options[at].disabled) return { no: name + ' is disabled' };
    want.push(at);
  }

  if (drop)
    Array.from(el.options).forEach((o, at) => {
      if (!wanted.length || want.includes(at)) o.selected = false;
    });
  else if (el.multiple)
    Array.from(el.options).forEach((o, at) => { o.selected = want.includes(at); });
  else el.selectedIndex = want[0];

  el.dispatchEvent(new Event('input', { bubbles: true }));
  el.dispatchEvent(new Event('change', { bubbles: true }));
  return { picked: Array.from(el.selectedOptions).map(o => o.text.trim()) };
}"#;

/// What a wait will take as a match, beyond the query itself.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Ready {
    /// Not `disabled`. A query matches a button that is in the document, which
    /// is not the same as one that can be pressed.
    pub enabled: bool,
}

const DESCRIBE: &str = r#"(el) => {
  const r = el.getBoundingClientRect();
  const x = Math.round(r.left + r.width / 2), y = Math.round(r.top + r.height / 2);
  const inside = x >= 0 && y >= 0 && x < innerWidth && y < innerHeight;

  // A label wrapping its field is read without the field, or its value would be its name.
  const wrapping = () => {
    const label = el.closest('label');
    if (!label) return undefined;
    const copy = label.cloneNode(true);
    copy.querySelectorAll('input, select, textarea, [aria-hidden=true]').forEach(n => n.remove());
    return (copy.textContent || '').trim() || undefined;
  };
  const label = el.getAttribute('aria-label') || el.getAttribute('placeholder') ||
                el.getAttribute('title') ||
                (el.id && (document.querySelector('label[for=' + JSON.stringify(el.id) + ']') || {}).innerText) ||
                wrapping() ||
                undefined;

  const aria = name => el.getAttribute('aria-' + name);
  const off = el.disabled === true || aria('disabled') === 'true';

  const states = [];
  if (off) states.push('disabled');
  if (aria('expanded') === 'true') states.push('expanded');
  if (aria('expanded') === 'false') states.push('collapsed');
  if (el.checked === true || aria('checked') === 'true') states.push('checked');
  if (aria('selected') === 'true') states.push('selected');
  if (el.required === true || aria('required') === 'true') states.push('required');

  const numbered = (window.__computerRefs || []).indexOf(el);

  return {
    text: (el.innerText || el.value || el.getAttribute('aria-label') || '')
            .replace(/\s+/g, ' ').trim().slice(0, 200),
    tag: el.tagName.toLowerCase(),
    kind: el.getAttribute('type') || undefined,
    role: el.getAttribute('role') || undefined,
    states,
    selector: (SELECTOR_FN)(el),
    label: label === undefined ? undefined
             : String(label).replace(/\s+/g, ' ').trim().replace(/[:*]\s*$/, '').slice(0, 200),
    at: inside ? { x, y } : undefined,
    visible: inside,
    width: Math.round(r.width),
    height: Math.round(r.height),
    enabled: !off,
    value: el.value === undefined ? undefined : String(el.value).slice(0, 200),
    ref: numbered < 0 ? undefined : 'e' + (numbered + 1),
    href: (el.tagName === 'A' || el.tagName === 'AREA') && el.href
            ? String(el.href).slice(0, 300) : undefined,
  };
}"#;

const SNAPSHOT: &str = r#"(root, limit, mode, scope) => {
  const controls = 'a[href], area[href], button, input:not([type=hidden]), select, textarea, ' +
    'summary, [role=button], [role=link], [role=textbox], [role=searchbox], [role=checkbox], ' +
    '[role=radio], [role=switch], [role=combobox], [role=listbox], [role=option], ' +
    '[role=menuitem], [role=menuitemcheckbox], [role=menuitemradio], [role=tab], ' +
    '[role=slider], [role=spinbutton], [contenteditable=true], [onclick], ' +
    '[tabindex]:not([tabindex="-1"]), h1, h2, h3, h4, h5, h6, [role=heading]';
  const shown = el => {
    const r = el.getBoundingClientRect();
    if (r.width < 1 || r.height < 1) return false;
    const style = getComputedStyle(el);
    return style.visibility !== 'hidden' && style.display !== 'none';
  };

  const found = Array.from(root.querySelectorAll(controls)).filter(shown);
  const page = { url: location.href, title: document.title, total: found.length };

  // Only a snapshot hands numbers out: an agent must have seen a number before it can
  // name one, or a number kept from the last page would land on this one.
  const numbered = Array.isArray(window.__computerRefs);
  if ((mode === 'prime' || mode === 'since') && !numbered) {
    return JSON.stringify({ ...page, elements: [] });
  }

  const refs = numbered ? window.__computerRefs : [];
  // A hole, not a gap: the numbers after it hold still and the page can free it.
  for (let i = 0; i < refs.length; i++) if (refs[i] && !refs[i].isConnected) refs[i] = null;
  for (const el of found) if (!refs.includes(el)) refs.push(el);
  window.__computerRefs = refs;

  const remembered = window.__computerLast || (window.__computerLast = {});
  const before = remembered[scope];

  // Priming never overwrites what a snapshot remembered.
  if (mode === 'prime' && before) return JSON.stringify({ ...page, elements: [] });

  const described = found.map(el => (DESCRIBE_FN)(el));
  const now = {};
  for (const one of described) now[one.ref] = one;
  remembered[scope] = now;

  if (mode !== 'delta' && mode !== 'since') {
    return JSON.stringify({ ...page, elements: mode === 'prime' ? [] : described.slice(0, limit) });
  }
  if (!before) {
    return JSON.stringify({
      ...page,
      elements: described.slice(0, limit),
      delta: { first: true, added: [], changed: [], gone: [], same: 0 },
    });
  }

  // Not where it sits: scrolling is not the page changing.
  const identity = one => [one.tag, one.kind, one.role, one.text, one.label, one.value,
                           (one.states || []).join(' '), one.enabled, one.href].join('\u0001');
  const added = [], changed = [], gone = [];
  let same = 0;
  for (const one of described) {
    const was = before[one.ref];
    if (!was) added.push(one);
    else if (identity(was) !== identity(one)) changed.push(one);
    else same += 1;
  }
  for (const ref of Object.keys(before)) if (!now[ref]) gone.push(before[ref]);

  return JSON.stringify({
    ...page,
    elements: [],
    delta: {
      first: false,
      added: added.slice(0, limit),
      changed: changed.slice(0, limit),
      gone: gone.slice(0, limit),
      same,
    },
  });
}"#;

const REACH: &str = r#"(el) => {
  const name = node => {
    const id = node.id ? '#' + node.id : '';
    const classes = typeof node.className === 'string' && node.className.trim()
      ? '.' + node.className.trim().split(/\s+/).slice(0, 2).join('.') : '';
    return '<' + node.tagName.toLowerCase() + id + classes + '>';
  };
  const centre = r => ({ x: Math.round(r.left + r.width / 2), y: Math.round(r.top + r.height / 2) });
  const inside = p => p.x >= 0 && p.y >= 0 && p.x < innerWidth && p.y < innerHeight;

  // The middle, then each line (a wrapped link has its middle in the gap between
  // lines), then the quarters: an icon covers the middle of a field, a dialog all of it.
  const box = el.getBoundingClientRect();
  const quarters = [[0.25, 0.5], [0.75, 0.5], [0.5, 0.25], [0.5, 0.75]].map(([fx, fy]) =>
    ({ x: Math.round(box.left + box.width * fx), y: Math.round(box.top + box.height * fy) }));
  const points = [centre(box), ...Array.from(el.getClientRects()).map(centre), ...quarters];
  let covered = null;
  for (const p of points) {
    if (!inside(p)) continue;
    const top = document.elementFromPoint(p.x, p.y);
    if (!top) continue;
    if (top === el || el.contains(top)) return { at: p };
    if (!covered) covered = name(top);
  }
  return covered ? { covered } : { off: true };
}"#;

const REF_STATE: &str = r#"(n) => {
  const refs = window.__computerRefs;
  if (!Array.isArray(refs)) return 'none';
  const el = refs[n - 1];
  if (el === undefined) return 'unknown';
  if (!el || !el.isConnected) return 'gone';
  return 'hidden';
}"#;

fn snapshot_script() -> String {
    SNAPSHOT.replace("DESCRIBE_FN", &describe())
}

// The page's own scripts see the global, so it carries this crate's name.
const WATCH: &str = r#"() => {
  const key = '__computerWatch';
  if (window[key]) window[key].observer.disconnect();
  const state = { changed: false, observer: null };
  state.observer = new MutationObserver(() => { state.changed = true; });
  state.observer.observe(document.documentElement, {
    subtree: true, childList: true, attributes: true, characterData: true,
  });
  window[key] = state;
}"#;

const CHANGED: &str = r#"() => {
  const key = '__computerWatch';
  const state = window[key];
  if (!state) return null;
  state.observer.disconnect();
  delete window[key];
  return state.changed;
}"#;

// A fetch that lands counts as activity before its data reaches the DOM, and a
// page still loading is never quiet.
const QUIET: &str = r#"(quietMs, withinMs) => new Promise(resolve => {
  const now = () => performance.now();
  const started = now();
  let last = started;
  const bump = () => { last = now(); };
  const mutations = new MutationObserver(bump);
  mutations.observe(document.documentElement, {
    subtree: true, childList: true, attributes: true, characterData: true,
  });
  let resources = null;
  try {
    resources = new PerformanceObserver(bump);
    resources.observe({ entryTypes: ['resource'] });
  } catch (e) {}
  const done = settled => {
    mutations.disconnect();
    if (resources) resources.disconnect();
    resolve(settled);
  };
  const tick = () => {
    const at = now();
    if (document.readyState !== 'complete') last = at;
    if (at - last >= quietMs) return done(true);
    if (at - started >= withinMs) return done(false);
    setTimeout(tick, Math.min(quietMs - (at - last), 100));
  };
  setTimeout(tick, Math.min(quietMs, 100));
})"#;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reading {
    #[default]
    Markdown,
    Text,
    Raw,
}

const TEXT: &str = r#"
  // The live tree: a detached clone has no layout, so innerText includes hidden text.
  const root = document.querySelector('main, article') || document.body;
  return (root ? root.innerText : '').replace(/\s+/g, ' ').trim();
"#;

const MARKDOWN: &str = r#"
  const skip = new Set(['SCRIPT','STYLE','NOSCRIPT','SVG','TEMPLATE','IFRAME','CANVAS']);
  const inline = t => t.replace(/\s+/g, ' ');
  const out = [];

  const walk = (node, depth) => {
    if (node.nodeType === 3) { out.push(inline(node.nodeValue)); return; }
    if (node.nodeType !== 1 || skip.has(node.tagName)) return;
    if (node.getAttribute && node.getAttribute('aria-hidden') === 'true') return;

    const shown = getComputedStyle(node);
    if (shown.display === 'none' || shown.visibility === 'hidden') return;

    const tag = node.tagName;
    const kids = () => Array.from(node.childNodes).forEach(c => walk(c, depth));

    if (/^H[1-6]$/.test(tag)) {
      out.push('\n\n' + '#'.repeat(+tag[1]) + ' '); kids(); out.push('\n');
      return;
    }
    switch (tag) {
      case 'BR': out.push('\n'); return;
      case 'HR': out.push('\n\n---\n'); return;
      case 'P': case 'DIV': case 'SECTION': case 'ARTICLE': case 'MAIN':
        out.push('\n\n'); kids(); return;
      case 'UL': case 'OL': out.push('\n'); kids(); out.push('\n'); return;
      case 'LI': {
        // The bullet is filled in afterwards so an empty item leaves none.
        const mark = out.length;
        out.push('');
        Array.from(node.childNodes).forEach(c => walk(c, depth + 1));
        const body = out.slice(mark + 1).join('').trim();
        out[mark] = body ? '\n' + '  '.repeat(depth) + '- ' : '';
        return;
      }
      case 'A': {
        const href = node.getAttribute('href') || '';
        const words = inline(node.innerText || node.textContent || '').trim();
        if (!words) return;
        out.push(href.startsWith('http') ? '[' + words + '](' + href + ')' : words);
        return;
      }
      case 'STRONG': case 'B': out.push('**'); kids(); out.push('**'); return;
      case 'EM': case 'I': out.push('_'); kids(); out.push('_'); return;
      case 'CODE':
        if (node.closest('pre')) { kids(); return; }
        out.push('`'); kids(); out.push('`'); return;
      case 'PRE':
        out.push('\n\n```\n' + (node.innerText || '') + '\n```\n'); return;
      case 'BLOCKQUOTE': out.push('\n\n> '); kids(); out.push('\n'); return;
      case 'IMG': {
        const alt = node.getAttribute('alt');
        if (alt) out.push('![' + inline(alt) + ']');
        return;
      }
      case 'TR': out.push('\n| '); kids(); out.push(' |'); return;
      case 'TH': case 'TD': kids(); out.push(' | '); return;
      case 'TABLE': out.push('\n'); kids(); out.push('\n'); return;
      default: kids();
    }
  };

  const root = document.querySelector('main, article') || document.body;
  if (root) walk(root, 0);
  return out.join('')
    .replace(/[ \t]+/g, ' ')
    .replace(/ ?\n ?/g, '\n')
    .replace(/\n{3,}/g, '\n\n')
    .trim();
"#;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageText {
    pub url: String,
    pub title: String,
    pub text: String,
    pub truncated: bool,
    pub links: Vec<Link>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Link {
    pub text: String,
    pub href: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub url: String,
    pub title: String,
    /// What the page offers; the listing stops at the limit.
    pub total: usize,
    /// Empty when a delta was asked for and there was a snapshot to compare with.
    pub elements: Vec<Element>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delta: Option<Changes>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Changes {
    /// Nothing to compare with, so `elements` holds the whole listing instead.
    #[serde(default)]
    pub first: bool,
    #[serde(default)]
    pub added: Vec<Element>,
    #[serde(default)]
    pub changed: Vec<Element>,
    /// As they were, since they are no longer on the page.
    #[serde(default)]
    pub gone: Vec<Element>,
    #[serde(default)]
    pub same: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Listing {
    Full,
    Delta,
    Prime,
    Since,
}

impl Listing {
    fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Delta => "delta",
            Self::Prime => "prime",
            Self::Since => "since",
        }
    }
}

#[derive(Debug, Deserialize)]
struct Reached {
    element: Element,
    reach: Reach,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Reach {
    At {
        at: Point,
    },
    Covered {
        covered: String,
    },
    Off {
        #[allow(dead_code)]
        off: bool,
    },
}

fn between_documents(error: &Error) -> bool {
    let said = error.to_string();
    said.contains("Cannot find default execution context")
        || said.contains("Execution context was destroyed")
}

fn ref_number(query: &str) -> Option<usize> {
    query.trim().strip_prefix("@e")?.parse().ok()
}

impl Page {
    pub fn target(&self) -> &Target {
        &self.target
    }

    pub async fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        self.connection.call(method, params).await
    }

    pub fn dropped_events(&self) -> usize {
        self.connection.dropped
    }

    pub fn take_events(&mut self) -> Vec<Event> {
        self.connection.dropped = 0;
        self.connection.events.drain(..).collect()
    }

    pub async fn next_event(&mut self, method: &str, within: Duration) -> Result<Event> {
        self.connection.next_event(method, within).await
    }

    pub async fn navigate(&mut self, url: &str) -> Result<()> {
        self.call("Page.enable", json!({})).await?;
        let answer = self.call("Page.navigate", json!({ "url": url })).await?;

        // A refused navigation answers with `errorText`, not an error.
        match answer.get("errorText").and_then(Value::as_str) {
            Some(error) => Err(Error::denied(format!("{url}: {error}"))),
            None => Ok(()),
        }
    }

    pub async fn bring_to_front(&mut self) -> Result<()> {
        self.call("Page.bringToFront", json!({})).await.map(|_| ())
    }

    pub async fn visible(&mut self) -> Result<bool> {
        Ok(self.evaluate("document.visibilityState").await? == "visible")
    }

    pub async fn evaluate(&mut self, javascript: &str) -> Result<Value> {
        self.evaluate_within(javascript, None).await
    }

    pub async fn evaluate_within(
        &mut self,
        javascript: &str,
        within: Option<Duration>,
    ) -> Result<Value> {
        let mut params = json!({
            "expression": javascript,
            "returnByValue": true,
            "awaitPromise": true,
        });

        let Some(within) = within else {
            return self.evaluated(params).await;
        };

        // The protocol's timeout bounds only synchronous work; the wall clock bounds the rest.
        params["timeout"] = json!(within.as_millis() as u64);

        match tokio::time::timeout(within + GRACE, self.evaluated(params)).await {
            Ok(answer) => answer,
            Err(_) => Err(Error::Timeout {
                after: within,
                detail: "the expression was still running".to_string(),
            }),
        }
    }

    async fn evaluated(&mut self, params: Value) -> Result<Value> {
        let deadline = Instant::now() + CONTEXT_WAIT;

        let answer = loop {
            match self.call("Runtime.evaluate", params.clone()).await {
                Ok(answer) => break answer,
                Err(error) if between_documents(&error) => {
                    if Instant::now() >= deadline {
                        return Err(Error::Timeout {
                            after: CONTEXT_WAIT,
                            detail: "the page is still loading, so there is nothing to ask yet"
                                .to_string(),
                        });
                    }
                    tokio::time::sleep(CONTEXT_POLL).await;
                }
                Err(error) => return Err(error),
            }
        };

        if let Some(thrown) = answer.get("exceptionDetails") {
            return Err(Error::denied(format!("the page threw: {thrown}")));
        }
        Ok(answer
            .get("result")
            .and_then(|result| result.get("value"))
            .cloned()
            .unwrap_or(Value::Null))
    }

    pub async fn read(
        &mut self,
        format: Reading,
        limit: Option<usize>,
        max_links: Option<usize>,
    ) -> Result<PageText> {
        let limit = limit.unwrap_or(TEXT_DEFAULT);
        let links = max_links.unwrap_or(LINKS_DEFAULT);

        let body = match format {
            Reading::Markdown => MARKDOWN,
            Reading::Text => TEXT,
            Reading::Raw => "return document.documentElement.outerHTML;",
        };

        let read = self
            .evaluate(&format!(
                r#"(() => {{
                     const body = (() => {{ {body} }})();
                     const text = String(body || '');
                     const links = Array.from(document.querySelectorAll('a[href]'))
                       .map(a => ({{ text: (a.innerText || '').replace(/\s+/g, ' ').trim(), href: a.href }}))
                       .filter(l => l.text && l.href.startsWith('http'))
                       .slice(0, {links});
                     return JSON.stringify({{
                       url: location.href,
                       title: document.title,
                       text: text.slice(0, {limit}),
                       truncated: text.length > {limit},
                       links,
                     }});
                   }})()"#
            ))
            .await?;

        let read = read
            .as_str()
            .ok_or_else(|| Error::denied("the page answered with something unreadable"))?;

        serde_json::from_str(read)
            .map_err(|error| Error::denied(format!("the page would not parse: {error}")))
    }

    /// Not done on drop: a close is a round trip that a drop cannot wait for.
    pub async fn close(&mut self) -> Result<()> {
        self.call("Page.close", json!({})).await.map(|_| ())
    }

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

    /// Counts what the DOM does from here until `changed` reads it back.
    pub async fn watch(&mut self) -> Result<()> {
        self.evaluate(&format!("({WATCH})()")).await.map(|_| ())
    }

    /// None when nothing is watching, which a document the page has since left also answers.
    pub async fn changed(&mut self) -> Result<Option<bool>> {
        Ok(self.evaluate(&format!("({CHANGED})()")).await?.as_bool())
    }

    /// Returns once nothing on the page has changed for `quiet`, and fails when
    /// `within` runs out first.
    pub async fn quiet(&mut self, quiet: Duration, within: Duration) -> Result<()> {
        let settled = self
            .evaluate_within(
                &format!("({QUIET})({}, {})", quiet.as_millis(), within.as_millis()),
                Some(within),
            )
            .await?;

        match settled.as_bool() {
            Some(true) => Ok(()),
            _ => Err(Error::Timeout {
                after: within,
                detail: "the page was still changing".to_string(),
            }),
        }
    }

    /// Polled rather than observed, so it sees a state that holds rather than
    /// one that flickers.
    pub async fn wait_until_true(&mut self, js: &str, within: Duration) -> Result<()> {
        let deadline = Instant::now() + within;

        loop {
            // A throw is not truthy and not a fault: an expression reaching
            // through something the page has not built yet raises until it has.
            let said = self.evaluate(&format!("!!({js})")).await;

            if said.map(|one| one.as_bool() == Some(true)).unwrap_or(false) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(Error::Timeout {
                    after: within,
                    detail: format!("{js} was never true"),
                });
            }

            tokio::time::sleep(POLL).await;
        }
    }

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

    pub async fn screenshot(&mut self) -> Result<Vec<u8>> {
        self.capture(&PageShot::default()).await
    }

    pub async fn capture(&mut self, shot: &PageShot) -> Result<Vec<u8>> {
        let mut params = json!({ "format": shot.format.name() });

        if let Some(map) = params.as_object_mut() {
            if shot.format == Picture::Jpeg {
                map.insert("quality".to_string(), json!(shot.quality));
            }
            if shot.full {
                map.insert("captureBeyondViewport".to_string(), json!(true));
            }
        }

        let answer = self.call("Page.captureScreenshot", params).await?;

        let encoded = answer
            .get("data")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::denied("the capture came back with no image"))?;

        base64_decode(encoded).ok_or_else(|| Error::denied("the capture is not valid base64"))
    }

    pub async fn click(&mut self, at: Point, button: Button) -> Result<()> {
        self.press(at, button, 1).await
    }

    /// Presses counting 1 then 2: anything else miscounts or raises no `dblclick`.
    pub async fn double_click(&mut self, at: Point, button: Button) -> Result<()> {
        self.press(at, button, 1).await?;
        self.press(at, button, 2).await
    }

    async fn press(&mut self, at: Point, button: Button, count: u32) -> Result<()> {
        self.button_event("mousePressed", at, button, count).await?;
        self.button_event("mouseReleased", at, button, count).await
    }

    async fn button_event(
        &mut self,
        kind: &str,
        at: Point,
        button: Button,
        count: u32,
    ) -> Result<()> {
        let (name, mask) = button_parts(button);
        // The protocol does not derive `buttons` from `button`.
        let buttons = match kind {
            "mouseReleased" => 0,
            _ => mask,
        };

        self.call(
            "Input.dispatchMouseEvent",
            json!({
                "type": kind,
                "x": at.x,
                "y": at.y,
                "button": name,
                "buttons": buttons,
                "clickCount": count,
            }),
        )
        .await
        .map(|_| ())
    }

    /// The page remembers where it ended, so the next path starts there.
    pub async fn move_along(&mut self, steps: &[Step], held: Option<Button>) -> Result<()> {
        let (name, buttons) = match held {
            Some(button) => button_parts(button),
            None => ("none", 0),
        };

        for step in steps {
            self.call(
                "Input.dispatchMouseEvent",
                json!({
                    "type": "mouseMoved",
                    "x": step.at.x,
                    "y": step.at.y,
                    "button": name,
                    "buttons": buttons,
                }),
            )
            .await?;
            if !step.pause.is_zero() {
                tokio::time::sleep(step.pause).await;
            }
        }

        match steps.last() {
            Some(last) => self.pointed(last.at).await,
            None => Ok(()),
        }
    }

    /// Where the page last saw the pointer, or the middle of the window before any move.
    async fn pointer(&mut self) -> Result<Point> {
        let at = self
            .evaluate(
                "JSON.stringify(window.__computerPointer || \
                 { x: Math.round(innerWidth / 2), y: Math.round(innerHeight / 2) })",
            )
            .await?;

        serde_json::from_str(at.as_str().unwrap_or("{}"))
            .map_err(|error| Error::denied(format!("the page would not parse: {error}")))
    }

    async fn pointed(&mut self, at: Point) -> Result<()> {
        self.evaluate(&format!(
            "void (window.__computerPointer = {{ x: {}, y: {} }})",
            at.x, at.y
        ))
        .await
        .map(|_| ())
    }

    async fn glide(&mut self, to: Point, motion: Motion, seed: u64) -> Result<()> {
        if motion.is_instant() {
            return self.pointed(to).await;
        }
        let from = self.pointer().await?;
        self.move_along(&path(from, to, motion, seed), None).await
    }

    pub async fn find(
        &mut self,
        query: &str,
        limit: Option<usize>,
        scroll: Option<bool>,
        exact: Option<bool>,
    ) -> Result<Vec<Element>> {
        let found = self
            .evaluate(&format!(
                r#"(() => {{
                     const found = ({MATCH})({}, {exact}).slice(0, {});
                     if ({scroll} && found[0]) {{
                       found[0].scrollIntoView({{ block: 'center', inline: 'center' }});
                     }}
                     return JSON.stringify(found.map({describe}));
                   }})()"#,
                json!(query),
                limit.unwrap_or(FOUND_DEFAULT),
                scroll = scroll.unwrap_or(false),
                exact = exact.unwrap_or(false),
                describe = describe()
            ))
            .await?;

        serde_json::from_str(found.as_str().unwrap_or("[]"))
            .map_err(|error| Error::denied(format!("the page would not parse: {error}")))
    }

    /// Every control in document order, with the headings between them, each
    /// numbered. `scope` is a query as `find` takes one. The numbers live in the page,
    /// so a second snapshot of the same document hands out the same ones.
    pub async fn snapshot(
        &mut self,
        scope: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Snapshot> {
        self.listing(scope, limit, Listing::Full).await
    }

    /// What appeared, changed or left since the last snapshot with this scope. The
    /// first of a document has nothing to compare with, so it answers in full.
    pub async fn snapshot_delta(
        &mut self,
        scope: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Snapshot> {
        self.listing(scope, limit, Listing::Delta).await
    }

    /// Remembers the page as it is now, unless a snapshot already has. A document no
    /// snapshot has numbered is left alone: numbering it here would hand out refs
    /// nobody has seen.
    pub async fn remember(&mut self) -> Result<()> {
        self.listing(None, Some(0), Listing::Prime)
            .await
            .map(|_| ())
    }

    /// What appeared, changed or left since what was last remembered; `None` when no
    /// snapshot has numbered this document. Never numbers a page itself.
    pub async fn changes(&mut self, limit: Option<usize>) -> Result<Option<Changes>> {
        Ok(self
            .listing(None, limit, Listing::Since)
            .await?
            .delta
            .filter(|changes| !changes.first))
    }

    async fn listing(
        &mut self,
        scope: Option<&str>,
        limit: Option<usize>,
        mode: Listing,
    ) -> Result<Snapshot> {
        let root = match scope {
            Some(query) => format!("({MATCH})({}, false).find(Boolean)", json!(query)),
            None => "document".to_string(),
        };

        let taken = self
            .evaluate(&format!(
                r#"(() => {{
                     const root = {root};
                     if (!root) return JSON.stringify({{ error: 'nothing on the page matched the scope' }});
                     return ({snapshot})(root, {limit}, {mode}, {key});
                   }})()"#,
                snapshot = snapshot_script(),
                limit = limit.unwrap_or(SNAPSHOT_DEFAULT),
                mode = json!(mode.as_str()),
                key = json!(scope.unwrap_or("")),
            ))
            .await?;

        let taken = taken
            .as_str()
            .ok_or_else(|| Error::denied("the page answered with something unreadable"))?;

        let taken: Value = serde_json::from_str(taken)
            .map_err(|error| Error::denied(format!("the page would not parse: {error}")))?;

        if let Some(error) = taken.get("error").and_then(Value::as_str) {
            return Err(Error::denied(error.to_string()));
        }

        serde_json::from_value(taken)
            .map_err(|error| Error::denied(format!("the page would not parse: {error}")))
    }

    pub async fn scroll(&mut self, what: Option<&str>, how: Scroll) -> Result<(i32, i32)> {
        let target = match what {
            Some(query) => format!("({MATCH})({}, false).find(Boolean)", json!(query)),
            None => "null".to_string(),
        };

        let moved = self
            .evaluate(&format!(
                r#"(() => {{
                     const el = {target};
                     const box = el || document.scrollingElement || document.body;
                     const to = {how};
                     if (to.x === null) {{
                       box.scrollBy(to.dx, to.dy);
                     }} else {{
                       box.scrollTo(to.x, to.y);
                     }}
                     return JSON.stringify([Math.round(box.scrollLeft), Math.round(box.scrollTop)]);
                   }})()"#,
                how = how.as_js()
            ))
            .await?;

        let moved: (i32, i32) = serde_json::from_str(moved.as_str().unwrap_or("[0,0]"))
            .map_err(|error| Error::denied(format!("the page would not parse: {error}")))?;

        Ok(moved)
    }

    pub async fn wait_for(
        &mut self,
        query: &str,
        gone: bool,
        within: Duration,
    ) -> Result<Option<Element>> {
        self.wait_for_any(query, &[], gone, within, false)
            .await
            .map(|(_, found)| found)
    }

    pub async fn wait_for_any(
        &mut self,
        query: &str,
        or: &[String],
        gone: bool,
        within: Duration,
        exact: bool,
    ) -> Result<(String, Option<Element>)> {
        self.wait_until(query, or, gone, within, exact, Ready::default())
            .await
    }

    pub async fn wait_until(
        &mut self,
        query: &str,
        or: &[String],
        gone: bool,
        within: Duration,
        exact: bool,
        ready: Ready,
    ) -> Result<(String, Option<Element>)> {
        let deadline = Instant::now() + within;
        let counts = |one: &Element| !ready.enabled || one.enabled;

        loop {
            let found = self.find(query, Some(1), None, Some(exact)).await?;
            let here = found.first().filter(|one| counts(one)).cloned();

            match (gone, &here) {
                (false, Some(_)) => return Ok((query.to_string(), here)),
                // Gone means gone, whatever was asked of a match that stayed.
                (true, None) if found.is_empty() => return Ok((query.to_string(), None)),
                _ => {}
            }

            for other in or {
                if let Some(one) = self
                    .find(other, Some(1), None, Some(exact))
                    .await?
                    .first()
                    .filter(|one| counts(one))
                    .cloned()
                {
                    return Ok((other.clone(), Some(one)));
                }
            }

            if Instant::now() >= deadline {
                let waited = match or.is_empty() {
                    true => query.to_string(),
                    false => format!("{query} (nor {})", or.join(", ")),
                };

                let wanted = match ready.enabled {
                    true => " enabled",
                    false => "",
                };

                return Err(Error::Timeout {
                    after: within,
                    detail: match gone {
                        true => format!("{waited} was still on the page"),
                        false => format!("nothing matching {waited} appeared{wanted}"),
                    },
                });
            }

            tokio::time::sleep(POLL).await;
        }
    }

    pub async fn back(&mut self) -> Result<()> {
        self.step_history(-1).await
    }

    pub async fn forward(&mut self) -> Result<()> {
        self.step_history(1).await
    }

    pub async fn reload(&mut self) -> Result<()> {
        self.call("Page.reload", json!({})).await.map(|_| ())
    }

    async fn step_history(&mut self, by: i64) -> Result<()> {
        let history = self.call("Page.getNavigationHistory", json!({})).await?;

        let at = history
            .get("currentIndex")
            .and_then(Value::as_i64)
            .ok_or_else(|| Error::denied("the page keeps no history"))?;

        let entries = history
            .get("entries")
            .and_then(Value::as_array)
            .ok_or_else(|| Error::denied("the page keeps no history"))?;

        let want = at + by;
        let entry = usize::try_from(want)
            .ok()
            .and_then(|want| entries.get(want))
            .and_then(|entry| entry.get("id"))
            .and_then(Value::as_i64)
            .ok_or_else(|| {
                Error::denied(match by < 0 {
                    true => "nothing before this page",
                    false => "nothing after this page",
                })
            })?;

        self.call("Page.navigateToHistoryEntry", json!({ "entryId": entry }))
            .await
            .map(|_| ())
    }

    pub async fn hover(&mut self, query: &str) -> Result<Element> {
        self.hover_with(query, Motion::Instant, 0).await
    }

    pub async fn hover_with(&mut self, query: &str, motion: Motion, seed: u64) -> Result<Element> {
        let (element, at) = self.reachable(query).await?;
        self.glide(at, motion, seed).await?;

        self.call(
            "Input.dispatchMouseEvent",
            json!({
                "type": "mouseMoved",
                "x": at.x,
                "y": at.y,
            }),
        )
        .await?;

        Ok(element)
    }

    pub async fn click_on(&mut self, query: &str, button: Button) -> Result<Element> {
        self.click_on_with(query, button, Motion::Instant, 0).await
    }

    pub async fn click_on_with(
        &mut self,
        query: &str,
        button: Button,
        motion: Motion,
        seed: u64,
    ) -> Result<Element> {
        let (element, at) = self.reachable(query).await?;
        self.glide(at, motion, seed).await?;
        self.click(at, button).await?;
        Ok(element)
    }

    pub async fn double_click_on(&mut self, query: &str, button: Button) -> Result<Element> {
        self.double_click_on_with(query, button, Motion::Instant, 0)
            .await
    }

    pub async fn double_click_on_with(
        &mut self,
        query: &str,
        button: Button,
        motion: Motion,
        seed: u64,
    ) -> Result<Element> {
        let (element, at) = self.reachable(query).await?;
        self.glide(at, motion, seed).await?;
        self.double_click(at, button).await?;
        Ok(element)
    }

    /// Presses on what `from` names, moves to what `to` names with the button held,
    /// and releases there. Both must fit in the window at once. An instant drag still
    /// passes through the middle: an application that tracks motion ignores a teleport.
    pub async fn drag_on(
        &mut self,
        from: &str,
        to: &str,
        button: Button,
        motion: Motion,
        seed: u64,
    ) -> Result<(Element, Element)> {
        let (target, _) = self.reachable(to).await?;
        let (source, start) = self.reachable(from).await?;
        let end = self
            .find(to, Some(1), Some(false), None)
            .await?
            .into_iter()
            .next()
            .and_then(|found| found.at)
            .ok_or_else(|| {
                Error::denied(format!(
                    "{to} is not in the window beside {from}: both ends of a drag must fit in it"
                ))
            })?;

        // A press on selected text, an image, a link or a draggable starts Chrome's own
        // drag, which swallows every pointer event after it. Intercepted, it is finished
        // by hand with a drop where the pointer ends, so a page built on either kind of
        // drag gets its drop.
        let intercepting = self
            .call("Input.setInterceptDrags", json!({ "enabled": true }))
            .await
            .is_ok();
        self.take_events();

        // Selected text is what most often turns a press into Chrome's own drag.
        self.evaluate("void (getSelection() && getSelection().removeAllRanges())")
            .await?;
        self.glide(start, motion, seed).await?;
        self.button_event("mousePressed", start, button, 1).await?;

        let steps = match motion.is_instant() {
            true => vec![
                Step {
                    at: Point {
                        x: start.x.midpoint(end.x),
                        y: start.y.midpoint(end.y),
                    },
                    pause: Duration::ZERO,
                },
                Step {
                    at: end,
                    pause: Duration::ZERO,
                },
            ],
            false => path(start, end, motion, seed.wrapping_add(1)),
        };
        self.move_along(&steps, Some(button)).await?;

        let intercepted = self
            .take_events()
            .into_iter()
            .find(|event| event.method == "Input.dragIntercepted")
            .map(|event| event.params["data"].clone());
        match intercepted {
            Some(data) => {
                for kind in ["dragEnter", "dragOver", "drop"] {
                    self.call(
                        "Input.dispatchDragEvent",
                        json!({ "type": kind, "x": end.x, "y": end.y, "data": data }),
                    )
                    .await?;
                }
            }
            None => self.button_event("mouseReleased", end, button, 1).await?,
        }
        if intercepting {
            let _ = self
                .call("Input.setInterceptDrags", json!({ "enabled": false }))
                .await;
        }

        Ok((
            source,
            Element {
                at: Some(end),
                ..target
            },
        ))
    }

    /// Typed where a person types, assigned where a person does not: a slider
    /// is dragged and a date is picked, and neither takes keystrokes.
    pub async fn fill(&mut self, query: &str, text: &str) -> Result<()> {
        let query = &self.control_named(query, "any").await?;
        let how = self
            .evaluate(&format!("({fill})({})", json!(query), fill = fillable()))
            .await?;

        match how.as_str() {
            Some("type") => {
                let (_, at) = self.reachable(query).await?;
                self.click(at, Button::Left).await?;

                self.evaluate(&format!(
                    "(({MATCH})({}, false).find(Boolean) || {{}}).value = ''",
                    json!(query)
                ))
                .await?;

                self.type_text(text).await
            }
            Some("rich") => {
                let (_, at) = self.reachable(query).await?;
                self.click(at, Button::Left).await?;

                self.evaluate(&format!(
                    "({rich})({}, {})",
                    json!(query),
                    text.is_empty(),
                    rich = rich()
                ))
                .await?;

                self.type_text(text).await
            }
            Some("assign") => {
                let took = self
                    .evaluate(&format!(
                        "({set})({}, {})",
                        json!(query),
                        json!(text),
                        set = assign()
                    ))
                    .await?;

                match took.as_str() {
                    Some("gone") => Err(Error::denied(format!("nothing matched {query}"))),
                    Some("rejected") => {
                        Err(Error::denied(format!("{query} will not hold {text:?}")))
                    }
                    _ => Ok(()),
                }
            }
            Some("gone") => Err(Error::denied(format!("nothing matched {query}"))),
            Some(shut @ ("disabled" | "read-only")) => {
                Err(Error::denied(format!("{query} is {shut}")))
            }
            Some(verb @ ("select" | "check" | "click" | "upload")) => Err(Error::denied(format!(
                "{query} does not take typing: use {verb}"
            ))),
            _ => Err(Error::denied(
                self.near(query, "any", format!("{query} holds no value"), "field")
                    .await,
            )),
        }
    }

    async fn near(&mut self, query: &str, kind: &str, plain: String, noun: &str) -> String {
        let Ok(seen) = self
            .evaluate(&format!(
                "({nearby})({}, {})",
                json!(query),
                json!(kind),
                nearby = nearby()
            ))
            .await
        else {
            return plain;
        };

        let tag = seen.get("tag").and_then(Value::as_str).unwrap_or_default();
        let fields: Vec<&str> = seen
            .get("fields")
            .and_then(Value::as_array)
            .map(|fields| fields.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let where_ = seen.get("where").and_then(Value::as_str).unwrap_or("in");
        let count = seen.get("count").and_then(Value::as_u64).unwrap_or(0);
        let crowd = seen.get("crowd").and_then(Value::as_bool).unwrap_or(false);

        match fields.as_slice() {
            [] if tag.is_empty() => plain,
            [] if crowd => format!(
                "{plain}: it matched <{tag}>, and {count} {noun}s are {where_} it, too many to \
                 name; take a snapshot"
            ),
            [] => format!("{plain}: it matched <{tag}>, with no {noun} in it or beside it"),
            [one] => format!("{plain}: it matched <{tag}>, and the {noun} {where_} it is {one}"),
            several => format!(
                "{plain}: it matched <{tag}>, and the {noun}s {where_} it are {}",
                several.join(", ")
            ),
        }
    }

    /// Keyboard focus, without the click that would otherwise carry it: a
    /// click on a menu opens it, and one on a submit sends the form.
    pub async fn focus(&mut self, query: &str) -> Result<Element> {
        let query = &self.control_named(query, "any").await?;
        let element = self.reach(query).await?;

        self.evaluate(&format!(
            r#"(() => {{
                 const el = ({MATCH})({}, false).find(Boolean);
                 if (!el) return 'gone';
                 el.focus({{ preventScroll: true }});
                 return document.activeElement === el ? 'ok' : 'refused';
               }})()"#,
            json!(query)
        ))
        .await
        .and_then(|said| match said.as_str() {
            // An element with no tabindex takes no focus, and saying so beats
            // answering as though the keyboard reaches it.
            Some("refused") => Err(Error::denied(format!("{query} will not take focus"))),
            Some("gone") => Err(Error::denied(format!("{query} left the page"))),
            _ => Ok(()),
        })?;

        Ok(element)
    }

    /// Tick a box, or untick it. Already in that state is not a click.
    ///
    /// Clicked rather than assigned: setting `checked` fires no `change`, so a
    /// page that validates on one never learns the box was ticked.
    pub async fn check(&mut self, query: &str, on: bool) -> Result<Element> {
        let state = self
            .evaluate(&format!(
                r#"(() => {{
                     const el = ({box})({});
                     if (!el) return 'gone';
                     if (!('checked' in el)) return 'not a box';
                     if (el.disabled) return 'disabled';
                     return el.checked ? 'on' : 'off';
                   }})()"#,
                json!(query),
                box = checkbox()
            ))
            .await?;

        match state.as_str() {
            Some("gone") => return Err(Error::denied(format!("nothing matched {query}"))),
            Some("not a box") => {
                return Err(Error::denied(format!(
                    "{query} is not a checkbox or a radio"
                )));
            }
            Some("disabled") => return Err(Error::denied(format!("{query} is disabled"))),
            _ => {}
        }

        let already = state.as_str() == Some("on");
        let (element, at) = self.reachable(query).await?;

        // A radio cannot be turned off by clicking it, so that is refused
        // rather than clicked and silently left on.
        if already && !on {
            let radio = self
                .evaluate(&format!(
                    "((({box})({})) || {{}}).type === 'radio'",
                    json!(query),
                    box = checkbox()
                ))
                .await?;

            if radio.as_bool() == Some(true) {
                return Err(Error::denied(format!(
                    "{query} is a radio; choose another in its group instead"
                )));
            }
        }

        if already != on {
            self.click(at, Button::Left).await?;
        }

        Ok(element)
    }

    pub async fn options(&mut self, query: &str) -> Result<Vec<String>> {
        let query = &self.control_named(query, "select").await?;
        let listed = self
            .evaluate(&format!(
                "JSON.stringify(Array.from((({MATCH})({}, false).find(e => e.tagName === 'SELECT') || {{ options: [] }}).options).map(o => o.text.trim()))",
                json!(query)
            ))
            .await?;

        serde_json::from_str(listed.as_str().unwrap_or("[]"))
            .map_err(|error| Error::denied(format!("the page would not parse: {error}")))
    }

    /// The options named become the whole selection; `drop` takes them out of
    /// it instead, or empties it when none is named.
    pub async fn choose(
        &mut self,
        query: &str,
        options: &[String],
        drop: bool,
    ) -> Result<Vec<String>> {
        let query = &self.control_named(query, "select").await?;
        let answered = self
            .evaluate(&format!(
                "({pick})({}, {}, {drop})",
                json!(query),
                json!(options),
                pick = dropdown()
            ))
            .await?;

        if let Some(no) = answered.get("no").and_then(Value::as_str) {
            let plain = format!("{no}: {query}");
            return Err(Error::denied(match no == NO_DROPDOWN {
                true => self.near(query, "select", plain, "dropdown").await,
                false => plain,
            }));
        }

        Ok(answered
            .get("picked")
            .and_then(Value::as_array)
            .map(|picked| {
                picked
                    .iter()
                    .filter_map(|one| one.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Paths are inside the box.
    pub async fn upload(&mut self, query: &str, paths: &[String]) -> Result<()> {
        let query = &self.control_named(query, "file").await?;
        let handle = self
            .call(
                "Runtime.evaluate",
                json!({
                    "expression": format!("({FILE_INPUT})({})", json!(query), FILE_INPUT = file_input()),
                    "returnByValue": false,
                }),
            )
            .await?;

        let Some(object) = handle
            .get("result")
            .and_then(|result| result.get("objectId"))
            .and_then(Value::as_str)
            .map(str::to_string)
        else {
            let plain = format!("no file input matched {query}");
            return Err(Error::denied(
                self.near(query, "file", plain, "file input").await,
            ));
        };

        self.call(
            "DOM.setFileInputFiles",
            json!({ "files": paths, "objectId": object }),
        )
        .await
        .map(|_| ())
    }

    async fn control_named(&mut self, query: &str, kind: &str) -> Result<String> {
        let control = self
            .evaluate(&format!(
                "({named})({}, {})",
                json!(query),
                json!(kind),
                named = named()
            ))
            .await?;

        Ok(control
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| query.to_string()))
    }

    async fn ref_state(&mut self, query: &str) -> Result<Option<String>> {
        let Some(number) = ref_number(query) else {
            return Ok(None);
        };

        let state = self.evaluate(&format!("({REF_STATE})({number})")).await?;

        Ok(Some(match state.as_str() {
            Some("none") => {
                format!("{query} names nothing: no snapshot has been taken of this page")
            }
            Some("unknown") => format!("{query} was not given out by a snapshot of this page"),
            Some("gone") => format!("{query} has left the page since the snapshot; take another"),
            _ => format!("{query} is on the page but hidden, so there is nowhere to press"),
        }))
    }

    async fn reachable(&mut self, query: &str) -> Result<(Element, Point)> {
        let element = self.reach(query).await?;

        match element.at {
            Some(at) => Ok((element, at)),
            None => Err(Error::denied(format!(
                "{query} could not be brought into view, so there is nowhere to press"
            ))),
        }
    }

    /// The point is asked what sits on top before a press is sent, and a dialog still
    /// fading out is given a moment to go. `at` is that point, not always the middle.
    async fn reach(&mut self, query: &str) -> Result<Element> {
        let deadline = Instant::now() + COVERED;

        loop {
            let found = self
                .evaluate(&format!(
                    r#"(() => {{
                         const el = ({MATCH})({}, false).find(Boolean);
                         if (!el) return 'null';
                         // Instant: a smooth scroll moves the element after it was measured.
                         el.scrollIntoView({{ block: 'center', inline: 'center', behavior: 'instant' }});
                         return JSON.stringify({{ element: ({describe})(el), reach: ({REACH})(el) }});
                       }})()"#,
                    json!(query),
                    describe = describe()
                ))
                .await?;

            let found = found.as_str().unwrap_or("null");
            if found == "null" {
                return Err(Error::denied(match self.ref_state(query).await? {
                    Some(why) => why,
                    None => format!("nothing on the page matched {query}"),
                }));
            }

            let reached: Reached = serde_json::from_str(found)
                .map_err(|error| Error::denied(format!("the page would not parse: {error}")))?;

            match reached.reach {
                Reach::At { at } => {
                    return Ok(Element {
                        at: Some(at),
                        ..reached.element
                    });
                }
                Reach::Covered { covered } => {
                    if Instant::now() >= deadline {
                        return Err(Error::denied(format!(
                            "{query} is covered by {covered}, so a press would land on that"
                        )));
                    }
                    tokio::time::sleep(COVERED_POLL).await;
                }
                Reach::Off { .. } => {
                    return Ok(Element {
                        at: None,
                        ..reached.element
                    });
                }
            }
        }
    }

    /// One insertion, not one per character: a picker that opens on the first
    /// keystroke takes the focus, and the rest would land in it.
    pub async fn type_text(&mut self, text: &str) -> Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        self.call("Input.insertText", json!({ "text": text }))
            .await?;

        let last = text
            .char_indices()
            .next_back()
            .and_then(|(_, typed)| Keystroke::of(typed));
        if let Some(stroke) = last {
            for event in ["rawKeyDown", "keyUp"] {
                self.call("Input.dispatchKeyEvent", stroke.event(event))
                    .await?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Keystroke {
    typed: char,
    code: String,
    virtual_key: u32,
    shifted: bool,
}

impl Keystroke {
    const SYMBOLS: [(char, char, &'static str, u32); 21] = [
        ('1', '!', "Digit1", 49),
        ('2', '@', "Digit2", 50),
        ('3', '#', "Digit3", 51),
        ('4', '$', "Digit4", 52),
        ('5', '%', "Digit5", 53),
        ('6', '^', "Digit6", 54),
        ('7', '&', "Digit7", 55),
        ('8', '*', "Digit8", 56),
        ('9', '(', "Digit9", 57),
        ('0', ')', "Digit0", 48),
        ('-', '_', "Minus", 189),
        ('=', '+', "Equal", 187),
        ('[', '{', "BracketLeft", 219),
        (']', '}', "BracketRight", 221),
        ('\\', '|', "Backslash", 220),
        (';', ':', "Semicolon", 186),
        ('\'', '"', "Quote", 222),
        (',', '<', "Comma", 188),
        ('.', '>', "Period", 190),
        ('/', '?', "Slash", 191),
        ('`', '~', "Backquote", 192),
    ];

    fn of(typed: char) -> Option<Self> {
        if typed == ' ' {
            return Some(Self {
                typed,
                code: "Space".to_string(),
                virtual_key: 32,
                shifted: false,
            });
        }
        if typed.is_ascii_alphabetic() {
            let upper = typed.to_ascii_uppercase();
            return Some(Self {
                typed,
                code: format!("Key{upper}"),
                virtual_key: upper as u32,
                shifted: typed.is_ascii_uppercase(),
            });
        }

        Self::SYMBOLS
            .iter()
            .find(|(plain, shifted, _, _)| typed == *plain || typed == *shifted)
            .map(|(_, shifted, code, virtual_key)| Self {
                typed,
                code: (*code).to_string(),
                virtual_key: *virtual_key,
                shifted: typed == *shifted,
            })
    }

    fn event(&self, kind: &str) -> Value {
        json!({
            "type": kind,
            "key": self.typed.to_string(),
            "code": self.code,
            "windowsVirtualKeyCode": self.virtual_key,
            "nativeVirtualKeyCode": self.virtual_key,
            "modifiers": if self.shifted { 8 } else { 0 },
        })
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

async fn read_http(socket: &mut TcpStream) -> Result<HttpAnswer> {
    let mut raw = Vec::new();
    let mut buffer = [0u8; 4096];

    loop {
        match tokio::time::timeout(TIMEOUT, socket.read(&mut buffer)).await {
            Ok(Ok(0)) | Err(_) => break,
            Ok(Ok(read)) => raw.extend_from_slice(&buffer[..read]),
            Ok(Err(error)) => return Err(Error::transport(error.to_string(), true)),
        }

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

/// Does not check `Sec-WebSocket-Accept`: loopback only, and a bad answer fails to parse.
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

    // Byte by byte: anything past the head is the first frame.
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

fn close_frame() -> [u8; 6] {
    let mask = nonce();
    [0x88, 0x80, mask[0], mask[1], mask[2], mask[3]]
}

async fn send_close(socket: &mut TcpStream) -> Result<()> {
    socket
        .write_all(&close_frame())
        .await
        .map_err(|error| Error::transport(error.to_string(), true))
}

async fn send_text(socket: &mut TcpStream, text: &str) -> Result<()> {
    let payload = text.as_bytes();
    let mut frame = vec![0x81u8];

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
            0x0..=0x2 => {
                assembled.extend_from_slice(&payload);
                if final_frame {
                    return String::from_utf8(assembled)
                        .map_err(|error| Error::transport(error.to_string(), false));
                }
            }
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

/// Not cryptographic: the mask only guards proxies, and this is loopback.
fn nonce() -> [u8; 16] {
    // The counter keeps two calls in one clock tick apart.
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

#[derive(Deserialize)]
struct IdbRead {
    databases: Vec<Database>,
    #[serde(default)]
    skipped: Vec<String>,
}

async fn read_items(page: &mut Page, which: &str) -> Option<BTreeMap<String, String>> {
    let read = page
        .evaluate(&format!(
            "JSON.stringify(Object.fromEntries(Object.keys({which}).map(k => [k, {which}.getItem(k)])))"
        ))
        .await
        .ok()?;

    let items: BTreeMap<String, String> = serde_json::from_str(read.as_str()?).ok()?;
    (!items.is_empty()).then_some(items)
}

async fn write_items(page: &mut Page, which: &str, items: &BTreeMap<String, String>) {
    let Ok(items) = serde_json::to_string(items) else {
        return;
    };

    page.evaluate(&format!(
        "Object.entries({items}).forEach(([k, v]) => {which}.setItem(k, v))"
    ))
    .await
    .ok();
}

/// Through the page: the protocol can read IndexedDB but not write it.
const IDB_EXPORT: &str = r#"(async () => {
  const skipped = [];
  const databases = [];

  if (!indexedDB.databases) return JSON.stringify({ databases, skipped: ['this browser will not list its databases'] });

  for (const { name, version } of await indexedDB.databases()) {
    if (!name) continue;
    let db;
    try {
      db = await new Promise((ok, no) => {
        const r = indexedDB.open(name);
        r.onsuccess = () => ok(r.result);
        r.onerror = () => no(r.error);
        r.onblocked = () => no(new Error('blocked'));
      });
    } catch (e) {
      skipped.push(`database ${name}: ${e}`);
      continue;
    }

    const stores = [];
    for (const store of Array.from(db.objectStoreNames)) {
      try {
        const tx = db.transaction(store, 'readonly');
        const os = tx.objectStore(store);
        const [values, keys] = await Promise.all([
          new Promise((ok, no) => { const r = os.getAll(); r.onsuccess = () => ok(r.result); r.onerror = () => no(r.error); }),
          new Promise((ok, no) => { const r = os.getAllKeys(); r.onsuccess = () => ok(r.result); r.onerror = () => no(r.error); }),
        ]);

        const records = [];
        for (let i = 0; i < values.length; i++) {
          try {
            JSON.stringify(values[i]);
            records.push([keys[i], values[i]]);
          } catch (e) {
            skipped.push(`${name}/${store}: a record could not be written as JSON`);
          }
        }

        stores.push({
          name: store,
          key_path: os.keyPath === null ? undefined : os.keyPath,
          auto_increment: os.autoIncrement,
          records,
        });
      } catch (e) {
        skipped.push(`${name}/${store}: ${e}`);
      }
    }

    databases.push({ name, version: version || db.version, stores });
    db.close();
  }

  return JSON.stringify({ databases, skipped });
})()"#;

const IDB_IMPORT: &str = r#"(async (databases) => {
  for (const wanted of databases) {
    const db = await new Promise((ok, no) => {
      const r = indexedDB.open(wanted.name, wanted.version);
      // Stores can only be created on upgrade, hence the saved version.
      r.onupgradeneeded = () => {
        for (const store of wanted.stores) {
          if (r.result.objectStoreNames.contains(store.name)) continue;
          r.result.createObjectStore(store.name, {
            keyPath: store.key_path ?? null,
            autoIncrement: !!store.auto_increment,
          });
        }
      };
      r.onsuccess = () => ok(r.result);
      r.onerror = () => no(r.error);
      r.onblocked = () => no(new Error('blocked'));
    });

    for (const store of wanted.stores) {
      if (!db.objectStoreNames.contains(store.name)) continue;
      const tx = db.transaction(store.name, 'readwrite');
      const os = tx.objectStore(store.name);
      for (const [key, value] of store.records) {
        // Passing a key to a store with a key path is an error.
        os.keyPath === null ? os.put(value, key) : os.put(value);
      }
      await new Promise(ok => { tx.oncomplete = ok; tx.onerror = ok; tx.onabort = ok; });
    }

    db.close();
  }
  return 'ok';
})"#;

fn covers(origin: &str, domain: &str) -> bool {
    let host = origin
        .split("//")
        .nth(1)
        .unwrap_or(origin)
        .split('/')
        .next()
        .unwrap_or_default()
        // Cookies are scoped to a host, never a port.
        .split(':')
        .next()
        .unwrap_or_default();
    let domain = domain.trim_start_matches('.');

    host == domain || host.ends_with(&format!(".{domain}"))
}

fn cookie_in(value: &Value) -> Option<Cookie> {
    Some(Cookie {
        name: value.get("name")?.as_str()?.to_string(),
        value: value.get("value")?.as_str()?.to_string(),
        domain: value.get("domain")?.as_str()?.to_string(),
        path: value
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or("/")
            .to_string(),
        // -1 means a session cookie, not a time.
        expires: value
            .get("expires")
            .and_then(Value::as_f64)
            .filter(|expires| *expires > 0.0),
        http_only: value
            .get("httpOnly")
            .and_then(Value::as_bool)
            .unwrap_or_default(),
        secure: value
            .get("secure")
            .and_then(Value::as_bool)
            .unwrap_or_default(),
        same_site: value
            .get("sameSite")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

fn cookie_out(cookie: &Cookie) -> Value {
    let mut out = json!({
        "name": cookie.name,
        "value": cookie.value,
        "domain": cookie.domain,
        "path": cookie.path,
        "httpOnly": cookie.http_only,
        "secure": cookie.secure,
    });

    if let Some(expires) = cookie.expires {
        out["expires"] = json!(expires);
    }
    if let Some(same_site) = &cookie.same_site {
        out["sameSite"] = json!(same_site);
    }

    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_a_cookie_belongs_to_the_origin_that_set_it() {
        assert!(covers("https://example.com", "example.com"));
        assert!(covers("https://www.example.com", ".example.com"));
        assert!(covers("https://example.com", ".example.com"));
    }

    #[test]
    fn test_a_port_is_not_part_of_a_cookies_home() {
        assert!(covers("http://127.0.0.1:8000", "127.0.0.1"));
        assert!(covers("https://example.com:8443", "example.com"));
    }

    #[test]
    fn test_one_site_does_not_cover_another() {
        assert!(!covers("https://example.com", "evil.com"));
        assert!(!covers("https://example.com", "notexample.com"));
        assert!(!covers("https://example.com.evil.com", "example.com"));
    }

    #[test]
    fn test_a_session_never_prints_what_it_holds() {
        let session = Session {
            origins: vec!["https://example.com".to_string()],
            cookies: vec![Cookie {
                name: "sid".to_string(),
                value: "the-secret-itself".to_string(),
                domain: "example.com".to_string(),
                path: "/".to_string(),
                expires: None,
                http_only: true,
                secure: true,
                same_site: None,
            }],
            storage: BTreeMap::new(),
            session_storage: BTreeMap::new(),
            databases: BTreeMap::new(),
            carried: Carry::default(),
            incomplete: Vec::new(),
        };

        let shown = format!("{session:?}");
        assert!(!shown.contains("the-secret-itself"), "{shown}");
        assert!(shown.contains("cookies: 1"), "{shown}");
    }

    #[test]
    fn test_the_default_carries_where_a_login_normally_is() {
        let carry = Carry::default();

        assert!(carry.cookies && carry.local_storage);
        assert!(!carry.session_storage && !carry.indexed_db);
    }

    #[test]
    fn test_a_set_can_be_built_up_or_cut_down() {
        let only_cookies = Carry {
            local_storage: false,
            ..Carry::default()
        };
        assert!(only_cookies.cookies && !only_cookies.local_storage);
        assert!(!only_cookies.needs_a_page(), "nothing has to be visited");

        let with_databases = Carry {
            indexed_db: true,
            ..Carry::default()
        };
        assert!(with_databases.needs_a_page());

        assert_eq!(
            Carry::none(),
            Carry {
                cookies: false,
                local_storage: false,
                session_storage: false,
                indexed_db: false,
            }
        );
    }

    #[test]
    fn test_a_session_cookie_is_not_given_a_time() {
        let cookie = cookie_in(&json!({
            "name": "s", "value": "v", "domain": "example.com", "path": "/", "expires": -1.0
        }))
        .expect("a cookie");

        assert_eq!(cookie.expires, None);
        assert!(cookie_out(&cookie).get("expires").is_none());
    }

    #[test]
    fn test_every_flag_survives_the_round_trip() {
        let cookie = cookie_in(&json!({
            "name": "s", "value": "v", "domain": "example.com", "path": "/x",
            "httpOnly": true, "secure": true, "sameSite": "Lax", "expires": 1e9
        }))
        .expect("a cookie");

        let out = cookie_out(&cookie);
        assert_eq!(out["httpOnly"], json!(true));
        assert_eq!(out["secure"], json!(true));
        assert_eq!(out["sameSite"], json!("Lax"));
        assert_eq!(out["path"], json!("/x"));
    }

    #[test]
    fn test_a_space_is_not_left_in_a_url() {
        assert_eq!(
            SearchProvider::DuckDuckGo.url_for("toby oyetoke"),
            "https://duckduckgo.com/html/?q=toby+oyetoke"
        );
    }

    #[test]
    fn test_a_quoted_phrase_survives_being_sent() {
        let url = SearchProvider::DuckDuckGo.url_for("\"exact words\"");

        assert!(url.ends_with("q=%22exact+words%22"), "{url}");
    }

    #[test]
    fn test_a_query_cannot_start_another_parameter() {
        let url = SearchProvider::Google.url_for("cats & dogs");

        assert!(url.ends_with("q=cats+%26+dogs"), "{url}");
        assert_eq!(url.matches('&').count(), 0);
    }

    #[test]
    fn test_a_fragment_cannot_cut_the_url_short() {
        let url = SearchProvider::Bing.url_for("c# tutorial");

        assert!(url.ends_with("q=c%23+tutorial"), "{url}");
    }

    #[test]
    fn test_every_provider_puts_the_query_in_the_url() {
        for provider in [
            SearchProvider::DuckDuckGo,
            SearchProvider::Google,
            SearchProvider::Bing,
        ] {
            let url = provider.url_for("rust");

            assert!(url.starts_with("https://"), "{provider:?}: {url}");
            assert!(url.ends_with("q=rust"), "{provider:?}: {url}");
        }
    }

    #[test]
    fn test_the_default_is_the_one_a_reader_gets_results_from() {
        assert_eq!(SearchProvider::default(), SearchProvider::DuckDuckGo);
    }

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
    fn test_a_listed_title_comes_back_as_the_page_wrote_it() {
        assert_eq!(unescaped("M&#39;Diq"), "M'Diq");
        assert_eq!(unescaped("Sea &amp; Spa"), "Sea & Spa");
        assert_eq!(unescaped("&lt;b&gt; &quot;x&quot;"), "<b> \"x\"");
        assert_eq!(
            unescaped("R&D &unknown; &"),
            "R&D &unknown; &",
            "what is not one of the five it escapes is left alone"
        );
        assert_eq!(unescaped("plain"), "plain");
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
    fn test_the_browser_socket_keeps_its_uuid_and_drops_its_private_port() {
        assert_eq!(
            websocket_path("ws://127.0.0.1:9222/devtools/browser/BROWSER-1").as_deref(),
            Some("/devtools/browser/BROWSER-1")
        );
    }

    #[test]
    fn test_devtools_clones_share_the_discovered_browser_path() {
        let devtools = Devtools::new("127.0.0.1", 9223);
        let other = devtools.clone();
        devtools
            .browser_path
            .set("/devtools/browser/BROWSER-1".to_string())
            .expect("the first path");

        assert_eq!(
            other.browser_path.get().map(String::as_str),
            Some("/devtools/browser/BROWSER-1")
        );
    }

    #[test]
    fn test_a_dropped_connection_sends_a_masked_close_frame() {
        let frame = close_frame();
        assert_eq!(frame.len(), 6);
        assert_eq!(frame[0], 0x88, "final close frame");
        assert_eq!(frame[1], 0x80, "client frames are masked");
    }

    #[test]
    fn test_a_group_target_names_its_context_and_url() {
        assert_eq!(
            create_target_params("CONTEXT-1", "https://example.com"),
            json!({
                "url": "https://example.com",
                "browserContextId": "CONTEXT-1",
            })
        );
        assert_eq!(
            browser_context_params("CONTEXT-1"),
            json!({ "browserContextId": "CONTEXT-1" }),
            "the disposal command must name only the context it removes"
        );
    }

    #[test]
    fn test_browser_context_ids_are_strings_or_the_answer_is_refused() {
        assert_eq!(
            browser_context_ids(&json!({
                "browserContextIds": ["CONTEXT-1", "CONTEXT-2"]
            }))
            .expect("context ids"),
            vec!["CONTEXT-1", "CONTEXT-2"]
        );
        assert!(matches!(
            browser_context_ids(&json!({ "browserContextIds": [1] })),
            Err(Error::Denied { .. })
        ));
        assert!(matches!(
            required_string(&json!({}), "targetId", "Target.createTarget"),
            Err(Error::Denied { .. })
        ));
    }

    #[test]
    fn test_only_targets_from_the_asked_for_context_are_grouped() {
        let result = json!({
            "targetInfos": [
                { "targetId": "DEFAULT" },
                { "targetId": "ONE", "browserContextId": "CONTEXT-1" },
                { "targetId": "TWO", "browserContextId": "CONTEXT-2" },
                { "targetId": "ONE-WORKER", "browserContextId": "CONTEXT-1" }
            ]
        });

        assert_eq!(
            target_ids_in_context(&result, "CONTEXT-1").expect("target ids"),
            HashSet::from(["ONE".to_string(), "ONE-WORKER".to_string()])
        );
        assert!(matches!(
            target_ids_in_context(&json!({}), "CONTEXT-1"),
            Err(Error::Denied { .. })
        ));
    }

    #[test]
    fn test_a_double_click_is_two_presses_and_the_second_counts_two() {
        let source = include_str!("cdp.rs");
        let method = source
            .split("pub async fn double_click(")
            .nth(1)
            .expect("double_click is there");

        assert!(method.contains("self.press(at, button, 1)"));
        assert!(method.contains("self.press(at, button, 2)"));
    }

    #[test]
    fn test_a_role_covers_every_way_a_page_builds_one() {
        let button = selector_for("button").expect("button is known");

        for way in ["button", "[role=button]", "input[type=submit]", "summary"] {
            assert!(button.contains(way), "a button is also written {way}");
        }

        assert!(selector_for("link").expect("link").contains("a[href]"));
        assert!(
            selector_for("textbox")
                .expect("textbox")
                .contains("textarea"),
            "a textbox is not only an input"
        );
    }

    #[test]
    fn test_a_role_nobody_serves_is_refused_and_the_rest_are_named() {
        assert!(selector_for("gizmo").is_none());

        for role in ROLES.split(',').map(str::trim) {
            assert!(
                selector_for(role).is_some(),
                "{role} is offered but not served"
            );
        }
    }

    #[tokio::test]
    async fn test_a_deadline_is_bounded_by_the_clock_and_not_only_the_protocol() {
        assert!(
            GRACE < Duration::from_secs(1),
            "the outer wait is the caller's answer, so it follows close behind"
        );
    }

    #[test]
    fn test_an_icon_button_is_found_by_what_its_icon_says() {
        assert!(MATCH.contains("img[alt], area[alt], input[alt]"));
        assert!(
            MATCH.contains("svg > title, svg > desc"),
            "an svg names itself in a child element, not an attribute"
        );
        assert!(
            MATCH.contains("el.getAttribute('title') ||\n                       icon(el)"),
            "and it is the last resort, after everything the element says itself"
        );
    }

    #[test]
    fn test_an_inert_element_is_not_called_enabled() {
        assert!(
            DESCRIBE.contains("aria('disabled') === 'true'"),
            "a div says it is disabled through aria, not through a property"
        );
        assert!(
            DESCRIBE.contains("enabled: !off"),
            "and enabled follows that rather than the property alone"
        );
    }

    #[test]
    fn test_a_disclosure_that_is_shut_says_so() {
        assert!(DESCRIBE.contains("states.push('collapsed')"));
        assert!(
            DESCRIBE.contains("aria('expanded') === 'false'"),
            "collapsed is not the absence of expanded: one says there is a \
             disclosure and it is shut, the other says there is none"
        );
    }

    #[test]
    fn test_a_role_is_read_and_never_inferred() {
        assert!(DESCRIBE.contains("el.getAttribute('role')"));
        assert!(!DESCRIBE.contains("'button' :"));
    }

    #[test]
    fn test_states_come_back_as_words() {
        let inert = r#"[{"text":"Next","tag":"div","role":"button",
                         "states":["disabled","collapsed"],"visible":true,
                         "width":10,"height":10,"enabled":false}]"#;
        let found: Vec<Element> = serde_json::from_str(inert).expect("it parses");

        assert_eq!(found[0].role.as_deref(), Some("button"));
        assert_eq!(found[0].states, vec!["disabled", "collapsed"]);
        assert!(!found[0].enabled);
    }

    #[test]
    fn test_an_element_with_neither_still_parses() {
        let plain = r#"[{"text":"Go","tag":"button","visible":true,
                         "width":10,"height":10,"enabled":true}]"#;
        let found: Vec<Element> = serde_json::from_str(plain).expect("it parses");

        assert!(found[0].role.is_none());
        assert!(found[0].states.is_empty());
    }

    #[test]
    fn test_a_selector_is_confirmed_before_it_is_handed_out() {
        assert!(
            SELECTOR.contains("querySelectorAll(q).length === 1"),
            "a selector matching two elements looks precise and reaches the wrong one"
        );
        assert!(
            SELECTOR.contains("data-testid"),
            "what a page meant as a handle comes before anything invented"
        );
        assert!(
            SELECTOR.contains("nth-of-type"),
            "and a path is the last resort rather than the first answer"
        );
    }

    #[test]
    fn test_describe_carries_the_selector_it_generated() {
        let script = describe();

        assert!(
            !script.contains("SELECTOR_FN"),
            "the placeholder was replaced: a page carries no imports"
        );
        assert!(script.contains("querySelectorAll(q).length === 1"));
    }

    #[test]
    fn test_an_element_says_what_it_is_called_and_whether_it_can_be_seen() {
        let named = r#"[{"text":"","tag":"input","selector":"[name=\"custname\"]",
                         "label":"Your name","visible":true,"at":{"x":10,"y":20},
                         "width":100,"height":20,"enabled":true}]"#;
        let found: Vec<Element> = serde_json::from_str(named).expect("it parses");

        assert_eq!(found[0].selector.as_deref(), Some("[name=\"custname\"]"));
        assert_eq!(found[0].label.as_deref(), Some("Your name"));
        assert!(found[0].visible);
    }

    #[test]
    fn test_a_substring_match_takes_the_innermost_of_them() {
        assert!(
            MATCH.contains("el.contains(other)"),
            "an ancestor that matches only through a descendant is dropped"
        );
        assert!(
            MATCH.contains("querySelectorAll(clickable)"),
            "and clickables are still swept first, so a button beats its own span"
        );
    }

    #[test]
    fn test_exact_matching_runs_one_pass_only() {
        assert!(
            MATCH.contains("exact\n    ? [el => words(el) === want]"),
            "exact drops the substring pass rather than reordering it"
        );
    }

    #[test]
    fn test_a_ref_names_one_element_and_nothing_else() {
        assert!(
            MATCH.contains("/^@e(\\d+)$/"),
            "a ref is the @ and a number, so words that happen to start with e are not one"
        );
        assert!(
            MATCH.contains("if (numbered && numbered.isConnected) add(numbered);\n    return out;"),
            "a ref resolves alone: no selector pass and no words pass after it"
        );
        for script in [MATCH, DESCRIBE, SNAPSHOT] {
            assert!(
                script.contains("window.__computerRefs"),
                "the three scripts share one global, or a number means different things"
            );
        }
    }

    #[test]
    fn test_a_number_survives_a_second_snapshot() {
        assert!(
            SNAPSHOT.contains("if (!refs.includes(el)) refs.push(el);"),
            "an element already numbered keeps its number, and a new one is appended"
        );
        assert!(
            SNAPSHOT.contains("const refs = numbered ? window.__computerRefs : [];"),
            "and the first snapshot starts from nothing"
        );
    }

    #[test]
    fn test_a_ref_that_left_is_a_hole_and_not_a_gap() {
        assert!(
            SNAPSHOT.contains("if (refs[i] && !refs[i].isConnected) refs[i] = null;"),
            "the numbers after it hold still, and the page can free the element"
        );
        assert!(
            REF_STATE.contains("if (!el || !el.isConnected) return 'gone';"),
            "and asking for it says so"
        );
    }

    #[test]
    fn test_a_delta_compares_what_the_page_says_and_not_where_it_sits() {
        assert!(
            SNAPSHOT.contains("[one.tag, one.kind, one.role, one.text, one.label, one.value,"),
            "identity is what the element is and says"
        );
        assert!(
            !SNAPSHOT.contains("one.at") && !SNAPSHOT.contains("one.visible"),
            "scrolling is not the page changing"
        );
        assert!(
            SNAPSHOT.contains("if (mode === 'prime' && before) return"),
            "priming never overwrites what a snapshot remembered"
        );
        assert!(
            SNAPSHOT.contains("delta: { first: true, added: [], changed: [], gone: [], same: 0 }"),
            "a first snapshot says it had nothing to compare with"
        );
    }

    #[test]
    fn test_a_press_asks_what_is_under_the_point_first() {
        assert!(
            REACH.contains("document.elementFromPoint(p.x, p.y)"),
            "the point is asked, not the element"
        );
        assert!(
            REACH.contains("if (top === el || el.contains(top)) return { at: p };"),
            "the element or anything inside it is a hit; an ancestor is not"
        );
        assert!(
            REACH.contains("...Array.from(el.getClientRects()).map(centre)"),
            "a link wrapped over two lines is tried line by line"
        );
        assert!(
            REACH.contains("return covered ? { covered } : { off: true };"),
            "covered outranks off-screen, since it names what to deal with"
        );
    }

    #[test]
    fn test_an_icon_over_the_middle_of_a_field_does_not_cover_it() {
        assert!(
            REACH.contains("[[0.25, 0.5], [0.75, 0.5], [0.5, 0.25], [0.5, 0.75]]"),
            "the quarters of the box are tried after its middle"
        );
        assert!(
            REACH.contains("...Array.from(el.getClientRects()).map(centre), ...quarters]"),
            "and after each line, so an inline element is still tried line by line first"
        );
    }

    #[test]
    fn test_typing_goes_in_one_piece() {
        let source = include_str!("cdp.rs");
        let typing = source
            .split("pub async fn type_text(&mut self, text: &str)")
            .nth(1)
            .and_then(|rest| rest.split("async fn connect(").next())
            .expect("type_text is here");
        assert!(
            !typing.contains("text.chars()"),
            "a picker that opens on the first keystroke would take the rest"
        );
        assert!(typing.contains(r#"json!({ "text": text })"#));
    }

    #[test]
    fn test_a_covered_target_waits_briefly_and_then_says_what_covers_it() {
        assert!(
            COVERED < Duration::from_secs(1),
            "a dialog that has not gone in this long is staying"
        );
        assert!(COVERED_POLL * 4 <= COVERED, "several looks, not one");

        let reached: Reached = serde_json::from_str(
            r#"{"element":{"text":"Search","tag":"button","width":107,"height":40,"enabled":true},
                "reach":{"covered":"<div#calendar.fade>"}}"#,
        )
        .expect("parses");
        assert!(
            matches!(reached.reach, Reach::Covered { ref covered } if covered == "<div#calendar.fade>")
        );

        let reached: Reached = serde_json::from_str(
            r#"{"element":{"text":"Search","tag":"button","width":107,"height":40,"enabled":true},
                "reach":{"at":{"x":632,"y":356}}}"#,
        )
        .expect("parses");
        assert!(matches!(reached.reach, Reach::At { at } if at.x == 632 && at.y == 356));

        let reached: Reached = serde_json::from_str(
            r#"{"element":{"text":"Top","tag":"a","width":10,"height":10,"enabled":true},
                "reach":{"off":true}}"#,
        )
        .expect("parses");
        assert!(matches!(reached.reach, Reach::Off { .. }));
    }

    #[test]
    fn test_a_scroll_before_a_press_is_never_smooth() {
        let source = include_str!("cdp.rs");
        let reach = source
            .split("async fn reach(&mut self, query: &str)")
            .nth(1)
            .expect("reach is here");
        assert!(
            reach.contains("behavior: 'instant'"),
            "a smooth scroll moves the element after it has been measured"
        );
    }

    #[test]
    fn test_a_page_between_documents_is_waited_for_and_not_failed() {
        assert!(between_documents(&Error::denied(
            "Runtime.evaluate: {\"code\":-32000,\"message\":\"Cannot find default execution context\"}"
        )));
        assert!(between_documents(&Error::denied(
            "Runtime.evaluate: {\"code\":-32000,\"message\":\"Execution context was destroyed.\"}"
        )));
        assert!(
            !between_documents(&Error::denied("the page threw: ReferenceError")),
            "a page's own error is not a page that is not there"
        );
        assert!(
            CONTEXT_WAIT <= Duration::from_secs(3),
            "a redirect, not a load"
        );
    }

    #[test]
    fn test_a_drag_finishes_chromes_own_drag_with_a_drop() {
        let source = include_str!("cdp.rs");
        let drag = source
            .split("pub async fn drag_on(")
            .nth(1)
            .and_then(|rest| rest.split("pub async fn type_text(").next())
            .expect("drag_on is here");

        assert!(drag.contains(r#""Input.setInterceptDrags", json!({ "enabled": true })"#));
        assert!(
            drag.contains(r#"event.method == "Input.dragIntercepted""#),
            "a drag Chrome started is noticed"
        );
        assert!(
            drag.contains(r#"for kind in ["dragEnter", "dragOver", "drop"]"#),
            "and dropped where the pointer ends, in the order a browser sends them"
        );
        assert!(
            drag.contains(r#"None => self.button_event("mouseReleased", end, button, 1).await?"#),
            "a pointer drag is released as before"
        );
    }

    #[test]
    fn test_only_a_snapshot_hands_numbers_out() {
        assert!(
            SNAPSHOT.contains("if ((mode === 'prime' || mode === 'since') && !numbered) {"),
            "an action on a page nobody has snapshotted does not number it"
        );
        assert!(
            SNAPSHOT.contains("const numbered = Array.isArray(window.__computerRefs);"),
            "and a navigation, which empties the globals, counts as nobody having"
        );
        assert!(
            SNAPSHOT.contains("if (mode !== 'delta' && mode !== 'since') {"),
            "since compares as delta does once the page is numbered"
        );
    }

    #[test]
    fn test_a_ref_is_the_at_and_a_number() {
        assert_eq!(ref_number("@e12"), Some(12));
        assert_eq!(ref_number(" @e3 "), Some(3));
        assert_eq!(ref_number("e12"), None, "without the @ it is words");
        assert_eq!(ref_number("@e"), None);
        assert_eq!(ref_number("@email"), None);
    }

    #[test]
    fn test_a_delta_parses_with_what_left() {
        let taken = r#"{"url":"https://example.com/","title":"Example","total":2,"elements":[],
                        "delta":{"first":false,"added":[{"text":"Confirm","tag":"button","ref":"e14",
                        "width":40,"height":12,"enabled":true}],"changed":[],
                        "gone":[{"text":"Bacon","tag":"input","kind":"checkbox","ref":"e7",
                        "width":10,"height":10,"enabled":true}],"same":11}}"#;
        let taken: Snapshot = serde_json::from_str(taken).expect("it parses");
        let delta = taken.delta.expect("a delta");

        assert!(!delta.first);
        assert_eq!(delta.added[0].r#ref.as_deref(), Some("e14"));
        assert_eq!(
            delta.gone[0].text, "Bacon",
            "as it was, since it is no longer there"
        );
        assert_eq!(delta.same, 11);

        let plain: Snapshot =
            serde_json::from_str(r#"{"url":"u","title":"t","total":0,"elements":[]}"#)
                .expect("a listing without a delta still parses");
        assert!(plain.delta.is_none());
    }

    #[test]
    fn test_a_snapshot_lists_the_headings_between_the_controls() {
        for kind in [
            "a[href]",
            "button",
            "input:not([type=hidden])",
            "select",
            "[role=tab]",
        ] {
            assert!(SNAPSHOT.contains(kind), "{kind} is a control");
        }
        assert!(
            SNAPSHOT.contains("h1, h2, h3, h4, h5, h6, [role=heading]"),
            "a heading is not a control, but it says where the controls are"
        );
    }

    #[test]
    fn test_a_snapshot_carries_the_describer() {
        let script = snapshot_script();

        assert!(
            !script.contains("DESCRIBE_FN"),
            "the placeholder was replaced"
        );
        assert!(
            script.contains("querySelectorAll(q).length === 1"),
            "and the describer brought its selector builder with it"
        );
    }

    #[test]
    fn test_a_snapshot_parses_with_its_count() {
        let taken = r#"{"url":"https://example.com/","title":"Example","total":12,
                        "elements":[{"text":"More","tag":"a","href":"https://example.com/more",
                        "ref":"e1","width":40,"height":12,"enabled":true}]}"#;
        let taken: Snapshot = serde_json::from_str(taken).expect("it parses");

        assert_eq!(taken.total, 12, "the page offered more than was listed");
        assert_eq!(taken.elements[0].r#ref.as_deref(), Some("e1"));
        assert_eq!(
            taken.elements[0].href.as_deref(),
            Some("https://example.com/more")
        );
    }

    #[test]
    fn test_a_label_wrapping_its_field_names_it() {
        assert!(
            DESCRIBE.contains("el.closest('label')"),
            "a form that wraps each field in its label has named it"
        );
        assert!(
            DESCRIBE.contains("copy.querySelectorAll('input, select, textarea, [aria-hidden=true]').forEach(n => n.remove());"),
            "read without the field itself, or a filled value becomes the name"
        );
        assert!(
            DESCRIBE.contains("wrapping() ||\n                undefined"),
            "and it comes after everything the field says about itself"
        );
        assert!(
            DESCRIBE.contains(".replace(/[:*]\\s*$/, '')"),
            "\"Customer name:\" is the label of \"Customer name\""
        );
    }

    #[test]
    fn test_the_name_a_snapshot_gives_a_field_reaches_the_field() {
        let script = named();

        assert!(
            !script.contains("MATCH_FN") && !script.contains("SELECTOR_FN"),
            "both helpers are spliced in, or the page throws on a name nothing defines"
        );
        assert!(
            NAMED.contains("el.tagName === 'LABEL' ? el.control : null"),
            "a form that wraps each field in its label is matched on the label, which holds \
             no value, takes no file and has no options: `control` is the field it names, \
             wrapped or pointed at with `for`"
        );
        assert!(
            NAMED.contains("if (fits(el)) return null;"),
            "a query that reaches a field by itself is left as it was asked"
        );
        assert!(
            NAMED.contains("kind === 'select'") && NAMED.contains("kind === 'file'"),
            "a dropdown and a file input are looked for past a label that names something else"
        );
    }

    #[test]
    fn test_a_refused_fill_names_the_field_it_could_have_meant() {
        let script = nearby();

        assert!(
            !script.contains("MATCH_FN") && !script.contains("SELECTOR_FN"),
            "both helpers are spliced in, or the refusal is the bare one"
        );
        assert!(
            NEARBY.contains("at >= 0 ? '@e' + (at + 1) : (SELECTOR_FN)(field)"),
            "the number a snapshot gave is the name the caller already has for the field; \
             without a snapshot the selector is one that works as a query"
        );
        assert!(
            NEARBY.contains("where = 'beside';") && NEARBY.contains("fields(el.parentElement)"),
            "\"Company\" in a div beside its field matches the div: the field is named, not \
             filled, because words and a field that only share a parent are a guess"
        );
        assert!(
            NEARBY.contains("if (crowd) found = [];"),
            "a match whose parent is the whole form would list the form"
        );
    }

    #[test]
    fn test_a_file_input_nobody_can_see_still_takes_a_file() {
        let script = file_input();

        assert!(!script.contains("MATCH_FN"), "the matcher is spliced in");
        assert!(
            FILE_INPUT.contains("document.querySelector(query)")
                && FILE_INPUT.contains("catch (_)"),
            "a page hides its file input behind a button it styles, and words are not a \
             selector, so a query that is not one finds nothing rather than throwing"
        );
        assert!(
            NEARBY.contains("kind === 'file' || field.getClientRects().length > 0"),
            "a refusal names only what the next call can reach: any file input, and the \
             other controls only where they are drawn"
        );
    }

    #[test]
    fn test_a_refused_dropdown_or_upload_names_only_controls_of_its_kind() {
        assert!(
            NEARBY.contains("kind === 'select' ? 'select'")
                && NEARBY.contains("kind === 'file' ? 'input[type=file]'"),
            "a text field beside the words is no dropdown, and naming it would send the \
             caller to a second refusal"
        );
        assert!(
            PICK.contains(&format!("return {{ no: '{NO_DROPDOWN}' }};")),
            "only the refusal that found no dropdown looks for one nearby; a disabled \
             dropdown was found"
        );
    }

    #[test]
    fn test_a_letter_is_typed_as_the_key_a_keyboard_has_for_it() {
        let small = Keystroke::of('a').expect("a key");
        assert_eq!(
            (small.code.as_str(), small.virtual_key, small.shifted),
            ("KeyA", 65, false)
        );

        let capital = Keystroke::of('A').expect("a key");
        assert_eq!(
            (capital.code.as_str(), capital.virtual_key, capital.shifted),
            ("KeyA", 65, true)
        );

        let colon = Keystroke::of(':').expect("a key");
        assert_eq!(
            (colon.code.as_str(), colon.virtual_key, colon.shifted),
            ("Semicolon", 186, true)
        );

        let slash = Keystroke::of('/').expect("a key");
        assert_eq!((slash.code.as_str(), slash.shifted), ("Slash", false));
        assert_eq!(Keystroke::of(' ').expect("a key").code, "Space");

        for typed in (0x20u8..0x7F).map(char::from) {
            assert!(
                Keystroke::of(typed).is_some(),
                "{typed:?} is on a US keyboard"
            );
        }
    }

    #[test]
    fn test_the_last_key_is_pressed_again_with_no_text_in_it() {
        let source = include_str!("cdp.rs");
        let typing = source
            .split("pub async fn type_text(&mut self, text: &str)")
            .nth(1)
            .and_then(|rest| rest.split("struct Keystroke").next())
            .expect("type_text is here");
        let inserted = typing.find("Input.insertText").expect("the text goes in");
        let pressed = typing
            .find(r#"["rawKeyDown", "keyUp"]"#)
            .expect("then one key");

        assert!(
            inserted < pressed,
            "inserted text fires no key event, and a date picker that reads its field on \
             keyup never read this one: it put its own empty date back when the field lost \
             the focus. One key after the text is in, and it reads all of it"
        );

        let stroke = Keystroke::of('A').expect("a key");
        let down = stroke.event("rawKeyDown");
        assert!(
            down.get("text").is_none() && stroke.event("keyUp").get("text").is_none(),
            "text on either would type the letter a second time"
        );
        assert_eq!(down["key"], "A");
        assert_eq!(down["code"], "KeyA");
        assert_eq!(
            down["modifiers"], 8,
            "a capital arrives with shift, as from a keyboard"
        );
    }

    #[test]
    fn test_text_that_ends_on_no_key_is_followed_by_none() {
        for typed in ['é', '日', '🙂', '\n', '\t'] {
            assert!(
                Keystroke::of(typed).is_none(),
                "{typed:?}: a new line as a key would send a one-line form, a tab would leave \
                 the field, and the rest are on no key here"
            );
        }
    }

    #[test]
    fn test_a_required_field_says_so() {
        assert!(
            DESCRIBE.contains(
                "if (el.required === true || aria('required') === 'true') states.push('required');"
            ),
            "a form's own rule is a state, the same as disabled or checked"
        );
    }

    #[test]
    fn test_an_element_out_of_view_still_parses() {
        let above = r#"[{"text":"Top","tag":"a","at":null,"width":10,"height":10,"enabled":true}]"#;
        let found: Vec<Element> = serde_json::from_str(above).expect("it parses");

        assert_eq!(found.len(), 1);
        assert!(
            found[0].at.is_none(),
            "and says it has nowhere to be pressed"
        );
        assert_eq!(found[0].text, "Top", "while still being worth naming");
    }

    #[test]
    fn test_an_element_in_view_keeps_its_point() {
        let here = r#"[{"text":"Buy","tag":"button","at":{"x":40,"y":80},
                        "width":10,"height":10,"enabled":true}]"#;
        let found: Vec<Element> = serde_json::from_str(here).expect("it parses");

        assert_eq!(found[0].at, Some(Point::new(40, 80)));
    }

    #[test]
    fn test_a_coordinate_is_only_reported_from_inside_the_window() {
        assert!(
            DESCRIBE.contains("x < innerWidth") && DESCRIBE.contains("y < innerHeight"),
            "a coordinate is measured against the window it was taken in"
        );
        assert!(DESCRIBE.contains("x >= 0 && y >= 0"));
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
