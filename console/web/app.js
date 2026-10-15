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
  const views = { "#geraete": "devices", "#benutzer": "users", "#gruppen": "groups", "#adressbuecher": "books", "#protokoll": "logs", "#konto": "account" };
  let view = views[location.hash] || (me.role === "admin" ? "devices" : "account");
  if (me.role !== "admin" && view !== "account") view = "account";
  for (const v of $$(".view")) v.hidden = v.id !== "view-" + view;
  for (const a of $$("nav a")) a.classList.toggle("active", a.dataset.view === view);
  ({ devices: loadDevices, users: loadUsers, groups: loadGroups, books: loadBooks, logs: () => loadLogs(false), account: loadAccount })[view]();
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
    .filter((d) => !q || [d.id, d.hostname, d.username, d.account, d.group, d.note, d.os, d.ip]
      .some((v) => (v || "").toLowerCase().includes(q)))
    .sort((a, b) => (b.online - a.online) || ((b.last_seen_at || 0) - (a.last_seen_at || 0)) || a.id.localeCompare(b.id));
  const online = devices.filter((d) => d.online).length;
  $("#device-summary").textContent = `${online} von ${devices.length} online`;
  $("#device-rows").replaceChildren(...rows.map(deviceRow));
  if (!rows.length) {
    $("#device-rows").append(h("tr", {}, h("td", { colspan: 11, class: "muted center" },
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
    h("td", {}, d.account || h("span", { class: "muted" }, "–")),
    h("td", {}, d.group || h("span", { class: "muted" }, "–")),
    h("td", { class: "clip", title: d.os }, d.os),
    h("td", {}, d.version),
    h("td", { title: formatDate(d.last_seen_at) }, relative(d.last_seen_at)),
    h("td", { class: "clip note", title: d.note }, d.note),
    h("td", { class: "row-actions" },
      h("button", { class: "link", onclick: () => editNote(d) }, "Notiz"),
      h("button", { class: "link", onclick: () => setDeviceGroup(d) }, "Gruppe"),
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

async function setDeviceGroup(d) {
  const groups = await api("GET", "/device-groups");
  if (!groups.length) {
    await dialog({ title: "Keine Gerätegruppen", text: "Lege zuerst unter «Gerätegruppen» eine Gruppe an.", cancel: false });
    return;
  }
  await dialog({
    title: `Gruppe für ${d.hostname || d.id}`,
    fields: [{
      name: "group_id", label: "Gruppe",
      options: [["", "keine"], ...groups.map((g) => [String(g.id), g.name])],
      value: d.group_id == null ? "" : String(d.group_id),
    }],
    okLabel: "Speichern",
    onOk: (v) => api("PUT", `/devices/${encodeURIComponent(d.id)}/group`, { group_id: v.group_id ? Number(v.group_id) : null }),
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

// --------------------------------------------------------- Gerätegruppen

async function loadGroups() {
  const [groups, settings] = await Promise.all([api("GET", "/device-groups"), api("GET", "/settings")]);
  for (const r of $$('input[name="access_mode"]')) r.checked = r.value === settings.access_mode;
  showAccessHint(settings.access_mode);
  $("#group-rows").replaceChildren(...groups.map(groupRow));
  if (!groups.length) {
    $("#group-rows").append(h("tr", {}, h("td", { colspan: 5, class: "muted center" }, "Noch keine Gerätegruppen")));
  }
}

function showAccessHint(mode) {
  const hint = $("#access-hint");
  hint.hidden = mode !== "groups";
  hint.textContent = "Aktiv, sofern hbbs die Datenbank der Konsole kennt (VIBEDESK_CONSOLE_DB). Wer im Client nicht angemeldet ist, kann jetzt keine Verbindung mehr aufbauen.";
}

function groupRow(g) {
  const users = g.users.length ? g.users.map((u) => u.username).join(", ") : h("span", { class: "muted" }, "nur Admins");
  return h("tr", {},
    h("td", {}, g.name),
    h("td", {}, String(g.device_count)),
    h("td", { class: "clip" }, users),
    h("td", { class: "clip note", title: g.note }, g.note),
    h("td", { class: "row-actions" },
      h("button", { class: "link", onclick: () => manageGroupUsers(g) }, "Benutzer"),
      h("button", { class: "link", onclick: () => editGroup(g) }, "Bearbeiten"),
      h("button", { class: "link danger", onclick: () => deleteGroup(g) }, "Löschen")));
}

async function newGroup() {
  await dialog({
    title: "Gerätegruppe anlegen",
    fields: [
      { name: "name", label: "Name", required: true },
      { name: "note", label: "Notiz", type: "textarea", maxlength: 1000 },
    ],
    okLabel: "Anlegen",
    onOk: (v) => api("POST", "/device-groups", v),
  }) && loadGroups();
}

async function editGroup(g) {
  await dialog({
    title: `${g.name} bearbeiten`,
    fields: [
      { name: "name", label: "Name", value: g.name, required: true },
      { name: "note", label: "Notiz", type: "textarea", value: g.note, maxlength: 1000 },
    ],
    okLabel: "Speichern",
    onOk: (v) => api("PATCH", `/device-groups/${g.id}`, v),
  }) && loadGroups();
}

async function deleteGroup(g) {
  await dialog({
    title: "Gerätegruppe löschen",
    text: `«${g.name}» löschen? Die ${g.device_count} Geräte bleiben erhalten, gehören dann aber zu keiner Gruppe mehr.`,
    okLabel: "Löschen",
    danger: true,
    onOk: () => api("DELETE", `/device-groups/${g.id}`),
  }) && loadGroups();
}

async function manageGroupUsers(g) {
  const all = await userOptions();
  const render = (group) => {
    const list = group.users.length
      ? h("ul", { class: "member-list" }, group.users.map((u) => h("li", {},
          h("span", {}, u.username),
          h("button", {
            class: "link danger", type: "button",
            onclick: async () => render(await api("DELETE", `/device-groups/${g.id}/users/${u.user_id}`)),
          }, "Entfernen"))))
      : h("p", { class: "muted" }, "Noch niemand. Admins haben immer Zugriff.");
    $("#member-list-slot").replaceChildren(list);
  };
  const free = all.filter(([id]) => !g.users.some((u) => u.user_id === Number(id)));
  const done = dialog({
    title: `Zugriff auf ${g.name}`,
    text: [h("div", { id: "member-list-slot" }), h("p", {}, "Benutzer hinzufügen:")],
    fields: [{ name: "user_id", label: "Benutzer", options: [["", "Bitte wählen"], ...free] }],
    okLabel: "Hinzufügen",
    onOk: async (v) => {
      if (!v.user_id) throw new Error("Bitte einen Benutzer wählen");
      await api("PUT", `/device-groups/${g.id}/users/${v.user_id}`);
    },
  });
  render(g);
  if (await done) {
    const fresh = (await api("GET", "/device-groups")).find((x) => x.id === g.id);
    if (fresh) await manageGroupUsers(fresh);
  }
  loadGroups();
}

// ----------------------------------------------------------- Adressbücher

const ruleLabels = { 1: "Lesen", 2: "Lesen und schreiben", 3: "Volle Kontrolle" };
const ruleOptions = Object.entries(ruleLabels);

async function loadBooks() {
  const books = await api("GET", "/address-books");
  $("#book-rows").replaceChildren(...books.map(bookRow));
  if (!books.length) {
    $("#book-rows").append(h("tr", {}, h("td", { colspan: 6, class: "muted center" }, "Noch keine geteilten Adressbücher")));
  }
}

function bookRow(b) {
  const members = b.members.length
    ? b.members.map((m) => `${m.username} (${ruleLabels[m.rule]})`).join(", ")
    : h("span", { class: "muted" }, "nur Besitzer");
  return h("tr", {},
    h("td", {}, b.name),
    h("td", {}, b.owner),
    h("td", {}, String(b.peer_count)),
    h("td", { class: "clip", title: typeof members === "string" ? members : "" }, members),
    h("td", { class: "clip note", title: b.note }, b.note),
    h("td", { class: "row-actions" },
      h("button", { class: "link", onclick: () => manageMembers(b) }, "Mitglieder"),
      h("button", { class: "link", onclick: () => showBookPeers(b) }, "Einträge"),
      h("button", { class: "link", onclick: () => editBook(b) }, "Bearbeiten"),
      h("button", { class: "link danger", onclick: () => deleteBook(b) }, "Löschen")));
}

async function userOptions() {
  const users = await api("GET", "/users");
  return users.map((u) => [String(u.id), u.display_name ? `${u.username} (${u.display_name})` : u.username]);
}

async function newBook() {
  const owners = await userOptions();
  await dialog({
    title: "Adressbuch anlegen",
    fields: [
      { name: "name", label: "Name", required: true },
      { name: "note", label: "Notiz", type: "textarea", maxlength: 1000 },
      { name: "owner_id", label: "Besitzer (volle Kontrolle)", options: owners, value: String(me.id) },
    ],
    okLabel: "Anlegen",
    onOk: (v) => api("POST", "/address-books", { name: v.name, note: v.note, owner_id: Number(v.owner_id) }),
  }) && loadBooks();
}

async function editBook(b) {
  const owners = await userOptions();
  await dialog({
    title: `${b.name} bearbeiten`,
    fields: [
      { name: "name", label: "Name", value: b.name, required: true },
      { name: "note", label: "Notiz", type: "textarea", value: b.note, maxlength: 1000 },
      { name: "owner_id", label: "Besitzer (volle Kontrolle)", options: owners, value: String(b.owner_id) },
    ],
    okLabel: "Speichern",
    onOk: (v) => api("PATCH", `/address-books/${b.guid}`, { name: v.name, note: v.note, owner_id: Number(v.owner_id) }),
  }) && loadBooks();
}

async function deleteBook(b) {
  await dialog({
    title: "Adressbuch löschen",
    text: `«${b.name}» mit ${b.peer_count} Einträgen endgültig löschen? Es verschwindet bei allen Mitgliedern.`,
    okLabel: "Löschen",
    danger: true,
    onOk: () => api("DELETE", `/address-books/${b.guid}`),
  }) && loadBooks();
}

async function manageMembers(b) {
  const users = (await userOptions()).filter(([id]) => Number(id) !== b.owner_id);
  const render = (book) => {
    const list = book.members.length
      ? h("ul", { class: "member-list" }, book.members.map((m) => h("li", {},
          h("span", {}, m.username),
          h("select", {
            onchange: async (ev) => {
              book = await api("PUT", `/address-books/${b.guid}/members/${m.user_id}`, { rule: Number(ev.target.value) });
              render(book);
            },
          }, ruleOptions.map(([value, label]) => h("option", { value, selected: Number(value) === m.rule }, label))),
          h("button", {
            class: "link danger", type: "button",
            onclick: async () => {
              book = await api("DELETE", `/address-books/${b.guid}/members/${m.user_id}`);
              render(book);
            },
          }, "Entfernen"))))
      : h("p", { class: "muted" }, "Noch keine Mitglieder.");
    $("#member-list-slot").replaceChildren(list);
  };
  const free = users.filter(([id]) => !b.members.some((m) => m.user_id === Number(id)));
  const slot = h("div", { id: "member-list-slot" });
  const done = dialog({
    title: `Mitglieder von ${b.name}`,
    text: [h("p", { class: "muted" }, `Besitzer: ${b.owner} (volle Kontrolle)`), slot,
      h("p", {}, "Mitglied hinzufügen:")],
    fields: [
      { name: "user_id", label: "Benutzer", options: [["", "Bitte wählen"], ...free] },
      { name: "rule", label: "Berechtigung", options: ruleOptions, value: "1" },
    ],
    okLabel: "Hinzufügen",
    onOk: async (v) => {
      if (!v.user_id) throw new Error("Bitte einen Benutzer wählen");
      await api("PUT", `/address-books/${b.guid}/members/${v.user_id}`, { rule: Number(v.rule) });
    },
  });
  render(b);
  if (await done) await manageMembers((await api("GET", "/address-books")).find((x) => x.guid === b.guid) || b);
  loadBooks();
}

async function showBookPeers(b) {
  const peers = await api("GET", `/address-books/${b.guid}/peers`);
  const table = peers.length
    ? h("div", { class: "table-wrap" }, h("table", {},
        h("thead", {}, h("tr", {}, h("th", {}, "ID"), h("th", {}, "Alias"), h("th", {}, "Name"), h("th", {}, "Tags"))),
        h("tbody", {}, peers.map((p) => h("tr", {},
          h("td", { class: "mono" }, p.id),
          h("td", {}, p.alias || ""),
          h("td", {}, p.hostname || ""),
          h("td", {}, (p.tags || []).join(", ")))))))
    : h("p", { class: "muted" }, "Noch keine Einträge. Mitglieder mit Schreibrecht fügen sie im Client hinzu.");
  await dialog({ title: `Einträge in ${b.name}`, text: [table], cancel: false });
}

// ------------------------------------------------------------- Protokoll

const LOG_PAGE = 100;
const connTypes = ["Fernsteuerung", "Dateiübertragung", "Port-Weiterleitung", "Kamera", "Terminal"];
const primaryAuth = { 1: "Klick", 2: "Einmalpasswort", 3: "festes Passwort", 4: "Seitenwechsel" };
const twoFactor = { 1: "TOTP", 2: "vertrautes Gerät" };
const alarmTypes = {
  0: "IP nicht auf Freigabeliste",
  1: "Mehr als 30 Fehlversuche",
  2: "6 Fehlversuche in einer Minute",
  6: "Zu viele Versuche aus einem IPv6-Netz",
  7: "Terminal-Anmeldung gebremst",
  8: "Zu viele Terminal-Anmeldungen gleichzeitig",
  9: "Unerlaubte Aktion in der Sitzung",
  10: "ID nicht auf Freigabeliste",
};
let logKind = "verbindungen";
let logItems = [];

function duration(secs) {
  if (secs < 60) return `${secs} s`;
  if (secs < 3600) return `${Math.round(secs / 60)} Min.`;
  return `${Math.floor(secs / 3600)} Std. ${Math.round((secs % 3600) / 60)} Min.`;
}

function deviceCell(l) {
  return h("td", {}, l.hostname || "", h("div", { class: "mono muted small" }, l.device_id));
}

function peerCell(l) {
  return h("td", {}, l.peer_name || "", h("div", { class: "mono muted small" }, l.peer_id || ""));
}

const logViews = {
  verbindungen: {
    head: ["Beginn", "Dauer", "Gerät", "Von", "IP", "Art", "Anmeldung", "Notiz"],
    hint: "Jede Verbindung auf ein Gerät, gemeldet vom gesteuerten Gerät.",
    row: (l) => {
      const end = l.ended_at ? duration(l.ended_at - l.started_at) : h("span", { class: "muted" }, "offen");
      const auth = [primaryAuth[l.primary_auth], twoFactor[l.two_factor]].filter(Boolean).join(" + ");
      return h("tr", {},
        h("td", {}, formatDate(l.started_at)),
        h("td", {}, end),
        deviceCell(l), peerCell(l),
        h("td", { class: "mono" }, l.ip),
        h("td", {}, l.conn_type == null ? h("span", { class: "muted" }, "nicht angemeldet") : connTypes[l.conn_type] || `Typ ${l.conn_type}`),
        h("td", {}, auth),
        h("td", { class: "clip note", title: l.note }, l.note));
    },
  },
  dateien: {
    head: ["Zeit", "Gerät", "Von", "Richtung", "Pfad", "Dateien"],
    hint: "Dateiübertragungen und kopierte Dateien, gemeldet vom gesteuerten Gerät.",
    row: (l) => {
      let files = [];
      try { files = JSON.parse(l.files); } catch (_) { /* leer lassen */ }
      const names = files.map((f) => f[0]).filter(Boolean).join(", ");
      return h("tr", {},
        h("td", {}, formatDate(l.created_at)),
        deviceCell(l), peerCell(l),
        h("td", {}, l.direction === 0 ? "vom Gerät geholt" : "auf das Gerät kopiert"),
        h("td", { class: "clip mono", title: l.path }, l.path),
        h("td", { class: "clip", title: names }, `${l.num}${names ? ": " + names : ""}`));
    },
  },
  alarme: {
    head: ["Zeit", "Gerät", "Alarm", "Von", "IP"],
    hint: "Warnungen der Geräte, etwa bei vielen falschen Passwörtern.",
    row: (l) => h("tr", {},
      h("td", {}, formatDate(l.created_at)),
      deviceCell(l),
      h("td", { title: l.info }, alarmTypes[l.typ] || `Alarm ${l.typ}`),
      peerCell(l),
      h("td", { class: "mono" }, l.ip)),
  },
  konsole: {
    head: ["Zeit", "Konto", "Aktion", "Details", "IP"],
    hint: "Anmeldungen und Änderungen in der Konsole und Anmeldungen im Client.",
    row: (l) => h("tr", {},
      h("td", {}, formatDate(l.created_at)),
      h("td", {}, l.username),
      h("td", {}, l.action),
      h("td", { class: "clip", title: l.detail }, l.detail),
      h("td", { class: "mono" }, l.ip)),
  },
};

async function loadLogs(append) {
  const view = logViews[logKind];
  const q = $("#log-filter").value.trim();
  const params = new URLSearchParams({ q, limit: String(LOG_PAGE) });
  if (append && logItems.length) params.set("before", String(logItems[logItems.length - 1].id));
  const items = await api("GET", `/logs/${logKind}?${params}`);
  logItems = append ? logItems.concat(items) : items;
  for (const b of $$("#log-kinds button")) b.classList.toggle("active", b.dataset.kind === logKind);
  $("#log-hint").textContent = view.hint;
  $("#log-head").replaceChildren(h("tr", {}, view.head.map((c) => h("th", {}, c))));
  $("#log-rows").replaceChildren(...logItems.map(view.row));
  if (!logItems.length) {
    $("#log-rows").append(h("tr", {}, h("td", { colspan: view.head.length, class: "muted center" }, "Keine Einträge")));
  }
  $("#log-more").hidden = items.length < LOG_PAGE;
  $("#log-csv").href = `/admin/api/logs/${logKind}/csv?${new URLSearchParams({ q })}`;
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
  $("#book-new").addEventListener("click", newBook);
  $("#group-new").addEventListener("click", newGroup);
  for (const r of $$('input[name="access_mode"]')) {
    r.addEventListener("change", async () => {
      $("#access-error").textContent = "";
      if (r.value === "groups") {
        const ok = await dialog({
          title: "Zugriff nur nach Regeln",
          text: "Ab sofort können nur noch im Client angemeldete Benutzer Verbindungen aufbauen, und nur zu erlaubten Geräten. Fortfahren?",
          okLabel: "Einschalten",
          danger: true,
        });
        if (!ok) { loadGroups(); return; }
      }
      try {
        const s = await api("PUT", "/settings", { access_mode: r.value });
        showAccessHint(s.access_mode);
      } catch (e) {
        $("#access-error").textContent = e.message;
        loadGroups();
      }
    });
  }
  for (const b of $$("#log-kinds button")) {
    b.addEventListener("click", () => { logKind = b.dataset.kind; loadLogs(false); });
  }
  let logTimer;
  $("#log-filter").addEventListener("input", () => {
    clearTimeout(logTimer);
    logTimer = setTimeout(() => loadLogs(false), 300);
  });
  $("#log-more").addEventListener("click", () => loadLogs(true));
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
