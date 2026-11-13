// Dimension editor: a tree with lazy loading and search, and the changes a modeler can make.

import * as api from './api.js';
import { h, render } from './dom.js';
import { confirmDialog, dialog, promptDialog } from './toast.js';

const PAGE = 200;

export class DimensionsPage {
  constructor(ctx) {
    this.ctx = ctx;
    this.dim = null;
    this.rows = []; // flat list of { member, depth, parent }
    this.expanded = new Set();
    this.selected = null;
    this.stop = null;
    this.ownChange = false; // true while this page renames or deletes a dimension itself
  }

  #label(d) {
    return `${d.name} (${d.members})${d.used === false ? ' · ungenutzt' : ''}`;
  }

  mount(host, keep) {
    this.host = host;
    const dims = this.ctx.model.dimensions;
    const modeler = this.ctx.can('modeler');
    this.stop?.();
    this.stop = this.ctx.onEvent((kind) => { if (kind === 'model' && this.dim) this.#reloadAfterChange(); });
    const create = modeler ? h('button', { type: 'button', onclick: () => this.#createDimension() }, 'Neue Dimension') : null;
    if (!dims.length) {
      this.dim = null;
      return render(host, h('div', { class: 'card' }, h('p', {}, 'Es gibt noch keine Dimensionen.'), create));
    }
    const current = dims.find((d) => d.name === keep) || dims[0];
    this.select = h('select', { 'aria-label': 'Dimension', onchange: () => this.open(this.select.value) },
      dims.map((d) => h('option', { value: d.name, selected: d.name === current.name ? true : null }, this.#label(d))));
    this.search = h('input', { type: 'search', placeholder: 'Element suchen (Name oder Alias) …', 'aria-label': 'Element suchen', onchange: () => this.#search() });
    this.tree = h('div', { class: 'tree', role: 'tree', 'aria-label': 'Elemente' });
    this.detail = h('div', { class: 'card' });
    this.dimButtons = h('span', { class: 'row' });
    render(host,
      h('div', { class: 'toolbar' }, h('label', {}, 'Dimension ', this.select), this.search, create, this.dimButtons),
      h('div', { class: 'split' }, this.tree, this.detail));
    this.open(current.name);
  }

  destroy() {
    this.stop?.();
  }

  async open(name) {
    this.dim = name;
    this.expanded.clear();
    this.selected = null;
    this.#dimButtons();
    this.#detail();
    await this.#loadRoots();
  }

  #info() {
    return this.ctx.model.dimensions.find((d) => d.name === this.dim);
  }

  #dimButtons() {
    const info = this.#info();
    const used = !info || info.used !== false;
    render(this.dimButtons, this.ctx.can('modeler') ? [
      h('button', { type: 'button', onclick: () => this.#add(null) }, 'Element oben anlegen'),
      h('button', { type: 'button', onclick: () => this.#importDialog() }, 'Import …'),
      h('button', { type: 'button', disabled: used ? true : null, title: used ? 'Die Dimension wird von einem Cube genutzt.' : null, onclick: () => this.#renameDimension() }, 'Dimension umbenennen'),
      h('button', { type: 'button', class: 'danger', disabled: used ? true : null, title: used ? 'Die Dimension wird von einem Cube genutzt.' : null, onclick: () => this.#deleteDimension() }, 'Dimension löschen'),
    ] : null);
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
      h('button', { type: 'button', onclick: () => this.#attr(m.name) }, 'Attribut'),
      row.parent ? h('button', { type: 'button', onclick: () => this.#weight(row.parent, m.name) }, 'Gewicht') : null,
      h('button', { type: 'button', onclick: () => this.#move(m.name, row.parent) }, 'Verschieben'),
      h('button', { type: 'button', class: 'danger', onclick: () => this.#remove(m) }, 'Löschen'),
    ] : [h('span', { class: 'muted' }, 'Änderungen brauchen die Rolle Modellierer.')];
    render(this.detail,
      h('h2', {}, m.name),
      h('p', { class: 'muted' }, `${m.leaf ? 'Basiselement' : 'Aggregat'} · Ebene ${m.level} · ${m.children} Kinder${m.aliases.length ? ` · Alias: ${m.aliases.join(', ')}` : ''}`),
      Object.keys(m.attributes || {}).length ? h('p', { class: 'muted' }, `Attribute: ${Object.entries(m.attributes).map(([k, v]) => `${k} = ${v}`).join(' · ')}`) : null,
      h('div', { class: 'row' }, buttons));
  }

  async #createDimension() {
    const r = await promptDialog('Neue Dimension', [
      { name: 'name', label: 'Name' },
      { name: 'kind', label: 'Art', type: 'select', options: ['generic', 'time', 'version', 'measures'], value: 'generic' },
    ], 'Anlegen');
    if (!r || !r.name.trim()) return;
    try {
      await api.post('/dimensions', { name: r.name.trim(), kind: r.kind });
      await this.ctx.reloadModel();
      this.ctx.toast(`Dimension „${r.name.trim()}“ angelegt.`, 'ok');
      this.mount(this.host, r.name.trim());
    } catch (e) {
      this.ctx.toast(e.message, 'error');
    }
  }

  async #renameDimension() {
    const r = await promptDialog(`Dimension „${this.dim}“ umbenennen`, [{ name: 'to', label: 'Neuer Name', value: this.dim }], 'Umbenennen');
    if (!r || !r.to.trim() || r.to.trim() === this.dim) return;
    this.ownChange = true;
    try {
      await api.post(`/dimensions/${api.enc(this.dim)}/rename`, { to: r.to.trim() });
      await this.ctx.reloadModel();
      this.ctx.toast('Umbenannt.', 'ok');
      this.mount(this.host, r.to.trim());
    } catch (e) {
      this.ctx.toast(e.message, 'error');
    } finally {
      this.ownChange = false;
    }
  }

  async #deleteDimension() {
    const info = this.#info();
    const ok = await confirmDialog(`Dimension „${this.dim}“ löschen?`, `Die Dimension mit ${info ? info.members : 0} Element(en) wird entfernt. Das lässt sich nicht rückgängig machen.`, 'Löschen');
    if (!ok) return;
    this.ownChange = true;
    try {
      await api.del(`/dimensions/${api.enc(this.dim)}`);
      await this.ctx.reloadModel();
      this.ctx.toast('Dimension gelöscht.', 'ok');
      this.mount(this.host);
    } catch (e) {
      this.ctx.toast(e.message, 'error');
    } finally {
      this.ownChange = false;
    }
  }

  async #attr(name) {
    const r = await promptDialog(`Attribut von „${name}“`, [{ name: 'attribute', label: 'Attribut' }, { name: 'value', label: 'Wert (Zahl oder Text)' }], 'Setzen');
    if (!r || !r.attribute.trim()) return;
    const raw = r.value.trim();
    const num = raw.replace(',', '.');
    const value = raw !== '' && Number.isFinite(Number(num)) ? Number(num) : raw;
    await this.#apply([{ op: 'attr', name, attribute: r.attribute.trim(), value }], 'Attribut gesetzt.');
  }

  /** Import of members from CSV (file or pasted text) with a dry run first. */
  async #importDialog() {
    const text = h('textarea', { rows: 8, class: 'mono', 'aria-label': 'CSV-Text', placeholder: 'child;parent;weight\nQ1;Jahr;1\nJan;Q1;1' });
    const file = h('input', { type: 'file', accept: '.csv,.txt,text/csv,text/plain', 'aria-label': 'CSV-Datei', onchange: async () => { if (file.files[0]) text.value = await file.files[0].text(); } });
    const format = h('select', { 'aria-label': 'Format' }, h('option', { value: 'parent-child' }, 'Eltern-Kind (child;parent;weight;alias;Attribute …)'), h('option', { value: 'levels' }, 'Ebenen (L1;L2;…, oberste zuerst)'));
    const delimiter = h('select', { 'aria-label': 'Trennzeichen' }, h('option', { value: ';' }, '; Semikolon'), h('option', { value: ',' }, ', Komma'), h('option', { value: 'tab' }, 'Tabulator'));
    const mode = h('select', { 'aria-label': 'Modus' }, h('option', { value: 'add' }, 'Nur hinzufügen'), h('option', { value: 'sync' }, 'Abgleichen (fehlende Elemente werden gelöscht)'));
    const body = h('div', { class: 'form' },
      h('label', {}, 'Datei', file), h('label', {}, 'oder Text', text),
      h('label', {}, 'Format', format), h('label', {}, 'Trennzeichen', delimiter), h('label', {}, 'Modus', mode));
    const go = await dialog(`Import in „${this.dim}“`, body, [{ label: 'Abbrechen', value: false }, { label: 'Vorschau', value: true, kind: 'primary' }]);
    if (!go) return;
    if (!text.value.trim()) return this.ctx.toast('Es gibt nichts zu importieren.', 'error');
    const req = (dry_run) => api.post(`/dimensions/${api.enc(this.dim)}/import`, { text: text.value, format: format.value, delimiter: delimiter.value, mode: mode.value, dry_run });
    try {
      const p = await req(true);
      const removed = p.removed.length ? ` Entfernt würden ${p.removed.length} Element(e): ${p.removed.slice(0, 10).join(', ')}${p.removed.length > 10 ? ' …' : ''}. Deren Werte werden in allen Cubes gelöscht.` : '';
      const ok = await confirmDialog('Import ausführen?', `${p.rows} Zeile(n), ${p.members_added} neue(s) Element(e), danach ${p.members} Elemente (${p.base_members} Basiselemente).${removed}`, 'Importieren');
      if (!ok) return;
      await req(false);
      this.ctx.toast('Import abgeschlossen.', 'ok');
      await this.ctx.reloadModel();
      this.mount(this.host, this.dim);
    } catch (e) {
      this.ctx.toast(e.message, 'error');
    }
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
    if (this.ownChange) return; // rename and delete rebuild the page themselves
    await this.ctx.reloadModel();
    if (!this.#info()) {
      // someone else renamed or deleted the dimension: start over with what exists
      this.mount(this.host);
      return;
    }
    for (const o of this.select.options) {
      const d = this.ctx.model.dimensions.find((x) => x.name === o.value);
      if (d) o.textContent = this.#label(d);
    }
    const names = [...this.expanded];
    await this.#loadRoots();
    // re-open what was open, top-down (children only exist once their parent is shown)
    for (const n of names) {
      const i = this.rows.findIndex((r) => r.member.name === n);
      if (i >= 0 && !this.expanded.has(n)) await this.#toggle(i);
    }
  }

  async #add(parent) {
    const r = await promptDialog(parent ? `Kind unter „${parent}“` : `Element auf oberster Ebene in „${this.dim}“`, [{ name: 'name', label: 'Name' }, { name: 'weight', label: 'Gewicht', value: '1' }], 'Hinzufügen');
    if (!r || !r.name.trim()) return;
    const weight = Number(r.weight.replace(',', '.'));
    if (!Number.isFinite(weight)) return this.ctx.toast('Das Gewicht muss eine Zahl sein.', 'error');
    await this.#apply([parent ? { op: 'add', name: r.name.trim(), parent, weight } : { op: 'add', name: r.name.trim() }], `„${r.name.trim()}“ hinzugefügt.`);
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
