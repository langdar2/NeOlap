// Dimension editor: a tree with lazy loading and search, and the changes a modeler can make.

import * as api from './api.js';
import { h, render } from './dom.js';
import { confirmDialog, promptDialog } from './toast.js';

const PAGE = 200;

export class DimensionsPage {
  constructor(ctx) {
    this.ctx = ctx;
    this.dim = null;
    this.rows = []; // flat list of { member, depth, parent }
    this.expanded = new Set();
    this.selected = null;
    this.stop = null;
  }

  mount(host) {
    const dims = this.ctx.model.dimensions;
    if (!dims.length) return render(host, h('div', { class: 'card' }, 'Es gibt noch keine Dimensionen.'));
    this.select = h('select', { 'aria-label': 'Dimension', onchange: () => this.open(this.select.value) }, dims.map((d) => h('option', { value: d.name }, `${d.name} (${d.members})`)));
    this.search = h('input', { type: 'search', placeholder: 'Element suchen (Name oder Alias) …', 'aria-label': 'Element suchen', onchange: () => this.#search() });
    this.tree = h('div', { class: 'tree', role: 'tree', 'aria-label': 'Elemente' });
    this.detail = h('div', { class: 'card' });
    render(host,
      h('div', { class: 'toolbar' }, h('label', {}, 'Dimension ', this.select), this.search),
      h('div', { class: 'split' }, this.tree, this.detail));
    this.stop = this.ctx.onEvent((kind) => { if (kind === 'model') this.#reloadAfterChange(); });
    this.open(dims[0].name);
  }

  destroy() {
    this.stop?.();
  }

  async open(name) {
    this.dim = name;
    this.expanded.clear();
    this.selected = null;
    this.#detail();
    await this.#loadRoots();
  }

  async #page(parent, offset = 0) {
    const r = await api.get(`/dimensions/${api.enc(this.dim)}/members${api.query({ parent, offset, limit: PAGE })}`);
    return r.members;
  }

  async #loadRoots() {
    try {
      const roots = await this.#page(undefined);
      this.rows = roots.map((member) => ({ member, depth: 0, parent: null }));
      this.#draw();
    } catch (e) {
      this.ctx.toast(e.message, 'error');
    }
  }

  async #toggle(i) {
    const row = this.rows[i];
    const name = row.member.name;
    if (this.expanded.has(name)) {
      let end = i + 1;
      while (end < this.rows.length && this.rows[end].depth > row.depth) end++;
      for (let k = i; k < end; k++) this.expanded.delete(this.rows[k].member.name);
      this.rows.splice(i + 1, end - i - 1);
    } else {
      try {
        const kids = await this.#page(name);
        this.expanded.add(name);
        this.rows.splice(i + 1, 0, ...kids.map((member) => ({ member, depth: row.depth + 1, parent: name })));
        if (kids.length === PAGE) this.ctx.toast(`Nur die ersten ${PAGE} Elemente von „${name}“ werden angezeigt. Mit der Suche finden Sie weitere.`);
      } catch (e) {
        this.ctx.toast(e.message, 'error');
        return;
      }
    }
    this.#draw();
  }

  async #search() {
    const text = this.search.value.trim();
    if (!text) return this.#loadRoots();
    try {
      const r = await api.get(`/dimensions/${api.enc(this.dim)}/members${api.query({ search: text, limit: 100 })}`);
      this.rows = r.members.map((member) => ({ member, depth: 0, parent: null }));
      this.#draw();
      if (!r.members.length) this.ctx.toast(`Kein Element mit „${text}“.`);
    } catch (e) {
      this.ctx.toast(e.message, 'error');
    }
  }

  #draw() {
    const items = this.rows.map((row, i) => {
      const m = row.member;
      const arrow = m.children > 0 ? (this.expanded.has(m.name) ? '▾' : '▸') : '·';
      const el = h('div', {
        class: `tree-item t-indent-${Math.min(row.depth, 12)}`, role: 'treeitem', tabindex: -1,
        'aria-selected': this.selected === m.name ? 'true' : 'false',
        'aria-expanded': m.children > 0 ? String(this.expanded.has(m.name)) : null,
      },
      h('span', { class: 'g-toggle', onclick: (e) => { e.stopPropagation(); if (m.children > 0) this.#toggle(i); } }, arrow),
      h('span', {}, m.name), m.leaf ? null : h('span', { class: 'muted' }, ` (${m.children})`));
      el.addEventListener('click', () => { this.selected = m.name; this.#draw(); this.#detail(row); });
      return el;
    });
    render(this.tree, items.length ? items : h('div', { class: 'muted tree-item' }, 'Keine Elemente.'));
  }

  #detail(row) {
    if (!row) return render(this.detail, h('p', { class: 'muted' }, 'Wählen Sie ein Element.'));
    const m = row.member;
    const buttons = this.ctx.can('modeler') ? [
      h('button', { type: 'button', onclick: () => this.#add(m.name) }, 'Kind hinzufügen'),
      h('button', { type: 'button', onclick: () => this.#rename(m.name) }, 'Umbenennen'),
      h('button', { type: 'button', onclick: () => this.#alias(m.name) }, 'Alias'),
      row.parent ? h('button', { type: 'button', onclick: () => this.#weight(row.parent, m.name) }, 'Gewicht') : null,
      h('button', { type: 'button', onclick: () => this.#move(m.name, row.parent) }, 'Verschieben'),
      h('button', { type: 'button', class: 'danger', onclick: () => this.#remove(m) }, 'Löschen'),
    ] : [h('span', { class: 'muted' }, 'Änderungen brauchen die Rolle Modellierer.')];
    render(this.detail,
      h('h2', {}, m.name),
      h('p', { class: 'muted' }, `${m.leaf ? 'Basiselement' : 'Aggregat'} · Ebene ${m.level} · ${m.children} Kinder${m.aliases.length ? ` · Alias: ${m.aliases.join(', ')}` : ''}`),
      h('div', { class: 'row' }, buttons));
  }

  async #apply(ops, done) {
    try {
      await api.patch(`/dimensions/${api.enc(this.dim)}`, { ops });
      this.ctx.toast(done, 'ok');
      await this.#reloadAfterChange();
    } catch (e) {
      this.ctx.toast(e.message, 'error');
    }
  }

  async #reloadAfterChange() {
    await this.ctx.reloadModel();
    const names = [...this.expanded];
    await this.#loadRoots();
    // re-open what was open, top-down (children only exist once their parent is shown)
    for (const n of names) {
      const i = this.rows.findIndex((r) => r.member.name === n);
      if (i >= 0 && !this.expanded.has(n)) await this.#toggle(i);
    }
  }

  async #add(parent) {
    const r = await promptDialog(`Kind unter „${parent}“`, [{ name: 'name', label: 'Name' }, { name: 'weight', label: 'Gewicht', value: '1' }], 'Hinzufügen');
    if (!r || !r.name.trim()) return;
    const weight = Number(r.weight.replace(',', '.'));
    if (!Number.isFinite(weight)) return this.ctx.toast('Das Gewicht muss eine Zahl sein.', 'error');
    await this.#apply([{ op: 'add', name: r.name.trim(), parent, weight }], `„${r.name.trim()}“ hinzugefügt.`);
  }

  async #rename(name) {
    const r = await promptDialog(`„${name}“ umbenennen`, [{ name: 'to', label: 'Neuer Name', value: name }], 'Umbenennen');
    if (r && r.to.trim() && r.to.trim() !== name) await this.#apply([{ op: 'rename', name, to: r.to.trim() }], 'Umbenannt.');
  }

  async #alias(name) {
    const r = await promptDialog(`Alias für „${name}“`, [{ name: 'alias', label: 'Alias' }], 'Hinzufügen');
    if (r && r.alias.trim()) await this.#apply([{ op: 'alias', name, alias: r.alias.trim() }], 'Alias gesetzt.');
  }

  async #weight(parent, name) {
    const r = await promptDialog(`Gewicht von „${name}“ unter „${parent}“`, [{ name: 'weight', label: 'Gewicht (z. B. 1 oder -1)', value: '1' }], 'Setzen');
    if (!r) return;
    const weight = Number(r.weight.replace(',', '.'));
    if (!Number.isFinite(weight)) return this.ctx.toast('Das Gewicht muss eine Zahl sein.', 'error');
    await this.#apply([{ op: 'weight', parent, name, weight }], 'Gewicht gesetzt.');
  }

  async #move(name, from) {
    const r = await promptDialog(`„${name}“ verschieben`, [{ name: 'to', label: 'Neues Elternelement (leer = oberste Ebene)' }], 'Verschieben');
    if (!r) return;
    const op = { op: 'move', name };
    if (from) op.from = from;
    if (r.to.trim()) op.to = r.to.trim();
    await this.#apply([op], 'Verschoben.');
  }

  async #remove(m) {
    const text = m.leaf
      ? 'Die Werte dieses Elements werden in allen Cubes gelöscht, die diese Dimension nutzen. Das lässt sich nicht rückgängig machen.'
      : `Das Aggregat wird entfernt. Seine ${m.children} Kind-Element(e) und deren Werte bleiben erhalten, sie verlieren nur dieses Elternelement. Das lässt sich nicht rückgängig machen.`;
    const ok = await confirmDialog(`„${m.name}“ löschen?`, text, 'Löschen');
    if (ok) await this.#apply([{ op: 'remove', name: m.name }], 'Gelöscht.');
  }
}
