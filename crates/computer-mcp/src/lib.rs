//! An MCP server that hands an agent a desktop.
//!
//! Speaks JSON-RPC over a pair of streams to whatever launched it, and HTTP to
//! a box server — so the boxes can be on this machine or on a fleet somewhere
//! else, and nothing about the tools changes.
//!
//! A library rather than a binary: `computer mcp` is where this is reached from,
//! so an agent that has the command already has this and nothing else has to be
//! installed or found on a PATH.
//!
//! # Stdout carries the protocol
//!
//! Nothing else may write there. Logs go to stderr, and a caller that has
//! already written a line to stdout has broken the session before this starts —
//! which is why [`serve`] takes the streams rather than reaching for them.

mod jsonrpc;
mod tools;

use computer_client::Client;
use jsonrpc::{INVALID_PARAMS, METHOD_NOT_FOUND, Request, Response};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

/// What an `initialize` with no version of its own is answered with.
pub const DEFAULT_PROTOCOL: &str = "2024-11-05";

/// Answers requests until the reader ends.
///
/// Generic over the streams so the loop can be exercised without a terminal:
/// a session is otherwise only testable by launching a process and talking to
/// it.
pub async fn serve<R, W>(client: &Client, input: R, output: W) -> std::io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut lines = BufReader::new(input).lines();
    let mut out = output;

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

        let response = answer(client, id, &request).await;
        let mut encoded = serde_json::to_vec(&response).map_err(std::io::Error::other)?;
        encoded.push(b'\n');
        out.write_all(&encoded).await?;
        out.flush().await?;
    }

    Ok(())
}

/// [`serve`] over this process's own streams.
pub async fn stdio(client: &Client) -> std::io::Result<()> {
    serve(client, tokio::io::stdin(), tokio::io::stdout()).await
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Nothing here reaches it: `ping` and a bad line are answered without a
    /// box, which is what makes the loop testable at all.
    fn nowhere() -> Client {
        Client::new("http://127.0.0.1:1")
    }

    async fn spoken(lines: &str) -> Vec<Value> {
        let mut out: Vec<u8> = Vec::new();
        serve(&nowhere(), lines.as_bytes(), &mut out)
            .await
            .expect("served");

        String::from_utf8(out)
            .expect("utf8")
            .lines()
            .map(|line| serde_json::from_str(line).expect("a response"))
            .collect()
    }

    #[tokio::test]
    async fn test_a_request_is_answered_under_its_own_id() {
        let said = spoken("{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"ping\"}\n").await;

        assert_eq!(said.len(), 1);
        assert_eq!(said[0]["id"], json!(7));
        assert_eq!(said[0]["result"], json!({}));
    }

    #[tokio::test]
    async fn test_a_notification_is_answered_with_silence() {
        let said = spoken("{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n").await;

        assert!(
            said.is_empty(),
            "answering a notification is a protocol error, not a courtesy"
        );
    }

    #[tokio::test]
    async fn test_a_line_that_is_not_a_request_does_not_end_the_session() {
        let said = spoken("not json\n\n{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n").await;

        assert_eq!(
            said.len(),
            1,
            "the rubbish and the blank line were skipped and the session went on"
        );
        assert_eq!(said[0]["id"], json!(1));
    }

    #[tokio::test]
    async fn test_initialize_says_what_it_speaks_and_what_it_has() {
        let said = spoken(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\
             \"params\":{\"protocolVersion\":\"2025-03-26\"}}\n",
        )
        .await;

        assert_eq!(
            said[0]["result"]["protocolVersion"],
            json!("2025-03-26"),
            "the caller's version is answered rather than ours"
        );
        assert_eq!(said[0]["result"]["serverInfo"]["name"], json!("computer"));
        assert!(said[0]["result"]["capabilities"]["tools"].is_object());
    }

    #[tokio::test]
    async fn test_every_tool_is_listed_with_a_schema() {
        let said = spoken("{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}\n").await;
        let tools = said[0]["result"]["tools"].as_array().expect("a list");

        assert!(!tools.is_empty());
        for tool in tools {
            assert!(tool["name"].is_string(), "{tool} has no name");
            assert!(tool["description"].is_string(), "{tool} has no description");
            assert!(
                tool["inputSchema"].is_object(),
                "{tool} has no schema, so a model has to guess its arguments"
            );
        }
    }

    #[tokio::test]
    async fn test_a_method_nobody_serves_is_refused_rather_than_ignored() {
        let said = spoken("{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"nonsense\"}\n").await;

        assert_eq!(said[0]["error"]["code"], json!(METHOD_NOT_FOUND));
    }
}
