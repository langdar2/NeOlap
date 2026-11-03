// DOM helper that cannot produce markup from data.
//
// Member names, user names and error texts come from users. Building HTML from them with
// innerHTML would let a modeler store a member called `<img src=x onerror=...>` that runs in every
// viewer's browser. Everything here goes through createElement and textContent instead, so there
// is no place in the UI where data is parsed as markup. The Content-Security-Policy of the server
// (no inline script) is the second line of defence.

const SAFE_ATTRS = new Set([
  'class', 'id', 'type', 'name', 'value', 'placeholder', 'title', 'role', 'tabindex', 'for',
  'disabled', 'checked', 'selected', 'readonly', 'rows', 'cols', 'min', 'max', 'step', 'href',
  'download', 'autocomplete', 'spellcheck', 'colspan', 'rowspan', 'multiple', 'accept', 'open',
  'draggable', 'method', 'maxlength', 'pattern', 'required', 'size', 'label', 'selected',
]);

/** Whether `value` may be used as a link target. Exported for the tests of this rule. */
export function safeHref(value) {
  // only same-page and same-site relative links: never javascript: or data:, never another site.
  // "//host/path" starts with a slash but is a link to another host, so it is refused.
  if (typeof value !== 'string' || value.startsWith('//') || value.startsWith('/\\')) return false;
  return /^(#|\/|\.\/|[a-z0-9_-]+(\/|\?|$))/i.test(value) && !/^[a-z]+:/i.test(value);
}

/**
 * `h('button', { class: 'primary', onclick: fn, 'aria-label': 'Close' }, 'Text', childNode, …)`.
 * Strings become text nodes, `null`/`false` are skipped, arrays are flattened. Attributes outside
 * the allow-list (except `aria-*` and `data-*`) are refused; `on*` takes a function only.
 */
export function h(tag, attrs = {}, ...children) {
  const el = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs || {})) {
    if (value === undefined || value === null || value === false) continue;
    if (key.startsWith('on')) {
      if (typeof value !== 'function') throw new Error(`h(): ${key} needs a function`);
      el.addEventListener(key.slice(2), value);
    } else if (key === 'text') {
      el.textContent = String(value);
    } else if (key === 'style') {
      throw new Error('h(): set styles with classes (the CSP allows no inline style attribute)');
    } else if (key.startsWith('aria-') || key.startsWith('data-') || SAFE_ATTRS.has(key)) {
      if (key === 'href' && !safeHref(value)) throw new Error(`h(): unsafe href ${JSON.stringify(value)}`);
      el.setAttribute(key, value === true ? '' : String(value));
    } else {
      throw new Error(`h(): attribute ${key} is not allowed`);
    }
  }
  append(el, children);
  return el;
}

/** Appends children (strings, nodes, arrays) to `el`. */
export function append(el, children) {
  for (const c of children.flat(Infinity)) {
    if (c === null || c === undefined || c === false) continue;
    el.append(c instanceof Node ? c : document.createTextNode(String(c)));
  }
  return el;
}

/** Replaces the content of `el`. */
export function render(el, ...children) {
  el.replaceChildren();
  return append(el, children);
}

/** Removes all children. */
export function clear(el) {
  el.replaceChildren();
}

/** `2026-11-03 20:45` from epoch seconds, in the viewer's time zone. */
export function formatTime(seconds) {
  const d = new Date(seconds * 1000);
  const p = (n) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

/** Number as shown in a cell: thousands separators, up to `digits` decimals, no trailing zeros. */
export function formatNumber(value, digits = 2, locale = undefined) {
  if (value === null || value === undefined) return '';
  if (typeof value === 'string') return value; // error values such as #DIV0
  return new Intl.NumberFormat(locale, { maximumFractionDigits: digits }).format(value);
}
