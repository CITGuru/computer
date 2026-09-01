//! What an agent is offered, and what it gets back.
//!
//! Every tool that moves the screen answers with the frame it produced, as an
//! image rather than as a hash. An agent that has to ask for a screenshot after
//! every click spends two round trips on one step, and the second one is where
//! it forgets to look.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use computer_api::{Action, ActionBatch, ForkMode, ForkRequest, Want};
use computer_client::{Client, frame_png};
use computer_types::{Button, Desktop, Feature, Placement, Point, Spec};
use serde_json::{Value, json};

/// What a tool call answers with: text a model reads, or a picture it looks at.
pub enum Answer {
    Text(String),
    Shot { text: String, png: Vec<u8> },
}

impl Answer {
    pub fn into_content(self) -> Value {
        match self {
            Self::Text(text) => json!({ "content": [{ "type": "text", "text": text }] }),
            Self::Shot { text, png } => json!({
                "content": [
                    { "type": "text", "text": text },
                    {
                        "type": "image",
                        "data": BASE64.encode(&png),
                        "mimeType": "image/png",
                    },
                ]
            }),
        }
    }

    pub fn failure(message: String) -> Value {
        json!({
            "content": [{ "type": "text", "text": message }],
            "isError": true,
        })
    }
}

pub fn catalogue() -> Value {
    json!([
        tool(
            "launch_box",
            "Start a fresh Linux desktop with a browser on it. Returns its id, which every \
             other tool takes, and a URL a person can watch it at. Remove it with remove_box \
             when finished — a box left running keeps its memory.",
            json!({
                "type": "object",
                "properties": {
                    "width": { "type": "integer", "description": "Screen width. Defaults to the image's." },
                    "height": { "type": "integer", "description": "Screen height." },
                    "wide_fonts": {
                        "type": "boolean",
                        "description": "Install Chinese, Japanese, Korean and emoji fonts. \
                                        Without them those pages render as empty boxes and \
                                        the screenshot still looks like a working page."
                    }
                }
            })
        ),
        tool(
            "list_boxes",
            "Every box running right now.",
            json!({ "type": "object", "properties": {} })
        ),
        tool(
            "remove_box",
            "Take a box away. Its files do not come back.",
            json!({
                "type": "object",
                "properties": { "box_id": { "type": "string" } },
                "required": ["box_id"]
            })
        ),
        tool(
            "screenshot",
            "Look at the screen. Coordinates for clicking come from this picture: its \
             top-left is (0, 0) and they are device pixels. The pointer is not drawn in it.",
            box_only()
        ),
        tool(
            "open_url",
            "Open a page in a new tab and raise it.",
            with_box(json!({ "url": { "type": "string" } }), &["url"])
        ),
        tool(
            "click",
            "Click at a point taken from the most recent screenshot. Work the coordinate out \
             from a picture you have just seen, not from an older one and never from a scaled \
             copy — a click against a stale frame lands somewhere else and nothing says so.",
            with_box(
                json!({
                    "x": { "type": "integer" },
                    "y": { "type": "integer" },
                    "button": { "type": "string", "enum": ["left", "right", "middle"] },
                    "double": { "type": "boolean", "description": "Double click instead of single." }
                }),
                &["x", "y"]
            )
        ),
        tool(
            "type_text",
            "Type into whatever has keyboard focus. Click the field first.",
            with_box(json!({ "text": { "type": "string" } }), &["text"])
        ),
        tool(
            "press_key",
            "Press a chord, such as `ctrl+a`, `enter`, `tab` or `cmd+shift+p`.",
            with_box(json!({ "chord": { "type": "string" } }), &["chord"])
        ),
        tool(
            "scroll",
            "Scroll at a point. Positive `dy` scrolls down.",
            with_box(
                json!({
                    "x": { "type": "integer" },
                    "y": { "type": "integer" },
                    "dy": { "type": "integer" }
                }),
                &["x", "y", "dy"]
            )
        ),
        tool(
            "drag",
            "Drag from one point to another, which is how text is selected.",
            with_box(
                json!({
                    "from_x": { "type": "integer" }, "from_y": { "type": "integer" },
                    "to_x": { "type": "integer" }, "to_y": { "type": "integer" }
                }),
                &["from_x", "from_y", "to_x", "to_y"]
            )
        ),
        tool(
            "run_command",
            "Run a command inside the box and read its output. This is a shell in the same \
             machine as the desktop, not a way to move the pointer.",
            with_box(
                json!({
                    "command": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Argument vector, e.g. [\"ls\", \"-la\", \"/tmp\"]."
                    }
                }),
                &["command"]
            )
        ),
        tool(
            "hand_over",
            "Give the screen to a person and stop driving it. Returns a URL they open. Your \
             own input is refused until reclaim_screen.",
            box_only()
        ),
        tool(
            "reclaim_screen",
            "Take the screen back from the person holding it.",
            box_only()
        ),
        tool(
            "fork_box",
            "Build a second box by doing again what was done to this one. It reconstructs \
             rather than copies, so the two will be close but rarely identical.",
            box_only()
        ),
    ])
}

fn tool(name: &str, description: &str, schema: Value) -> Value {
    json!({ "name": name, "description": description, "inputSchema": schema })
}

fn box_only() -> Value {
    json!({
        "type": "object",
        "properties": { "box_id": { "type": "string" } },
        "required": ["box_id"]
    })
}

fn with_box(mut properties: Value, required: &[&str]) -> Value {
    let map = properties.as_object_mut().expect("an object of properties");
    map.insert("box_id".to_string(), json!({ "type": "string" }));

    let mut needed = vec!["box_id".to_string()];
    needed.extend(required.iter().map(|name| name.to_string()));

    json!({ "type": "object", "properties": properties, "required": needed })
}

pub async fn call(client: &Client, name: &str, arguments: &Value) -> Result<Answer, String> {
    match name {
        "launch_box" => launch(client, arguments).await,
        "list_boxes" => list(client).await,
        "remove_box" => {
            let id = text(arguments, "box_id")?;
            client.delete(&id).await.map_err(|e| e.to_string())?;
            Ok(Answer::Text(format!("removed {id}")))
        }
        "screenshot" => {
            let id = text(arguments, "box_id")?;
            let frame = client
                .frame(&id, 0, None)
                .await
                .map_err(|e| e.to_string())?;
            let png = frame_png(&frame)
                .map_err(|e| e.to_string())?
                .unwrap_or_default();
            Ok(Answer::Shot {
                text: "the screen now".to_string(),
                png,
            })
        }
        "open_url" => {
            act(
                client,
                arguments,
                Action::OpenUrl {
                    url: text(arguments, "url")?,
                },
                2500,
            )
            .await
        }
        "click" => {
            let at = Some(point(arguments, "x", "y")?);
            let button = button(arguments);
            let action = if flag(arguments, "double") {
                Action::DoubleClick { at, button }
            } else {
                Action::Click { at, button }
            };
            act(client, arguments, action, 600).await
        }
        "type_text" => {
            act(
                client,
                arguments,
                Action::Type {
                    text: text(arguments, "text")?,
                },
                400,
            )
            .await
        }
        "press_key" => {
            act(
                client,
                arguments,
                Action::Key {
                    chord: text(arguments, "chord")?,
                },
                400,
            )
            .await
        }
        "scroll" => {
            let at = point(arguments, "x", "y")?;
            let dy = number(arguments, "dy")? as i32;
            act(client, arguments, Action::Scroll { at, dx: 0, dy }, 400).await
        }
        "drag" => {
            let from = point(arguments, "from_x", "from_y")?;
            let to = point(arguments, "to_x", "to_y")?;
            act(
                client,
                arguments,
                Action::Drag {
                    from,
                    to,
                    button: Button::Left,
                },
                400,
            )
            .await
        }
        "run_command" => run(client, arguments).await,
        "hand_over" => {
            let id = text(arguments, "box_id")?;
            let view = client
                .takeover(&id, 0, false)
                .await
                .map_err(|e| e.to_string())?;
            Ok(Answer::Text(match view.url {
                Some(url) => format!("the screen is theirs; they open {url}"),
                None => "the screen is theirs, and no viewer port is published".to_string(),
            }))
        }
        "reclaim_screen" => {
            let id = text(arguments, "box_id")?;
            client
                .end_takeover(&id, 0)
                .await
                .map_err(|e| e.to_string())?;
            Ok(Answer::Text("the screen is yours again".to_string()))
        }
        "fork_box" => {
            let id = text(arguments, "box_id")?;
            let forked = client
                .fork(
                    &id,
                    &ForkRequest {
                        mode: ForkMode::Replay,
                        up_to: None,
                        placement: None,
                    },
                    None,
                )
                .await
                .map_err(|e| e.to_string())?;
            Ok(Answer::Text(format!(
                "forked into {}: {} of {} actions replayed{}",
                forked.created.id,
                forked.replay.ok,
                forked.replay.attempted,
                if forked.replay.truncated {
                    ", and it ran out of time"
                } else {
                    ""
                }
            )))
        }
        other => Err(format!("no tool called {other}")),
    }
}

async fn launch(client: &Client, arguments: &Value) -> Result<Answer, String> {
    let spec = Spec {
        desktop: Desktop {
            width: arguments
                .get("width")
                .and_then(Value::as_u64)
                .map(|n| n as u32),
            height: arguments
                .get("height")
                .and_then(Value::as_u64)
                .map(|n| n as u32),
            features: if flag(arguments, "wide_fonts") {
                vec![Feature::WideFonts]
            } else {
                Vec::new()
            },
            ..Desktop::default()
        },
        ..Spec::default()
    };

    let created = client
        .create(&spec, &Placement::default(), None)
        .await
        .map_err(|e| e.to_string())?;

    Ok(Answer::Text(format!(
        "box {} is up at {}x{}{}",
        created.id,
        created.width,
        created.height,
        created
            .viewer_url
            .map(|url| format!("\nwatch it at {url}"))
            .unwrap_or_default()
    )))
}

async fn list(client: &Client) -> Result<Answer, String> {
    let boxes = client.list().await.map_err(|e| e.to_string())?;

    if boxes.is_empty() {
        return Ok(Answer::Text("no boxes are running".to_string()));
    }

    let listed = boxes
        .iter()
        .map(|found| {
            format!(
                "{}  {}x{}  {} screen(s)",
                found.id, found.width, found.height, found.screens
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok(Answer::Text(listed))
}

async fn run(client: &Client, arguments: &Value) -> Result<Answer, String> {
    let id = text(arguments, "box_id")?;
    let argv: Vec<String> = arguments
        .get("command")
        .and_then(Value::as_array)
        .map(|listed| {
            listed
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .ok_or_else(|| "command must be an array of strings".to_string())?;

    let ran = client
        .exec(&id, &argv, None)
        .await
        .map_err(|e| e.to_string())?;
    Ok(Answer::Text(format!(
        "exit {}{}{}",
        ran.code,
        if ran.stdout.is_empty() {
            String::new()
        } else {
            format!("\n{}", ran.stdout)
        },
        if ran.stderr.is_empty() {
            String::new()
        } else {
            format!("\nstderr:\n{}", ran.stderr)
        },
    )))
}

/// Do the thing, let the screen settle, and hand back what it looks like now.
async fn act(
    client: &Client,
    arguments: &Value,
    action: Action,
    settle_ms: u64,
) -> Result<Answer, String> {
    let id = text(arguments, "box_id")?;

    let result = client
        .act(
            &id,
            0,
            &ActionBatch {
                actions: vec![action],
                settle_ms: Some(settle_ms),
                want: vec![Want::Frame],
                have_frame: None,
            },
            None,
        )
        .await
        .map_err(|e| e.to_string())?;

    let refused = result.results.iter().find(|one| !one.ok);
    if let Some(refused) = refused {
        let why = refused
            .error
            .as_ref()
            .map(|error| error.message.clone())
            .unwrap_or_else(|| "it was refused".to_string());
        return Err(why);
    }

    let png = result
        .frame
        .as_ref()
        .and_then(|frame| frame_png(frame).ok().flatten())
        .unwrap_or_default();

    Ok(Answer::Shot {
        text: "done; the screen now".to_string(),
        png,
    })
}

fn text(arguments: &Value, name: &str) -> Result<String, String> {
    arguments
        .get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("{name} is required and must be a string"))
}

fn number(arguments: &Value, name: &str) -> Result<i64, String> {
    arguments
        .get(name)
        .and_then(Value::as_i64)
        .ok_or_else(|| format!("{name} is required and must be a number"))
}

fn point(arguments: &Value, x: &str, y: &str) -> Result<Point, String> {
    Ok(Point {
        x: number(arguments, x)?.max(0) as u32,
        y: number(arguments, y)?.max(0) as u32,
    })
}

fn button(arguments: &Value) -> Button {
    match arguments.get("button").and_then(Value::as_str) {
        Some("right") => Button::Right,
        Some("middle") => Button::Middle,
        _ => Button::Left,
    }
}

fn flag(arguments: &Value, name: &str) -> bool {
    arguments
        .get(name)
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_every_tool_names_itself_and_says_what_it_takes() {
        let listed = catalogue();
        let tools = listed.as_array().expect("a list");

        assert!(tools.len() >= 12);
        for one in tools {
            assert!(one["name"].as_str().is_some_and(|name| !name.is_empty()));
            assert!(
                one["description"]
                    .as_str()
                    .is_some_and(|said| said.len() > 20)
            );
            assert_eq!(one["inputSchema"]["type"], "object");
        }
    }

    #[test]
    fn test_a_tool_that_needs_a_box_says_so() {
        let listed = catalogue();
        for one in listed.as_array().expect("a list") {
            if one["name"] == "launch_box" || one["name"] == "list_boxes" {
                continue;
            }
            let required = one["inputSchema"]["required"].as_array().expect("required");
            assert!(
                required.iter().any(|name| name == "box_id"),
                "{} does not ask which box",
                one["name"]
            );
        }
    }
}
