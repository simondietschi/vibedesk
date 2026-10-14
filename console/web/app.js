"use strict";

const $ = (sel, root = document) => root.querySelector(sel);
const $$ = (sel, root = document) => [...root.querySelectorAll(sel)];

let me = null;
let devices = [];

// ------------------------------------------------------------------ Hilfen

class ApiError extends Error {
  constructor(status, body) {
    super(body.error || `Fehler ${status}`);
    this.status = status;
    this.body = body;
  }
}

async function api(method, path, body) {
  const opts = { method, headers: { "X-VibeDesk": "1" }, credentials: "same-origin" };
  if (body !== undefined) {
    opts.headers["Content-Type"] = "application/json";
    opts.body = JSON.stringify(body);
  }
  const res = await fetch("/admin/api" + path, opts);
  const text = await res.text();
  const data = text ? JSON.parse(text) : {};
  if (!res.ok) {
    if (res.status === 401 && path !== "/login" && !data.needs_code) {
      showLogin();
    }
    throw new ApiError(res.status, data);
  }
  return data;
}

function h(tag, attrs = {}, ...children) {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (v === false || v === null || v === undefined) continue;
    if (k.startsWith("on")) el.addEventListener(k.slice(2), v);
    else if (k === "class") el.className = v;
    else el.setAttribute(k, v === true ? "" : v);
  }
  for (const c of children.flat()) {
    if (c === null || c === undefined || c === false) continue;
    el.append(c instanceof Node ? c : document.createTextNode(String(c)));
  }
  return el;
}

const dateFmt = new Intl.DateTimeFormat("de-CH", { dateStyle: "medium", timeStyle: "short" });

function formatDate(ts) {
  return ts ? dateFmt.format(new Date(ts * 1000)) : "–";
}

function relative(ts) {
  if (!ts) return "nie";
  const secs = Math.max(0, Date.now() / 1000 - ts);
  if (secs < 60) return "gerade eben";
  if (secs < 3600) return `vor ${Math.floor(secs / 60)} Min.`;
  if (secs < 86400) return `vor ${Math.floor(secs / 3600)} Std.`;
  if (secs < 86400 * 30) return `vor ${Math.floor(secs / 86400)} Tagen`;
  return formatDate(ts);
}

function formError(form, message) {
  const el = $(".error", form);
  if (el) el.textContent = message || "";
}

function formValues(form) {
  return Object.fromEntries(new FormData(form).entries());
}

// ------------------------------------------------------------------ Dialog

/**
 * Zeigt einen Dialog. `fields` sind Eingabefelder, `onOk` erhält deren Werte
 * und darf eine Exception werfen, dann bleibt der Dialog mit der Meldung offen.
 */
function dialog({ title, text, fields = [], okLabel = "OK", danger = false, cancel = true, onOk }) {
  const dlg = $("#dialog");
  const form = $("#dialog-form");
  $("#dialog-title").textContent = title;
  const body = $("#dialog-body");
  body.replaceChildren();
  if (text) body.append(...[].concat(text).map((t) => (t instanceof Node ? t : h("p", {}, t))));
  for (const f of fields) {
    let input;
    if (f.options) {
      input = h("select", { name: f.name },
        f.options.map(([value, label]) => h("option", { value, selected: value === f.value }, label)));
    } else if (f.type === "textarea") {
      input = h("textarea", { name: f.name, rows: 4, maxlength: f.maxlength });
      input.value = f.value || "";
    } else {
      input = h("input", {
        name: f.name, type: f.type || "text", required: f.required,
        minlength: f.minlength, autocomplete: f.autocomplete || "off", placeholder: f.placeholder,
      });
      input.value = f.value || "";
    }
    body.append(h("label", {}, f.label, input));
  }
  formError(form, "");
  const ok = $("#dialog-ok");
  ok.textContent = okLabel;
  ok.className = danger ? "danger" : "primary";
  $("#dialog-cancel").hidden = !cancel;

  return new Promise((resolve) => {
    const onSubmit = async (ev) => {
      if (ev.submitter && ev.submitter.value === "cancel") return;
      ev.preventDefault();
      if (!form.reportValidity()) return;
      try {
        ok.disabled = true;
        const values = formValues(form);
        if (onOk) await onOk(values);
        cleanup();
        dlg.close();
        resolve(values);
      } catch (e) {
        formError(form, e.message);
      } finally {
        ok.disabled = false;
      }
    };
    const onClose = () => {
      cleanup();
      resolve(null);
    };
    const cleanup = () => {
      form.removeEventListener("submit", onSubmit);
      dlg.removeEventListener("close", onClose);
    };
    form.addEventListener("submit", onSubmit);
    dlg.addEventListener("close", onClose);
    dlg.showModal();
    const first = $("input, select, textarea", body);
    if (first) first.focus();
  });
}

function showPassword(username, password) {
  return dialog({
    title: "Neues Passwort",
    text: [
      `Das Passwort für «${username}» wird nur jetzt angezeigt. Bitte sicher weitergeben.`,
      h("p", {}, h("code", { class: "secret" }, password)),
    ],
    cancel: false,
  });
}

// ------------------------------------------------------------- Navigation

function showLogin() {
  me = null;
  $("#app").hidden = true;
  $("#login").hidden = false;
  $("#login-code").hidden = true;
  $("#login-form").reset();
  $('#login-form [name="username"]').focus();
}

async function showApp() {
  me = await api("GET", "/me");
  $("#login").hidden = true;
  $("#app").hidden = false;
  $(".whoami").textContent = me.display_name || me.username;
  for (const el of $$("[data-admin]")) el.hidden = me.role !== "admin";
  route();
}

function route() {
  if (!me) return;
  const views = { "#geraete": "devices", "#benutzer": "users", "#konto": "account" };
  let view = views[location.hash] || (me.role === "admin" ? "devices" : "account");
  if (me.role !== "admin" && view !== "account") view = "account";
  for (const v of $$(".view")) v.hidden = v.id !== "view-" + view;
  for (const a of $$("nav a")) a.classList.toggle("active", a.dataset.view === view);
  ({ devices: loadDevices, users: loadUsers, account: loadAccount })[view]();
}

// ----------------------------------------------------------------- Geräte

async function loadDevices() {
  const [list, status] = await Promise.all([api("GET", "/devices"), api("GET", "/status")]);
  devices = list;
  const hint = $("#device-hint");
  hint.hidden = status.hbbs;
  hint.textContent = status.hbbs ? "" :
    "Die Konsole kennt die hbbs-Datenbank nicht (VIBEDESK_HBBS_DB). Es erscheinen nur Geräte, die einen Heartbeat senden.";
  renderDevices();
}

function renderDevices() {
  const q = $("#device-filter").value.trim().toLowerCase();
  const rows = devices
    .filter((d) => !q || [d.id, d.hostname, d.username, d.note, d.os, d.ip]
      .some((v) => (v || "").toLowerCase().includes(q)))
    .sort((a, b) => (b.online - a.online) || ((b.last_seen_at || 0) - (a.last_seen_at || 0)) || a.id.localeCompare(b.id));
  const online = devices.filter((d) => d.online).length;
  $("#device-summary").textContent = `${online} von ${devices.length} online`;
  $("#device-rows").replaceChildren(...rows.map(deviceRow));
  if (!rows.length) {
    $("#device-rows").append(h("tr", {}, h("td", { colspan: 9, class: "muted center" },
      devices.length ? "Keine Treffer" : "Noch keine Geräte")));
  }
}

function deviceRow(d) {
  const status = d.online ? h("span", { class: "dot on" }, "online")
    : h("span", { class: "dot" }, "offline");
  const title = [d.cpu, d.memory, d.ip && `IP ${d.ip}`, d.registered_at && `registriert ${formatDate(d.registered_at)}`]
    .filter(Boolean).join(" · ");
  return h("tr", {},
    h("td", {}, status),
    h("td", { class: "mono" }, d.id),
    h("td", { title }, d.hostname || h("span", { class: "muted" }, "unbekannt")),
    h("td", {}, d.username),
    h("td", { class: "clip", title: d.os }, d.os),
    h("td", {}, d.version),
    h("td", { title: formatDate(d.last_seen_at) }, relative(d.last_seen_at)),
    h("td", { class: "clip note", title: d.note }, d.note),
    h("td", { class: "row-actions" },
      h("button", { class: "link", onclick: () => editNote(d) }, "Notiz"),
      d.registered === true ? null :
        h("button", { class: "link danger", onclick: () => removeDevice(d) }, "Entfernen")));
}

async function editNote(d) {
  await dialog({
    title: `Notiz zu ${d.hostname || d.id}`,
    fields: [{ name: "note", label: "Notiz", type: "textarea", value: d.note, maxlength: 1000 }],
    okLabel: "Speichern",
    onOk: (v) => api("PATCH", `/devices/${encodeURIComponent(d.id)}`, { note: v.note }),
  }) && loadDevices();
}

async function removeDevice(d) {
  await dialog({
    title: "Gerät entfernen",
    text: `${d.hostname || d.id} aus der Liste entfernen? Meldet es sich wieder, erscheint es erneut.`,
    okLabel: "Entfernen",
    danger: true,
    onOk: () => api("DELETE", `/devices/${encodeURIComponent(d.id)}`),
  }) && loadDevices();
}

// --------------------------------------------------------------- Benutzer

async function loadUsers() {
  const users = await api("GET", "/users");
  $("#user-rows").replaceChildren(...users.map(userRow));
}

function userRow(u) {
  const self = u.id === me.id;
  return h("tr", { class: u.disabled ? "disabled" : "" },
    h("td", {}, u.username, self ? h("span", { class: "muted" }, " (du)") : null),
    h("td", {}, u.display_name),
    h("td", {}, u.role === "admin" ? "Admin" : "Benutzer"),
    h("td", {}, u.disabled ? h("span", { class: "badge warn" }, "gesperrt") : "aktiv"),
    h("td", {}, u.totp_enabled ? "eingerichtet" : h("span", { class: "muted" }, "nein")),
    h("td", { title: formatDate(u.last_login_at) }, relative(u.last_login_at)),
    h("td", { class: "row-actions" },
      h("button", { class: "link", onclick: () => editUser(u) }, "Bearbeiten"),
      self ? null : h("button", { class: "link", onclick: () => resetPassword(u) }, "Passwort"),
      u.totp_enabled && !self ? h("button", { class: "link", onclick: () => resetTotp(u) }, "2FA zurücksetzen") : null,
      self ? null : h("button", { class: "link", onclick: () => toggleUser(u) }, u.disabled ? "Entsperren" : "Sperren"),
      self ? null : h("button", { class: "link danger", onclick: () => deleteUser(u) }, "Löschen")));
}

const roleOptions = [["user", "Benutzer"], ["admin", "Admin"]];

async function newUser() {
  let created;
  await dialog({
    title: "Benutzer anlegen",
    fields: [
      { name: "username", label: "Benutzername", required: true },
      { name: "display_name", label: "Name" },
      { name: "role", label: "Rolle", options: roleOptions, value: "user" },
      { name: "password", label: "Passwort (leer lassen für ein zufälliges)", type: "password", autocomplete: "new-password" },
    ],
    okLabel: "Anlegen",
    onOk: async (v) => { created = await api("POST", "/users", v); },
  });
  if (!created) return;
  if (created.password) await showPassword(created.user.username, created.password);
  loadUsers();
}

async function editUser(u) {
  const fields = [{ name: "display_name", label: "Name", value: u.display_name }];
  if (u.id !== me.id) fields.push({ name: "role", label: "Rolle", options: roleOptions, value: u.role });
  await dialog({
    title: `${u.username} bearbeiten`,
    fields,
    okLabel: "Speichern",
    onOk: (v) => api("PATCH", `/users/${u.id}`, v),
  }) && loadUsers();
}

async function resetPassword(u) {
  let result;
  await dialog({
    title: `Passwort für ${u.username} zurücksetzen`,
    text: "Alle Sitzungen dieses Kontos werden beendet.",
    fields: [{ name: "password", label: "Neues Passwort (leer lassen für ein zufälliges)", type: "password", autocomplete: "new-password" }],
    okLabel: "Zurücksetzen",
    onOk: async (v) => { result = await api("POST", `/users/${u.id}/password`, v); },
  });
  if (result && result.password) await showPassword(u.username, result.password);
}

async function resetTotp(u) {
  await dialog({
    title: "Zwei-Faktor zurücksetzen",
    text: `${u.username} kann sich danach nur mit dem Passwort anmelden und muss Zwei-Faktor neu einrichten.`,
    okLabel: "Zurücksetzen",
    danger: true,
    onOk: () => api("POST", `/users/${u.id}/reset-2fa`),
  }) && loadUsers();
}

async function toggleUser(u) {
  await dialog({
    title: u.disabled ? "Konto entsperren" : "Konto sperren",
    text: u.disabled ? `${u.username} kann sich wieder anmelden.` : `${u.username} wird sofort abgemeldet und kann sich nicht mehr anmelden.`,
    okLabel: u.disabled ? "Entsperren" : "Sperren",
    danger: !u.disabled,
    onOk: () => api("PATCH", `/users/${u.id}`, { disabled: !u.disabled }),
  }) && loadUsers();
}

async function deleteUser(u) {
  await dialog({
    title: "Benutzer löschen",
    text: `${u.username} endgültig löschen?`,
    okLabel: "Löschen",
    danger: true,
    onOk: () => api("DELETE", `/users/${u.id}`),
  }) && loadUsers();
}

// ----------------------------------------------------------- Mein Konto

async function loadAccount() {
  me = await api("GET", "/me");
  $("#totp-status").textContent = me.totp_enabled
    ? "Eingeschaltet. Die Anmeldung braucht einen Code aus der Authenticator-App."
    : "Ausgeschaltet.";
  $("#totp-start").hidden = me.totp_enabled;
  $("#totp-confirm").hidden = true;
  $("#totp-off").hidden = !me.totp_enabled;
  for (const f of ["#totp-start", "#totp-confirm", "#totp-off", "#password-form"]) {
    $(f).reset();
    formError($(f), "");
  }
}

function bindForm(selector, handler) {
  const form = $(selector);
  form.addEventListener("submit", async (ev) => {
    ev.preventDefault();
    formError(form, "");
    const button = $('button[type="submit"]', form);
    button.disabled = true;
    try {
      await handler(formValues(form), form);
    } catch (e) {
      formError(form, e.message);
    } finally {
      button.disabled = false;
    }
  });
}

function init() {
  bindForm("#login-form", async (v) => {
    try {
      await api("POST", "/login", v);
    } catch (e) {
      if (e.body && e.body.needs_code) {
        const wasHidden = $("#login-code").hidden;
        $("#login-code").hidden = false;
        $('#login-form [name="code"]').focus();
        if (wasHidden) return;
      }
      throw e;
    }
    await showApp();
  });

  bindForm("#password-form", async (v, form) => {
    if (v.new_password !== v.repeat) throw new Error("Die neuen Passwörter stimmen nicht überein");
    await api("POST", "/me/password", { current_password: v.current_password, new_password: v.new_password });
    form.reset();
    formError(form, "");
    await dialog({ title: "Passwort geändert", text: "Andere Sitzungen wurden abgemeldet.", cancel: false });
  });

  bindForm("#totp-start", async (v) => {
    const setup = await api("POST", "/me/2fa/setup", v);
    $("#totp-qr").src = "data:image/svg+xml;charset=utf-8," + encodeURIComponent(setup.qr_svg);
    $("#totp-secret").textContent = setup.secret.replace(/(.{4})/g, "$1 ").trim();
    $("#totp-start").hidden = true;
    $("#totp-confirm").hidden = false;
    $('#totp-confirm [name="code"]').focus();
  });

  bindForm("#totp-confirm", async (v) => {
    await api("POST", "/me/2fa/enable", v);
    await loadAccount();
  });

  bindForm("#totp-off", async (v) => {
    await api("POST", "/me/2fa/disable", v);
    await loadAccount();
  });

  $("#logout").addEventListener("click", async () => {
    try { await api("POST", "/logout"); } finally { showLogin(); }
  });
  $("#user-new").addEventListener("click", newUser);
  $("#device-filter").addEventListener("input", renderDevices);
  window.addEventListener("hashchange", route);

  // Geräteliste regelmässig auffrischen, solange sie sichtbar ist.
  setInterval(() => {
    if (me && !$("#view-devices").hidden && !document.hidden && !$("#dialog").open) {
      loadDevices().catch(() => {});
    }
  }, 15000);

  showApp().catch(showLogin);
}

document.addEventListener("DOMContentLoaded", init);
