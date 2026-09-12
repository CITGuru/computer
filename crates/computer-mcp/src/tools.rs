//! What an agent is offered, and what it gets back.
//!
//! Every tool that moves the screen answers with the frame it produced, as an
//! image rather than as a hash. An agent that has to ask for a screenshot after
//! every click spends two round trips on one step, and the second one is where
//! it forgets to look.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use computer_api::{
    Action, ActionBatch, Arrange, ForkMode, ForkRequest, Frame, Held, OnElement, OpenIn, Reading,
    Rect, ScrollTo, Shot, Want, Where,
};
use computer_client::{Client, frame_png};
use computer_types::{Button, Desktop, Feature, Placement, Point, Spec};
use serde_json::{Value, json};

/// What a tool call answers with: text a model reads, or a picture it looks at.
#[derive(Debug)]
pub enum Answer {
    Text(String),
    Shot {
        text: String,
        png: Vec<u8>,
    },
    /// A refusal the model reads and acts on, with the screen it was refused
    /// on where there is one.
    Failed {
        text: String,
        png: Option<Vec<u8>>,
    },
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
            Self::Failed { text, png } => {
                let mut content = vec![json!({ "type": "text", "text": text })];

                if let Some(png) = png.filter(|png| !png.is_empty()) {
                    content.push(json!({
                        "type": "image",
                        "data": BASE64.encode(&png),
                        "mimeType": "image/png",
                    }));
                }

                json!({ "content": content, "isError": true })
            }
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
            "Look at the screen, or at one window or rectangle of it. Coordinates for \
             clicking come from this picture: its top-left is (0, 0) and they are device \
             pixels, so a cropped or scaled picture is for reading rather than for aiming. \
             The pointer is not drawn in it.",
            with_frame(
                json!({
                    "window": {
                        "type": "string",
                        "description": "A window id, as the `window` tool's `list` reports \
                                        it. Captured where the window is now."
                    },
                    "x": { "type": "integer", "description": "With y, width and height, a \
                                                              rectangle of the screen." },
                    "y": { "type": "integer" },
                    "width": { "type": "integer" },
                    "height": { "type": "integer" },
                    "scale": {
                        "type": "integer",
                        "description": "A percentage of full size, 1 to 400. Halving a \
                                        screen leaves most text readable and costs a \
                                        fraction of the bytes."
                    }
                }),
                &[]
            )
        ),
        tool(
            "open_url",
            "Open a page in a new tab and raise it.",
            with_frame(
                json!({
                    "url": { "type": "string" },
                    "target": {
                        "type": "string",
                        "enum": ["blank", "current"],
                        "description": "`blank`, the default, opens a tab and raises it, and the \
                                        answer carries its id. `current` navigates the page on \
                                        screen, leaving every other tab where it was."
                    }
                }),
                &["url"]
            )
        ),
        tool(
            "open_app",
            "Open an application by name and wait until it has drawn. The name has to be one \
             the box was created with — `list_apps` says which. The picture that comes back is \
             of the app once it is ready to be clicked, not of the moment its window appeared.",
            with_frame(
                json!({
                    "app": { "type": "string" },
                    "args": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Arguments for the app itself, such as a file to open."
                    }
                }),
                &["app"]
            )
        ),
        tool(
            "tabs",
            "List the pages this box has open, switch to one, or close one. Every page carries \
             an id, which the page tools take as `tab` so a read or a click cannot land on \
             whatever happens to be in front. `open_url` answers with the id of what it opened.",
            with_box(
                json!({
                    "op": {
                        "type": "string",
                        "enum": ["list", "switch", "close"],
                        "description": "`list` is the default."
                    },
                    "tab": { "type": "string", "description": "Which page, for switch and close." }
                }),
                &[]
            ),
        ),
        tool(
            "read_page",
            "Read the page on screen as text, with the links and the addresses behind them. \
             Use it to find out what a page says: a screenshot is a picture of text, shows \
             one viewport of a page that may be far longer, and renders a link as its label \
             rather than its URL. Take a screenshot when you need to know where to click — \
             this says what is there, that says where it is.",
            with_box(
                json!({
                    "format": {
                        "type": "string",
                        "enum": ["markdown", "text", "raw"],
                        "description": "markdown (default) keeps headings, lists, tables and \
                                        inline links; text is the flat rendering; raw is the \
                                        page's own HTML, for what the other two do not carry."
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Characters to return. Defaults to the server's cap."
                    },
                    "tab": {
                        "type": "string",
                        "description": "Which page, from `tabs`. The one on screen where it is \
                                        left out."
                    }
                }),
                &[]
            )
        ),
        tool(
            "find",
            "Find things on the page by their words, or by a name, id, placeholder or CSS \
             selector. Answers with what each one is, whether it is enabled, and where it sits \
             in the page. Use it to see what is there before acting, then act by the same query \
             rather than by a coordinate — a point taken from a screenshot is wrong the moment \
             the page moves under it. Each match ends with a selector that named exactly one \
             element when it was read: pass that back as `query` rather than the words, which \
             may match more than one thing. A match outside the window is still listed, as \
             `out of view`; the element tools scroll to it, a coordinate cannot reach it.",
            with_box(
                json!({
                    "query": { "type": "string" },
                    "tab": {
                        "type": "string",
                        "description": "Which page, from `tabs`. The one on screen where it is \
                                        left out — which also costs a lookup, so naming it is \
                                        both safer and quicker."
                    },
                    "limit": { "type": "integer", "description": "How many matches to return." },
                    "exact": {
                        "type": "boolean",
                        "description": "Match the whole of an element's words rather than any \
                                        part of them. Use it when you know the label."
                    },
                    "scroll": {
                        "type": "boolean",
                        "description": "Bring the best match into view first. A match below the \
                                        fold is otherwise measured where the window is not \
                                        looking, and its coordinates address nothing."
                    }
                }),
                &["query"]
            )
        ),
        tool(
            "click_element",
            "Bring the thing this query names into view and click it. Prefer this over `click` \
             for anything on a web page: it finds the element itself, so nothing depends on a \
             coordinate being still correct. `right` opens the page's own context menu, which \
             a site draws inside the page — not the browser's, which no screenshot holds.",
            with_page(
                json!({
                    "query": { "type": "string" },
                    "button": { "type": "string", "enum": ["left", "right", "middle"] }
                }),
                &["query"]
            )
        ),
        tool(
            "fill_field",
            "Put text in the field this query names. Types it rather than assigning it, so a \
             page watching for keystrokes — a box that filters as you type, a form that \
             validates — sees what it expects.",
            with_page(
                json!({ "query": { "type": "string" }, "text": { "type": "string" } }),
                &["query", "text"]
            )
        ),
        tool(
            "dropdown",
            "List what a dropdown offers, or choose one of them. A native dropdown opens a menu \
             the operating system draws, which no screenshot shows and no click can reach, so \
             this is the only way to work one.",
            with_page(
                json!({
                    "query": { "type": "string" },
                    "op": { "type": "string", "enum": ["list", "select"] },
                    "option": { "type": "string", "description": "Required for select." }
                }),
                &["query", "op"]
            )
        ),
        tool(
            "wait_for",
            "Wait until something matching the query is on the page, or with `gone` until it \
             leaves. The query is words, a name, an id, a placeholder or a CSS selector, the \
             same as `find` takes. Use this after anything that makes the page fetch — a sleep is either short \
             enough to act too early or long enough to be paid on every step. Give `or` the \
             text a page shows when what you asked for is never coming, and the answer says \
             which arrived. Answers with what it found, and fails saying what never appeared.",
            with_page(
                json!({
                    "query": { "type": "string" },
                    "gone": {
                        "type": "boolean",
                        "description": "Wait for it to leave instead: a spinner ending, a dialog closing."
                    },
                    "within_ms": { "type": "integer", "description": "How long to wait." },
                    "exact": {
                        "type": "boolean",
                        "description": "Match the whole of an element's words, not any part of \
                                        them: a heading can carry the words a price will arrive \
                                        beside."
                    },
                    "or": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Other text worth stopping for, such as \"sold out\" or \
                                        \"unavailable\". Answers with which one matched, instead \
                                        of spending the whole timeout on a success the page has \
                                        already ruled out."
                    }
                }),
                &["query"]
            )
        ),
        tool(
            "hover",
            "Put the pointer over something without pressing anything. A menu that opens on \
             hover has nothing to click until the pointer arrives.",
            with_page(json!({ "query": { "type": "string" } }), &["query"])
        ),
        tool(
            "history",
            "Go back, forward, or reload. Back is not the same as opening the previous URL \
             again: that discards what the page held, and a form half filled in comes back empty.",
            with_page(
                json!({ "go": { "type": "string", "enum": ["back", "forward", "reload"] } }),
                &["go"]
            )
        ),
        tool(
            "scroll_page",
            "Move the page itself, or one scrollable thing on it, in pixels. Use `to: bottom` on \
             a page that loads more as you reach the end — the answer says where it stopped, and \
             the same position twice means there is no more. Different from `scroll`, which \
             sends wheel clicks at a screen point and moves whatever sits under the pointer.",
            with_page(
                json!({
                    "to": {
                        "type": "string",
                        "enum": ["by", "top", "bottom"],
                        "description": "Defaults to by."
                    },
                    "dy": { "type": "integer", "description": "Pixels down, for to: by." },
                    "dx": { "type": "integer", "description": "Pixels right, for to: by." },
                    "query": {
                        "type": "string",
                        "description": "A scrollable element to move instead of the page."
                    }
                }),
                &[]
            )
        ),
        tool(
            "upload_file",
            "Hand files to a file input. The paths are the box's own — put the file there first \
             with write_file. A file chooser is the operating system's window rather than the \
             page's, so nothing on screen can be clicked to fill one in.",
            with_page(
                json!({
                    "query": { "type": "string" },
                    "paths": { "type": "array", "items": { "type": "string" } }
                }),
                &["query", "paths"]
            )
        ),
        tool(
            "list_apps",
            "The application names this server can open. Read them rather than guessing one.",
            json!({ "type": "object", "properties": {} })
        ),
        tool(
            "click",
            "Click at a point taken from the most recent screenshot. Work the coordinate out \
             from a picture you have just seen, not from an older one and never from a scaled \
             copy — a click against a stale frame lands somewhere else and nothing says so.",
            with_frame(
                json!({
                    "x": { "type": "integer" },
                    "y": { "type": "integer" },
                    "button": { "type": "string", "enum": ["left", "right", "middle"] },
                    "double": { "type": "boolean", "description": "Double click instead of single." },
                    "held": {
                        "type": "array",
                        "items": { "type": "string", "enum": ["shift", "ctrl", "alt", "super"] },
                        "description": "Modifiers held down around it: shift to extend a \
                                        selection, ctrl to add to one. press_key first does \
                                        not do this — that press ends before this arrives."
                    }
                }),
                &["x", "y"]
            )
        ),
        tool(
            "type_text",
            "Type into whatever has keyboard focus. Click the field first.",
            with_frame(json!({ "text": { "type": "string" } }), &["text"])
        ),
        tool(
            "press_key",
            "Press a chord, such as `ctrl+a`, `enter`, `tab` or `cmd+shift+p`.",
            with_frame(json!({ "chord": { "type": "string" } }), &["chord"])
        ),
        tool(
            "scroll",
            "Turn the wheel at a point, in notches. Positive `dy` scrolls down and positive \
             `dx` scrolls right. Whatever sits under the point is what moves, so this reaches \
             a list or a sidebar without focusing it first.",
            with_frame(
                json!({
                    "x": { "type": "integer" },
                    "y": { "type": "integer" },
                    "dy": { "type": "integer" },
                    "dx": { "type": "integer", "description": "Notches right. Defaults to 0." }
                }),
                &["x", "y", "dy"]
            )
        ),
        tool(
            "wait_until_still",
            "Wait until the screen stops changing, instead of guessing at a pause. Use it \
             after anything that draws — a menu opening, a dialog appearing, a page \
             painting — so the picture you read is the finished one. A screen with \
             something animating on it never settles and reaches the deadline instead.",
            with_frame(
                json!({
                    "settle_ms": {
                        "type": "integer",
                        "description": "How long it has to hold still. Defaults to 400."
                    },
                    "within_ms": {
                        "type": "integer",
                        "description": "How long to go on waiting. Defaults to 10000."
                    }
                }),
                &[]
            )
        ),
        tool(
            "drag",
            "Drag from one point to another, which is how text is selected.",
            with_frame(
                json!({
                    "from_x": { "type": "integer" }, "from_y": { "type": "integer" },
                    "to_x": { "type": "integer" }, "to_y": { "type": "integer" },
                    "button": { "type": "string", "enum": ["left", "right", "middle"] },
                    "held": {
                        "type": "array",
                        "items": { "type": "string", "enum": ["shift", "ctrl", "alt", "super"] },
                        "description": "Modifiers held down around it: shift to extend a \
                                        selection, ctrl to add to one. press_key first does \
                                        not do this — that press ends before this arrives."
                    }
                }),
                &["from_x", "from_y", "to_x", "to_y"]
            )
        ),
        tool(
            "window",
            "Work with the windows on the desktop rather than with the pixels they drew. \
             `op` is one of: `list` for what is open, with the id, class, position and size \
             of each; `active` for the one typing would reach; `focus` or `close` for the \
             window an id names; `arrange` to move it, resize it or maximise it; `wait` to \
             return once a window of a class has appeared and held still. Wait for a dialog \
             rather than sleeping and hoping — a sleep is either too short to be right or \
             too long to be paid on every step.",
            with_frame(
                json!({
                    "op": {
                        "type": "string",
                        "enum": ["list", "active", "focus", "close", "arrange", "wait"]
                    },
                    "window": {
                        "type": "string",
                        "description": "The window id, as `list` reports it. Needed by \
                                        focus, close and arrange."
                    },
                    "how": {
                        "type": "string",
                        "enum": ["move", "size", "max", "min", "restore"],
                        "description": "For arrange. `move` takes x and y; `size` takes \
                                        width and height."
                    },
                    "x": { "type": "integer" },
                    "y": { "type": "integer" },
                    "width": { "type": "integer" },
                    "height": { "type": "integer" },
                    "class": {
                        "type": "string",
                        "description": "For wait: what the program calls itself, as `list` \
                                        reports it."
                    },
                    "within_ms": { "type": "integer", "description": "For wait." }
                }),
                &["op"]
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

/// [`with_frame`], and which page to act on.
///
/// Only for the tools that reach a page. A coordinate click reaches the screen,
/// which shows whatever tab is in front, so naming one there would promise
/// something it cannot do.
fn with_page(mut properties: Value, required: &[&str]) -> Value {
    if let Some(map) = properties.as_object_mut() {
        map.insert("tab".to_string(), tab_field());
    }

    with_frame(properties, required)
}

fn tab_field() -> Value {
    json!({
        "type": "string",
        "description": "Which page, from `tabs`. The one on screen where it is left out — \
                        naming it also skips the lookup that finds which that is."
    })
}

/// [`with_box`], and the hash of the picture the caller already holds.
///
/// For the tools that answer with the screen. Passing back the hash from the
/// last answer turns an unmoved screen into a line of text instead of an image
/// the caller is already looking at.
fn with_frame(mut properties: Value, required: &[&str]) -> Value {
    if let Some(map) = properties.as_object_mut() {
        map.insert(
            "have_frame".to_string(),
            json!({
                "type": "string",
                "description": "The `frame` hash from the last answer. A screen that has not \
                                moved since then answers `unchanged` and sends no picture."
            }),
        );
        map.insert(
            "screenshot".to_string(),
            json!({
                "type": "string",
                "enum": ["auto", "always", "never"],
                "description": "When the answer carries the screen. `auto`, the default, sends \
                                it when the screen moved and whenever something was refused. \
                                `always` sends it even where nothing moved. `never` does not \
                                capture one at all — use it for a run of steps you already know \
                                the shape of."
            }),
        );
    }

    with_box(properties, required)
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
            // No mode here: asking for the screen and asking for no screen is
            // a contradiction, and `never` on this tool would answer nothing.
            let frame = client
                .capture(&id, 0, &framing(arguments)?, have(arguments).as_deref())
                .await
                .map_err(|e| e.to_string())?;

            Ok(framed("the screen now", Some(&frame)))
        }
        "open_url" => {
            act(
                client,
                arguments,
                Action::OpenUrl {
                    target: match arguments.get("target").and_then(Value::as_str) {
                        None | Some("blank") => OpenIn::Blank,
                        Some("current") => OpenIn::Current,
                        Some(other) => {
                            return Err(format!("target is blank or current, not {other:?}"));
                        }
                    },
                    url: text(arguments, "url")?,
                },
                2500,
            )
            .await
        }
        "open_app" => {
            let args = arguments
                .get("args")
                .and_then(Value::as_array)
                .map(|args| {
                    args.iter()
                        .filter_map(|arg| arg.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();

            // No settle of its own: the launch already waited for the app to
            // stop drawing, and a second wait on top would be paid for
            // nothing.
            act(
                client,
                arguments,
                Action::Launch {
                    app: text(arguments, "app")?,
                    args,
                },
                0,
            )
            .await
        }
        "tabs" => {
            let id = text(arguments, "box_id")?;

            match arguments
                .get("op")
                .and_then(Value::as_str)
                .unwrap_or("list")
            {
                "list" => {
                    let tabs = client.tabs(&id).await.map_err(|e| e.to_string())?;

                    Ok(Answer::Text(match tabs.is_empty() {
                        true => "no pages are open".to_string(),
                        false => tabs
                            .iter()
                            .map(|tab| {
                                format!(
                                    "{}{} {:?} {}",
                                    tab.id,
                                    match tab.visible {
                                        true => " (on screen)",
                                        false => "",
                                    },
                                    tab.title,
                                    tab.url
                                )
                            })
                            .collect::<Vec<_>>()
                            .join("\n"),
                    }))
                }
                "switch" => {
                    let tab = text(arguments, "tab")?;
                    client
                        .focus_tab(&id, &tab)
                        .await
                        .map_err(|e| e.to_string())?;

                    shot(
                        client,
                        &id,
                        &format!("switched to {tab}"),
                        have(arguments).as_deref(),
                        shots(arguments)?,
                    )
                    .await
                }
                "close" => {
                    let tab = text(arguments, "tab")?;
                    client
                        .close_tab(&id, &tab)
                        .await
                        .map_err(|e| e.to_string())?;

                    Ok(Answer::Text(format!("closed {tab}")))
                }
                other => Err(format!("no such op: {other}")),
            }
        }
        "read_page" => {
            let id = text(arguments, "box_id")?;
            let limit = arguments
                .get("limit")
                .and_then(Value::as_u64)
                .map(|limit| limit as usize);

            let format = match arguments.get("format").and_then(Value::as_str) {
                Some("text") => Reading::Text,
                Some("raw") => Reading::Raw,
                _ => Reading::Markdown,
            };

            let max_links = arguments
                .get("max_links")
                .and_then(Value::as_u64)
                .map(|links| links as usize);

            let page = client
                .page(&id, format, limit, max_links)
                .await
                .map_err(|e| e.to_string())?;

            let links = page
                .links
                .iter()
                .map(|link| format!("{} -> {}", link.text, link.href))
                .collect::<Vec<_>>()
                .join("\n");

            Ok(Answer::Text(format!(
                "{}\n{}\n\n{}{}\n\nLinks:\n{links}",
                page.title,
                page.url,
                page.text,
                match page.truncated {
                    true => " …(cut)",
                    false => "",
                }
            )))
        }
        "find" => {
            let id = text(arguments, "box_id")?;
            let limit = arguments
                .get("limit")
                .and_then(Value::as_u64)
                .map(|n| n as usize);

            let found = client
                .find(
                    &id,
                    &text(arguments, "query")?,
                    limit,
                    arguments.get("scroll").and_then(Value::as_bool),
                    arguments.get("exact").and_then(Value::as_bool),
                )
                .await
                .map_err(|e| e.to_string())?;

            Ok(Answer::Text(match found.is_empty() {
                true => "nothing in view matched. read_page sees the whole \
                     document, and scroll: true looks past the fold"
                    .to_string(),
                false => found
                    .iter()
                    .map(|one| {
                        format!(
                            "{} {}{}{} {} ({}x{}){}{}",
                            one.tag,
                            one.kind.as_deref().unwrap_or(""),
                            match one.text.is_empty() {
                                true => String::new(),
                                false => format!(" {:?}", one.text),
                            },
                            // Only where it adds something: a button whose
                            // label is its words would say it twice.
                            match one.label.as_deref() {
                                Some(label) if label != one.text => format!(" [{label}]"),
                                _ => String::new(),
                            },
                            match one.at {
                                Some(at) => format!("at {},{}", at.x, at.y),
                                None => "out of view".to_string(),
                            },
                            one.width,
                            one.height,
                            match one.enabled {
                                true => "",
                                false => "  disabled",
                            },
                            // What to act by. Words can match more than one
                            // thing; this matched exactly one when it was read.
                            match one.selector.as_deref() {
                                Some(selector) => format!("  {selector}"),
                                None => String::new(),
                            }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            }))
        }
        "click_element" => {
            let what = OnElement::Click {
                query: text(arguments, "query")?,
                button: button(arguments),
            };
            element(client, arguments, what, "clicked").await
        }
        "fill_field" => {
            let what = OnElement::Fill {
                query: text(arguments, "query")?,
                text: text(arguments, "text")?,
            };
            element(client, arguments, what, "filled").await
        }
        "upload_file" => {
            let paths = arguments
                .get("paths")
                .and_then(Value::as_array)
                .map(|paths| {
                    paths
                        .iter()
                        .filter_map(|p| p.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();

            let what = OnElement::Upload {
                query: text(arguments, "query")?,
                paths,
            };
            element(client, arguments, what, "handed over").await
        }
        "wait_for" => {
            let what = OnElement::WaitFor {
                query: text(arguments, "query")?,
                gone: flag(arguments, "gone"),
                within_ms: arguments.get("within_ms").and_then(Value::as_u64),
                or: strings(arguments, "or"),
                exact: flag(arguments, "exact"),
            };
            element(client, arguments, what, "there").await
        }
        "hover" => {
            let what = OnElement::Hover {
                query: text(arguments, "query")?,
            };
            element(client, arguments, what, "hovering over").await
        }
        "history" => {
            let go = match text(arguments, "go")?.as_str() {
                "back" => Where::Back,
                "forward" => Where::Forward,
                "reload" => Where::Reload,
                other => return Err(format!("no such direction: {other}")),
            };
            element(client, arguments, OnElement::History { go }, "went").await
        }
        "scroll_page" => {
            let to = match arguments.get("to").and_then(Value::as_str) {
                Some("top") => ScrollTo::Top,
                Some("bottom") => ScrollTo::Bottom,
                _ => ScrollTo::By,
            };
            let axis = |name| {
                arguments
                    .get(name)
                    .and_then(Value::as_i64)
                    .unwrap_or_default() as i32
            };

            let what = OnElement::Scroll {
                query: arguments
                    .get("query")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                to,
                dx: axis("dx"),
                dy: axis("dy"),
            };
            element(client, arguments, what, "scrolled").await
        }
        "dropdown" => {
            let query = text(arguments, "query")?;
            let what = match text(arguments, "op")?.as_str() {
                "list" => OnElement::Options { query },
                "select" => OnElement::Choose {
                    query,
                    option: text(arguments, "option")?,
                },
                other => return Err(format!("no such op: {other}; use list or select")),
            };
            element(client, arguments, what, "done").await
        }
        "window" => window(client, arguments).await,
        "list_apps" => {
            let names = client.catalog().await.map_err(|e| e.to_string())?;

            Ok(Answer::Text(match names.is_empty() {
                true => "this server knows no apps".to_string(),
                false => names.join(", "),
            }))
        }
        "click" => {
            let at = Some(point(arguments, "x", "y")?);
            let button = button(arguments);
            let action = if flag(arguments, "double") {
                Action::DoubleClick { at, button }
            } else {
                Action::Click {
                    at,
                    button,
                    held: held(arguments)?,
                }
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
            let dx = arguments
                .get("dx")
                .and_then(Value::as_i64)
                .unwrap_or_default() as i32;

            act(client, arguments, Action::Scroll { at, dx, dy }, 400).await
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
                    button: button(arguments),
                    held: held(arguments)?,
                },
                400,
            )
            .await
        }
        "wait_until_still" => {
            let ms = |name| arguments.get(name).and_then(Value::as_u64);

            act(
                client,
                arguments,
                Action::WaitStill {
                    settle_ms: ms("settle_ms"),
                    within_ms: ms("within_ms"),
                },
                0,
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
/// One element operation, and a screenshot of what it did.
async fn window(client: &Client, arguments: &Value) -> Result<Answer, String> {
    let id = text(arguments, "box_id")?;
    let named = |name| text(arguments, name);
    let size = |name| Ok::<u32, String>(number(arguments, name)?.max(0) as u32);
    let fail = |error: computer_client::Error| error.to_string();

    let one = match text(arguments, "op")?.as_str() {
        "list" => {
            let open = client.windows(&id, 0).await.map_err(fail)?;

            return Ok(Answer::Text(match open.is_empty() {
                true => "nothing is open on this screen".to_string(),
                false => open.iter().map(said).collect::<Vec<_>>().join("\n"),
            }));
        }
        "active" => match client.active_window(&id, 0).await.map_err(fail)? {
            Some(window) => window,
            None => return Ok(Answer::Text("nothing has the keyboard".to_string())),
        },
        "focus" => {
            client
                .focus_window(&id, 0, &named("window")?)
                .await
                .map_err(fail)?;

            return shot(
                client,
                &id,
                "raised",
                have(arguments).as_deref(),
                shots(arguments)?,
            )
            .await;
        }
        "close" => {
            client
                .close_window(&id, 0, &named("window")?)
                .await
                .map_err(fail)?;

            return shot(
                client,
                &id,
                "closed",
                have(arguments).as_deref(),
                shots(arguments)?,
            )
            .await;
        }
        "arrange" => {
            let how = match named("how")?.as_str() {
                "move" => Arrange::At {
                    to: Point {
                        x: size("x")?,
                        y: size("y")?,
                    },
                },
                "size" => Arrange::Size {
                    width: size("width")?,
                    height: size("height")?,
                },
                "max" => Arrange::Maximise,
                "min" => Arrange::Minimise,
                "restore" => Arrange::Restore,
                other => return Err(format!("no such arrangement: {other}")),
            };

            client
                .arrange_window(&id, 0, &named("window")?, how)
                .await
                .map_err(fail)?
        }
        "wait" => {
            let within = arguments
                .get("within_ms")
                .and_then(Value::as_u64)
                .map(std::time::Duration::from_millis);

            client
                .wait_for_window(&id, 0, &named("class")?, within)
                .await
                .map_err(fail)?
        }
        other => return Err(format!("no such op: {other}")),
    };

    shot(
        client,
        &id,
        &said(&one),
        have(arguments).as_deref(),
        shots(arguments)?,
    )
    .await
}

fn said(window: &computer_api::Window) -> String {
    format!(
        "{}\t{}\t{}x{}+{}+{}\t{}",
        window.id,
        window.class,
        window.width,
        window.height,
        window.at.x,
        window.at.y,
        window.title
    )
}

async fn shot(
    client: &Client,
    id: &str,
    said: &str,
    have: Option<&str>,
    how: Shots,
) -> Result<Answer, String> {
    if !how.wanted() {
        return Ok(Answer::Text(said.to_string()));
    }

    let frame = client.frame(id, 0, have).await.ok();

    Ok(framed(said, frame.as_ref()))
}

async fn element(
    client: &Client,
    arguments: &Value,
    what: OnElement,
    did: &str,
) -> Result<Answer, String> {
    let id = text(arguments, "box_id")?;

    // The same pause the coordinate tools take. Without it the URL and the
    // frame below are the page on its way rather than the page it reached,
    // which made the element tools less reliable than the ones they replace.
    let settle_ms = match &what {
        OnElement::WaitFor { .. } | OnElement::Options { .. } => 0,
        _ => SETTLE_MS,
    };

    let how = shots(arguments)?;

    let tab = arguments.get("tab").and_then(Value::as_str);

    let result = match client.on_element(&id, &what, settle_ms, tab).await {
        Ok(result) => result,
        // Nothing was captured with it, so the screen is asked for here. The
        // settle was already paid by the call that failed.
        Err(why) => {
            let png = match how.wanted() {
                true => screen_now(client, &id).await,
                false => None,
            };

            return Ok(Answer::Failed {
                text: why.to_string(),
                png,
            });
        }
    };

    if !result.options.is_empty() {
        return Ok(Answer::Text(result.options.join("\n")));
    }

    let said = match (&result.element, result.at) {
        (Some(one), _) => format!("{did} {:?}", one.text),
        (None, Some(at)) => format!("{did} to {},{}", at.x, at.y),
        (None, None) => did.to_string(),
    };

    // Where it left the page. A click that navigated and one that did nothing
    // read the same without this.
    let said = match (&result.url, result.navigated) {
        (Some(url), true) => format!("{said} — {url}"),
        (Some(_), false) => format!("{said} — the page did not move"),
        (None, _) => said,
    };

    let said = match &result.matched {
        Some(matched) => format!("{said} ({matched:?})"),
        None => said,
    };

    // The frame it produced, like every other tool that moves the screen: an
    // agent that has to ask for one after each step spends two round trips on
    // one, and forgets to look on the second.
    if !how.wanted() {
        return Ok(Answer::Text(said));
    }

    let frame = client.frame(&id, 0, have(arguments).as_deref()).await.ok();

    Ok(framed(&said, frame.as_ref()))
}

/// The screen, for an answer that has none of its own.
async fn screen_now(client: &Client, id: &str) -> Option<Vec<u8>> {
    client
        .frame(id, 0, None)
        .await
        .ok()
        .and_then(|frame| frame_png(&frame).ok().flatten())
        .filter(|png| !png.is_empty())
}

async fn act(
    client: &Client,
    arguments: &Value,
    action: Action,
    settle_ms: u64,
) -> Result<Answer, String> {
    let id = text(arguments, "box_id")?;
    let how = shots(arguments)?;

    let result = client
        .act(
            &id,
            0,
            &ActionBatch {
                actions: vec![action],
                settle_ms: Some(settle_ms),
                want: match how.wanted() {
                    true => vec![Want::Frame],
                    false => Vec::new(),
                },
                have_frame: have(arguments),
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

        // The batch already captured one, after its settle. A refusal is when a
        // caller most wants to see the screen and least wants a second call.
        return Ok(Answer::Failed {
            text: why,
            png: result
                .frame
                .as_ref()
                .and_then(|frame| frame_png(frame).ok().flatten()),
        });
    }

    match how.wanted() {
        true => Ok(framed("done; the screen now", result.frame.as_ref())),
        false => Ok(Answer::Text("done".to_string())),
    }
}

fn held(arguments: &Value) -> Result<Vec<Held>, String> {
    let Some(given) = arguments.get("held").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };

    given
        .iter()
        .map(|one| match one.as_str().unwrap_or_default() {
            "shift" => Ok(Held::Shift),
            "ctrl" | "control" => Ok(Held::Ctrl),
            "alt" | "option" => Ok(Held::Alt),
            "super" | "cmd" | "command" | "meta" | "win" => Ok(Held::Super),
            other => Err(format!("no such modifier: {other}")),
        })
        .collect()
}

fn framing(arguments: &Value) -> Result<Shot, String> {
    let whole = |name| {
        arguments
            .get(name)
            .and_then(Value::as_u64)
            .map(|n| n as u32)
    };

    let region = match (whole("x"), whole("y"), whole("width"), whole("height")) {
        (None, None, None, None) => None,
        (Some(x), Some(y), Some(width), Some(height)) => Some(Rect {
            at: Point { x, y },
            width,
            height,
        }),
        // Half a rectangle would be read as a corner and a guess, and
        // answered with a picture of the wrong thing.
        _ => return Err("a rectangle takes x, y, width and height together".to_string()),
    };

    Ok(Shot {
        window: arguments
            .get("window")
            .and_then(Value::as_str)
            .map(str::to_string),
        region,
        scale: whole("scale"),
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

/// What a click, a fill or a history step is given to finish in.
///
/// The same as the coordinate tools take, because the same page is settling.
const SETTLE_MS: u64 = 600;

/// When an answer carries the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shots {
    /// The screen when it moved, and whenever something was refused. A screen
    /// the caller already holds costs a hash — see `have_frame`.
    Auto,
    /// Every time, even where nothing moved.
    Always,
    /// Never. The screen is not captured at all, so it costs nothing either
    /// end.
    Never,
}

impl Shots {
    fn wanted(&self) -> bool {
        !matches!(self, Self::Never)
    }

    /// Whether the caller's hash is worth sending. Under `Always` it is not:
    /// answering `unchanged` is exactly what it asked not to happen.
    fn deduped(&self) -> bool {
        matches!(self, Self::Auto)
    }
}

fn shots(arguments: &Value) -> Result<Shots, String> {
    match arguments.get("screenshot").and_then(Value::as_str) {
        None | Some("auto") => Ok(Shots::Auto),
        Some("always") => Ok(Shots::Always),
        Some("never") => Ok(Shots::Never),
        Some(other) => Err(format!(
            "screenshot is auto, always or never, not {other:?}"
        )),
    }
}

/// The hash the caller says it already has.
fn have(arguments: &Value) -> Option<String> {
    if !shots(arguments).is_ok_and(|how| how.deduped()) {
        return None;
    }

    arguments
        .get("have_frame")
        .and_then(Value::as_str)
        .filter(|hash| !hash.is_empty())
        .map(str::to_string)
}

/// What an answer carries: the picture, or a note that it is the one the caller
/// already has.
///
/// The hash goes in either way. Without it in the answer there is nothing for
/// the caller to pass back, and every step pays for an image again.
fn framed(said: &str, frame: Option<&Frame>) -> Answer {
    let Some(frame) = frame else {
        return Answer::Text(said.to_string());
    };

    let said = format!("{said}\nframe {}", frame.hash);

    match frame_png(frame)
        .ok()
        .flatten()
        .filter(|png| !png.is_empty())
    {
        Some(png) => Answer::Shot { text: said, png },
        None => Answer::Text(match frame.unchanged {
            true => format!("{said} (unchanged — the screen you already have)"),
            false => said,
        }),
    }
}

fn strings(arguments: &Value, name: &str) -> Vec<String> {
    arguments
        .get(name)
        .and_then(Value::as_array)
        .map(|all| {
            all.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
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
    fn test_every_tool_that_answers_with_the_screen_takes_a_frame_hash() {
        let listed = catalogue();
        let answers_with_screen = [
            "screenshot",
            "open_url",
            "open_app",
            "click_element",
            "fill_field",
            "dropdown",
            "wait_for",
            "hover",
            "history",
            "scroll_page",
            "click",
            "type_text",
            "press_key",
            "scroll",
            "drag",
            "window",
        ];

        for name in answers_with_screen {
            let tool = listed
                .as_array()
                .expect("a list")
                .iter()
                .find(|tool| tool["name"] == name)
                .unwrap_or_else(|| panic!("{name} is offered"));

            assert_eq!(
                tool["inputSchema"]["properties"]["have_frame"]["type"], "string",
                "{name} hands back a picture, so it must take the hash of one"
            );
        }
    }

    #[test]
    fn test_an_unchanged_screen_costs_a_line_rather_than_an_image() {
        let same = Frame {
            hash: "aaa".to_string(),
            unchanged: true,
            png_base64: None,
        };

        match framed("clicked", Some(&same)) {
            Answer::Text(said) => {
                assert!(
                    said.contains("aaa"),
                    "the hash comes back to be passed again"
                );
                assert!(said.contains("unchanged"));
            }
            other => panic!("a screen the caller has was sent again: {other:?}"),
        }
    }

    #[test]
    fn test_a_moved_screen_carries_the_picture_and_its_hash() {
        let moved = Frame {
            hash: "bbb".to_string(),
            unchanged: false,
            png_base64: Some("aGk=".to_string()),
        };

        match framed("clicked", Some(&moved)) {
            Answer::Shot { text, png } => {
                assert!(text.contains("bbb"));
                assert_eq!(png, b"hi");
            }
            other => panic!("the picture was dropped: {other:?}"),
        }
    }

    #[test]
    fn test_only_the_tools_that_reach_a_page_take_a_tab() {
        let listed = catalogue();
        let takes_tab: Vec<&str> = listed
            .as_array()
            .expect("a list")
            .iter()
            .filter(|tool| tool["inputSchema"]["properties"].get("tab").is_some())
            .filter_map(|tool| tool["name"].as_str())
            .collect();

        for page_tool in [
            "read_page",
            "find",
            "click_element",
            "fill_field",
            "dropdown",
            "wait_for",
            "hover",
            "history",
            "scroll_page",
            "upload_file",
        ] {
            assert!(takes_tab.contains(&page_tool), "{page_tool} reaches a page");
        }

        // A coordinate reaches the screen, which shows whatever is in front.
        for screen_tool in [
            "click",
            "type_text",
            "press_key",
            "scroll",
            "drag",
            "screenshot",
        ] {
            assert!(
                !takes_tab.contains(&screen_tool),
                "{screen_tool} acts on pixels, so naming a tab would promise what it cannot do"
            );
        }
    }

    #[test]
    fn test_open_url_says_where_to_put_the_page() {
        let listed = catalogue();
        let open = listed
            .as_array()
            .expect("a list")
            .iter()
            .find(|tool| tool["name"] == "open_url")
            .expect("open_url is offered");

        assert_eq!(
            open["inputSchema"]["properties"]["target"]["enum"],
            json!(["blank", "current"])
        );
    }

    #[test]
    fn test_the_default_is_auto_and_a_bad_mode_is_refused() {
        assert_eq!(shots(&json!({})), Ok(Shots::Auto));
        assert_eq!(shots(&json!({ "screenshot": "auto" })), Ok(Shots::Auto));
        assert_eq!(shots(&json!({ "screenshot": "always" })), Ok(Shots::Always));
        assert_eq!(shots(&json!({ "screenshot": "never" })), Ok(Shots::Never));
        assert!(
            shots(&json!({ "screenshot": "sometimes" })).is_err(),
            "a mode nobody serves is said so rather than treated as the default"
        );
    }

    #[test]
    fn test_always_does_not_send_the_hash() {
        // Sending it invites `unchanged`, which is what `always` asked against.
        assert!(have(&json!({ "screenshot": "always", "have_frame": "abc" })).is_none());
        assert_eq!(
            have(&json!({ "screenshot": "auto", "have_frame": "abc" })),
            Some("abc".to_string())
        );
        assert!(have(&json!({ "screenshot": "never", "have_frame": "abc" })).is_none());
    }

    #[test]
    fn test_a_refusal_carries_the_screen_and_is_still_an_error() {
        let answered = Answer::Failed {
            text: "it was refused".to_string(),
            png: Some(b"png".to_vec()),
        }
        .into_content();

        assert_eq!(answered["isError"], json!(true));
        let content = answered["content"].as_array().expect("content");
        assert_eq!(content.len(), 2, "the words and the screen");
        assert_eq!(content[1]["type"], "image");
    }

    #[test]
    fn test_a_refusal_with_no_screen_is_words_alone() {
        let answered = Answer::Failed {
            text: "no box".to_string(),
            png: None,
        }
        .into_content();

        assert_eq!(answered["isError"], json!(true));
        assert_eq!(answered["content"].as_array().expect("content").len(), 1);
    }

    #[test]
    fn test_every_frame_tool_offers_the_three_modes() {
        let listed = catalogue();

        for tool in listed.as_array().expect("a list") {
            let properties = &tool["inputSchema"]["properties"];
            if properties.get("have_frame").is_none() {
                continue;
            }

            assert_eq!(
                properties["screenshot"]["enum"],
                json!(["auto", "always", "never"]),
                "{} takes a hash, so it must take a mode",
                tool["name"]
            );
        }
    }

    #[test]
    fn test_a_hash_is_only_sent_when_the_caller_gave_one() {
        assert_eq!(
            have(&json!({ "have_frame": "abc" })),
            Some("abc".to_string())
        );
        assert!(
            have(&json!({ "have_frame": "" })).is_none(),
            "empty is none"
        );
        assert!(have(&json!({})).is_none());
    }

    #[test]
    fn test_a_wait_can_be_told_what_else_to_stop_for() {
        let listed = catalogue();
        let wait = listed
            .as_array()
            .expect("a list")
            .iter()
            .find(|tool| tool["name"] == "wait_for")
            .expect("wait_for is offered");

        let or = &wait["inputSchema"]["properties"]["or"];
        assert_eq!(or["type"], "array", "so a failure can end the wait early");
        assert_eq!(or["items"]["type"], "string");
    }

    #[test]
    fn test_everything_that_presses_offers_every_button() {
        let listed = catalogue();
        let tools = listed.as_array().expect("a list");

        for name in ["click", "click_element", "drag"] {
            let one = tools
                .iter()
                .find(|tool| tool["name"] == name)
                .unwrap_or_else(|| panic!("{name} is offered"));

            assert_eq!(
                one["inputSchema"]["properties"]["button"]["enum"],
                json!(["left", "right", "middle"]),
                "{name} offers no way to ask for another button"
            );
        }

        assert_eq!(button(&json!({ "button": "right" })), Button::Right);
        assert_eq!(button(&json!({ "button": "middle" })), Button::Middle);
        assert_eq!(
            button(&json!({})),
            Button::Left,
            "named none means the left"
        );
    }

    #[test]
    fn test_a_list_argument_is_read_and_a_missing_one_is_empty() {
        let given = json!({ "or": ["sold out", "unavailable"], "n": 1 });
        assert_eq!(
            strings(&given, "or"),
            vec!["sold out".to_string(), "unavailable".to_string()]
        );
        assert!(strings(&given, "absent").is_empty());
        // Not an array, so nothing rather than a panic.
        assert!(strings(&given, "n").is_empty());
    }

    #[test]
    fn test_find_says_where_to_look_when_nothing_is_in_view() {
        let listed = catalogue();
        let find = listed
            .as_array()
            .expect("a list")
            .iter()
            .find(|tool| tool["name"] == "find")
            .expect("find is offered");

        let said = find["description"].as_str().unwrap_or_default();
        assert!(
            said.contains("out of\nview") || said.contains("out of view"),
            "a match outside the window is listed rather than dropped: {said}"
        );
    }

    #[test]
    fn test_a_tool_that_needs_a_box_says_so() {
        let listed = catalogue();
        // The three that ask the server rather than a box: two list what it
        // holds, and one lists what it can install.
        let serverwide = ["launch_box", "list_boxes", "list_apps"];

        for one in listed.as_array().expect("a list") {
            if serverwide.iter().any(|name| one["name"] == *name) {
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
