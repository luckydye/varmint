// Renders state from data.js and forwards user events to it. Live
// connections (tile previews, the machine's screen and shell) are opened and
// closed as the rendered state asks for them.

import * as data from "./data.js";

const $ = (id) => document.getElementById(id);

function element(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

const selected = () => data.state.vms.find((vm) => vm.name === data.state.selected);

// A button that asks once more before `action` while `needed` says so.
function confirming(button, label, action, needed = () => true) {
  let armed = null;
  let text = "";
  const disarm = () => {
    clearTimeout(armed);
    armed = null;
    button.textContent = text;
    button.classList.remove("armed");
  };
  button.onclick = () => {
    if (!needed()) return action();
    if (armed) {
      disarm();
      return action();
    }
    text = button.textContent;
    button.textContent = label;
    button.classList.add("armed");
    armed = setTimeout(disarm, 3000);
  };
}

function paintInto(canvas) {
  const context = canvas.getContext("2d");
  return ({ width, height, image }) => {
    if (canvas.width !== width || canvas.height !== height) {
      canvas.width = width;
      canvas.height = height;
    }
    context.drawImage(image, 0, 0);
    image.close();
  };
}

function kind(hardware) {
  return hardware.persist ? "Persistent" : "Ephemeral";
}

// Moves the screen between its tile and the machine page with a view
// transition: `from` and the element `to` returns once `update` has rendered
// share a name, so the browser morphs one into the other.
function morph(from, update, to) {
  from.style.viewTransitionName = "screen";
  const transition = document.startViewTransition(() => {
    from.style.viewTransitionName = "";
    update();
    to().style.viewTransitionName = "screen";
  });
  transition.finished.then(() => (to().style.viewTransitionName = ""));
}

// Opens a machine; a running one's screen grows out of its tile, starting from
// the tile's last frame while the page's own stream connects.
function openMachine(name) {
  const vm = data.state.vms.find((vm) => vm.name === name);
  if (!vm.running) return data.select(name);
  const source = tiles.get(name).canvas;
  morph(source, () => {
    data.select(name);
    const screen = $("screen");
    screen.width = source.width;
    screen.height = source.height;
    screen.getContext("2d").drawImage(source, 0, 0);
  }, () => $("screen"));
}

// Picks the column count that shows the 16:10 screens largest in the wall's area.
function layoutWall() {
  const count = Math.max(1, $("grid").children.length);
  const { width, height } = $("wall-area").getBoundingClientRect();
  const size = (columns) => Math.min(width / columns, (height / Math.ceil(count / columns)) * 1.6);
  let columns = 1;
  for (let candidate = 2; candidate <= count; candidate++) if (size(candidate) > size(columns)) columns = candidate;
  $("grid").style.setProperty("--columns", columns);
  $("grid").style.setProperty("--rows", Math.ceil(count / columns));
}

new ResizeObserver(layoutWall).observe($("wall-area"));

// Overview: a wall of live screens, one tile per machine kept across renders
// so its stream keeps running, and a summary of the fleet beside it.

const tiles = new Map();

function tile(name) {
  const node = element("li", "tile");
  node.tabIndex = 0;
  const canvas = element("canvas");
  const label = element("div", "tile-label");
  label.append(element("span", "dot"), element("span", "tile-name", name));
  node.append(canvas, element("span", "tile-empty", "Stopped"), label, element("span", "badge", "Persistent"));
  node.onclick = () => openMachine(name);
  node.onkeydown = (event) => event.key === "Enter" && openMachine(name);
  return { node, canvas, stream: null };
}

function renderOverview(state) {
  const showing = !state.selected;
  $("overview").hidden = !showing;
  const names = new Set(showing ? state.vms.filter((vm) => vm.running).map((vm) => vm.name) : []);
  for (const [name, entry] of tiles) {
    if (names.has(name) || !entry.stream) continue;
    entry.stream.close();
    entry.stream = null;
  }
  if (!showing) return;

  const nodes = state.starting.map((run) => {
    const node = element("li", "tile starting");
    const label = element("div", "tile-label");
    label.append(element("span", "dot"), element("span", "tile-name", run.name ?? "Starting…"));
    node.append(label);
    return node;
  });
  for (const vm of state.vms) {
    if (!tiles.has(vm.name)) tiles.set(vm.name, tile(vm.name));
    const entry = tiles.get(vm.name);
    entry.node.classList.toggle("running", vm.running);
    entry.node.classList.toggle("persistent", vm.hardware.persist);
    if (vm.running && !entry.stream) {
      entry.stream = data.screenStream(vm.name, 10, paintInto(entry.canvas));
      entry.stream.done.catch(() => (entry.stream = null));
    }
    nodes.push(entry.node);
  }
  for (const name of tiles.keys()) if (!state.vms.some((vm) => vm.name === name)) tiles.delete(name);
  $("grid").replaceChildren(...nodes);
  layoutWall();
  $("grid").hidden = nodes.length === 0;
  $("blank").hidden = nodes.length > 0;

  const summary = data.fleet(state.vms);
  const rows = (target, entries) =>
    $(target).replaceChildren(...entries.map(([term, value, className]) => {
      const row = element("div");
      row.append(element("dt", className, term), element("dd", "", String(value)));
      return row;
    }));
  rows("fleet", [
    ["Running", summary.running, "on"],
    ["Stopped", summary.stopped, "off"],
    ["Persistent", summary.persistent],
    ["Ephemeral", summary.ephemeral],
  ]);
  rows("allocated", [
    ["CPU", `${summary.cpus} cores`],
    ["Memory", `${+summary.memoryGib.toFixed(1)} GiB`],
    ["Disk", `${summary.diskGib} GiB`],
  ]);
  $("machine-list").replaceChildren(...state.vms.map((vm) => {
    const item = element("li");
    const button = element("button");
    button.append(element("span", vm.running ? "dot on" : "dot"), element("span", "name", vm.name), element("span", "meta", data.specs(vm.hardware).slice(0, 2).join(" · ")));
    button.onclick = () => data.select(vm.name);
    item.append(button);
    return item;
  }));
  if (!state.vms.length) $("machine-list").append(element("li", "none", "None yet"));
}

// Machine page

function renderMachine(state) {
  const vm = selected();
  $("machine").hidden = !vm;
  $("machine-power").hidden = !vm;
  $("machine-delete").hidden = !vm?.hardware.persist;
  $("new").hidden = Boolean(vm);
  $("crumbs").replaceChildren();
  if (vm) {
    const status = element("span", "state");
    status.append(element("span", vm.running ? "dot on" : "dot"), vm.running ? "Running" : "Stopped");
    $("crumbs").append(vm.name, status);
    const facts = [
      ["Kind", kind(vm.hardware)],
      ["CPU", `${vm.hardware.cpus} cores`],
      ["Memory", data.specs(vm.hardware)[1]],
      ["Disk", `${vm.hardware.disk_gib} GiB`],
      ["Display", `${vm.hardware.width}×${vm.hardware.height}`],
    ];
    $("machine-facts").replaceChildren(...facts.map(([term, value]) => {
      const row = element("div");
      row.append(element("dt", "", term), element("dd", "", value));
      return row;
    }));

    const power = $("machine-power");
    const busy = state.pending.has(`stop ${vm.name}`) || state.pending.has(`start ${vm.name}`);
    power.disabled = busy;
    if (!power.classList.contains("armed")) {
      power.textContent = busy ? (vm.running ? "Stopping…" : "Starting…") : vm.running ? (vm.hardware.persist ? "Stop" : "Stop and discard") : "Start";
    }
    const views = $("views");
    views.dataset.view = vm.running ? "running" : "stopped";
    views.dataset.expanded = state.screenExpanded;
    $("screen-toggle").textContent = state.screenExpanded ? "Collapse" : "Expand";
    $("screen").tabIndex = state.screenExpanded ? 0 : -1;
    if (!state.screenExpanded && document.activeElement === $("screen")) $("screen").blur();
  }
  if (session?.name !== (vm?.running ? vm.name : null)) {
    closeSession();
    if (vm?.running) openSession(vm.name);
  }
  session?.hint();
}

confirming(
  $("machine-power"),
  "Discard machine?",
  () => (selected().running ? data.stop(selected().name) : data.start(selected().name)),
  () => selected().running && !selected().hardware.persist,
);
confirming($("machine-delete"), "Delete machine?", () => data.remove(selected().name));
$("machine-start").onclick = () => data.start(selected().name);
// The display is a preview beside the shell until expanded into its place.
$("screen-toggle").onclick = () => data.setScreenExpanded(!data.state.screenExpanded);
$("shell-return").onclick = () => data.setScreenExpanded(false);
document.addEventListener("keydown", (event) => {
  if (event.key === "Escape" && data.state.screenExpanded && !event.target.closest("canvas, .xterm, dialog")) {
    data.setScreenExpanded(false);
  }
});
$("home").onclick = () => {
  const vm = selected();
  if (!vm?.running) return data.select(null);
  morph($("screen"), () => data.select(null), () => tiles.get(vm.name).canvas);
};

// The selected machine's screen with input, and a shell sized to its view.

let session = null;

function caption(...parts) {
  $("caption").replaceChildren(...parts);
}

function openSession(name) {
  const canvas = $("screen");
  const frameBox = $("screen-frame");
  const events = new AbortController();
  const on = (target, type, listener) => target.addEventListener(type, listener, { signal: events.signal });
  const paint = paintInto(canvas);
  let frame = null;
  const idle = () => caption(data.state.screenExpanded ? "Click to control" : "Click to expand");

  caption("Connecting…");
  const screen = data.screenStream(name, 30, (next) => {
    if (!frame) idle();
    frame = next;
    $("views").style.setProperty("--aspect", frame.width / frame.height);
    paint(frame);
  });
  screen.done.catch((error) => {
    frame = null;
    if (session?.screen === screen) caption(`Screen disconnected: ${error.message}`);
  });

  const input = data.inputStream(name);
  input.ready.then(() => {
    const held = new Set();
    const pointer = (event) => {
      event.preventDefault();
      // The preview is too small to aim at, so a click there only expands it.
      if (!data.state.screenExpanded) {
        if (event.type === "mousedown") data.setScreenExpanded(true);
        return;
      }
      if (event.type === "mousedown") canvas.focus();
      if (!frame) return;
      const box = canvas.getBoundingClientRect();
      const detail = event.type === "wheel" ? event.deltaY : event.button;
      data.pointerEvents(event.type, (event.clientX - box.left) / box.width, (event.clientY - box.top) / box.height, frame, detail)
        .forEach(input.send);
    };
    for (const type of ["mousemove", "mousedown", "mouseup", "wheel"]) on(canvas, type, pointer);
    on(canvas, "contextmenu", (event) => event.preventDefault());
    const key = (event) => {
      event.preventDefault();
      const keyInput = data.keyEvent(event, held);
      if (keyInput) input.send(keyInput);
    };
    on(canvas, "keydown", key);
    on(canvas, "keyup", key);
    let flash = null;
    on(canvas, "focus", () => {
      frameBox.classList.add("captured", "flash");
      caption(element("span", "dot on"), "Keyboard and mouse go to the machine · click outside to release");
      clearTimeout(flash);
      flash = setTimeout(() => frameBox.classList.remove("flash"), 2000);
    });
    on(canvas, "blur", () => {
      frameBox.classList.remove("captured", "flash");
      idle();
      data.releaseKeys(held).forEach(input.send);
    });
  }).catch((error) => caption(`Input unavailable: ${error.message}`));

  // Refreshes the hint when the display is expanded or collapsed.
  const hint = () => frame && document.activeElement !== canvas && idle();
  session = { name, screen, input, events, shell: null, hint };
  openShell(name);
}

function terminalTheme() {
  const style = getComputedStyle(document.documentElement);
  const token = (name) => style.getPropertyValue(name).trim();
  const dark = matchMedia("(prefers-color-scheme: dark)").matches;
  return {
    background: token("--background"),
    foreground: token("--gray-1000"),
    cursor: token("--gray-1000"),
    cursorAccent: token("--background"),
    selectionBackground: dark ? "#333333" : "#d9d9d9",
    black: dark ? "#2e2e2e" : "#171717",
    brightBlack: token("--gray-700"),
    white: dark ? "#ededed" : "#8f8f8f",
    brightWhite: dark ? "#ffffff" : "#666666",
    red: token("--red"),
    brightRed: token("--red"),
    green: token("--green"),
    brightGreen: token("--green"),
    yellow: token("--amber"),
    brightYellow: token("--amber"),
    blue: dark ? "#52a8ff" : "#0068d6",
    brightBlue: dark ? "#52a8ff" : "#0068d6",
    magenta: dark ? "#c472fb" : "#7d00cc",
    brightMagenta: dark ? "#c472fb" : "#7d00cc",
    cyan: dark ? "#3bd4c9" : "#00838f",
    brightCyan: dark ? "#3bd4c9" : "#00838f",
  };
}

function openShell(name) {
  const host = $("shell");
  host.replaceChildren();
  $("shell-status").replaceChildren();
  const terminal = new Terminal({
    fontFamily: '"Geist Mono", ui-monospace, Menlo, monospace',
    fontSize: 13,
    cursorBlink: true,
    theme: terminalTheme(),
  });
  const fit = new FitAddon.FitAddon();
  terminal.loadAddon(fit);
  terminal.open(host);
  terminal.loadAddon(new CanvasAddon.CanvasAddon());
  fit.fit();
  const shell = data.shellSession(name, terminal.cols, terminal.rows, (bytes) => terminal.write(bytes));
  terminal.onData(shell.write);
  terminal.onResize(({ cols, rows }) => shell.resize(cols, rows));
  const resizes = new ResizeObserver(() => host.clientWidth && host.clientHeight && fit.fit());
  resizes.observe(host);
  session.shell = { shell, terminal, resizes };
  const ended = (text) => {
    if (session?.shell?.shell !== shell) return;
    const restart = element("button", "btn", "New shell");
    restart.onclick = () => {
      closeShell();
      openShell(name);
    };
    $("shell-status").replaceChildren(text, restart);
  };
  shell.exited.then((code) => ended(`Shell exited with ${code}.`), (error) => ended(`Shell disconnected: ${error.message}.`));
}

function closeShell() {
  session.shell.shell.close();
  session.shell.terminal.dispose();
  session.shell.resizes.disconnect();
  session.shell = null;
}

function closeSession() {
  if (!session) return;
  session.screen.close();
  session.input.close();
  session.events.abort();
  closeShell();
  $("screen").blur();
  session = null;
}

// New machine

const sizes = element("div", "size-row");
sizes.append(...data.PRESETS.map((preset) => {
  const label = element("label", "size");
  const input = element("input");
  Object.assign(input, { type: "radio", name: "preset", value: preset.id, defaultChecked: preset.id === "standard" });
  const text = element("span");
  text.append(element("strong", "", preset.label), element("small", "", data.specs(preset).slice(0, 2).join(" · ")));
  label.append(input, text);
  return label;
}));
$("presets").append(sizes);

function renderSheet(state) {
  const sheet = $("sheet");
  if (state.creating && !sheet.open) {
    $("create").reset();
    sheet.showModal();
    $("create").elements.name.focus();
  }
  if (!state.creating && sheet.open) sheet.close();
}

$("sheet").addEventListener("close", () => {
  if ($("sheet").returnValue === "create") {
    const form = $("create").elements;
    data.create({
      preset: form.preset.value,
      name: form.name.value.trim(),
      persist: form.persist.checked,
      wait_network: form.wait_network.checked,
      base: form.base.value.trim(),
      disk_gib: form.disk_gib.value,
      size: form.size.value.trim(),
      env: form.env.value,
    });
  } else {
    data.setCreating(false);
  }
  $("sheet").returnValue = "";
});

for (const id of ["new", "blank-new"]) $(id).onclick = () => data.setCreating(true);

// Asks for the token until there is one, also after serve rejected it.
function renderLogin(state) {
  const login = $("login");
  if (!state.token && !login.open) {
    $("login-form").reset();
    login.showModal();
  }
  if (state.token && login.open) login.close();
}

$("login").addEventListener("cancel", (event) => event.preventDefault());
$("login").addEventListener("close", () => {
  const token = $("login-form").elements.token.value.trim();
  if (token) data.setToken(token);
  else renderLogin(data.state);
});

// Header and toasts

function renderHeader(state) {
  $("host").textContent = state.listError ? "Listing failed" : "Connected";
  $("host").title = state.listError ?? "";
  $("host").classList.toggle("down", Boolean(state.listError));
}

function renderToasts(state) {
  $("toasts").replaceChildren(...state.toasts.map((toast) => {
    const node = element("li", `toast ${toast.tone}`, toast.text);
    node.onclick = () => data.dismissToast(toast.id);
    return node;
  }));
}

// xterm measures its cell size once, so the font must be there before any terminal opens.
await document.fonts.load('13px "Geist Mono"');
for (const render of [renderLogin, renderHeader, renderOverview, renderMachine, renderSheet, renderToasts]) data.subscribe(render);
data.poll();
