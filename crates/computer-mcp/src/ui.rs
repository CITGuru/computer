//! The page a host renders beside a screen tool's result, and the metadata that links
//! the two. Both the MCP Apps keys and ChatGPT's older aliases are sent: a host reads
//! the ones it knows and ignores the rest.

use serde_json::{Value, json};

pub const URI: &str = "ui://computer/screen.html";
pub const MIME: &str = "text/html;profile=mcp-app";

/// Built by `ui/build.sh`, and committed so the crate needs no node to compile.
const PAGE: &str = include_str!("../ui/screen.html");

const DESCRIPTION: &str = "The live screen of a box, with take over and record controls.";

pub fn listed(origin: &str) -> Value {
    json!([{
        "uri": URI,
        "name": "screen",
        "title": "Computer screen",
        "description": DESCRIPTION,
        "mimeType": MIME,
        "_meta": meta(origin),
    }])
}

pub fn read(origin: &str, uri: &str) -> Option<Value> {
    (uri == URI).then(|| {
        json!({
            "contents": [{
                "uri": URI,
                "mimeType": MIME,
                "text": PAGE,
                "_meta": meta(origin),
            }]
        })
    })
}

/// The page opens one WebSocket, back to the server that served the tool. Both spellings
/// of the origin are listed: in a Content Security Policy an `https:` source does not
/// match a `wss:` URL, so a host that copies the list into `connect-src` needs the socket
/// scheme spelled out.
fn meta(origin: &str) -> Value {
    let origins = [origin.to_string(), socket_origin(origin)];
    json!({
        "ui": {
            "csp": { "connectDomains": origins },
            "prefersBorder": true,
        },
        "openai/widgetCSP": { "connect_domains": origins },
        "openai/widgetPrefersBorder": true,
        "openai/widgetDescription": DESCRIPTION,
    })
}

pub fn socket_origin(origin: &str) -> String {
    if let Some(rest) = origin.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = origin.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        origin.to_string()
    }
}

/// On a tool whose result the page renders.
pub fn renders(invoking: &str, invoked: &str) -> Value {
    json!({
        "ui": { "resourceUri": URI },
        "openai/outputTemplate": URI,
        "openai/widgetAccessible": true,
        "openai/toolInvocation/invoking": invoking,
        "openai/toolInvocation/invoked": invoked,
    })
}

/// On a tool a button on the page calls, which the model may call as well.
pub fn callable() -> Value {
    json!({ "openai/widgetAccessible": true })
}

/// On a tool only the page calls.
pub fn page_only() -> Value {
    json!({
        "ui": { "visibility": ["app"] },
        "openai/widgetAccessible": true,
        "openai/visibility": "private",
    })
}
