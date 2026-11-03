// NeOlap web UI: entry module. Login, navigation, live connection.

import * as api from './api.js';
import { h, render } from './dom.js';
import { toast } from './toast.js';
import { PivotPage } from './page-pivot.js';
import { DimensionsPage } from './page-dimensions.js';
import { RulesPage } from './page-rules.js';
import { ImportPage } from './page-import.js';
import { JournalPage } from './page-journal.js';
import { AdminPage } from './page-admin.js';

const app = document.getElementById('app');

/** Pages by route. `role` is the lowest role that sees the page in the navigation. */
const PAGES = [
  { id: 'pivot', title: 'Pivot', role: 'reader', make: (ctx) => new PivotPage(ctx) },
  { id: 'dimensions', title: 'Dimensionen', role: 'modeler', make: (ctx) => new DimensionsPage(ctx) },
  { id: 'rules', title: 'Regeln', role: 'reader', make: (ctx) => new RulesPage(ctx) },
  { id: 'import', title: 'Import', role: 'planner', make: (ctx) => new ImportPage(ctx) },
  { id: 'journal', title: 'Journal', role: 'reader', make: (ctx) => new JournalPage(ctx) },
  { id: 'admin', title: 'Verwaltung', role: 'admin', make: (ctx) => new AdminPage(ctx) },
];
const RANK = { reader: 0, planner: 1, modeler: 2, admin: 3 };

const state = { me: null, model: null, page: null, closeEvents: null, live: 'off', listeners: new Set() };

/** What pages get: the session, the model and a way to react to live events. */
const ctx = {
  get me() { return state.me; },
  get model() { return state.model; },
  can: (role) => state.me && RANK[state.me.role] >= RANK[role],
  reloadModel: async () => { state.model = await api.get('/model'); return state.model; },
  onEvent: (fn) => { state.listeners.add(fn); return () => state.listeners.delete(fn); },
  toast,
};

api.setUnauthorizedHandler(() => {
  if (state.me) {
    state.me = null;
    toast('Die Sitzung ist abgelaufen. Bitte neu anmelden.', 'error');
    showLogin();
  }
});

// ------------------------------------------------------------------ login

function showLogin(message) {
  state.page?.destroy?.();
  state.page = null;
  state.closeEvents?.();
  state.closeEvents = null;
  const user = h('input', { type: 'text', name: 'user', autocomplete: 'username', 'aria-label': 'Benutzer' });
  const password = h('input', { type: 'password', name: 'password', autocomplete: 'current-password', 'aria-label': 'Passwort' });
  const error = h('div', { class: 'error', role: 'alert' }, message || '');
  const submit = h('button', { class: 'primary', type: 'submit' }, 'Anmelden');
  const form = h('form', { onsubmit: async (e) => {
    e.preventDefault();
    submit.disabled = true;
    error.textContent = '';
    try {
      state.me = await api.login(user.value.trim(), password.value);
      password.value = '';
      await start();
    } catch (err) {
      error.textContent = err.status === 429 ? err.message : err.status === 401 ? 'Benutzername oder Passwort stimmt nicht.' : `Anmeldung fehlgeschlagen: ${err.message}`;
      submit.disabled = false;
      password.focus();
    }
  } },
    h('label', {}, 'Benutzer', user),
    h('label', {}, 'Passwort', password),
    error,
    submit,
  );
  render(app, h('main', { class: 'login card' }, h('h1', {}, 'NeOlap'), h('p', { class: 'muted' }, 'Bitte anmelden.'), form));
  user.focus();
}

// ------------------------------------------------------------------ shell

function currentRoute() {
  const id = location.hash.replace(/^#\/?/, '').split('/')[0];
  return PAGES.find((p) => p.id === id && ctx.can(p.role)) || PAGES[0];
}

function liveBadge() {
  const label = { on: 'live', off: 'getrennt', wait: 'verbinde …' }[state.live];
  const el = h('span', { class: `live ${state.live}`, id: 'live', title: 'Verbindung für Live-Aktualisierung' }, label);
  return el;
}

async function logout() {
  try { await api.logout(); } catch { /* the session may be gone already */ }
  state.me = null;
  showLogin();
}

function shell() {
  const route = currentRoute();
  const nav = h('nav', { class: 'nav', 'aria-label': 'Bereiche' },
    PAGES.filter((p) => ctx.can(p.role)).map((p) =>
      h('button', {
        type: 'button', 'aria-current': p.id === route.id ? 'page' : null,
        onclick: () => { location.hash = `#/${p.id}`; },
      }, p.title)));
  const top = h('header', { class: 'topbar' },
    h('span', { class: 'brand' }, 'NeOlap'), nav, h('span', { class: 'spacer' }),
    liveBadge(),
    h('span', { class: 'who' }, `${state.me.user} · ${roleName(state.me.role)}`),
    h('button', { class: 'ghost', type: 'button', onclick: logout }, 'Abmelden'));
  const main = h('main', { class: 'page', id: 'page' });
  render(app, top, main);
  state.page?.destroy?.();
  state.page = route.make(ctx);
  state.page.mount(main);
}

function roleName(role) {
  return { reader: 'Leser', planner: 'Planer', modeler: 'Modellierer', admin: 'Admin' }[role] || role;
}

function setLive(value) {
  state.live = value;
  const el = document.getElementById('live');
  if (el) {
    el.className = `live ${value}`;
    el.textContent = { on: 'live', off: 'getrennt', wait: 'verbinde …' }[value];
  }
}

function emit(kind, data) {
  for (const fn of state.listeners) {
    try { fn(kind, data); } catch (e) { console.error(e); }
  }
}

let wasLost = false;
let recheck = null;

/**
 * Opens the live stream. When it drops, the browser retries by itself, but a retry cannot succeed
 * if the session is gone (a server restart ends all sessions). So after a loss the session is
 * checked: if it is gone, the login form is shown at once instead of letting the next action fail;
 * if it holds, the stream comes back by itself and every open view is told to reload, because
 * changes may have been missed while it was down.
 */
function connectEvents() {
  state.closeEvents?.();
  setLive('wait');
  wasLost = false;
  state.closeEvents = api.listen(emit, (s) => {
    if (s === 'open') {
      setLive('on');
      clearTimeout(recheck);
      if (wasLost) {
        wasLost = false;
        emit('resync', null);
      }
      return;
    }
    setLive('off');
    wasLost = true;
    clearTimeout(recheck);
    checkSession(1500);
  });
}

/** Looks at the session after a loss of the live stream; while the server is away it asks again, less and less often. */
function checkSession(delay) {
  clearTimeout(recheck);
  recheck = setTimeout(async () => {
    if (!state.me || state.live === 'on') return;
    try {
      const me = await api.whoAmI();
      if (!me) {
        state.me = null;
        toast('Die Sitzung ist beendet (Server neu gestartet?). Bitte neu anmelden.', 'error');
        showLogin();
      } else if (state.live === 'off') {
        connectEvents(); // the old EventSource gave up on a 401; open a fresh one
      }
    } catch {
      checkSession(Math.min(delay * 2, 30000)); // the server is still away
    }
  }, delay);
}

async function start() {
  try {
    state.model = await api.get('/model');
  } catch (e) {
    if (!e.isUnauthorized) toast(`Modell konnte nicht geladen werden: ${e.message}`, 'error');
    return;
  }
  shell();
  connectEvents();
}

window.addEventListener('hashchange', () => { if (state.me) shell(); });

// a reload keeps the session (HttpOnly cookie); /me also hands out the CSRF token again
state.me = await api.whoAmI();
if (state.me) await start();
else showLogin();
