// Journal: who changed what, when. Filter by cube and user.

import * as api from './api.js';
import { h, render, formatTime } from './dom.js';

const OPS = {
  splash: 'Eingabe', set: 'Eingabe', undo: 'Rückgängig', import: 'Import', dimension: 'Dimension', rules: 'Regeln',
  create_cube: 'Cube angelegt', import_spec: 'Import-Spezifikation', user_add: 'Benutzer angelegt', user_role: 'Rolle geändert',
  user_password: 'Passwort geändert', user_remove: 'Benutzer gelöscht', token_create: 'Token angelegt', token_revoke: 'Token widerrufen',
  grant: 'Recht erteilt', revoke: 'Recht entzogen',
};

export class JournalPage {
  constructor(ctx) {
    this.ctx = ctx;
  }

  mount(host) {
    const cubes = this.ctx.model.cubes;
    this.cube = h('select', { 'aria-label': 'Cube', onchange: () => this.load() }, [h('option', { value: '' }, 'Alle Cubes'), ...cubes.map((c) => h('option', { value: c.name }, c.name))]);
    this.user = h('input', { type: 'search', placeholder: 'Benutzer', 'aria-label': 'Benutzer', onchange: () => this.load() });
    this.body = h('div', {});
    render(host, h('div', { class: 'toolbar' }, h('label', {}, 'Cube ', this.cube), this.user, h('button', { type: 'button', onclick: () => this.load() }, 'Aktualisieren')), this.body);
    this.stop = this.ctx.onEvent(() => { clearTimeout(this.t); this.t = setTimeout(() => this.load(), 500); });
    this.load();
  }

  destroy() {
    this.stop?.();
    clearTimeout(this.t);
  }

  async load() {
    try {
      const r = await api.get(`/journal${api.query({ cube: this.cube.value, user: this.user.value.trim(), limit: 200 })}`);
      const rows = r.entries.map((e) => h('tr', {},
        h('td', {}, formatTime(e.ts)), h('td', {}, e.user), h('td', {}, OPS[e.op] || e.op), h('td', {}, e.cube),
        h('td', { class: 'num' }, e.n_cells ? String(e.n_cells) : ''), h('td', { class: 'muted' }, e.detail)));
      render(this.body, rows.length
        ? h('table', { class: 'list' }, h('thead', {}, h('tr', {}, ['Zeit', 'Benutzer', 'Aktion', 'Cube', 'Zellen', 'Details'].map((t) => h('th', {}, t)))), h('tbody', {}, rows))
        : h('p', { class: 'muted' }, 'Keine Einträge.'));
    } catch (e) {
      this.ctx.toast(e.message, 'error');
    }
  }
}
