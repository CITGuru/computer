// The page beside a screen tool's result. Pixels come over one WebSocket to the box
// server; every button is a tool call the host relays, so the page holds no API key.
import RFB from "@novnc/novnc";
import { App } from "@modelcontextprotocol/ext-apps/app-with-deps";

const root = document.documentElement;
const el = (id) => document.getElementById(id);
const screen = el("screen");
const buttons = {
  takeover: el("takeover"),
  giveback: el("giveback"),
  record: el("record"),
  stop: el("stop"),
  fullscreen: el("fullscreen"),
};

const state = {
  box_id: null,
  screen: 0,
  width: 0,
  height: 0,
  taken_over: false,
  recording: false,
  recording_path: null,
  watching: 0,
  driving: 0,
  vnc: null,
};

let app = null;
let rfb = null;
let link = { mode: null, alive: false };
let busy = false;
let displayMode = "inline";

function say(text) {
  el("note").textContent = text || "";
}

function textOf(result) {
  return (result.content || [])
    .filter((part) => part.type === "text")
    .map((part) => part.text)
    .join("\n");
}

// Every screen tool answers with the same shape, so one reader serves them all.
function apply(result) {
  if (!result) return;
  if (result.isError) {
    say(textOf(result) || "the tool failed");
    return;
  }
  const data = result.structuredContent;
  if (data) {
    for (const key of Object.keys(state)) {
      if (key in data) state[key] = data[key];
    }
  }
  if (result._meta && result._meta.vnc) state.vnc = result._meta.vnc;
  say("");
  render();
  connect();
}

function wanted() {
  return state.taken_over ? "control" : "view";
}

function connect() {
  if (!state.vnc || !state.vnc.url) return;
  const mode = wanted();
  if (rfb && link.alive && link.mode === mode) return;
  if (rfb) {
    try {
      rfb.disconnect();
    } catch {}
    rfb = null;
  }

  link = { mode, alive: false };
  const url = `${state.vnc.url}&mode=${mode}`;
  rfb = new RFB(screen, url, { wsProtocols: ["binary"] });
  // The view socket refuses input anyway; this stops noVNC from grabbing the keyboard.
  rfb.viewOnly = mode !== "control";
  rfb.scaleViewport = true;
  rfb.clipViewport = false;
  rfb.background = "#0e0e10";
  rfb.addEventListener("connect", () => {
    link.alive = true;
    render();
  });
  rfb.addEventListener("disconnect", (event) => {
    link.alive = false;
    render();
    if (!event.detail.clean) say("lost the screen");
  });
  rfb.addEventListener("securityfailure", (event) => {
    say(`the screen refused the connection: ${event.detail.reason}`);
  });
}

function render() {
  const pieces = [];
  if (state.taken_over) pieces.push(link.alive ? "you are driving" : "taking over");
  else pieces.push(link.alive ? "watching" : state.vnc ? "connecting" : "no screen yet");
  if (state.recording) pieces.push("recording");
  const others = state.watching + state.driving - (link.alive ? 1 : 0);
  if (others > 0) pieces.push(`${others} more watching`);
  el("status").textContent = pieces.join(" · ");

  root.classList.toggle("live", link.alive);
  root.classList.toggle("driving", state.taken_over);
  root.classList.toggle("recording", state.recording);
  root.classList.toggle("fullscreen", displayMode === "fullscreen");

  if (state.width && state.height) {
    screen.style.aspectRatio = `${state.width} / ${state.height}`;
  }

  buttons.takeover.hidden = state.taken_over;
  buttons.giveback.hidden = !state.taken_over;
  buttons.record.hidden = state.recording;
  buttons.stop.hidden = !state.recording;
  const idle = !busy && app && state.box_id;
  for (const button of Object.values(buttons)) button.disabled = !idle;
}

async function call(name, extra, then) {
  if (!app) return;
  busy = true;
  render();
  try {
    const result = await app.callServerTool({
      name,
      arguments: { box_id: state.box_id, ...extra },
    });
    apply(result);
    if (then && !result.isError) await then(result);
  } catch (error) {
    say(String((error && error.message) || error));
  } finally {
    busy = false;
    render();
  }
}

// Told without starting a turn: the model reads it before its next action.
async function tell(text) {
  try {
    await app.updateModelContext({ content: [{ type: "text", text }] });
  } catch {}
}

buttons.takeover.onclick = () =>
  call("hand_over", {}, () =>
    tell("The person took over the screen from the page. Do not act on it until they hand it back."),
  );
buttons.giveback.onclick = () =>
  call("reclaim_screen", {}, () =>
    tell("The person handed the screen back from the page. You may act on it again."),
  );
buttons.record.onclick = () => call("record", { op: "start" });
buttons.stop.onclick = () =>
  call("record", { op: "stop" }, () => {
    if (state.recording_path) tell(`The person stopped a recording; it is at ${state.recording_path} inside the box.`);
  });
buttons.fullscreen.onclick = async () => {
  if (!app) return;
  const mode = displayMode === "fullscreen" ? "inline" : "fullscreen";
  try {
    const granted = await app.requestDisplayMode({ mode });
    displayMode = (granted && granted.mode) || mode;
  } catch {
    displayMode = mode;
  }
  render();
};

function theme(context) {
  if (!context) return;
  if (context.theme) root.dataset.theme = context.theme;
  if (context.displayMode) displayMode = context.displayMode;
  render();
}

async function start() {
  // Opened as a plain page rather than by a host: take the socket from the query, for a look.
  if (window.parent === window) {
    const query = new URLSearchParams(location.search);
    if (query.get("vnc")) {
      state.vnc = { url: query.get("vnc") };
      state.box_id = query.get("box") || "";
      state.taken_over = query.get("mode") === "control";
    }
    render();
    connect();
    return;
  }

  app = new App({ name: "computer", version: "0.1.0" });
  app.ontoolresult = (params) => apply(params);
  app.onhostcontextchanged = (context) => theme(context);
  app.onteardown = async () => {
    if (rfb) rfb.disconnect();
    return {};
  };
  await app.connect();
  theme(app.getHostContext());
  render();
}

start().catch((error) => say(String((error && error.message) || error)));
