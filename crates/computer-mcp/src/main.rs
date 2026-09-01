//! An MCP server that hands an agent a desktop.
//!
//! Speaks JSON-RPC over stdio to whatever launched it, and HTTP to a
//! `computer-server` — so the boxes can be on this machine or on a fleet
//! somewhere else, and nothing about the tools changes.
//!
//! Logs go to stderr. Stdout carries the protocol and nothing else: a stray
//! line there is a parse error at the other end.

mod jsonrpc;
mod tools;

use computer_client::Client;
use jsonrpc::{INVALID_PARAMS, METHOD_NOT_FOUND, Request, Response};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const DEFAULT_PROTOCOL: &str = "2024-11-05";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "computer_mcp=info".into()),
        )
        .init();

    let base = std::env::var("COMPUTER_SERVER_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:8080".to_string());
    let mut client = Client::new(&base);
    if let Ok(token) = std::env::var("COMPUTER_SERVER_TOKEN") {
        client = client.with_token(token);
    }

    tracing::info!(%base, "computer-mcp is talking to a box server");

    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut out = tokio::io::stdout();

    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }

        let request: Request = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(error) => {
                tracing::warn!(%error, "a line that is not a request");
                continue;
            }
        };

        // A notification is answered with silence, and answering one is a
        // protocol error rather than a courtesy.
        let Some(id) = request.id.clone() else {
            continue;
        };

        let response = answer(&client, id, &request).await;
        let mut encoded = serde_json::to_vec(&response)?;
        encoded.push(b'\n');
        out.write_all(&encoded).await?;
        out.flush().await?;
    }

    Ok(())
}

async fn answer(client: &Client, id: Value, request: &Request) -> Response {
    let params = request.params.clone().unwrap_or(Value::Null);

    match request.method.as_str() {
        "initialize" => {
            let protocol = params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(DEFAULT_PROTOCOL)
                .to_string();

            Response::ok(
                id,
                json!({
                    "protocolVersion": protocol,
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "computer", "version": env!("CARGO_PKG_VERSION") },
                    "instructions":
                        "Each box is a real Linux desktop with a browser. Start one with \
                         launch_box, work out coordinates from the picture the tools hand \
                         back, and remove it with remove_box when you are done. Every action \
                         answers with the screen it produced, so you rarely need screenshot \
                         on its own.",
                }),
            )
        }
        "ping" => Response::ok(id, json!({})),
        "tools/list" => Response::ok(id, json!({ "tools": tools::catalogue() })),
        "tools/call" => {
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return Response::failed(id, INVALID_PARAMS, "a call needs a tool name");
            };
            let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

            // A tool that failed is a result the model reads and can act on,
            // not a protocol error that hides the reason from it.
            match tools::call(client, name, &arguments).await {
                Ok(answer) => Response::ok(id, answer.into_content()),
                Err(why) => Response::ok(id, tools::Answer::failure(why)),
            }
        }
        other => Response::failed(id, METHOD_NOT_FOUND, format!("no method {other}")),
    }
}
