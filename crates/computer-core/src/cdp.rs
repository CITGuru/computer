//! Driving the browser through the DevTools protocol.
//!
//! [`Devtools`] manages default-context targets and isolated [`BrowserGroup`]s.
//! [`Page`] drives one target without display coordinates.
//!
//! The WebSocket client is written here rather than taken as a dependency,
//! for one connection to loopback, and negotiates no compression extension.
//! Anything this does not wrap is reachable through [`Page::call`].

use crate::error::{Error, Result};
use crate::{BrowserEndpoint, Button, Point};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
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
            ws_path: websocket_path(ws)?,
        })
    }

    /// Whether this is a page rather than a worker or an extension.
    pub fn is_page(&self) -> bool {
        self.kind == "page"
    }
}

fn websocket_path(url: &str) -> Option<String> {
    let with_authority = url.split_once("://")?.1;
    let (_, path) = with_authority.split_once('/')?;
    Some(format!("/{path}"))
}

/// The browser's DevTools endpoint, as reached from this machine.
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

    /// Take the logged-in state for these origins out of the browser.
    ///
    /// An origin is `https://example.com` — scheme and host, no path. Only
    /// what belongs to one of them comes out, so a session for one site never
    /// carries another site's cookies along with it.
    ///
    /// [`Carry`] says what to take. What could not be taken is named in
    /// [`Session::incomplete`] rather than left to be discovered by a box that
    /// comes up signed out.
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

        // Storage is per origin and only reachable from a page on it, so each
        // one is visited. A site that will not load is said so rather than
        // silently contributing nothing.
        for origin in origins {
            // A tab already on this origin if there is one, because session
            // storage belongs to a tab: a fresh one carries none of what the
            // first one holds.
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

            // Only what this opened: a tab the caller was using is left alone.
            if ours {
                page.close().await.ok();
            }
        }

        Ok(session)
    }

    /// Put one back.
    ///
    /// Ignores anything the session does not claim: a cookie whose domain no
    /// listed origin covers, and storage for an origin that is not in it.
    /// Without that filter a vault entry could hand one site's cookies to
    /// another.
    /// Hands back the tabs it left open. Session storage belongs to a tab, so
    /// one restored into a tab nobody keeps is one nobody has: a caller that
    /// asked for it should go on working in these.
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
                    // Left open where session storage went into it: closing
                    // the tab would throw away what was just put there.
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
                // Either the caller's own tab or one holding session storage,
                // and both are theirs to use.
                false => held.push(page),
            }
        }

        Ok(held)
    }

    /// A tab already showing this origin, if one is.
    async fn page_on(&self, origin: &str) -> Option<Page> {
        for target in self.pages().await.ok()? {
            if target.url.starts_with(origin) {
                return self.attach(&target).await.ok();
            }
        }

        None
    }

    /// Put a query to a search engine, and hand back the results page.
    ///
    /// The URL is built rather than written by a caller: a query with `&` or
    /// `#` in it, pasted into a template, searches for something other than
    /// what was asked for.
    pub async fn search(
        &self,
        query: &str,
        provider: SearchProvider,
        within: Duration,
    ) -> Result<Page> {
        self.open_page(&provider.url_for(query), within).await
    }

    /// Close pages beyond the newest `keep`, and answer how many went.
    ///
    /// `open_url` raises a new tab every time, by design — a person opening a
    /// link expects one. A program doing it fifty times leaves fifty behind,
    /// and a browser holding them all gets slower at everything.
    ///
    /// Never the visible one, whatever its age: it is the page the screen is
    /// showing and a caller is probably reading it.
    pub async fn tidy(&self, keep: usize) -> Result<usize> {
        let pages = self.pages().await?;
        if pages.len() <= keep {
            return Ok(0);
        }

        // By id, not by URL: a run comparing two pages of one site has several
        // tabs holding the same address, and a caller holding an id for one of
        // them would watch it go.
        let showing = match self.visible_page().await {
            Ok(Some(page)) => Some(page.target().id.clone()),
            _ => None,
        };

        let mut closed = 0;
        // Newest first, as the debugger lists them, so the tail is what has
        // been sitting there longest.
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

    /// Attach to the first page, opening one if the browser has none.
    pub async fn first_page(&self) -> Result<Page> {
        let target = match self.pages().await?.into_iter().next() {
            Some(target) => target,
            None => self.open("about:blank").await?,
        };
        self.attach(&target).await
    }

    /// Create an isolated cookie and storage context.
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

    /// List non-default browser contexts.
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

/// An isolated cookie and storage context inside screen 0's Chromium.
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

    /// Dispose the context and its pages. Repeated handles close idempotently.
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

/// A message the browser sent that nobody asked for.
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

/// What a session should carry.
///
/// Cookies and local storage by default, because that is where a login
/// normally is. The other two are asked for: one because the site said it
/// should not outlive a tab, the other because it is expensive and imperfect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Carry {
    pub cookies: bool,
    pub local_storage: bool,
    /// A site putting a token here has chosen that the login dies with the
    /// tab. Carrying it overrides that, so it is named rather than assumed —
    /// and it also holds the throwaway state of a redirect part-way through,
    /// which is worth nothing once restored.
    pub session_storage: bool,
    /// Where Firebase keeps a login, so without it those sites come back
    /// signed out. Only what survives being written as JSON: a value holding
    /// a blob or a key that cannot be cloned is left, and named in
    /// [`Session::incomplete`].
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
    /// Nothing, to add to.
    pub fn none() -> Self {
        Self {
            cookies: false,
            local_storage: false,
            session_storage: false,
            indexed_db: false,
        }
    }

    /// Everything this can take. Not everything a browser holds — see
    /// [`Session::incomplete`].
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

/// A logged-in state, taken out of one box so another can be given it.
///
/// Cookies and local storage, which is where a session actually lives. Not the
/// profile directory: that is a third of a gigabyte, tied to the Chromium
/// build that wrote it, and holds the whole of a browser's history besides.
///
/// **This is the account.** A password may sit behind a second factor; a
/// session has already passed one, so whoever holds this file is the user.
/// It carries no `Display` and no `Debug` that would put it in a log.
#[derive(Clone, Serialize, Deserialize)]
pub struct Session {
    /// The origins this was taken from, and the only ones it may be put back
    /// into. A session for one site restored into another is an account handed
    /// to a stranger.
    pub origins: Vec<String>,
    pub cookies: Vec<Cookie>,
    /// Local storage, per origin. Where most applications keep their token.
    pub storage: BTreeMap<String, BTreeMap<String, String>>,
    /// Session storage, per origin, where it was asked for.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub session_storage: BTreeMap<String, BTreeMap<String, String>>,
    /// Databases, per origin, where they were asked for.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub databases: BTreeMap<String, Vec<Database>>,
    /// What was asked for, so an import puts back the same kinds.
    #[serde(default)]
    pub carried: Carry,
    /// What could not be taken, and why.
    ///
    /// A session that is quietly short of what a site needs is worse than one
    /// that fails: the box comes up looking signed in and is not. Anything
    /// left behind says so here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub incomplete: Vec<String>,
}

/// One database, as much of it as JSON can hold.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Database {
    pub name: String,
    pub version: u64,
    pub stores: Vec<BrowserStore>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserStore {
    pub name: String,
    /// The path a record's key is read from, where the store has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_path: Option<Value>,
    #[serde(default)]
    pub auto_increment: bool,
    /// Records as `[key, value]`. The key is carried separately because a
    /// store without a key path does not keep one inside its value.
    pub records: Vec<Value>,
}

impl std::fmt::Debug for Session {
    /// Counts, never contents.
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

/// One cookie, as the protocol gives and takes it.
///
/// Every flag round-trips: a `Secure` cookie restored without its flag is sent
/// over plain HTTP, and a `SameSite` one restored without its own is sent
/// where the site said not to.
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
    /// Its plain endpoint, which renders results as ordinary HTML.
    ///
    /// The default because it is the one a reader gets anything from. It also
    /// refuses a shell that asks too often — a browser it serves, `curl` it
    /// answers with a puzzle about ducks.
    #[default]
    DuckDuckGo,
    /// Answers, but as an application rather than a document: results arrive
    /// after script runs, behind a consent page in much of the world.
    Google,
    /// Answers, and sometimes to a different question. A quoted name here
    /// returned land records for an Indian state.
    Bing,
}

impl SearchProvider {
    /// Where to send this query.
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

/// Percent-encode one query.
///
/// By hand because the crate carries no URL library, and wrongly by hand is
/// how a search for a quoted phrase becomes a search for something else: `&`
/// starts another parameter, `#` ends the URL, and a space is not a `+`
/// everywhere it is written as one.
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

/// One attached target.
pub struct Page {
    connection: Connection,
    target: Target,
}

/// What a read carries when the caller names no numbers.
///
/// Defaults, not limits: a library imposes no ceiling on its caller, and a
/// deployment that needs one puts it in front — see `computer-server`.
const TEXT_DEFAULT: usize = 100_000;
const LINKS_DEFAULT: usize = 100;

/// How often a wait asks whether the page has caught up.
const POLL: Duration = Duration::from_millis(120);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scroll {
    /// This far from where it is now. Positive `y` moves down the page.
    By {
        x: i32,
        y: i32,
    },
    /// To this position, counted from the top.
    To {
        x: i32,
        y: i32,
    },
    /// As far down as it goes. What a page that loads more on arrival wants.
    Bottom,
    Top,
}

impl Scroll {
    fn as_js(self) -> String {
        match self {
            Self::By { x, y } => format!("{{ x: null, dx: {x}, dy: {y} }}"),
            Self::To { x, y } => format!("{{ x: {x}, y: {y} }}"),
            // Larger than any document, since the browser clamps it and
            // `scrollHeight` on the wrong element is a different number.
            Self::Bottom => "{ x: 0, y: 1e9 }".to_string(),
            Self::Top => "{ x: 0, y: 0 }".to_string(),
        }
    }
}

/// How many matches a find carries when the caller names no number.
const FOUND_DEFAULT: usize = 20;

/// What the protocol calls a mouse button, and the bit a page reads out of
/// `event.buttons` while that button is down.
fn button_parts(button: Button) -> (&'static str, u8) {
    match button {
        Button::Left => ("left", 1),
        Button::Right => ("right", 2),
        Button::Middle => ("middle", 4),
    }
}

/// One thing on a page a caller can act on.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Element {
    /// What it says, which is usually what a caller was looking for.
    pub text: String,
    /// Its tag, lowercased: `button`, `input`, `select`, `a`.
    pub tag: String,
    /// An `input`'s type, where it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// The shortest selector that names this element and nothing else.
    ///
    /// Takes the place of the words a caller found it by: every method here
    /// accepts a selector as its query, and one of these is unambiguous where
    /// words are not. `None` where the page offers nothing that identifies it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
    /// What the page calls it, where that is not what it says: a field with no
    /// words of its own still has a name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Whether its middle is inside the window.
    #[serde(default)]
    pub visible: bool,
    /// The middle of it, in the viewport's coordinates, which is what
    /// [`Page::click`] takes and is not where it sits on the screen.
    ///
    /// `None` where the middle is outside the viewport. A coordinate for
    /// something scrolled out of view points somewhere else, and an unsigned
    /// one cannot even be written down: an element above the fold has a
    /// negative offset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Point>,
    pub width: u32,
    pub height: u32,
    /// A disabled control is worth knowing about before it is clicked and
    /// nothing happens.
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

/// Elements matching a query, best first.
///
/// A CSS selector where the query is one, a `name`, `id` or placeholder where
/// it names a field, and visible words otherwise: an agent knows a button says
/// "Sign in" and rarely knows its class.
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

  try { document.querySelectorAll(q).forEach(add); } catch (e) {}

  // A field is usually named rather than worded: `custname` is what a form
  // calls it, and a bare word is not a selector that finds it.
  for (const by of ['[name=', '[id=', '[placeholder=', '[aria-label=']) {
    try { document.querySelectorAll(by + JSON.stringify(q) + ']').forEach(add); } catch (e) {}
  }

  const want = q.trim().toLowerCase();
  const words = el => (el.innerText || el.value || el.getAttribute('aria-label') ||
                       el.getAttribute('placeholder') || el.getAttribute('title') || '')
                        .replace(/\s+/g, ' ').trim().toLowerCase();
  const clickable = 'a,button,input,select,textarea,[role=button],[role=link],[onclick]';

  const passes = exact
    ? [el => words(el) === want]
    : [el => words(el) === want, el => words(el).includes(want)];

  for (const pass of passes) {
    for (const el of document.querySelectorAll(clickable)) if (pass(el)) add(el);

    // The innermost of them only. An ancestor's innerText holds all of its
    // descendants', so a substring match walks out to <body> — which comes
    // first in document order and would be the one answer a wait acts on.
    // Clickables are swept above and already held, so a button whose label
    // sits in a span still beats the span.
    const hits = [];
    for (const el of document.querySelectorAll('*')) if (pass(el)) hits.push(el);
    for (const el of hits) {
      if (hits.some(other => other !== el && el.contains(other))) continue;
      add(el);
    }
  }

  // Not sorted by position: the passes above are the ranking, and a form
  // containing a button sits higher on the page than the button does. Sorting
  // by top would hand back the form.
  return out;
}"#;

/// The shortest selector that names one element and nothing else.
///
/// Confirmed rather than generated: a selector matching two elements looks
/// precise and is worse than the words it replaces, because an action on it
/// silently reaches the wrong one.
const SELECTOR: &str = r#"(el) => {
  const alone = q => {
    try { return document.querySelectorAll(q).length === 1; } catch (e) { return false; }
  };

  if (el.id && alone('#' + CSS.escape(el.id))) return '#' + CSS.escape(el.id);

  // What a page meant as a handle, before anything this has to invent.
  for (const by of ['data-testid', 'data-test', 'data-qa', 'name', 'aria-label',
                    'placeholder', 'title']) {
    const value = el.getAttribute(by);
    if (!value) continue;
    const q = '[' + by + '=' + JSON.stringify(value) + ']';
    if (alone(q)) return q;
  }

  // Outwards until it is unambiguous, so the answer is as short as the page
  // allows rather than a path from the root every time.
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

/// [`DESCRIBE`] with [`SELECTOR`] put in place of its call.
///
/// One calls the other, and a script evaluated in a page carries no imports.
fn describe() -> String {
    // The name alone: the parens around it in DESCRIBE are what let the
    // function it becomes be called, and replacing them too leaves an arrow
    // literal invoked where it stands, which does not parse.
    DESCRIBE.replace("SELECTOR_FN", SELECTOR)
}

/// One element, as a caller sees it.
const DESCRIBE: &str = r#"(el) => {
  const r = el.getBoundingClientRect();
  const x = Math.round(r.left + r.width / 2), y = Math.round(r.top + r.height / 2);
  // A coordinate is the viewport's, so one for an element off it points
  // somewhere else — and an unsigned one cannot even hold a negative offset.
  const inside = x >= 0 && y >= 0 && x < innerWidth && y < innerHeight;

  // What the page calls it, which is not always what it says: a field with no
  // words of its own still has a name a caller can recognise.
  const label = el.getAttribute('aria-label') || el.getAttribute('placeholder') ||
                el.getAttribute('title') ||
                (el.id && (document.querySelector('label[for=' + JSON.stringify(el.id) + ']') || {}).innerText) ||
                undefined;

  return {
    text: (el.innerText || el.value || el.getAttribute('aria-label') || '')
            .replace(/\s+/g, ' ').trim().slice(0, 200),
    tag: el.tagName.toLowerCase(),
    kind: el.getAttribute('type') || undefined,
    selector: (SELECTOR_FN)(el),
    label: label === undefined ? undefined : String(label).replace(/\s+/g, ' ').trim().slice(0, 200),
    at: inside ? { x, y } : undefined,
    visible: inside,
    width: Math.round(r.width),
    height: Math.round(r.height),
    enabled: !el.disabled,
    value: el.value === undefined ? undefined : String(el.value).slice(0, 200),
  };
}"#;

/// How a page should be read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reading {
    /// Headings, lists, tables and inline links kept. The default, because a
    /// reader that flattens them cannot tell a heading from body text.
    #[default]
    Markdown,
    /// Rendered text, whitespace collapsed. The cheapest answer to "what does
    /// this say".
    Text,
    /// The document's own HTML. An escape hatch for what the readers above do
    /// not carry, and megabytes where they are kilobytes.
    Raw,
}

/// Rendered text, scripts and styles removed.
///
/// Navigation stays: telling a site's chrome from its content is a heuristic,
/// and a wrong one silently drops the page.
const TEXT: &str = r#"
  // The live tree, not a copy of it: `innerText` is what the page renders, and
  // a detached clone has no layout — it falls back to every character in the
  // document, including whatever is hidden. Scripts and styles are not
  // rendered either, so nothing has to be stripped.
  const root = document.querySelector('main, article') || document.body;
  return (root ? root.innerText : '').replace(/\s+/g, ' ').trim();
"#;

/// The document as markdown.
///
/// A walk rather than a library: nothing may be fetched into the box to read a
/// page, and the shapes worth keeping — headings, lists, tables, code, links —
/// are few enough to name.
const MARKDOWN: &str = r#"
  const skip = new Set(['SCRIPT','STYLE','NOSCRIPT','SVG','TEMPLATE','IFRAME','CANVAS']);
  const inline = t => t.replace(/\s+/g, ' ');
  const out = [];

  const walk = (node, depth) => {
    if (node.nodeType === 3) { out.push(inline(node.nodeValue)); return; }
    if (node.nodeType !== 1 || skip.has(node.tagName)) return;
    if (node.getAttribute && node.getAttribute('aria-hidden') === 'true') return;

    // What the page is not showing is not what it says: a wizard keeps its
    // later steps in the document, and a reader that returns them describes a
    // page nobody is looking at.
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
        // Written after its content, so an item that renders to nothing
        // leaves no bullet behind.
        const mark = out.length;
        out.push('');
        Array.from(node.childNodes).forEach(c => walk(c, depth + 1));
        const body = out.slice(mark + 1).join('').trim();
        out[mark] = body ? '\n' + '  '.repeat(depth) + '- ' : '';
        return;
      }
      case 'A': {
        const href = node.getAttribute('href') || '';
        // `innerText` is empty for anything not rendered — a collapsed menu,
        // a hidden tab — and those links would come out as bare bullets.
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

  // `main` and `article` are not guesses: HTML defines them as the document's
  // content, and a page that marks one has already said which part matters.
  // Wikipedia otherwise spends a reader's whole budget on its language
  // sidebar before the article begins. Nothing is dropped where neither
  // exists — telling chrome from content without them is a real heuristic,
  // and a wrong one loses the page.
  const root = document.querySelector('main, article') || document.body;
  if (root) walk(root, 0);
  return out.join('')
    .replace(/[ \t]+/g, ' ')
    .replace(/ ?\n ?/g, '\n')
    .replace(/\n{3,}/g, '\n\n')
    .trim();
"#;

/// What a page is showing.
///
/// Text rather than a picture of text, and links with the addresses behind
/// them: a frame says where to click, and this says what it says.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageText {
    pub url: String,
    pub title: String,
    /// Rendered text, whitespace collapsed and cut at the caller's limit.
    pub text: String,
    /// Whether the cut lost anything.
    pub truncated: bool,
    pub links: Vec<Link>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Link {
    pub text: String,
    pub href: String,
}

impl Page {
    pub fn target(&self) -> &Target {
        &self.target
    }

    /// Any method in the protocol, and whatever it answers.
    ///
    /// Events arriving meanwhile are queued rather than returned.
    pub async fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        self.connection.call(method, params).await
    }

    /// How many events were dropped because the queue was full.
    ///
    /// The oldest go first, so a caller can tell nothing happened from not
    /// having listened.
    pub fn dropped_events(&self) -> usize {
        self.connection.dropped
    }

    /// Everything the browser reported that nobody asked for, oldest first.
    pub fn take_events(&mut self) -> Vec<Event> {
        self.connection.dropped = 0;
        self.connection.events.drain(..).collect()
    }

    /// Wait for one event, keeping everything else that arrives meanwhile.
    pub async fn next_event(&mut self, method: &str, within: Duration) -> Result<Event> {
        self.connection.next_event(method, within).await
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

    /// What this page is showing.
    ///
    /// Read in the page rather than over the wire: a document is megabytes of
    /// markup, and what a reader wants is what it renders.
    ///
    /// `limit` and `max_links` default when they are `None`. A caller deciding
    /// whether a page is worth reading asks for a few hundred characters
    /// rather than paying for all of it; one that wants the whole thing says
    /// so.
    pub async fn read(
        &mut self,
        format: Reading,
        limit: Option<usize>,
        max_links: Option<usize>,
    ) -> Result<PageText> {
        let limit = limit.unwrap_or(TEXT_DEFAULT);
        let links = max_links.unwrap_or(LINKS_DEFAULT);

        let body = match format {
            // Structure survives: a heading stays a heading, a list stays a
            // list, and a link keeps its address beside its words rather than
            // in a separate list nothing can place back in context.
            Reading::Markdown => MARKDOWN,
            Reading::Text => TEXT,
            // The document as it came. An escape hatch for what the reader
            // above did not carry — a `meta` tag, embedded JSON-LD — and
            // costly enough that it is nobody's default.
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

    /// Close it.
    ///
    /// Whoever opened a page closes it. Nothing does so on drop, because a
    /// close is a round trip and a drop cannot wait for one — so a program
    /// that opens a page per step and never says this leaves the browser
    /// holding every one of them.
    pub async fn close(&mut self) -> Result<()> {
        self.call("Page.close", json!({})).await.map(|_| ())
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
    ///
    /// A right click opens the page's own context menu, not the browser's: a
    /// menu the window manager draws is outside the page and no screenshot of
    /// the viewport holds it.
    pub async fn click(&mut self, at: Point, button: Button) -> Result<()> {
        let (name, mask) = button_parts(button);

        // The protocol does not derive `buttons` from `button`, and a page
        // that reads `event.buttons` to tell which one is down sees none.
        for (kind, buttons) in [("mousePressed", mask), ("mouseReleased", 0)] {
            self.call(
                "Input.dispatchMouseEvent",
                json!({
                    "type": kind,
                    "x": at.x,
                    "y": at.y,
                    "button": name,
                    "buttons": buttons,
                    "clickCount": 1,
                }),
            )
            .await?;
        }
        Ok(())
    }

    /// Elements matching `query`, best match first.
    ///
    /// Exact words beat a substring, and something clickable beats a `div`
    /// that happens to contain them.
    ///
    /// `scroll` brings the best match into view before anything is measured.
    /// Without it a match below the fold is described where it sits in a
    /// document the window is not showing, and its coordinates address
    /// nothing.
    ///
    /// A CSS selector where the query is one, and visible text otherwise: an
    /// agent knows a button says "Sign in" and rarely knows its class.
    /// `exact` matches the whole of an element's words rather than any part of
    /// them, for a caller that knows the label it is looking for.
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
                     // Scrolled before it is measured, not after: a coordinate
                     // is the viewport's, so one taken for an element below
                     // the fold points past the bottom of the window and a
                     // click on it lands nowhere.
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

    /// Move the page, or one scrollable thing on it.
    ///
    /// The desktop's own scroll sends wheel clicks at a screen point, so it
    /// needs a coordinate and moves whatever sits under the pointer. This
    /// moves the page itself, in pixels.
    ///
    /// Answers with where it ended up, which is how a caller tells that it
    /// arrived: a page that loads more as you reach the bottom returns the
    /// same position twice when there is no more to load.
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

    /// Wait until something matching `query` is on the page.
    ///
    /// The alternative is a sleep, which is either short enough to act too
    /// early or long enough to be paid on every step. A page that answers a
    /// click by fetching says nothing when it starts and everything when the
    /// result appears.
    ///
    /// `gone` waits for the opposite — a spinner leaving, a dialog closing.
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

    /// [`Self::wait_for`], stopping for any of `or` as well.
    ///
    /// Answers with which query matched, so a caller waiting for a price or
    /// for "sold out" knows which arrived. Waiting out a full timeout for
    /// something a failure has already ruled out is how the time goes.
    pub async fn wait_for_any(
        &mut self,
        query: &str,
        or: &[String],
        gone: bool,
        within: Duration,
        exact: bool,
    ) -> Result<(String, Option<Element>)> {
        let deadline = Instant::now() + within;

        loop {
            let found = self.find(query, Some(1), None, Some(exact)).await?;
            let here = found.first().cloned();

            match (gone, &here) {
                (false, Some(_)) => return Ok((query.to_string(), here)),
                (true, None) => return Ok((query.to_string(), None)),
                _ => {}
            }

            // Only ever an arrival: a caller waiting for one thing to go and
            // another to appear is waiting for two different shapes, and would
            // not be able to tell which answer it had.
            for other in or {
                if let Some(one) = self
                    .find(other, Some(1), None, Some(exact))
                    .await?
                    .first()
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

                return Err(Error::Timeout {
                    after: within,
                    detail: match gone {
                        true => format!("{waited} was still on the page"),
                        false => format!("nothing matching {waited} appeared"),
                    },
                });
            }

            tokio::time::sleep(POLL).await;
        }
    }

    /// Back through this page's own history.
    ///
    /// Not the same as opening the previous URL again: that discards whatever
    /// the page had put in it, and a form half filled in comes back empty.
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

    /// Put the pointer over something without pressing anything.
    ///
    /// A menu that opens on hover has no click to send: the thing worth
    /// clicking does not exist until the pointer arrives.
    pub async fn hover(&mut self, query: &str) -> Result<Element> {
        let (element, at) = self.reachable(query).await?;

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

    /// Bring one into view and click its middle.
    ///
    /// Its own coordinates rather than a caller's: a point worked out from a
    /// screenshot is stale the moment the page moves under it, and a click
    /// against a stale point lands on whatever took that place.
    pub async fn click_on(&mut self, query: &str, button: Button) -> Result<Element> {
        let (element, at) = self.reachable(query).await?;
        self.click(at, button).await?;
        Ok(element)
    }

    /// Put `text` in a field, as typing rather than as an assignment.
    ///
    /// A page watching for keystrokes — a search box filtering as you type, a
    /// form validating a field — sees nothing when a value is only assigned.
    pub async fn fill(&mut self, query: &str, text: &str) -> Result<()> {
        let (_, at) = self.reachable(query).await?;
        self.click(at, Button::Left).await?;

        self.evaluate(&format!(
            "(({MATCH})({}, false).find(Boolean) || {{}}).value = ''",
            json!(query)
        ))
        .await?;

        self.type_text(text).await
    }

    /// What a dropdown offers.
    pub async fn options(&mut self, query: &str) -> Result<Vec<String>> {
        let listed = self
            .evaluate(&format!(
                "JSON.stringify(Array.from((({MATCH})({}, false).find(e => e.tagName === 'SELECT') || {{ options: [] }}).options).map(o => o.text.trim()))",
                json!(query)
            ))
            .await?;

        serde_json::from_str(listed.as_str().unwrap_or("[]"))
            .map_err(|error| Error::denied(format!("the page would not parse: {error}")))
    }

    /// Choose one of them, by its visible words.
    ///
    /// Through the element rather than through the screen: a native dropdown
    /// opens a menu the compositor owns, which a screenshot does not show and
    /// a click cannot reach.
    pub async fn choose(&mut self, query: &str, option: &str) -> Result<()> {
        let chose = self
            .evaluate(&format!(
                r#"(() => {{
                     const el = ({MATCH})({}, false).find(e => e.tagName === 'SELECT');
                     if (!el) return 'no dropdown matched';
                     const want = {}.trim().toLowerCase();
                     const at = Array.from(el.options)
                       .findIndex(o => o.text.trim().toLowerCase() === want ||
                                       String(o.value).toLowerCase() === want);
                     if (at < 0) return 'no such option';
                     el.selectedIndex = at;
                     el.dispatchEvent(new Event('input', {{ bubbles: true }}));
                     el.dispatchEvent(new Event('change', {{ bubbles: true }}));
                     return 'ok';
                   }})()"#,
                json!(query),
                json!(option)
            ))
            .await?;

        match chose.as_str() {
            Some("ok") => Ok(()),
            other => Err(Error::denied(format!(
                "{}: {query} / {option}",
                other.unwrap_or("the page answered nothing")
            ))),
        }
    }

    /// Hand files to a file input.
    ///
    /// The paths are the box's own. A file chooser is the operating system's
    /// window, not the page's, so nothing on screen can be clicked to fill one
    /// in — this sets the input directly.
    pub async fn upload(&mut self, query: &str, paths: &[String]) -> Result<()> {
        let handle = self
            .call(
                "Runtime.evaluate",
                json!({
                    "expression": format!(
                        "({MATCH})({}, false).find(e => e.tagName === 'INPUT' && e.type === 'file')",
                        json!(query)
                    ),
                    "returnByValue": false,
                }),
            )
            .await?;

        let object = handle
            .get("result")
            .and_then(|result| result.get("objectId"))
            .and_then(Value::as_str)
            .ok_or_else(|| Error::denied(format!("no file input matched {query}")))?;

        self.call(
            "DOM.setFileInputFiles",
            json!({ "files": paths, "objectId": object }),
        )
        .await
        .map(|_| ())
    }

    /// The first match, brought into view.
    /// [`Self::reach`], and where to press it.
    ///
    /// Scrolling is what usually puts a coordinate there, and a thing it
    /// cannot reach — fixed off-screen, or collapsed to nothing — has none.
    async fn reachable(&mut self, query: &str) -> Result<(Element, Point)> {
        let element = self.reach(query).await?;

        match element.at {
            Some(at) => Ok((element, at)),
            None => Err(Error::denied(format!(
                "{query} could not be brought into view, so there is nowhere to press"
            ))),
        }
    }

    async fn reach(&mut self, query: &str) -> Result<Element> {
        let found = self
            .evaluate(&format!(
                r#"(() => {{
                     const el = ({MATCH})({}, false).find(Boolean);
                     if (!el) return 'null';
                     el.scrollIntoView({{ block: 'center', inline: 'center' }});
                     return JSON.stringify(({describe})(el));
                   }})()"#,
                json!(query),
                describe = describe()
            ))
            .await?;

        let found = found.as_str().unwrap_or("null");
        if found == "null" {
            return Err(Error::denied(format!(
                "nothing on the page matched {query}"
            )));
        }

        serde_json::from_str(found)
            .map_err(|error| Error::denied(format!("the page would not parse: {error}")))
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

/// What a page answers when its databases are read.
#[derive(Deserialize)]
struct IdbRead {
    databases: Vec<Database>,
    /// What the page could not hand over, in its own words.
    #[serde(default)]
    skipped: Vec<String>,
}

/// Everything in one kind of web storage, as JSON.
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

/// Read every database an origin holds.
///
/// Through the page rather than the protocol: the debugger can read a database
/// but has no way to write one, so a restore has to run here anyway — and a
/// reader that took a different route would carry what the writer cannot put
/// back.
///
/// Only what JSON holds. A value carrying a blob, a stream or a key that
/// cannot be cloned is left behind and named, because a login that comes back
/// short is worse when nothing says so.
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
        // Something else holding it open at an older version blocks this.
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
            // The round trip a restore will make, made here: a value that
            // cannot survive it is found now rather than lost silently.
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

/// Put them back, creating the stores an upgrade needs.
const IDB_IMPORT: &str = r#"(async (databases) => {
  for (const wanted of databases) {
    const db = await new Promise((ok, no) => {
      const r = indexedDB.open(wanted.name, wanted.version);
      // Stores can only be made here, which is why the version comes with the
      // session: opening at a lower one would never reach this.
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
        // A store with a key path keeps the key inside the value, and putting
        // one alongside is an error rather than an override.
        os.keyPath === null ? os.put(value, key) : os.put(value);
      }
      await new Promise(ok => { tx.oncomplete = ok; tx.onerror = ok; tx.onabort = ok; });
    }

    db.close();
  }
  return 'ok';
})"#;

/// Whether a cookie domain belongs to an origin.
///
/// A leading dot means the domain and everything under it, which is how a
/// cookie set for `.example.com` reaches `www.example.com`.
fn covers(origin: &str, domain: &str) -> bool {
    let host = origin
        .split("//")
        .nth(1)
        .unwrap_or(origin)
        .split('/')
        .next()
        .unwrap_or_default()
        // A cookie is scoped to a host, never to a port: one set on
        // `127.0.0.1:8000` is sent to `127.0.0.1:9000` as well, and an origin
        // compared with its port still attached matches nothing.
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
        // -1 is the protocol's "when the browser closes", which is not a time
        // and must not be written back as one.
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
        // Every origin in the first tests was portless, which is why this was
        // wrong for a while: a session on a development server exported
        // nothing at all.
        assert!(covers("http://127.0.0.1:8000", "127.0.0.1"));
        assert!(covers("https://example.com:8443", "example.com"));
    }

    #[test]
    fn test_one_site_does_not_cover_another() {
        // The failure this exists to prevent: a session for one site carrying
        // another's cookies, which is an account handed to a stranger.
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
        // Both are asked for: one overrides what a site decided, the other is
        // expensive and imperfect.
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
        // -1 is the protocol's "until the browser closes". Written back as a
        // time it is 1969, and the cookie is expired on arrival.
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
        // The failure this exists to prevent: `&` unescaped turns the rest of
        // the query into somebody else's parameter, and the search runs on
        // half of what was asked for.
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
        // What used to hand back <body>: every ancestor's innerText holds its
        // descendants', and document order puts the outermost first.
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
    fn test_an_element_out_of_view_still_parses() {
        // What a match above the fold used to send: `getBoundingClientRect`
        // is the viewport's, so its middle is negative, and one such match
        // used to fail the whole listing rather than itself.
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
        // The guard in DESCRIBE, which is what stops a negative one being
        // written down at all.
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
