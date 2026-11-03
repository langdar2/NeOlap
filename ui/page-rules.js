// Rule editor: text with line numbers, a syntax check without saving, and per-rule status.

import * as api from './api.js';
import { h, render } from './dom.js';

export class RulesPage {
  constructor(ctx) {
    this.ctx = ctx;
    this.cube = null;
    this.saved = '';
  }

  mount(host) {
    const cubes = this.ctx.model.cubes;
    if (!cubes.length) return render(host, h('div', { class: 'card' }, 'Es gibt keinen Cube.'));
    this.canEdit = this.ctx.can('modeler');
    this.select = h('select', { 'aria-label': 'Cube', onchange: () => this.open(this.select.value) }, cubes.map((c) => h('option', { value: c.name }, c.name)));
    this.numbers = h('pre', { class: 'rule-numbers', 'aria-hidden': 'true' });
    this.text = h('textarea', { class: 'editor-text', spellcheck: false, 'aria-label': 'Regeln', readonly: this.canEdit ? null : true });
    this.text.addEventListener('input', () => { this.#numbers(); this.#dirty(); });
    this.text.addEventListener('scroll', () => { this.numbers.scrollTop = this.text.scrollTop; });
    this.status = h('div', { class: 'rule-lines', role: 'status' });
    this.saveButton = h('button', { class: 'primary', type: 'button', onclick: () => this.save(), disabled: true }, 'Speichern');
    this.checkButton = h('button', { type: 'button', onclick: () => this.check() }, 'Prüfen');
    render(host,
      h('div', { class: 'toolbar' }, h('label', {}, 'Cube ', this.select),
        this.canEdit ? [this.checkButton, this.saveButton] : h('span', { class: 'muted' }, 'Nur Lesen: Regeln ändern braucht die Rolle Modellierer.'),
        h('span', { class: 'spacer' }), h('span', { class: 'muted', id: 'rule-state' })),
      h('div', { class: 'rule-editor' }, this.numbers, this.text),
      this.status,
      h('p', { class: 'muted' }, 'Beispiel: ', h('code', {}, "['Profit in %'] = IF(['Sales'] = 0, NULL, ['Profit'] / ['Sales']);"), '  Präfix N: = nur Basiszellen, C: = nur Aggregate.'));
    this.open(cubes[0].name);
  }

  destroy() {}

  async open(name) {
    this.cube = name;
    try {
      const r = await api.get(`/cubes/${api.enc(name)}/rules`);
      this.saved = r.text;
      this.text.value = r.text;
      this.#numbers();
      this.#showStatus(r.status);
      this.#dirty();
    } catch (e) {
      this.ctx.toast(e.message, 'error');
    }
  }

  #numbers() {
    const n = this.text.value.split('\n').length;
    this.numbers.textContent = Array.from({ length: n }, (_, i) => i + 1).join('\n');
  }

  #dirty() {
    const changed = this.text.value !== this.saved;
    this.saveButton.disabled = !changed;
    const el = document.getElementById('rule-state');
    if (el) el.textContent = changed ? 'ungespeicherte Änderungen' : '';
  }

  #showStatus(status, error) {
    if (error) return render(this.status, h('div', { class: 'bad error' }, error));
    const bad = status.filter((s) => !s.valid);
    render(this.status,
      status.length ? h('div', { class: bad.length ? '' : 'ok' }, `${status.length} Regel(n), ${bad.length ? `${bad.length} ungültig (werden übersprungen)` : 'alle gültig'}`) : h('div', { class: 'muted' }, 'Keine Regeln.'),
      bad.map((s) => h('div', { class: 'bad' }, `Zeile ${s.line}: ${s.message}`)));
  }

  async check() {
    try {
      const r = await api.put(`/cubes/${api.enc(this.cube)}/rules`, { text: this.text.value, check: true });
      this.#showStatus(r.status);
    } catch (e) {
      this.#showStatus([], e.message);
    }
  }

  async save() {
    try {
      const r = await api.put(`/cubes/${api.enc(this.cube)}/rules`, { text: this.text.value });
      this.saved = this.text.value;
      this.#showStatus(r.status);
      this.#dirty();
      this.ctx.toast('Regeln gespeichert.', 'ok');
    } catch (e) {
      // a syntax error rejects the text and changes nothing; its place is in the message
      this.#showStatus([], e.message);
      this.ctx.toast('Nicht gespeichert: Syntaxfehler.', 'error');
    }
  }
}
