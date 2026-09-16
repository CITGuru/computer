use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use computer_api::{
    Action, ActionBatch, Arrange, BoxState, ElementResult, Evaluate, Find, ForkMode, ForkRequest,
    Frame, Held, NodeQuery, OnElement, OnNode, OpenIn, PageShot, Picture, Reading, Rect, ScrollTo,
    Shot, Want, Where,
};
use computer_client::{Client, captured_image, frame_png};
use computer_types::{Button, Desktop, Feature, Placement, Point, Spec};
use serde_json::{Value, json};

use crate::ui;

#[derive(Debug)]
pub enum Answer {
    Text(String),
    /// Text for the model, a record for the page, and under `_meta` what only the page may see.
    Structured {
        text: String,
        structured: Value,
        meta: Value,
    },
    Shot {
        text: String,
        png: Vec<u8>,
    },
    Drawn {
        text: String,
        image: Vec<u8>,
        mime: &'static str,
    },
    Failed {
        text: String,
        png: Option<Vec<u8>>,
    },
}

impl Answer {
    pub fn into_content(self) -> Value {
        match self {
            Self::Text(text) => json!({ "content": [{ "type": "text", "text": text }] }),
            Self::Structured {
                text,
                structured,
                meta,
            } => json!({
                "content": [{ "type": "text", "text": text }],
                "structuredContent": structured,
                "_meta": meta,
            }),
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
            Self::Drawn { text, image, mime } => json!({
                "content": [
                    { "type": "text", "text": text },
                    {
                        "type": "image",
                        "data": BASE64.encode(&image),
                        "mimeType": mime,
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
        tool_with(
            "launch_box",
            "Start a fresh Linux desktop with a browser on it. Returns its id, which every \
             other tool takes, and a URL a person can watch it at. Remove it with remove_box \
             when finished — a box left running keeps its memory.",
            json!({
                "type": "object",
                "properties": {
                    "width": { "type": "integer", "description": "Screen width. Defaults to the image's." },
                    "height": { "type": "integer", "description": "Screen height." },
                    "video": {
                        "type": "boolean",
                        "description": "Put ffmpeg in the box, so `record` and the Record button \
                                        on the screen page work. A running box cannot be given \
                                        it afterwards."
                    },
                    "wide_fonts": {
                        "type": "boolean",
                        "description": "Install Chinese, Japanese, Korean and emoji fonts. \
                                        Without them those pages render as empty boxes and \
                                        the screenshot still looks like a working page."
                    },
                    "accessibility": {
                        "type": "boolean",
                        "description": "Let the `widget` tool read native windows by the \
                                        names of their widgets rather than by their pixels. \
                                        Ask for it here if the work involves anything that \
                                        is not a web page — a file dialog, a settings \
                                        panel, an installer. A running box cannot be given \
                                        it afterwards."
                    }
                }
            }),
            ui::renders("Starting a desktop", "Desktop ready")
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
             This is the desktop, frame and address bar and all; `page_screenshot` is what \
             the browser drew.",
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
                    },
                    "pointer": {
                        "type": "boolean",
                        "description": "Draw the pointer. Left out otherwise, which is why \
                                        the `cursor` tool exists."
                    },
                    "tab": {
                        "type": "string",
                        "description": "Bring this page to the front first, so the screen \
                                        shows it. Still a picture of the desktop."
                    }
                }),
                &[]
            )
        ),
        tool(
            "page_screenshot",
            "The page as the browser drew it: no window frame, no address bar, no pointer, \
             and the same on a box with no display. Use it to read a page as a picture, and \
             `screenshot` to see the desktop the page sits on. `full` reaches past the \
             viewport to the whole scrollable page, which is as tall as the page is — an \
             article measured 33585 pixels — so it answers JPEG unless you ask otherwise.",
            with_tab(
                json!({
                    "full": {
                        "type": "boolean",
                        "description": "The whole scrollable page rather than what is in \
                                        view. Slower and much larger."
                    },
                    "format": {
                        "type": "string",
                        "enum": ["png", "jpeg"],
                        "description": "png unless `full`, which is jpeg unless you say."
                    },
                    "quality": {
                        "type": "integer",
                        "description": "jpeg only, 1 to 100. 70 unless said."
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
             selector. An icon button is found by what its icon says — the alt text of the \
             image inside it, or the title of its svg. Answers with what each one is, whether it is enabled, and where it sits \
             in the page. Use it to see what is there before acting, then act by the same query \
             rather than by a coordinate — a point taken from a screenshot is wrong the moment \
             the page moves under it. Each match ends with a selector that named exactly one \
             element when it was read: pass that back as `query` rather than the words, which \
             may match more than one thing. A match outside the window is still listed, as \
             `out of view`; the element tools scroll to it, a coordinate cannot reach it. A \
             match also carries what the page declares it to be and what state it is in — a \
             `collapsed` menu or a `disabled` control will not answer a click.",
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
                    "role": {
                        "type": "string",
                        "enum": ["button", "link", "textbox", "checkbox", "radio", "combobox",
                                 "option", "heading", "image", "tab", "dialog"],
                        "description": "Everything built as this kind of thing, however it was \
                                        built: `button` finds a <div role=button> as well as a \
                                        <button> and a submit input. Use it instead of a query \
                                        to see what a page offers."
                    },
                    "scroll": {
                        "type": "boolean",
                        "description": "Bring the best match into view first. A match below the \
                                        fold is otherwise measured where the window is not \
                                        looking, and its coordinates address nothing."
                    }
                }),
                &[]
            )
        ),
        tool(
            "click_element",
            "Bring the thing this query names into view and click it. Prefer this over `click` \
             for anything on a web page: it finds the element itself, so nothing depends on a \
             coordinate being still correct. `right` opens the page's own context menu, which \
             a site draws inside the page — not the browser's, which no screenshot holds. \
             The answer ends with what the click did to the page: a new URL, a change, or \
             nothing within 600 ms.",
            with_page(
                json!({
                    "query": { "type": "string" },
                    "button": { "type": "string", "enum": ["left", "right", "middle"] },
                    "double": {
                        "type": "boolean",
                        "description": "Click it twice: a file to open, a word to select, a row \
                                        to expand. A page counts two clicks, which two separate \
                                        calls to this do not give it."
                    }
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
             which arrived. Answers with what it found, and fails saying what never appeared. \
             `quiet_ms` waits until nothing on the page has changed for that long, with or \
             without a query: the prices a calendar fills in after its dates arrive.",
            with_page(
                json!({
                    "query": { "type": "string" },
                    "gone": {
                        "type": "boolean",
                        "description": "Wait for it to leave instead: a spinner ending, a dialog closing."
                    },
                    "within_ms": { "type": "integer", "description": "How long to wait." },
                    "quiet_ms": {
                        "type": "integer",
                        "description": "Return once nothing on the page has changed for this \
                                        long, after the query if one is given. 500 covers a \
                                        fetch that renders in pieces."
                    },
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
                &[]
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
            "Type into whatever has keyboard focus. Click the field first. `delay_ms` paces \
             the keystrokes: a few inputs act on every keystroke and drop characters that \
             arrive at full speed, and 30 to 50 is usually enough for one of those.",
            with_frame(
                json!({
                    "text": { "type": "string" },
                    "delay_ms": {
                        "type": "integer",
                        "description": "Milliseconds between keystrokes. Full speed unless said."
                    }
                }),
                &["text"]
            )
        ),
        tool(
            "press_key",
            "Press one key or several at once: `enter`, `tab`, `escape`, `up`, `pagedown`, \
             `ctrl+a`, `cmd+shift+p`. Names are matched loosely — `esc`, `return`, `pgdn`, \
             `cmd` and `win` all land where you would expect. `then` and `held` are for the \
             one thing a combination cannot do: hold a modifier open across several keys. \
             `chord: \"tab\", then: [\"tab\"], held: [\"alt\"]` reaches the third window, where \
             pressing `alt+tab` twice only ever reaches the second.",
            with_frame(
                json!({
                    "chord": { "type": "string" },
                    "then": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "More keys, pressed in turn while `held` stays down."
                    },
                    "held": {
                        "type": "array",
                        "items": { "type": "string", "enum": ["shift", "ctrl", "alt", "cmd"] },
                        "description": "Modifiers kept down across every key named."
                    }
                }),
                &["chord"]
            )
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
            "widget",
            "Work with a native window by the names of its widgets rather than by its pixels: \
             a file dialog, a settings panel, an installer. Use this for anything that is not \
             a web page — a page has `click_element` and the rest, which know more about it \
             than any tree does. `op` is one of: `find` for what matches a query, with each \
             match's role, name and rectangle; `tree` for everything an application \
             publishes; `press` to run a widget's own action; `fill` to put a value in a \
             field; `focus` to give one the keyboard. \
             `press` sends no pointer event at all, so it reaches a widget that is covered \
             or scrolled out of view — and an application watching the pointer sees nothing. \
             Where that matters, `find` answers with a rectangle and `click` is still there. \
             A form field usually has no name of its own, so a query is matched against the \
             label beside it too: ask for `Street` and you get the box next to the word.",
            with_frame(
                json!({
                    "op": {
                        "type": "string",
                        "enum": ["find", "tree", "press", "fill", "focus"]
                    },
                    "query": {
                        "type": "string",
                        "description": "The words on it, or beside it. Needed by find, \
                                        press, fill and focus."
                    },
                    "value": { "type": "string", "description": "For fill." },
                    "role": {
                        "type": "string",
                        "description": "The toolkit's own word — `push button`, `text`, \
                                        `label` — where the query alone is ambiguous. \
                                        `find` reports the role of every match."
                    },
                    "exact": {
                        "type": "boolean",
                        "description": "Match the whole of the words rather than any part."
                    },
                    "app": { "type": "string", "description": "One application's widgets only." },
                    "action": {
                        "type": "string",
                        "description": "For press, where a widget offers more than one. The \
                                        first is used by default, and `find` lists them: GTK \
                                        spells it `click` where Qt spells it `Press`."
                    },
                    "depth": { "type": "integer", "description": "For tree. Defaults to a window's worth." },
                    "limit": { "type": "integer" }
                }),
                &["op"]
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
            "cursor",
            "Where the pointer is. A screenshot does not draw it, and a click given no point \
             of its own presses wherever it already is — so this is the only way to know what \
             such a click would hit. Every move, click, drag and scroll answers with it too.",
            box_only(),
        ),
        tool(
            "inspect_box",
            "Everything the server knows about one box: its state, its size, when it was made, \
             when it expires, and where to watch it. `list_boxes` names them; this describes \
             one.",
            box_only(),
        ),
        tool(
            "stop_box",
            "Stop a box, keeping its files. Cheaper than pausing — the memory goes back — and \
             costlier to come back from: resuming it gives a fresh desktop, so anything you had \
             open is gone and the viewer URL changes. Pause instead if you are coming straight \
             back. `resume_box` brings it back either way.",
            box_only(),
        ),
        tool(
            "pause_box",
            "Freeze a box. It keeps its memory and its ports and costs no processor until you \
             resume it, and comes back as the box it was — the windows that were open are \
             still open. Use it when you are done with a box for now but not done with it. \
             Every other tool will hang on a paused box rather than fail, so resume it first.",
            box_only(),
        ),
        tool(
            "resume_box",
            "Make a box usable again, whichever way you put it down. A paused box wakes as it \
             was, with its windows where they were. A stopped one starts a fresh desktop with \
             nothing open and a new viewer URL — the answer says which you got, so read it \
             rather than assuming what is on screen.",
            box_only(),
        ),
        tool_with(
            "record",
            "Record the screen to a video file, or stop a recording and read where it landed. \
             ffmpeg writes it inside the box, so the frames never cross the wire and the cost \
             is the same however far away you are. Needs a box opened with video. You cannot \
             watch the file — it is for the person who reads what you did afterwards, so say \
             where it is when you stop.",
            with_box(
                json!({
                    "op": {
                        "type": "string",
                        "enum": ["start", "stop", "status"],
                        "description": "start begins one, stop ends it and names the file, \
                                        status says whether one is running."
                    },
                    "fps": {
                        "type": "integer",
                        "description": "For start: frames a second, 1 to 60. 12 unless said, \
                                        which is enough to follow a pointer and cheap."
                    }
                }),
                &["op"]
            ),
            ui::callable()
        ),
        tool(
            "evaluate",
            "Run javascript in the page and read what it evaluated to. The box is an isolated \
             sandbox, so anything the page can do is yours: read a value the tools do not \
             expose, drive a widget that answers to no click, pull structured data straight out \
             of the document rather than reading it as text. `await` works. Return what you want \
             to read rather than the thing itself: a DOM node comes back as `{}` and anything \
             cyclic is refused.",
            with_tab(
                json!({
                    "expression": {
                        "type": "string",
                        "description": "Javascript. Its own value is the answer, such as \
                                        `document.title`."
                    },
                    "timeout_ms": {
                        "type": "integer",
                        "description": "How long it may take. The screen is held across it, so \
                                        the server keeps a ceiling."
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Characters of the answer to return."
                    }
                }),
                &["expression"]
            ),
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
        tool_with(
            "hand_over",
            "Give the screen to a person and stop driving it. Returns a URL they open. Your \
             own input is refused until reclaim_screen.",
            box_only(),
            ui::renders("Handing the screen over", "The screen is theirs")
        ),
        tool_with(
            "reclaim_screen",
            "Take the screen back from the person holding it.",
            box_only(),
            ui::callable()
        ),
        tool_with(
            "open_screen",
            "Show the person the live screen of a box, with buttons to take it over and to \
             record it. Call it when they ask to see or to drive the desktop; it changes \
             nothing on the box.",
            box_only(),
            ui::renders("Opening the screen", "Screen ready")
        ),
        tool_with(
            "screen_status",
            "Who holds the screen, whether it is recording, and a fresh ticket to its viewer.",
            box_only(),
            ui::page_only()
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

fn tool_with(name: &str, description: &str, schema: Value, meta: Value) -> Value {
    let mut listed = tool(name, description, schema);
    listed["_meta"] = meta;
    listed
}

fn box_only() -> Value {
    json!({
        "type": "object",
        "properties": { "box_id": { "type": "string" } },
        "required": ["box_id"]
    })
}

fn with_page(mut properties: Value, required: &[&str]) -> Value {
    if let Some(map) = properties.as_object_mut() {
        map.insert("tab".to_string(), tab_field());
    }

    with_frame(properties, required)
}

fn with_tab(mut properties: Value, required: &[&str]) -> Value {
    if let Some(map) = properties.as_object_mut() {
        map.insert("tab".to_string(), tab_field());
    }

    with_box(properties, required)
}

fn tab_field() -> Value {
    json!({
        "type": "string",
        "description": "Which page, from `tabs`. The one on screen where it is left out — \
                        naming it also skips the lookup that finds which that is."
    })
}

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

pub async fn call(
    client: &Client,
    origin: &str,
    name: &str,
    arguments: &Value,
) -> Result<Answer, String> {
    match name {
        "launch_box" => launch(client, origin, arguments).await,
        "list_boxes" => list(client).await,
        "remove_box" => {
            let id = text(arguments, "box_id")?;
            client.delete(&id).await.map_err(|e| e.to_string())?;
            Ok(Answer::Text(format!("removed {id}")))
        }
        "screenshot" => {
            let id = text(arguments, "box_id")?;
            let frame = client
                .capture(&id, 0, &framing(arguments)?, have(arguments).as_deref())
                .await
                .map_err(|e| e.to_string())?;

            Ok(framed("the screen now", Some(&frame)))
        }
        "page_screenshot" => {
            let id = text(arguments, "box_id")?;
            let full = arguments
                .get("full")
                .and_then(Value::as_bool)
                .unwrap_or_default();

            let format = match arguments.get("format").and_then(Value::as_str) {
                Some("png") => Some(Picture::Png),
                Some("jpeg") => Some(Picture::Jpeg),
                Some(other) => return Err(format!("no such format: {other}")),
                None => None,
            };

            let taken = client
                .page_screenshot(
                    &id,
                    &PageShot {
                        full,
                        format,
                        quality: arguments
                            .get("quality")
                            .and_then(Value::as_u64)
                            .map(|quality| quality as u32),
                        tab: arguments
                            .get("tab")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                    },
                )
                .await
                .map_err(|e| e.to_string())?;

            let image = captured_image(&taken).map_err(|e| e.to_string())?;

            Ok(Answer::Drawn {
                text: match full {
                    true => "the whole page".to_string(),
                    false => "the page, as far as it is in view".to_string(),
                },
                image,
                mime: match taken.format {
                    Picture::Jpeg => "image/jpeg",
                    Picture::Png => "image/png",
                },
            })
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
        "cursor" => {
            let id = text(arguments, "box_id")?;
            let at = client.cursor(&id, 0).await.map_err(|e| e.to_string())?;

            Ok(Answer::Text(format!("the pointer is at {},{}", at.x, at.y)))
        }
        "inspect_box" => {
            let id = text(arguments, "box_id")?;
            let found = client.get(&id).await.map_err(|e| e.to_string())?;

            let mut said = format!(
                "{} — {:?}, {} screen(s), {}x{}",
                found.id, found.state, found.screens, found.width, found.height
            );
            if let Some(url) = &found.viewer_url {
                said.push_str(&format!("\nwatch it at {url}"));
            }
            match found.expires_at_ms {
                Some(at) => said.push_str(&format!("\nit expires at {at}ms")),
                None => said.push_str("\nit has no deadline"),
            }

            Ok(Answer::Text(said))
        }
        "stop_box" => {
            let id = text(arguments, "box_id")?;
            client.stop(&id).await.map_err(|e| e.to_string())?;

            Ok(Answer::Text(format!(
                "{id} is stopped and its files are kept. Resume it before anything reaches it, \
                 and expect a desktop with nothing open."
            )))
        }
        "pause_box" => {
            let id = text(arguments, "box_id")?;
            client.pause(&id).await.map_err(|e| e.to_string())?;

            Ok(Answer::Text(format!(
                "{id} is frozen; resume it before anything else reaches it"
            )))
        }
        "resume_box" => {
            let id = text(arguments, "box_id")?;

            let was = client.get(&id).await.map_err(|e| e.to_string())?;
            let found = client.resume(&id).await.map_err(|e| e.to_string())?;

            if was.state != BoxState::Stopped {
                return Ok(Answer::Text(format!("{id} is awake, as you left it")));
            }

            let mut said = format!("{id} was stopped, so it is running again with nothing open");
            if let Some(url) = &found.viewer_url {
                said.push_str(&format!("\nwatch it at {url}, which is a new address"));
            }

            Ok(Answer::Text(said))
        }
        "record" => {
            let id = text(arguments, "box_id")?;
            let fps = arguments
                .get("fps")
                .and_then(Value::as_u64)
                .map(|fps| fps as u32);

            let said = match text(arguments, "op")?.as_str() {
                "start" => {
                    let view = client
                        .start_recording(&id, 0, fps)
                        .await
                        .map_err(|e| e.to_string())?;

                    format!(
                        "recording to {} inside the box; stop it to finish the file",
                        view.path.unwrap_or_default()
                    )
                }
                "stop" => {
                    let view = client
                        .stop_recording(&id, 0)
                        .await
                        .map_err(|e| e.to_string())?;

                    format!(
                        "recorded to {} inside the box; read it out with the files route",
                        view.path.unwrap_or_default()
                    )
                }
                "status" => match client
                    .recording(&id, 0)
                    .await
                    .map_err(|e| e.to_string())?
                    .path
                {
                    Some(path) => format!("recording to {path}"),
                    None => "nothing is recording".to_string(),
                },
                other => return Err(format!("no such op: {other}")),
            };

            status(client, origin, &id, said).await
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
                .page(
                    &id,
                    format,
                    limit,
                    max_links,
                    arguments.get("tab").and_then(Value::as_str),
                )
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

            let role = arguments.get("role").and_then(Value::as_str);
            let query = match arguments.get("query").and_then(Value::as_str) {
                Some(query) => query.to_string(),
                None if role.is_some() => String::new(),
                None => return Err("find needs a query or a role".to_string()),
            };

            let found = client
                .find(
                    &id,
                    &Find {
                        query,
                        limit,
                        scroll: arguments.get("scroll").and_then(Value::as_bool),
                        exact: arguments.get("exact").and_then(Value::as_bool),
                        role: role.map(str::to_string),
                        tab: arguments
                            .get("tab")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                    },
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
                            "{} {}{}{}{} {} ({}x{}){}{}",
                            one.tag,
                            one.kind.as_deref().unwrap_or(""),
                            match one.text.is_empty() {
                                true => String::new(),
                                false => format!(" {:?}", one.text),
                            },
                            match one.label.as_deref() {
                                Some(label) if label != one.text => format!(" [{label}]"),
                                _ => String::new(),
                            },
                            match one.role.as_deref() {
                                Some(role) => format!(" role={role}"),
                                None => String::new(),
                            },
                            match one.at {
                                Some(at) => format!("at {},{}", at.x, at.y),
                                None => "out of view".to_string(),
                            },
                            one.width,
                            one.height,
                            match (one.enabled, one.states.is_empty()) {
                                (false, true) => "  disabled".to_string(),
                                (_, false) => format!("  [{}]", one.states.join(" ")),
                                _ => String::new(),
                            },
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
            let double = flag(arguments, "double");
            let what = OnElement::Click {
                query: text(arguments, "query")?,
                button: button(arguments),
                double,
            };
            element(
                client,
                arguments,
                what,
                match double {
                    true => "double clicked",
                    false => "clicked",
                },
            )
            .await
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
            let quiet_ms = arguments.get("quiet_ms").and_then(Value::as_u64);
            let query = match quiet_ms {
                Some(_) => text(arguments, "query").unwrap_or_default(),
                None => text(arguments, "query")?,
            };
            let did = waited(&query, quiet_ms);
            let what = OnElement::WaitFor {
                query,
                gone: flag(arguments, "gone"),
                within_ms: arguments.get("within_ms").and_then(Value::as_u64),
                or: strings(arguments, "or"),
                exact: flag(arguments, "exact"),
                quiet_ms,
            };
            element(client, arguments, what, &did).await
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
        "widget" => widget(client, arguments).await,
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
                    delay_ms: arguments.get("delay_ms").and_then(Value::as_u64),
                },
                400,
            )
            .await
        }
        "press_key" => {
            act(
                client,
                arguments,
                Action::Press {
                    chord: text(arguments, "chord")?,
                    then: strings(arguments, "then"),
                    held: strings(arguments, "held")
                        .iter()
                        .filter_map(|word| Held::named(word))
                        .collect(),
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
        "evaluate" => {
            let id = text(arguments, "box_id")?;
            let what = Evaluate {
                expression: text(arguments, "expression")?,
                timeout_ms: arguments.get("timeout_ms").and_then(Value::as_u64),
                limit: arguments
                    .get("limit")
                    .and_then(Value::as_u64)
                    .map(|n| n as usize),
            };

            let answered = client
                .evaluate(&id, &what, arguments.get("tab").and_then(Value::as_str))
                .await
                .map_err(|e| e.to_string())?;

            Ok(Answer::Text(match answered.truncated {
                true => format!("{} … (truncated)", answered.json),
                false => answered.json,
            }))
        }
        "run_command" => run(client, arguments).await,
        "hand_over" => {
            let id = text(arguments, "box_id")?;
            let view = client
                .takeover(&id, 0, false)
                .await
                .map_err(|e| e.to_string())?;
            let said = match view.url {
                Some(url) => format!("the screen is theirs; they open {url}"),
                None => "the screen is theirs, and no viewer port is published".to_string(),
            };
            status(client, origin, &id, said).await
        }
        "reclaim_screen" => {
            let id = text(arguments, "box_id")?;
            client
                .end_takeover(&id, 0)
                .await
                .map_err(|e| e.to_string())?;
            status(client, origin, &id, "the screen is yours again".to_string()).await
        }
        "open_screen" => {
            let id = text(arguments, "box_id")?;
            status(
                client,
                origin,
                &id,
                format!("the screen of {id} is open for the person"),
            )
            .await
        }
        "screen_status" => {
            let id = text(arguments, "box_id")?;
            status(client, origin, &id, format!("the screen of {id}")).await
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

async fn launch(client: &Client, origin: &str, arguments: &Value) -> Result<Answer, String> {
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
            features: [
                flag(arguments, "video").then_some(Feature::Video),
                flag(arguments, "wide_fonts").then_some(Feature::WideFonts),
                flag(arguments, "accessibility").then_some(Feature::Accessibility),
            ]
            .into_iter()
            .flatten()
            .collect(),
            ..Desktop::default()
        },
        ..Spec::default()
    };

    let created = client
        .create(&spec, &Placement::default(), None)
        .await
        .map_err(|e| e.to_string())?;

    let said = format!(
        "box {} is up at {}x{}{}",
        created.id,
        created.width,
        created.height,
        created
            .viewer_url
            .as_deref()
            .map(|url| format!("\nwatch it at {url}"))
            .unwrap_or_default()
    );
    status(client, origin, &created.id, said).await
}

/// What every screen tool answers: the same record, so the page reads one shape. The
/// socket goes under `_meta`, which no host shows the model, so a ticket never lands in
/// a transcript.
async fn status(client: &Client, origin: &str, id: &str, said: String) -> Result<Answer, String> {
    let view = client.get(id).await.map_err(|e| e.to_string())?;
    let viewers = client.viewers(id, 0).await.map_err(|e| e.to_string())?;
    // A box opened without video has nothing to say here, and that is not a failure.
    let recording = client.recording(id, 0).await.ok();
    let ticket = client
        .viewer_ticket(id, 0)
        .await
        .map_err(|e| e.to_string())?;

    Ok(Answer::Structured {
        text: said,
        structured: json!({
            "box_id": id,
            "screen": 0,
            "width": view.width,
            "height": view.height,
            "state": view.state,
            "taken_over": viewers.taken_over,
            "watching": viewers.watching,
            "driving": viewers.driving,
            "recording": recording.as_ref().is_some_and(|r| r.recording),
            "recording_path": recording.and_then(|r| r.path),
        }),
        meta: json!({
            "vnc": {
                "url": format!(
                    "{}/v1/boxes/{id}/screens/0/viewer/socket?ticket={}",
                    ui::socket_origin(origin),
                    ticket.ticket
                ),
                "expires_at_ms": ticket.expires_at_ms,
            }
        }),
    })
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

fn said_node(node: &computer_api::Node) -> String {
    let where_ = match node.at {
        Some(at) => format!(" at {},{} {}x{}", at.x, at.y, node.width, node.height),
        None => " (not drawn)".to_string(),
    };
    let named = match (&node.labelled, node.name.is_empty()) {
        (Some(label), _) => format!("labelled {label:?}"),
        (None, false) => format!("{:?}", node.name),
        (None, true) => "unnamed".to_string(),
    };
    let does = match node.actions.is_empty() {
        true => ", nothing to press".to_string(),
        false => format!(", press runs {}", node.actions.join(" or ")),
    };
    let holds = match &node.value {
        Some(value) if !value.is_empty() => format!(", holding {value:?}"),
        _ => String::new(),
    };

    format!("{} {named}{where_}{does}{holds} [{}]", node.role, node.app)
}

async fn widget(client: &Client, arguments: &Value) -> Result<Answer, String> {
    let id = text(arguments, "box_id")?;
    let fail = |error: computer_client::Error| error.to_string();

    let query = || -> Result<NodeQuery, String> {
        Ok(NodeQuery {
            query: text(arguments, "query")?,
            role: arguments
                .get("role")
                .and_then(Value::as_str)
                .map(str::to_string),
            exact: flag(arguments, "exact"),
            app: arguments
                .get("app")
                .and_then(Value::as_str)
                .map(str::to_string),
        })
    };

    let what = match text(arguments, "op")?.as_str() {
        "find" => OnNode::Find {
            node: query()?,
            limit: arguments
                .get("limit")
                .and_then(Value::as_u64)
                .map(|limit| limit as usize),
        },
        "tree" => OnNode::Tree {
            app: arguments
                .get("app")
                .and_then(Value::as_str)
                .map(str::to_string),
            depth: arguments
                .get("depth")
                .and_then(Value::as_u64)
                .map(|depth| depth as u32),
        },
        "press" => OnNode::Invoke {
            node: query()?,
            action: arguments
                .get("action")
                .and_then(Value::as_str)
                .map(str::to_string),
        },
        "fill" => OnNode::Set {
            node: query()?,
            value: text(arguments, "value")?,
        },
        "focus" => OnNode::Focus { node: query()? },
        other => return Err(format!("no such op: {other}")),
    };

    let reading = matches!(what, OnNode::Find { .. } | OnNode::Tree { .. });
    let result = client.on_node(&id, 0, &what).await.map_err(fail)?;

    if reading {
        return Ok(Answer::Text(match result.nodes.is_empty() {
            true => "nothing in the tree matched".to_string(),
            false => result
                .nodes
                .iter()
                .map(said_node)
                .collect::<Vec<_>>()
                .join("\n"),
        }));
    }

    let did = match (&result.node, &result.action) {
        (Some(node), Some(action)) => format!("{action} on {}", said_node(node)),
        (Some(node), None) => said_node(node),
        _ => "done".to_string(),
    };

    shot(
        client,
        &id,
        &did,
        have(arguments).as_deref(),
        shots(arguments)?,
    )
    .await
}

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

    // Without the settle, the URL and frame are the page mid-navigation.
    let settle_ms = match &what {
        OnElement::WaitFor { .. } | OnElement::Options { .. } => 0,
        _ => SETTLE_MS,
    };

    let how = shots(arguments)?;

    let tab = arguments.get("tab").and_then(Value::as_str);

    let result = match client.on_element(&id, &what, settle_ms, tab).await {
        Ok(result) => result,
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

    let said = ended(said, &result);

    let said = match &result.matched {
        Some(matched) => format!("{said} ({matched:?})"),
        None => said,
    };

    if !how.wanted() {
        return Ok(Answer::Text(said));
    }

    let frame = client.frame(&id, 0, have(arguments).as_deref()).await.ok();

    Ok(framed(&said, frame.as_ref()))
}

fn waited(query: &str, quiet_ms: Option<u64>) -> String {
    match (query.is_empty(), quiet_ms) {
        (true, Some(ms)) => format!("the page has been still for {ms} ms"),
        _ => "there".to_string(),
    }
}

// The window is part of the claim: a slow site answers after it.
fn ended(said: String, result: &ElementResult) -> String {
    match (&result.url, result.navigated, result.changed) {
        (Some(url), true, _) => format!("{said} — {url}"),
        (_, _, Some(true)) => format!("{said} — the page changed"),
        (_, _, Some(false)) => {
            format!("{said} — nothing on the page changed within {SETTLE_MS} ms")
        }
        _ => said,
    }
}

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
                // A screenshot does not draw the pointer.
                want: match how.wanted() {
                    true => vec![Want::Frame, Want::Cursor],
                    false => vec![Want::Cursor],
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

        return Ok(Answer::Failed {
            text: why,
            png: result
                .frame
                .as_ref()
                .and_then(|frame| frame_png(frame).ok().flatten()),
        });
    }

    let said = match result.cursor {
        Some(at) => format!("done; the pointer is at {},{}", at.x, at.y),
        None => "done".to_string(),
    };

    match how.wanted() {
        true => Ok(framed(&said, result.frame.as_ref())),
        false => Ok(Answer::Text(said)),
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
        _ => return Err("a rectangle takes x, y, width and height together".to_string()),
    };

    let named = |name| {
        arguments
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_string)
    };

    Ok(Shot {
        window: named("window"),
        region,
        scale: whole("scale"),
        pointer: arguments
            .get("pointer")
            .and_then(Value::as_bool)
            .unwrap_or_default(),
        tab: named("tab"),
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

const SETTLE_MS: u64 = 600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shots {
    Auto,
    Always,
    Never,
}

impl Shots {
    fn wanted(&self) -> bool {
        !matches!(self, Self::Never)
    }

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
    fn test_a_press_says_what_it_did_to_the_page() {
        let at = |navigated, changed| ElementResult {
            url: Some("https://example.com/".to_string()),
            navigated,
            changed,
            ..ElementResult::default()
        };

        assert_eq!(
            ended("clicked".to_string(), &at(true, None)),
            "clicked — https://example.com/"
        );
        assert_eq!(
            ended("clicked".to_string(), &at(false, Some(true))),
            "clicked — the page changed"
        );
        assert_eq!(
            ended("clicked".to_string(), &at(false, Some(false))),
            "clicked — nothing on the page changed within 600 ms"
        );
        assert_eq!(
            ended("filled".to_string(), &at(false, None)),
            "filled",
            "a fill is not watched, and the URL alone is no news"
        );
        assert_eq!(
            ended("clicked".to_string(), &ElementResult::default()),
            "clicked",
            "an older server says nothing about the page"
        );
    }

    #[test]
    fn test_the_pointer_can_be_asked_for_on_its_own() {
        let listed = catalogue();
        let tool = listed
            .as_array()
            .expect("a list")
            .iter()
            .find(|tool| tool["name"] == "cursor")
            .expect("cursor is offered");

        assert_eq!(
            tool["inputSchema"]["properties"]["box_id"]["type"],
            "string"
        );
        assert_eq!(
            tool["inputSchema"]["required"],
            json!(["box_id"]),
            "a box and nothing else"
        );
    }

    #[test]
    fn test_a_quiet_wait_needs_no_words() {
        let listed = catalogue();
        let wait_for = listed
            .as_array()
            .expect("a list")
            .iter()
            .find(|tool| tool["name"] == "wait_for")
            .expect("wait_for is offered");

        assert!(
            wait_for["inputSchema"]["properties"]
                .get("quiet_ms")
                .is_some()
        );
        let required = wait_for["inputSchema"]["required"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(!required.iter().any(|name| name == "query"), "{required:?}");

        assert_eq!(waited("", Some(500)), "the page has been still for 500 ms");
        assert_eq!(waited("Calendar", Some(500)), "there");
        assert_eq!(waited("Calendar", None), "there");
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
            "page_screenshot",
        ] {
            assert!(takes_tab.contains(&page_tool), "{page_tool} reaches a page");
        }

        for screen_tool in ["click", "type_text", "press_key", "scroll", "drag"] {
            assert!(
                !takes_tab.contains(&screen_tool),
                "{screen_tool} acts on pixels, so naming a tab would promise what it cannot do"
            );
        }

        assert!(takes_tab.contains(&"screenshot"));
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
