pub mod boundaries;
mod jsonrpc;
mod tools;
mod ui;

use computer_client::Client;
use jsonrpc::{INVALID_PARAMS, METHOD_NOT_FOUND, Request, Response};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

pub const DEFAULT_PROTOCOL: &str = "2024-11-05";

/// The code the specification gives an unknown resource.
const RESOURCE_NOT_FOUND: i32 = -32002;

pub struct Server {
    client: Client,
    /// Where a browser reaches the box server: the client's base unless a proxy hides it.
    origin: String,
    bounded: bool,
}

impl Server {
    pub fn new(client: Client) -> Self {
        let origin = client.base().to_string();
        Self {
            client,
            origin,
            bounded: boundaries::asked(),
        }
    }

    pub fn content_boundaries(mut self, bounded: bool) -> Self {
        self.bounded = bounded;
        self
    }

    pub fn reached_at(mut self, origin: impl Into<String>) -> Self {
        self.origin = origin.into().trim_end_matches('/').to_string();
        self
    }

    pub async fn serve<R, W>(&self, input: R, output: W) -> std::io::Result<()>
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

            let message: Value = match serde_json::from_str(&line) {
                Ok(message) => message,
                Err(error) => {
                    tracing::warn!(%error, "a line that is not a request");
                    continue;
                }
            };

            let Some(response) = self.handle(message).await else {
                continue;
            };
            let mut encoded = serde_json::to_vec(&response).map_err(std::io::Error::other)?;
            encoded.push(b'\n');
            out.write_all(&encoded).await?;
            out.flush().await?;
        }

        Ok(())
    }

    pub async fn stdio(&self) -> std::io::Result<()> {
        self.serve(tokio::io::stdin(), tokio::io::stdout()).await
    }

    /// `None` for a notification, which is answered with silence.
    pub async fn handle(&self, message: Value) -> Option<Value> {
        let request: Request = match serde_json::from_value(message) {
            Ok(request) => request,
            Err(error) => {
                tracing::warn!(%error, "a message that is not a request");
                return None;
            }
        };

        let id = request.id.clone()?;
        serde_json::to_value(self.answer(id, &request).await).ok()
    }

    async fn answer(&self, id: Value, request: &Request) -> Response {
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
                        "capabilities": { "tools": {}, "resources": {} },
                        "serverInfo": { "name": "computer", "version": env!("CARGO_PKG_VERSION") },
                        "instructions":
                            "Each box is a real Linux desktop with a browser. Start one with \
                             launch_box, work out coordinates from the picture the tools hand \
                             back, and remove it with remove_box when you are done. Every action \
                             answers with the screen it produced, so you rarely need screenshot \
                             on its own. A host that renders apps shows the person the live \
                             screen beside launch_box, open_screen and hand_over, with buttons \
                             to take over and to record; the person may press them without \
                             telling you first.",
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

                match tools::call(&self.client, &self.origin, name, &arguments).await {
                    Ok(tools::Answer::Text(text))
                        if self.bounded && boundaries::PAGE_TEXT.contains(&name) =>
                    {
                        Response::ok(
                            id,
                            tools::Answer::Text(boundaries::wrap(&text)).into_content(),
                        )
                    }
                    Ok(answer) => Response::ok(id, answer.into_content()),
                    Err(why) => Response::ok(id, tools::Answer::failure(why)),
                }
            }
            "resources/list" => Response::ok(id, json!({ "resources": ui::listed(&self.origin) })),
            "resources/templates/list" => Response::ok(id, json!({ "resourceTemplates": [] })),
            "resources/read" => {
                let Some(uri) = params.get("uri").and_then(Value::as_str) else {
                    return Response::failed(id, INVALID_PARAMS, "a read needs a uri");
                };

                match ui::read(&self.origin, uri) {
                    Some(contents) => Response::ok(id, contents),
                    None => Response::failed(id, RESOURCE_NOT_FOUND, format!("no resource {uri}")),
                }
            }
            other => Response::failed(id, METHOD_NOT_FOUND, format!("no method {other}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nowhere() -> Server {
        Server::new(Client::new("http://127.0.0.1:1"))
    }

    async fn spoken(lines: &str) -> Vec<Value> {
        let mut out: Vec<u8> = Vec::new();
        nowhere()
            .serve(lines.as_bytes(), &mut out)
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
        assert!(
            said[0]["result"]["capabilities"]["resources"].is_object(),
            "a host only asks for the page if resources were promised"
        );
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
    async fn test_the_screen_tools_point_at_the_page_and_the_page_only_tool_hides() {
        let said = spoken("{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}\n").await;
        let tools = said[0]["result"]["tools"].as_array().expect("a list");
        let named = |name: &str| {
            tools
                .iter()
                .find(|tool| tool["name"] == name)
                .unwrap_or_else(|| panic!("no tool {name}"))
        };

        for name in ["launch_box", "open_screen", "hand_over"] {
            assert_eq!(named(name)["_meta"]["ui"]["resourceUri"], json!(ui::URI));
            assert_eq!(
                named(name)["_meta"]["openai/outputTemplate"],
                json!(ui::URI),
                "{name}: ChatGPT's older path reads its own key"
            );
        }
        assert_eq!(
            named("screen_status")["_meta"]["ui"]["visibility"],
            json!(["app"]),
            "the page polls this one; the model has open_screen"
        );
        assert!(
            named("reclaim_screen")["_meta"]["ui"]
                .get("resourceUri")
                .is_none()
        );
    }

    #[tokio::test]
    async fn test_the_page_is_listed_and_read_with_the_origin_it_connects_to() {
        let server = nowhere().reached_at("https://boxes.example.com/");

        let listed = server
            .handle(json!({ "jsonrpc": "2.0", "id": 1, "method": "resources/list" }))
            .await
            .expect("an answer");
        let resources = listed["result"]["resources"].as_array().expect("a list");
        assert_eq!(resources.len(), 1);
        assert_eq!(resources[0]["uri"], json!(ui::URI));
        assert_eq!(resources[0]["mimeType"], json!(ui::MIME));
        assert_eq!(
            resources[0]["_meta"]["ui"]["csp"]["connectDomains"],
            json!(["https://boxes.example.com", "wss://boxes.example.com"]),
            "the host only opens the socket to an origin the resource named, and a \
             CSP https source does not cover wss"
        );

        let read = server
            .handle(json!({
                "jsonrpc": "2.0", "id": 2, "method": "resources/read",
                "params": { "uri": ui::URI }
            }))
            .await
            .expect("an answer");
        let page = read["result"]["contents"][0]["text"]
            .as_str()
            .expect("the page");
        assert!(
            page.contains("<script type=\"module\">"),
            "the page carries its own script"
        );
        assert!(
            page.contains("id=\"takeover\""),
            "the page has a take over button"
        );
    }

    #[tokio::test]
    async fn test_a_resource_nobody_serves_is_refused_with_the_specified_code() {
        let said = nowhere()
            .handle(json!({
                "jsonrpc": "2.0", "id": 1, "method": "resources/read",
                "params": { "uri": "ui://computer/nothing.html" }
            }))
            .await
            .expect("an answer");

        assert_eq!(said["error"]["code"], json!(RESOURCE_NOT_FOUND));
    }

    #[tokio::test]
    async fn test_a_method_nobody_serves_is_refused_rather_than_ignored() {
        let said = spoken("{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"nonsense\"}\n").await;

        assert_eq!(said[0]["error"]["code"], json!(METHOD_NOT_FOUND));
    }
}
