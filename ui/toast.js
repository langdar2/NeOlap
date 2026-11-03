// Short messages and a confirm dialog, both built without innerHTML.

import { h } from './dom.js';

/** Shows a message for a few seconds. `kind`: '' | 'ok' | 'error'. Errors stay longer. */
export function toast(text, kind = '') {
  const host = document.getElementById('toasts');
  if (!host) return;
  // the same message while it is still visible is not shown again (typing into a locked cell
  // would otherwise stack one error per key press)
  for (const old of host.children) {
    if (old.textContent === text) {
      old.classList.remove('toast-bump');
      void old.offsetWidth; // restart the animation
      old.classList.add('toast-bump');
      return;
    }
  }
  while (host.children.length >= 4) host.firstElementChild.remove();
  const el = h('div', { class: `toast ${kind}`, role: kind === 'error' ? 'alert' : 'status' }, text);
  host.append(el);
  setTimeout(() => el.remove(), kind === 'error' ? 9000 : 3500);
}

/**
 * A modal dialog. `buttons`: `[{ label, value, kind }]`; resolves with the value of the clicked
 * button, or `null` if closed with Escape. `body` is a node or text.
 */
export function dialog(title, body, buttons) {
  return new Promise((resolve) => {
    const dlg = h('dialog', { 'aria-labelledby': 'dlg-title' });
    let result = null;
    const actions = h('div', { class: 'actions' },
      buttons.map((b) => h('button', { type: 'button', class: b.kind || '', onclick: () => { result = b.value; dlg.close(); } }, b.label)));
    dlg.append(h('h2', { id: 'dlg-title' }, title), typeof body === 'string' ? h('p', {}, body) : body, actions);
    dlg.addEventListener('close', () => { dlg.remove(); resolve(result); });
    document.body.append(dlg);
    dlg.showModal();
    dlg.querySelector('button.primary, button')?.focus();
  });
}

/** Yes/no question; resolves to `true` or `false`. */
export async function confirmDialog(title, text, yes = 'Ja', no = 'Abbrechen') {
  const r = await dialog(title, text, [{ label: no, value: false }, { label: yes, value: true, kind: 'primary' }]);
  return r === true;
}

/** A form dialog with text fields; resolves with `{name: value}` or `null`. */
export function promptDialog(title, fields, ok = 'OK') {
  return new Promise((resolve) => {
    const inputs = fields.map((f) => ({ f, el: h(f.type === 'select' ? 'select' : 'input', { type: f.type === 'select' ? null : f.type || 'text', name: f.name, value: f.value ?? '', placeholder: f.placeholder ?? '', autocomplete: 'off' },
      f.type === 'select' ? f.options.map((o) => h('option', { value: o, selected: o === f.value ? true : null }, o)) : null) }));
    const dlg = h('dialog', { 'aria-labelledby': 'dlg-title' });
    let result = null;
    const form = h('form', { class: 'form', method: 'dialog', onsubmit: (e) => {
      e.preventDefault();
      result = Object.fromEntries(inputs.map(({ f, el }) => [f.name, el.value]));
      dlg.close();
    } },
      inputs.map(({ f, el }) => h('label', {}, f.label, el)),
      h('div', { class: 'actions' },
        h('button', { type: 'button', onclick: () => dlg.close() }, 'Abbrechen'),
        h('button', { type: 'submit', class: 'primary' }, ok)));
    dlg.append(h('h2', { id: 'dlg-title' }, title), form);
    dlg.addEventListener('close', () => { dlg.remove(); resolve(result); });
    document.body.append(dlg);
    dlg.showModal();
    inputs[0]?.el.focus();
  });
}
