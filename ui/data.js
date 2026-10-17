// State and everything that talks to serve. The UI reads state through
// subscribe and changes it only through the functions exported here.

const TOKEN_KEY = "varmint-token";

// The token from the URL `varmint serve --ui` prints, kept for later visits
// and taken out of the address bar.
function initialToken() {
  const fromUrl = new URLSearchParams(location.hash.slice(1)).get("token");
  if (fromUrl) {
    localStorage.setItem(TOKEN_KEY, fromUrl);
    history.replaceState(null, "", location.pathname);
  }
  return localStorage.getItem(TOKEN_KEY);
}

// The view a URL shows: `/`, `/vm/NAME`, or `/vm/NAME/display` with the
// display expanded in place of the shell.
export function route(path) {
  const match = path.match(/^\/vm\/([^/]+)(\/display)?$/);
  return { selected: match ? decodeURIComponent(match[1]) : null, screenExpanded: Boolean(match?.[2]) };
}

function pathOf({ selected, screenExpanded }) {
  return selected ? `/vm/${encodeURIComponent(selected)}${screenExpanded ? "/display" : ""}` : "/";
}

export const state = {
  // Sent with every request; null until the user gives one.
  token: initialToken(),
  vms: [],
  // The machine shown and whether its display fills the page instead of the
  // shell, both kept in the URL.
  ...route(location.pathname),
  creating: false,
  starting: [],
  pending: new Set(),
  toasts: [],
  listError: null,
};

const listeners = new Set();

export function subscribe(listener) {
  listeners.add(listener);
  listener(state);
}

function changed() {
  for (const listener of listeners) listener(state);
}

export function setToken(token) {
  localStorage.setItem(TOKEN_KEY, token);
  state.token = token;
  changed();
}

async function call(op, fields = {}) {
  const response = await fetch("/api", { method: "POST", body: JSON.stringify({ op, token: state.token, ...fields }) });
  const reply = await response.json();
  if (reply.error === "invalid token") {
    localStorage.removeItem(TOKEN_KEY);
    state.token = null;
  }
  if (reply.error) throw new Error(reply.error);
  return reply;
}

async function refresh() {
  state.vms = (await call("ls")).vms;
  if (state.selected && !state.vms.some((vm) => vm.name === state.selected)) {
    Object.assign(state, route("/"));
    history.replaceState(null, "", "/");
  }
  changed();
}

// Polls one request at a time, so slow replies never pile up.
export async function poll() {
  if (!state.token) return setTimeout(poll, 2000);
  try {
    await refresh();
    state.listError = null;
  } catch (error) {
    state.listError = error.message;
    changed();
  }
  setTimeout(poll, 2000);
}

let toastIds = 0;

function toast(text, tone) {
  const id = ++toastIds;
  state.toasts.push({ id, text, tone });
  setTimeout(() => dismissToast(id), tone === "error" ? 8000 : 4000);
}

export function dismissToast(id) {
  state.toasts = state.toasts.filter((toast) => toast.id !== id);
  changed();
}

async function act(key, op, fields, done) {
  state.pending.add(key);
  changed();
  try {
    toast(done(await call(op, fields)), "done");
  } catch (error) {
    toast(error.message, "error");
  }
  state.pending.delete(key);
  await refresh();
}

export const stop = (name) => act(`stop ${name}`, "stop", { name }, () => `Stopped ${name}`);
export const start = (name) => act(`start ${name}`, "run", { name }, () => `Started ${name}`);
export const remove = (name) => act(`rm ${name}`, "rm", { name }, () => `Deleted ${name}`);

export const PRESETS = [
  { id: "small", label: "Small", cpus: 2, memory_mib: 2048 },
  { id: "standard", label: "Standard", cpus: 4, memory_mib: 6144 },
  { id: "large", label: "Large", cpus: 8, memory_mib: 16384 },
];

// Shows the view of the current URL, after the browser went back or forward.
export function followLocation() {
  Object.assign(state, route(location.pathname));
  changed();
}

// Adds the view to the browser history, so back returns to the one before.
function navigate() {
  const path = pathOf(state);
  if (path !== location.pathname) history.pushState(null, "", path);
  changed();
}

export function select(name) {
  state.selected = name;
  state.screenExpanded = false;
  navigate();
}

export function setScreenExpanded(expanded) {
  state.screenExpanded = expanded;
  navigate();
}

export function setCreating(creating) {
  state.creating = creating;
  changed();
}

let runs = 0;

// `form` holds the fields of the new VM sheet, as strings and booleans.
export async function create(form) {
  let fields;
  try {
    fields = runFields(form);
  } catch (error) {
    toast(error.message, "error");
    changed();
    return;
  }
  const run = { id: ++runs, name: fields.name ?? null };
  state.starting.push(run);
  state.creating = false;
  changed();
  try {
    const { name } = await call("run", fields);
    state.selected = name;
    toast(`Started ${name}`, "done");
  } catch (error) {
    toast(error.message, "error");
  }
  state.starting = state.starting.filter((other) => other !== run);
  await refresh();
}

function runFields(form) {
  const preset = PRESETS.find((preset) => preset.id === form.preset);
  const fields = { cpus: preset.cpus, memory_mib: preset.memory_mib, persist: form.persist, wait_network: form.wait_network };
  if (form.name) fields.name = form.name;
  if (form.base) fields.base = form.base;
  if (form.disk_gib) fields.disk_gib = Number(form.disk_gib);
  if (form.size) {
    const size = form.size.split("x").map(Number);
    if (size.length !== 2 || size.some((value) => !Number.isInteger(value))) throw new Error(`size is not WIDTHxHEIGHT: ${form.size}`);
    fields.size = size;
  }
  const lines = form.env.split("\n").filter((line) => line.trim());
  if (lines.length) {
    fields.env = Object.fromEntries(lines.map((line) => {
      const at = line.indexOf("=");
      if (at < 1) throw new Error(`not NAME=value: ${line}`);
      return [line.slice(0, at).trim(), line.slice(at + 1)];
    }));
  }
  return fields;
}

// Counts of the fleet and what its running machines take from the host.
export function fleet(vms) {
  const running = vms.filter((vm) => vm.running);
  const sum = (key) => running.reduce((total, vm) => total + vm.hardware[key], 0);
  return {
    running: running.length,
    stopped: vms.length - running.length,
    persistent: vms.filter((vm) => vm.hardware.persist).length,
    ephemeral: vms.filter((vm) => !vm.hardware.persist).length,
    cpus: sum("cpus"),
    memoryGib: sum("memory_mib") / 1024,
    diskGib: sum("disk_gib"),
  };
}

export function specs(hardware) {
  const memory = hardware.memory_mib >= 1024 ? `${+(hardware.memory_mib / 1024).toFixed(1)} GiB` : `${hardware.memory_mib} MiB`;
  return [`${hardware.cpus} CPU`, memory, `${hardware.disk_gib} GiB disk`];
}

// Attach: connections that serve hands to a VM's control socket, through the
// relay's WebSocket bridge. Each starts with one serve reply line.

function attach(name, request) {
  const socket = new WebSocket(`ws://${location.host}/attach`);
  socket.binaryType = "arraybuffer";
  socket.onopen = () => socket.send(JSON.stringify({ op: "attach", name, request, token: state.token }));
  let buffer = new Uint8Array(0);
  let wake = () => {};
  let closed = false;
  socket.onmessage = (event) => {
    const chunk = new Uint8Array(event.data);
    const joined = new Uint8Array(buffer.length + chunk.length);
    joined.set(buffer);
    joined.set(chunk, buffer.length);
    buffer = joined;
    wake();
  };
  socket.onclose = () => {
    closed = true;
    wake();
  };
  async function until(ready) {
    while (!ready()) {
      if (closed) throw new Error("connection closed");
      await new Promise((resolve) => (wake = resolve));
    }
  }
  async function bytes(count) {
    await until(() => buffer.length >= count);
    const taken = buffer.slice(0, count);
    buffer = buffer.subarray(count);
    return taken;
  }
  async function reply() {
    await until(() => buffer.includes(10));
    const line = JSON.parse(new TextDecoder().decode(await bytes(buffer.indexOf(10) + 1)));
    if (line.error) throw new Error(line.error);
    return line;
  }
  return { socket, bytes, reply, close: () => socket.close() };
}

// Calls onFrame with { width, height, image } whenever the screen changes,
// `image` a decoded VideoFrame the callback must close, until the returned
// connection is closed; `done` settles when the stream ends. Frames arrive as
// H.264 and are decoded in hardware, so a busy screen costs a few Mbit/s.
export function screenStream(name, fps, onFrame) {
  const attached = attach(name, { op: "stream", fps, codec: "h264" });
  let failed = null;
  const decoder = new VideoDecoder({
    output: (image) => onFrame({ width: image.displayWidth, height: image.displayHeight, image }),
    error: (error) => (failed = error),
  });
  const done = (async () => {
    try {
      await attached.reply();
      await attached.reply();
      for (let timestamp = 0; ; timestamp++) {
        const header = new DataView((await attached.bytes(13)).buffer);
        const keyframe = header.getUint8(8) === 1;
        const data = await attached.bytes(header.getUint32(9, true));
        if (failed) throw failed;
        if (keyframe) decoder.configure({ codec: h264Codec(data), optimizeForLatency: true, hardwareAcceleration: "prefer-hardware" });
        decoder.decode(new EncodedVideoChunk({ type: keyframe ? "key" : "delta", timestamp, data }));
      }
    } finally {
      if (decoder.state !== "closed") decoder.close();
    }
  })();
  return { close: attached.close, done };
}

// The codec string of an Annex B keyframe, from its sequence parameter set.
function h264Codec(data) {
  for (let index = 0; index + 6 < data.length; index++) {
    if (data[index] === 0 && data[index + 1] === 0 && data[index + 2] === 1 && (data[index + 3] & 0x1f) === 7) {
      return "avc1." + [...data.subarray(index + 4, index + 7)].map((byte) => byte.toString(16).padStart(2, "0")).join("");
    }
  }
  throw new Error("H.264 keyframe without a sequence parameter set");
}

export function inputStream(name) {
  const attached = attach(name, { op: "inputstream" });
  const ready = attached.reply().then(() => attached.reply());
  return {
    close: attached.close,
    ready,
    send: (event) => attached.socket.send(JSON.stringify(event) + "\n"),
  };
}

const FRAME = { DATA: 2, STDERR: 3, EXIT: 5, RESIZE: 6 };

function encodeFrame(kind, payload) {
  const bytes = new Uint8Array(9 + payload.length);
  const view = new DataView(bytes.buffer);
  view.setUint8(4, kind);
  view.setUint32(5, payload.length, true);
  bytes.set(payload, 9);
  return bytes;
}

// A login shell on a terminal; `exited` resolves with its exit code.
export function shellSession(name, cols, rows, onOutput) {
  const attached = attach(name, {
    op: "open",
    session: { type: "pty", argv: [], cols, rows, terminal: { TERM: "xterm-256color", COLORTERM: "truecolor" } },
  });
  const exited = (async () => {
    await attached.reply();
    for (;;) {
      const header = new DataView((await attached.bytes(9)).buffer);
      const kind = header.getUint8(4);
      const payload = await attached.bytes(header.getUint32(5, true));
      if (kind === FRAME.DATA || kind === FRAME.STDERR) onOutput(payload);
      if (kind === FRAME.EXIT) return new DataView(payload.buffer).getInt32(0, true);
    }
  })();
  return {
    close: attached.close,
    exited,
    write: (text) => attached.socket.send(encodeFrame(FRAME.DATA, new TextEncoder().encode(text))),
    resize: (cols, rows) => {
      const payload = new Uint8Array(4);
      const view = new DataView(payload.buffer);
      view.setUint16(0, cols, true);
      view.setUint16(2, rows, true);
      attached.socket.send(encodeFrame(FRAME.RESIZE, payload));
    },
  };
}

// Linux evdev codes for KeyboardEvent.code.
const KEY_CODES = {
  Escape: 1, Minus: 12, Equal: 13, Backspace: 14, Tab: 15, BracketLeft: 26, BracketRight: 27, Enter: 28,
  ControlLeft: 29, Semicolon: 39, Quote: 40, Backquote: 41, ShiftLeft: 42, Backslash: 43, Comma: 51, Period: 52,
  Slash: 53, ShiftRight: 54, AltLeft: 56, Space: 57, CapsLock: 58, F11: 87, F12: 88, ControlRight: 97,
  AltRight: 100, Home: 102, ArrowUp: 103, PageUp: 104, ArrowLeft: 105, ArrowRight: 106, End: 107,
  ArrowDown: 108, PageDown: 109, Insert: 110, Delete: 111, MetaLeft: 125, MetaRight: 126,
};
[..."1234567890"].forEach((digit, index) => (KEY_CODES[`Digit${digit}`] = 2 + index));
for (const [row, first] of [["QWERTYUIOP", 16], ["ASDFGHJKL", 30], ["ZXCVBNM", 44]]) {
  [...row].forEach((letter, index) => (KEY_CODES[`Key${letter}`] = first + index));
}
for (let index = 1; index <= 10; index++) KEY_CODES[`F${index}`] = 58 + index;

const BUTTONS = [0x110, 0x112, 0x111];

// `x` and `y` are fractions of the displayed screen.
export function pointerEvents(type, x, y, frame, detail) {
  const move = { move: { x: Math.floor(x * frame.width), y: Math.floor(y * frame.height) } };
  if (type === "mousemove") return [move];
  if (type === "wheel") return [move, { scroll: { value: -Math.sign(detail), horizontal: false } }];
  return [move, { button: { button: BUTTONS[detail], down: type === "mousedown" } }];
}

export function keyEvent(event, held) {
  const code = KEY_CODES[event.code];
  if (code === undefined || event.repeat) return null;
  const down = event.type === "keydown";
  if (down) held.add(code);
  else held.delete(code);
  return { key: { code, down } };
}

export function releaseKeys(held) {
  const events = [...held].map((code) => ({ key: { code, down: false } }));
  held.clear();
  return events;
}
