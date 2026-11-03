// API client: one place for requests, the CSRF token and errors.
//
// The CSRF token lives only in memory. The session cookie is HttpOnly (scripts cannot read it),
// so after a reload the token is fetched again from /me. Nothing secret goes to localStorage.

const BASE = 'api/v1';

/** An error response of the server: `status`, `code` (e.g. "conflict") and optional `details`. */
export class ApiError extends Error {
  constructor(status, body) {
    super((body && body.error) || `HTTP ${status}`);
    this.name = 'ApiError';
    this.status = status;
    this.code = (body && body.code) || 'error';
    this.details = (body && body.details) || null;
  }

  get isConflict() { return this.status === 409; }
  get isUnauthorized() { return this.status === 401; }
}

let csrf = null;
let onUnauthorized = () => {};

/** Called when a request finds the session gone (to show the login form). */
export function setUnauthorizedHandler(fn) {
  onUnauthorized = fn;
}

async function request(method, path, body, { raw = false } = {}) {
  const headers = {};
  if (body !== undefined && !(body instanceof FormData)) headers['content-type'] = 'application/json';
  if (csrf && method !== 'GET') headers['x-csrf-token'] = csrf;
  const res = await fetch(`${BASE}${path}`, {
    method,
    headers,
    credentials: 'same-origin',
    body: body === undefined ? undefined : body instanceof FormData ? body : JSON.stringify(body),
  });
  if (!res.ok) {
    let parsed = null;
    try { parsed = await res.json(); } catch { /* not JSON */ }
    const err = new ApiError(res.status, parsed);
    // 401 on /login is "wrong password", not "session gone"
    if (err.isUnauthorized && path !== '/login') onUnauthorized();
    throw err;
  }
  if (raw) return res;
  if (res.status === 204) return null;
  return res.json();
}

export const get = (path) => request('GET', path);
export const post = (path, body = {}) => request('POST', path, body);
export const put = (path, body) => request('PUT', path, body);
export const patch = (path, body) => request('PATCH', path, body);
export const del = (path, body = {}) => request('DELETE', path, body);

/** `{user, role}` after a successful login. */
export async function login(user, password) {
  const r = await post('/login', { user, password });
  csrf = r.csrf;
  return { user: r.user, role: r.role };
}

/** The current session, or `null` if there is none. Refreshes the CSRF token. */
export async function whoAmI() {
  const r = await get('/session'); // 200 also when nobody is logged in (no console error)
  if (!r.user) return null;
  csrf = r.csrf;
  return { user: r.user, role: r.role };
}

export async function logout() {
  try { await post('/logout'); } finally { csrf = null; }
}

/** Query string from an object; empty values are left out. */
export function query(params) {
  const q = new URLSearchParams();
  for (const [k, v] of Object.entries(params)) if (v !== undefined && v !== null && v !== '') q.set(k, v);
  const s = q.toString();
  return s ? `?${s}` : '';
}

export const enc = encodeURIComponent;

/** Downloads a file response and returns `{ blob, name }`. */
export async function download(path) {
  const res = await request('GET', path, undefined, { raw: true });
  const cd = res.headers.get('content-disposition') || '';
  const m = /filename="([^"]+)"/.exec(cd);
  return { blob: await res.blob(), name: m ? m[1] : 'download' };
}

/** Server-sent events; returns a function that closes the stream. `onEvent(kind, data)`. */
export function listen(onEvent, onState = () => {}) {
  const source = new EventSource(`${BASE}/events`, { withCredentials: true });
  for (const kind of ['cells', 'model', 'rules', 'import', 'resync']) {
    source.addEventListener(kind, (e) => {
      let data = null;
      try { data = JSON.parse(e.data); } catch { data = e.data; }
      onEvent(kind, data);
    });
  }
  source.onopen = () => onState('open');
  source.onerror = () => onState('lost');
  return () => source.close();
}
