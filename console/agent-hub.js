"use strict";

window.DevCoordinatorAgentHub = (() => {
  const mounts = new WeakMap();
  const steps = ["key", "connect", "rooms", "delivery"];
  const escape = (value) =>
    String(value ?? "").replace(
      /[&<>"']/g,
      (char) =>
        ({
          "&": "&amp;",
          "<": "&lt;",
          ">": "&gt;",
          '"': "&quot;",
          "'": "&#39;",
        })[char],
    );
  const t = (key, params) =>
    window.DevCoordinatorI18n.t(`agent-hub.${key}`, params);
  const h = (key, params) => escape(t(key, params));
  const icon = (name) =>
    `<span class="hub-icon hub-icon-${name}" aria-hidden="true">
</span>`;
  const date = (value) =>
    value && Number.isFinite(Date.parse(value))
      ? window.DevCoordinatorI18n.date(value, {
          dateStyle: "medium",
          timeStyle: "short",
        })
      : t("unavailable");
  const bytes = (value) =>
    `${window.DevCoordinatorI18n.number(Number(value) / 1024, { maximumFractionDigits: 1 })} KiB`;
  const button = (action, label, options = "") =>
    `<button type="button" class="hub-button" data-hub-action="${action}" ${options}>${h(label)}</button>`;

  async function mount(root, { api, administrator = false, signal }) {
    mounts.get(root)?.();
    let disposed = false;
    let timer;
    const urls = new Set();
    const initialQuery = new URLSearchParams(location.hash.split("?")[1] || "");
    const state = {
      step: steps.includes(
        new URLSearchParams(location.hash.split("?")[1] || "").get("step"),
      )
        ? new URLSearchParams(location.hash.split("?")[1]).get("step")
        : "key",
      settings: null,
      secret: "",
      overlap: "",
      publicOpen: false,
      client: initialQuery.get("space") === "public" ? "public" : "local",
      space: initialQuery.get("space") === "public" ? "public" : "private",
      agents: [],
      rooms: [],
      deliveries: [],
      agentNext: null,
      roomNext: null,
      messageNext: null,
      roomId: initialQuery.get("room") || "",
      revealMessage: "",
      drafts: new Map(),
      busy: false,
      loading: true,
      error: "",
      notice: "",
      updated: "",
      version: 0,
      loaded: { agents: false, rooms: false, deliveries: false },
      preview: null,
      activity: null,
      rotateConfirm: false,
      previewTrigger: null,
    };
    const alive = () =>
      !disposed && !signal?.aborted && mounts.get(root) === dispose;
    function dispose() {
      disposed = true;
      state.secret = "";
      state.drafts.clear();
      clearInterval(timer);
      closePreview();
      const previewDialog = root.querySelector(".hub-preview");
      if (previewDialog) {
        previewDialog.close();
        previewDialog.remove();
      }
      root.querySelector("#hub-key")?.replaceChildren();
      const secretInput = root.querySelector("#hub-key");
      if (secretInput) secretInput.value = "";
      for (const url of urls) URL.revokeObjectURL(url);
      urls.clear();
      root.removeEventListener("click", onClick);
      root.removeEventListener("change", onChange);
      root.removeEventListener("input", onInput);
      root.removeEventListener("submit", onSubmit);
    }
    mounts.set(root, dispose);
    signal?.addEventListener("abort", dispose, { once: true });
    await window.DevCoordinatorI18n.ensure("agent-hub");
    if (!alive()) return dispose;
    const request = async (operation, params = {}) => {
      if (!alive()) throw new DOMException("Aborted", "AbortError");
      const write =
        ["key.rotate", "settings.set", "console.send"].includes(operation) ||
        operation.startsWith("admin.");
      const result = await api(`agent.hub.${operation}`, params, !write);
      if (!alive()) throw new DOMException("Aborted", "AbortError");
      return result;
    };
    const draftKey = () => `${state.space}:${state.roomId}`;
    const currentRoom = () =>
      state.rooms.find((room) => room.room_id === state.roomId);
    const canReply = () =>
      state.space === "private" ||
      (state.settings?.public_enabled && !state.settings?.public_paused);
    const publicState = () =>
      !state.settings
        ? "unavailable"
        : !state.settings.public_enabled
          ? "publicDisabled"
          : state.settings.public_paused
            ? "publicPaused"
            : "publicEnabled";
    const fail = (error, key = "requestFailed") => {
      if (alive() && error?.name !== "AbortError") {
        state.error = t(key);
        render();
      }
    };
    const busy = async (fn) => {
      if (state.busy) return;
      state.busy = true;
      state.error = "";
      state.notice = "";
      render();
      try {
        await fn();
      } catch (error) {
        fail(error, "writeFailed");
      } finally {
        if (alive()) {
          state.busy = false;
          render();
        }
      }
    };
    const dataAttributes = (value) => escape(value);

    function navigate(step) {
      if (state.step === "key" && step !== "key") {
        state.secret = "";
        state.overlap = "";
        state.rotateConfirm = false;
      }
      state.step = step;
      updateRoute();
      render();
      root.querySelector("#hub-panel-title")?.focus();
    }
    function updateRoute() {
      const query = new URLSearchParams({
        step: state.step,
        space: state.space,
      });
      if (state.roomId) query.set("room", state.roomId);
      history.replaceState(history.state, "", `#/agent-hub?${query}`);
    }

    function render() {
      if (!alive()) return;
      const active = root.contains(document.activeElement)
        ? document.activeElement
        : null;
      const focusId = active?.id;
      const focusAction = active?.dataset.hubAction;
      const selection =
        active && typeof active.selectionStart === "number"
          ? [active.selectionStart, active.selectionEnd]
          : null;
      const scroll = { x: window.scrollX, y: window.scrollY };
      root.innerHTML = `<section class="hub-shell" data-agent-hub data-ui-region="agent-hub">
        <header class="hub-heading">
<h1>${h("title")}</h1>
<p>${h("intro")}</p>
</header>
        <div class="hub-layout">
<nav class="hub-steps" aria-label="${h("steps")}">${steps
        .map(
          (
            step,
            i,
          ) => `<button type="button" class="hub-step" data-hub-step="${step}" ${state.step === step ? 'aria-current="step"' : ""}>
<span class="hub-step-number">${i + 1}</span>
<span>
<strong>${h(`${step}Title`)}</strong>
<span class="hub-step-description">${h(`${step}Description`)}</span>
</span>
</button>`,
        )
        .join("")}</nav>
        <div class="hub-panel" aria-busy="${state.loading || state.busy}">
<div data-hub-feedback>${state.error ? `<div class="hub-error" role="alert">${escape(state.error)} ${button("refresh", "refresh")}</div>` : ""}${state.notice ? `<p class="hub-notice" role="status">${escape(state.notice)}</p>` : ""}</div>
        ${state.step === "key" ? keyPanel() : state.step === "connect" ? connectPanel() : roomsPanel()}
        ${state.preview ? previewPanel() : ""}</div>
</div>
</section>`;
      const keyInput = root.querySelector("#hub-key");
      if (keyInput) keyInput.value = state.secret;
      const reply = root.querySelector("#hub-reply");
      if (reply) reply.value = state.drafts.get(draftKey()) || "";
      if (state.busy)
        root
          .querySelectorAll(
            '[data-hub-action]:not([data-hub-action="dismiss-key"]):not([data-hub-action="close-preview"]), [data-hub-step], select, [type="submit"]',
          )
          .forEach((control) => {
            control.disabled = true;
          });
      const restored = focusId
        ? root.querySelector(`#${CSS.escape(focusId)}`)
        : focusAction
          ? root.querySelector(`[data-hub-action="${CSS.escape(focusAction)}"]`)
          : null;
      if (restored && !restored.disabled) {
        restored.focus({ preventScroll: true });
        if (selection && restored.setSelectionRange)
          restored.setSelectionRange(...selection);
      }
      if (scroll.y !== window.scrollY || scroll.x !== window.scrollX)
        window.scrollTo(scroll.x, scroll.y);
      const previewDialog = root.querySelector(".hub-preview");
      if (previewDialog) {
        previewDialog.showModal();
        previewDialog.addEventListener("cancel", (event) => {
          event.preventDefault();
          closePreview();
          render();
          restorePreviewFocus();
        });
      }
      if (
        state.preview?.kind === "pdf" &&
        !root.querySelector("[data-hub-pdf] canvas")
      )
        void renderPdf();
    }

    function keyPanel() {
      const settings = state.settings;
      return `<div class="hub-private">${icon("lock")}<span>${h("privateStatus")} <strong>${settings ? h("active") : h(state.loading ? "loading" : "unavailable")}</strong>
</span>
</div>
        <h2 id="hub-panel-title" tabindex="-1">${h("keyTitle")}</h2>
<p class="hub-lead">${h("keyIntro")}</p>
        ${
          state.secret
            ? `<section class="hub-secret" data-hub-secret>
<h3>${h("keyReveal")}</h3>
<label for="hub-key">${h("keyLabel")}</label>
<input id="hub-key" data-hub-key-value readonly autocomplete="off" spellcheck="false" type="text" data-private-value>
<p>${h("keyOnce")}</p>
<div class="hub-actions">${button("copy-key", "copyKey")}${button("dismiss-key", "dismissKey")}</div>${state.overlap ? `<p>${h("keyOverlap", { date: date(state.overlap) })}</p>` : ""}</section>`
            : administrator
              ? `<button type="button" class="hub-button hub-primary hub-key-button" data-hub-action="generate-key" ${!settings || state.loading ? "disabled" : ""}>${icon("lock")}${h(settings?.current_key_created_at ? "rotateKey" : "generateKey")}</button>
${rotationConfirmation()}<p class="hub-key-note">${icon("info-circle")}${h("keyOnce")}</p>${settings?.current_key_created_at ? `<p class="hub-muted">${h("keyCreated", { date: date(settings.current_key_created_at) })}</p>` : ""}`
              : `<p class="hub-note">${h("keyAdmin")}</p>`
        }
        <p class="hub-local-note">${h("keyLocal")} <button type="button" class="hub-link" data-hub-step="connect">${h("continue")}</button>
</p>
        <button type="button" class="hub-link hub-public-link" data-hub-action="public-settings" aria-expanded="${state.publicOpen}">${icon("world")}${h("publicSettings")}</button>
        ${state.publicOpen ? publicPanel() : ""}`;
    }

    function rotationConfirmation() {
      return state.rotateConfirm
        ? `<section class="hub-confirm" role="group" aria-label="${h("rotateKey")}"><p>${h("rotateConfirm")}</p><div class="hub-actions">${button("confirm-rotate", "rotateKey")}${button("cancel-rotate", "cancel")}</div></section>`
        : "";
    }

    function publicPanel() {
      return `<section class="hub-settings">
<h3 id="hub-public-title" tabindex="-1">${h("publicTitle")}</h3>
<p>${h("publicDescription")}</p>
<p class="hub-status">${h(publicState())}</p>${administrator ? `<div class="hub-actions">${!state.settings?.public_enabled ? button("enable", "enable", !state.settings ? "disabled" : "") : `${button(state.settings.public_paused ? "resume" : "pause", state.settings.public_paused ? "resume" : "pause")}${button("disable", "disable")}`}</div>` : `<p class="hub-muted">${h("publicAdmin")}</p>`}</section>`;
    }

    function connection() {
      const space = state.client === "public" ? "public" : "private";
      const auth = { agent_id: "<agent-id>", token: "<registration-token>" };
      const register = {
        space,
        display_name: "Coding agent",
        repository_label: "Project",
        capabilities: ["coding"],
      };
      const heartbeat = { auth, current_subject: "Current task" };
      const directory = { space, limit: 50 };
      const room = {
        auth,
        space,
        name: "Task handoff",
        members: ["<peer-agent-id>"],
      };
      const send = {
        auth,
        room_id: "<room-id>",
        body: "Please review the task and reply here.",
        client_message_id: "<unique-message-id>",
        attachments: [],
      };
      const receive = { auth, limit: 50 };
      const ack = { auth, delivery_ids: ["<delivery-id>"] };
      const local = state.client === "local";
      const command = (kind, params) =>
        local
          ? `devcoordinator2 call agent.hub.${kind.replaceAll("_", ".")} --params '${JSON.stringify(params)}'`
          : JSON.stringify({ action: { kind, ...params } }, null, 2);
      return {
        endpoint: `${location.origin}/.well-known/devcoordinator2/agent-hub`,
        register: command("register", register),
        heartbeat: command("heartbeat", heartbeat),
        directory: command("directory", directory),
        room: command("room_create", room),
        send: command("send", send),
        receive: command("receive", receive),
        ack: command("ack", ack),
      };
    }
    const snippet = (heading, text) =>
      `<section class="hub-instruction">
<h3>${h(heading)}</h3>
<pre tabindex="0">
<code>${escape(text)}</code>
</pre>
</section>`;
    function connectPanel() {
      const info = connection();
      return `<h2 id="hub-panel-title" tabindex="-1">${h("connectTitle")}</h2>
<p class="hub-lead">${h("connectIntro")}</p>
<label class="hub-field" for="hub-client">${h("client")}<select id="hub-client">${["local", "remote", "public"].map((value) => `<option value="${value}"${state.client === value ? " selected" : ""}>${h(`${value}Client`)}</option>`).join("")}</select>
</label>
<p>${h(`${state.client}Help`)}</p>
      ${
        state.client !== "local"
          ? `<div class="hub-endpoint">
<strong>${h("endpoint")}</strong>
<code>${escape(info.endpoint)}</code>
<code>POST · Content-Type: application/json</code>${state.client === "remote" ? "<code>Authorization: Bearer &lt;instance-key&gt;</code>" : ""}</div>`
          : ""
      }
      ${state.client === "public" && (!state.settings?.public_enabled || state.settings?.public_paused) ? `<p class="hub-note">${h("publicUnavailable", { state: t(publicState()) })}</p>` : ""}
      ${snippet("registration", info.register)}${snippet("heartbeat", info.heartbeat)}<p class="hub-muted">${h("leaseHelp")}</p>
      <div class="hub-actions">${button("copy-instructions", "copyInstructions")}<button type="button" class="hub-button hub-primary" data-hub-step="rooms">${h("viewRooms")}</button>
</div>
      <details class="hub-details">
<summary>${h("commands")}</summary>${snippet("discoverCommand", info.directory)}${snippet("roomCommand", info.room)}${snippet("sendCommand", info.send)}${snippet("pollCommand", info.receive)}${snippet("ackCommand", info.ack)}<p>${h("deliveryHelp")}</p>
<p>${h("callbackHelp")}</p>
</details>${directoryPanel()}`;
    }

    function toolbar() {
      return `<div class="hub-toolbar">
<label for="hub-space">${h("space")}<select id="hub-space">
<option value="private"${state.space === "private" ? " selected" : ""}>${h("private")}</option>
<option value="public"${state.space === "public" ? " selected" : ""}>${h("public")}</option>
</select>
</label>${button("refresh", state.loading ? "refreshing" : "refresh")}<span class="hub-muted" role="status">${state.updated ? h("updated", { date: date(state.updated) }) : h("loading")}</span>
</div>`;
    }
    function directoryPanel() {
      return `<section class="hub-directory">
<h3>${h("directory")}</h3>${state.step === "connect" ? toolbar() : ""}${
        !state.loaded.agents
          ? `<p class="hub-muted">${h(state.loading ? "loading" : "unavailable")}</p>`
          : !state.agents.length
            ? `<p class="hub-empty">${h("noAgents")}</p>`
            : `<ul class="hub-agents">${state.agents
                .map((agent) => {
                  const active =
                    agent.state === "active" &&
                    Date.parse(agent.lease_expires_at) > Date.now();
                  return `<li data-hub-agent="${dataAttributes(agent.agent_id)}">
<div class="hub-agent-heading">
<strong>${escape(agent.display_name)}</strong>
<span class="hub-presence${active ? " is-active" : ""}">${h(active ? "active" : agent.state === "expired" || agent.state === "active" ? "expired" : "inactive")}</span>
</div>
<p class="hub-muted">${escape(agent.handle)}${agent.repository_label ? ` · ${escape(agent.repository_label)}` : ""}${agent.domain_label ? ` · ${escape(agent.domain_label)}` : ""}</p>${agent.current_subject ? `<p>${escape(agent.current_subject)}</p>` : ""}${agent.capabilities?.length ? `<p class="hub-muted">${agent.capabilities.map(escape).join(" · ")}</p>` : ""}<p class="hub-muted">${h("leaseExpires", { date: date(agent.lease_expires_at) })}</p>${agent.callback_configured ? `<p class="hub-muted">${h("callbackConfigured")}</p>` : ""}${administrator ? button("remove-agent", "removeAgent", `data-id="${dataAttributes(agent.agent_id)}"`) : ""}</li>`;
                })
                .join("")}</ul>`
      }${state.agentNext ? button("load-agents", "moreAgents") : ""}</section>`;
    }

    function roomsPanel() {
      const room = currentRoom();
      return `<h2 id="hub-panel-title" tabindex="-1">${h(`${state.step}Title`)}</h2>${
        state.step === "rooms"
          ? `<p class="hub-lead">${h("roomHelp")}</p>
<details class="hub-details">
<summary>${h("createRoom")}</summary>${snippet("discoverCommand", connection().directory)}${snippet("roomCommand", connection().room)}${snippet("sendCommand", connection().send)}</details>`
          : `<p class="hub-lead">${h("receiptHelp")}</p>
<details class="hub-details">
<summary>${h("commands")}</summary>${snippet("pollCommand", connection().receive)}${snippet("ackCommand", connection().ack)}<p>${h("deliveryHelp")}</p>
<p>${h("callbackHelp")}</p>
</details>`
      }
      ${toolbar()}${
        state.step === "delivery" && administrator && state.activity
          ? `<section class="hub-activity">
<h3>${h("activityTitle")}</h3>
<p>${h("activitySummary", state.activity)}</p>
<p>${h("backlogSummary", state.activity)}</p>
</section>`
          : ""
      }<div class="hub-conversations">
<section class="hub-room-list" aria-label="${h("rooms")}">
<h3>${h("rooms")}</h3>${
        !state.loaded.rooms
          ? `<p class="hub-muted">${h(state.loading ? "loading" : "unavailable")}</p>`
          : !state.rooms.length
            ? `<p class="hub-empty">${h("noRooms")}</p>`
            : state.rooms
                .map(
                  (
                    item,
                  ) => `<button type="button" class="hub-room" data-hub-room="${dataAttributes(item.room_id)}" data-hub-action="select-room" data-id="${dataAttributes(item.room_id)}" ${state.roomId === item.room_id ? 'aria-current="true"' : ""}>
<strong>${escape(item.name)}</strong>
<span>${h("roomMembers", { count: item.member_count })}</span>
<span class="hub-id">${escape(item.room_id)}</span>
</button>`,
                )
                .join("")
      }${state.roomNext ? button("load-rooms", "moreRooms") : ""}</section>
      <section class="hub-conversation" aria-label="${h("messages")}">${
        room
          ? `<header class="hub-room-heading">
<h3>${escape(room.name)}</h3>${administrator ? button("remove-room", "removeRoom", `data-id="${dataAttributes(room.room_id)}"`) : ""}</header>${messageList()}<form data-hub-reply>
<label for="hub-reply">${h("reply")}</label>
<textarea id="hub-reply" name="body" maxlength="32768" rows="4" placeholder="${h("replyPlaceholder")}" required>
</textarea>
${canReply() ? "" : `<p class="hub-note">${h("publicUnavailable", { state: t(publicState()) })}</p>`}
<div class="hub-actions">
<button class="hub-button hub-primary" type="submit" data-hub-action="send" ${canReply() ? "" : "disabled"}>${h("send")}</button>
</div>
</form>`
          : `<p class="hub-empty">${h("chooseRoom")}</p>`
      }</section>
</div>${
        state.step === "rooms"
          ? `<div class="hub-actions">
<button type="button" class="hub-link" data-hub-step="delivery">${h("watchDelivery")}</button>
</div>`
          : ""
      }${directoryPanel()}`;
    }

    function messageList() {
      return `<div class="hub-messages">${
        !state.loaded.deliveries
          ? `<p class="hub-muted">${h(state.loading ? "loading" : "unavailable")}</p>`
          : !state.deliveries.length
            ? `<p class="hub-empty">${h("noMessages")}</p>`
            : state.deliveries
                .map((delivery) => {
                  const message = delivery.message;
                  const counts = delivery.receipt_counts;
                  return `<article class="hub-message" data-hub-message="${dataAttributes(message.message_id)}" data-hub-delivery="${dataAttributes(message.message_id)}">
<header>
<strong>${escape(message.sender?.display_name || t("consoleSender"))}</strong>
<time datetime="${escape(message.created_at)}">${escape(date(message.created_at))}</time>
</header>
<p class="hub-message-body">${escape(message.body)}</p>${
                    message.attachments?.length
                      ? `<ul class="hub-files">${message.attachments
                          .map(
                            (file) => `<li>
<div>
<strong>${escape(file.name)}</strong>
<span class="hub-muted">${escape(bytes(file.byte_size))}</span>
</div>
<div class="hub-actions">${previewKind(file) ? button("preview", "preview", `data-id="${dataAttributes(file.attachment_id)}"`) : ""}${button("download", "download", `data-id="${dataAttributes(file.attachment_id)}"`)}</div>
</li>`,
                          )
                          .join("")}</ul>`
                      : ""
                  }<footer>
<span class="hub-muted">${counts ? (counts.recipients ? h("receipts", counts) : h("noRecipients")) : h("receiptsUnavailable")}</span>${administrator ? button("remove-message", "removeMessage", `data-id="${dataAttributes(message.message_id)}"`) : ""}</footer>
</article>`;
                })
                .join("")
      }</div>${state.messageNext ? button("load-messages", "moreMessages") : ""}`;
    }

    async function loadCollection(kind, more = false, version = state.version) {
      const config = {
        agents: ["console.directory", "agents", "agentNext"],
        rooms: ["console.rooms", "rooms", "roomNext"],
        deliveries: ["console.receive", "deliveries", "messageNext"],
      }[kind];
      if (kind === "deliveries" && !state.roomId) return;
      const params = {
        space: state.space,
        limit: 50,
        ...(more && state[config[2]] ? { after: state[config[2]] } : {}),
        ...(kind === "deliveries" ? { room_id: state.roomId } : {}),
      };
      const targetLength = more ? 0 : Math.max(state[kind].length, 1);
      const values = [];
      const seen = new Set();
      let next = null;
      do {
        const result = await request(config[0], params);
        if (version !== state.version) return;
        const page = result[config[1]];
        if (!Array.isArray(page)) throw new Error("Invalid collection");
        if (kind === "agents" && !administrator)
          state.settings = {
            public_enabled: result.public_enabled,
            public_paused: result.public_paused,
          };
        values.push(...page);
        next = page.length ? result.next : null;
        if (!next || more || seen.has(next)) break;
        seen.add(next);
        params.after = next;
        const selectedMissing =
          kind === "rooms" &&
          state.roomId &&
          !values.some((room) => room.room_id === state.roomId);
        const messageMissing =
          kind === "deliveries" &&
          state.revealMessage &&
          !values.some(
            (delivery) => delivery.message.message_id === state.revealMessage,
          );
        if (
          values.length >= targetLength &&
          !selectedMissing &&
          !messageMissing
        )
          break;
      } while (alive());
      state[kind] = more ? [...state[kind], ...values] : values;
      state[config[2]] = next;
      state.loaded[kind] = true;
    }
    async function refresh() {
      const version = ++state.version;
      state.loading = true;
      state.error = "";
      render();
      const results = await Promise.allSettled([
        ...(administrator
          ? [
              request("settings.get").then((value) => {
                if (version === state.version) state.settings = value;
              }),
              request("activity").then((value) => {
                if (version === state.version) state.activity = value;
              }),
            ]
          : []),
        loadCollection("agents", false, version),
        loadCollection("rooms", false, version),
        loadCollection("deliveries", false, version),
      ]);
      if (!alive() || version !== state.version) return;
      state.loading = false;
      if (results.some((result) => result.status === "rejected"))
        state.error = t("readError");
      else state.updated = new Date().toISOString();
      if (
        state.roomId &&
        state.loaded.rooms &&
        !state.rooms.some((room) => room.room_id === state.roomId)
      ) {
        state.roomId = "";
        state.deliveries = [];
        state.loaded.deliveries = false;
      }
      render();
    }

    async function copy(text) {
      try {
        await navigator.clipboard.writeText(text);
        if (alive()) {
          state.notice = t("copied");
          render();
        }
      } catch {
        if (alive()) {
          state.error = t("copyFailed");
          render();
        }
      }
    }
    function onInput(event) {
      if (event.target.id === "hub-reply")
        state.drafts.set(draftKey(), event.target.value);
    }
    function onChange(event) {
      if (event.target.id === "hub-client") {
        state.client = event.target.value;
        render();
      }
      if (event.target.id === "hub-space") {
        state.space = event.target.value;
        state.roomId = "";
        state.agents = [];
        state.rooms = [];
        state.deliveries = [];
        state.agentNext = null;
        state.roomNext = null;
        state.messageNext = null;
        state.loaded = { agents: false, rooms: false, deliveries: false };
        state.client = state.space === "public" ? "public" : "local";
        updateRoute();
        void refresh();
      }
    }
    function onSubmit(event) {
      if (!event.target.matches("[data-hub-reply]")) return;
      event.preventDefault();
      if (!canReply()) return;
      const body = state.drafts.get(draftKey()) || "";
      if (!body.trim() || !currentRoom()) return;
      if (new TextEncoder().encode(body).length > 32768) {
        state.error = t("replyTooLong");
        render();
        return;
      }
      const sentKey = draftKey();
      const roomId = state.roomId;
      const space = state.space;
      void busy(async () => {
        const sent = await request("console.send", {
          space,
          room_id: roomId,
          body,
          attachments: [],
        });
        if (state.drafts.get(sentKey) === body) state.drafts.delete(sentKey);
        state.revealMessage = sent.message_id;
        await refresh();
        if (
          state.deliveries.some(
            (delivery) => delivery.message.message_id === sent.message_id,
          )
        )
          state.notice = t("sent");
        else state.error = t("replyReadFailed");
        state.revealMessage = "";
      });
    }
    function onClick(event) {
      const step = event.target.closest("[data-hub-step]");
      if (step && root.contains(step) && !state.busy) {
        navigate(step.dataset.hubStep);
        return;
      }
      const target = event.target.closest("[data-hub-action]");
      if (!target || !root.contains(target) || target.disabled) return;
      const action = target.dataset.hubAction;
      const id = target.dataset.id;
      if (action === "send") return;
      if (action === "public-settings") {
        state.publicOpen = !state.publicOpen;
        render();
        if (state.publicOpen)
          root
            .querySelector("#hub-public-title")
            ?.focus({ preventScroll: true });
        return;
      }
      if (action === "dismiss-key") {
        state.secret = "";
        state.overlap = "";
        navigate("connect");
        return;
      }
      if (action === "copy-key") {
        void copy(state.secret);
        return;
      }
      if (action === "copy-instructions") {
        const info = connection();
        void copy(
          [
            t(`${state.client}Help`),
            state.client === "local"
              ? ""
              : `POST ${info.endpoint}\nContent-Type: application/json${state.client === "remote" ? "\nAuthorization: Bearer <instance-key>" : ""}`,
            info.register,
            t("leaseHelp"),
            info.heartbeat,
            t("roomHelp"),
            info.directory,
            info.room,
            info.send,
            info.receive,
            info.ack,
            t("deliveryHelp"),
            t("callbackHelp"),
          ]
            .filter(Boolean)
            .join("\n\n"),
        );
        return;
      }
      if (action === "refresh") {
        void refresh();
        return;
      }
      if (action.startsWith("load-")) {
        const kind = {
          "load-agents": "agents",
          "load-rooms": "rooms",
          "load-messages": "deliveries",
        }[action];
        void busy(() => loadCollection(kind, true));
        return;
      }
      if (action === "select-room") {
        state.roomId = id;
        state.deliveries = [];
        state.messageNext = null;
        state.loaded.deliveries = false;
        ++state.version;
        updateRoute();
        void busy(() => loadCollection("deliveries"));
        return;
      }
      if (action === "close-preview") {
        closePreview();
        render();
        restorePreviewFocus();
        return;
      }
      if (action === "download" || action === "preview") {
        state.previewTrigger = { action, id };
        void busy(() => fileAction(id, action));
        return;
      }
      if (!administrator) return;
      if (action === "cancel-rotate") {
        state.rotateConfirm = false;
        render();
        root.querySelector('[data-hub-action="generate-key"]')?.focus();
        return;
      }
      if (action === "generate-key" || action === "confirm-rotate") {
        if (!state.settings) return;
        if (
          action === "generate-key" &&
          state.settings.current_key_created_at
        ) {
          state.rotateConfirm = true;
          render();
          root.querySelector('[data-hub-action="confirm-rotate"]')?.focus();
          return;
        }
        state.rotateConfirm = false;
        const hadKey = Boolean(state.settings.current_key_created_at);
        void busy(async () => {
          const result = await request("key.rotate", {
            expected_revision: state.settings.revision,
          });
          state.secret = result.bearer_key;
          state.overlap = hadKey ? result.previous_key_expires_at : "";
          await refresh();
          root.querySelector("#hub-key")?.focus({ preventScroll: true });
        });
        return;
      }
      if (["enable", "pause", "resume", "disable"].includes(action)) {
        if (
          !state.settings ||
          (action !== "resume" && !window.confirm(t(`${action}Confirm`)))
        )
          return;
        void busy(async () => {
          await request("settings.set", {
            expected_revision: state.settings.revision,
            public_enabled: action !== "disable",
            public_paused: action === "pause",
          });
          await refresh();
        });
        return;
      }
      if (action.startsWith("remove-")) {
        const kind = action.slice(7);
        const room = currentRoom();
        const item =
          kind === "agent"
            ? state.agents.find((agent) => agent.agent_id === id)
            : kind === "room"
              ? state.rooms.find((value) => value.room_id === id)
              : state.deliveries.find(
                  (value) => value.message.message_id === id,
                )?.message;
        if (!item) return;
        const name =
          kind === "agent"
            ? item.display_name
            : kind === "room"
              ? item.name
              : room?.name || item.room_id;
        if (
          !window.confirm(
            t(`remove${kind[0].toUpperCase()}${kind.slice(1)}Confirm`, {
              id,
              name,
              space: t(state.space),
            }),
          )
        )
          return;
        void busy(async () => {
          await request(`admin.${kind}.remove`, { [`${kind}_id`]: id });
          if (kind === "room" && state.roomId === id) {
            state.drafts.delete(draftKey());
            state.roomId = "";
          }
          state.notice = t("removed");
          await refresh();
        });
      }
    }

    function previewKind(file) {
      const type = String(file.content_type || "")
        .split(";")[0]
        .trim()
        .toLowerCase();
      return [
        "image/png",
        "image/jpeg",
        "image/gif",
        "image/webp",
        "image/avif",
      ].includes(type)
        ? "image"
        : type === "text/plain"
          ? "text"
          : type === "application/pdf"
            ? "pdf"
            : null;
    }
    function restorePreviewFocus() {
      const trigger = state.previewTrigger;
      if (trigger)
        root
          .querySelector(
            `[data-hub-action="${CSS.escape(trigger.action)}"][data-id="${CSS.escape(trigger.id)}"]`,
          )
          ?.focus({ preventScroll: true });
    }
    function closePreview() {
      const task = state.preview?.task;
      if (task) {
        state.preview.task = null;
        void task.destroy();
      }
      if (state.preview?.url) {
        URL.revokeObjectURL(state.preview.url);
        urls.delete(state.preview.url);
      }
      state.preview = null;
    }
    async function fileAction(id, action) {
      const file = state.deliveries
        .flatMap((value) => value.message.attachments || [])
        .find((value) => value.attachment_id === id);
      if (!file) return;
      if (
        !Number.isSafeInteger(file.byte_size) ||
        file.byte_size < 0 ||
        file.byte_size > 16 * 1024 * 1024
      )
        throw new Error(t("fileTooLarge"));
      const kind = previewKind(file);
      if (
        action === "preview" &&
        (!kind || (kind === "text" && file.byte_size > 1024 * 1024))
      ) {
        state.error = t(kind ? "previewLimit" : "previewUnavailable");
        return;
      }
      const roomId = state.roomId;
      const space = state.space;
      let offset = 0;
      const chunks = [];
      do {
        const chunk = await request("console.file", {
          space,
          room_id: roomId,
          file_id: id,
          offset,
        });
        const data = Uint8Array.from(atob(chunk.data_base64), (char) =>
          char.charCodeAt(0),
        );
        if (
          chunk.offset !== offset ||
          chunk.attachment.attachment_id !== id ||
          data.length > 32768 ||
          chunk.attachment.sha256 !== file.sha256 ||
          chunk.attachment.byte_size !== file.byte_size ||
          offset + data.length > file.byte_size
        )
          throw new Error(t("fileInvalid"));
        chunks.push(data);
        offset += data.length;
        if (chunk.next_offset == null) break;
        if (chunk.next_offset !== offset || !data.length)
          throw new Error(t("fileInvalid"));
      } while (offset < file.byte_size);
      if (offset !== file.byte_size) throw new Error(t("fileInvalid"));
      const content = new Uint8Array(offset);
      let position = 0;
      for (const chunk of chunks) {
        content.set(chunk, position);
        position += chunk.length;
      }
      const digest = [
        ...new Uint8Array(await crypto.subtle.digest("SHA-256", content)),
      ]
        .map((value) => value.toString(16).padStart(2, "0"))
        .join("");
      if (digest !== file.sha256.toLowerCase() || !alive())
        throw new Error(t("fileInvalid"));
      const blob = new Blob([content], {
        type:
          action === "download"
            ? "application/octet-stream"
            : file.content_type,
      });
      const url = URL.createObjectURL(blob);
      urls.add(url);
      if (action === "download") {
        const link = document.createElement("a");
        link.href = url;
        link.download = file.name;
        root.append(link);
        link.click();
        link.remove();
        setTimeout(() => {
          URL.revokeObjectURL(url);
          urls.delete(url);
        }, 30000);
      } else {
        closePreview();
        state.preview = {
          kind,
          name: file.name,
          url,
          content,
          text: kind === "text" ? new TextDecoder().decode(content) : "",
        };
      }
    }
    function previewPanel() {
      const preview = state.preview;
      return `<dialog class="hub-preview" aria-label="${escape(preview.name)}">
<header>
<h3>${escape(preview.name)}</h3>${button("close-preview", "closePreview")}</header>${
        preview.kind === "image"
          ? `<img src="${escape(preview.url)}" alt="${escape(preview.name)}">`
          : preview.kind === "text"
            ? `<pre>${escape(preview.text)}</pre>`
            : `<p class="hub-muted">${h("firstPagePreview")}</p>
<div data-hub-pdf>
</div>`
      }</dialog>`;
    }
    async function renderPdf() {
      const preview = state.preview;
      const host = root.querySelector("[data-hub-pdf]");
      if (!preview || !host || host.dataset.loading) return;
      host.dataset.loading = "true";
      host.textContent = t("fileLoading");
      let task;
      try {
        const pdfjs = await import("/vendor/pdfjs/pdf.min.mjs");
        if (!alive() || state.preview !== preview || !host.isConnected) return;
        pdfjs.GlobalWorkerOptions.workerSrc =
          "/vendor/pdfjs/pdf.worker.min.mjs";
        task = pdfjs.getDocument({
          data: preview.content.slice(),
          cMapUrl: "/vendor/pdfjs/cmaps/",
          cMapPacked: true,
          standardFontDataUrl: "/vendor/pdfjs/standard_fonts/",
          wasmUrl: "/vendor/pdfjs/wasm/",
          isEvalSupported: false,
          enableXfa: false,
        });
        preview.task = task;
        const pdf = await task.promise;
        if (!alive() || state.preview !== preview || !host.isConnected) return;
        // Rasterize the first page without links, forms, scripts, or document markup.
        const page = await pdf.getPage(1);
        const viewport = page.getViewport({ scale: 1 });
        const canvas = window.document.createElement("canvas");
        canvas.setAttribute("role", "img");
        canvas.setAttribute("aria-label", preview.name);
        canvas.width = Math.min(viewport.width, 1600);
        canvas.height = Math.min(viewport.height, 2200);
        await page.render({
          canvasContext: canvas.getContext("2d"),
          annotationMode: pdfjs.AnnotationMode.DISABLE,
          viewport: page.getViewport({
            scale: Math.min(
              canvas.width / viewport.width,
              canvas.height / viewport.height,
            ),
          }),
        }).promise;
        if (alive() && state.preview === preview && host.isConnected)
          host.replaceChildren(canvas);
      } catch {
        if (alive() && state.preview === preview && host.isConnected)
          host.textContent = t("previewUnavailable");
      } finally {
        if (task && preview.task === task) {
          preview.task = null;
          await task.destroy();
        }
      }
    }
    root.addEventListener("click", onClick);
    root.addEventListener("change", onChange);
    root.addEventListener("input", onInput);
    root.addEventListener("submit", onSubmit);
    render();
    await refresh();
    // Refresh is explicit so an operator reading a long conversation keeps its
    // paginated position and draft. Repaint presence against actual lease time.
    timer = setInterval(() => {
      if (
        alive() &&
        !state.busy &&
        !state.loading &&
        !root.contains(document.activeElement)
      )
        render();
    }, 30000);
    return dispose;
  }
  return { mount };
})();
