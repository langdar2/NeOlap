// Pivot page: choose a cube, arrange dimensions, expand hierarchies, edit cells.

import * as api from './api.js';
import { h, render, formatNumber } from './dom.js';
import { Grid } from './grid.js';
import { Axis, PivotState, pasteWrites, normalizeInput, looksLikeInput } from './pivot-state.js';
import { confirmDialog, dialog } from './toast.js';

const MEMBER_PAGE = 500;
/** Inputs that reach more base cells than this are confirmed first. */
const CONFIRM_ABOVE = 1000;

export class PivotPage {
  constructor(ctx) {
    this.ctx = ctx;
    this.pivot = null;
    this.grid = null;
    this.generation = 0;
    this.stopEvents = null;
    this.reloadTimer = null;
    this.busy = false;
    this.members = new Map(); // dimension → root members (cache for the filter pickers)
    this.reachToken = 0;
    this.reachTimer = null;
  }

  mount(host) {
    this.host = host;
    const cubes = this.ctx.model.cubes;
    if (!cubes.length) {
      render(host, h('div', { class: 'card' }, h('h2', {}, 'Keine Cubes'), h('p', { class: 'muted' }, this.ctx.can('modeler') ? 'Es gibt noch keinen Cube, auf den Sie Zugriff haben. Legen Sie ihn mit der Kommandozeile an (neolap model import).' : 'Sie haben noch auf keinen Cube Zugriff. Bitte wenden Sie sich an einen Administrator.')));
      return;
    }
    this.cubeSelect = h('select', { 'aria-label': 'Cube', onchange: () => this.openCube(this.cubeSelect.value) },
      cubes.map((c) => h('option', { value: c.name }, c.name)));
    this.banner = h('div', { class: 'hidden' });
    this.status = h('div', { class: 'statusbar', 'aria-live': 'polite' });
    this.axes = h('div', { class: 'pivot-side' });
    this.gridHost = h('div', {});
    this.toolbar = h('div', { class: 'toolbar' },
      h('label', {}, 'Cube ', this.cubeSelect),
      this.#button('Neu laden', () => this.load()),
      this.#button('Rückgängig', () => this.undo(), 'Letzte eigene Änderung zurücknehmen (Strg+Z)'),
      h('label', {}, h('input', { type: 'checkbox', id: 'sup', onchange: (e) => { this.pivot.suppressEmpty = e.target.checked; this.#show(); } }), ' Leere ausblenden'),
      h('label', {}, h('input', { type: 'checkbox', id: 'rules', checked: true, onchange: (e) => { this.pivot.useRules = e.target.checked; this.load(); } }), ' Regeln'),
      h('span', { class: 'spacer' }),
      this.#button('CSV', () => this.export('csv')),
      this.#button('Excel', () => this.export('xlsx')));
    render(host, this.toolbar, this.banner,
      h('div', { class: 'pivot' }, this.axes, h('div', { class: 'pivot-main' }, this.gridHost, this.status)));
    this.grid = new Grid(this.gridHost, {
      onSelect: () => this.#updateStatus(),
      onEdit: (cells, text, info) => this.#edit(cells, text, info),
      onToggle: (axis, index) => this.#toggle(axis, index),
      onPaste: (rows, rect) => this.#paste(rows, rect),
    });
    this.keyHandler = (e) => {
      if ((e.ctrlKey || e.metaKey) && e.key === 'z' && !e.shiftKey && !this.grid?.editor && document.activeElement?.tagName !== 'INPUT') {
        e.preventDefault();
        this.undo();
      }
    };
    document.addEventListener('keydown', this.keyHandler);
    this.stopEvents = this.ctx.onEvent((kind, data) => this.#onEvent(kind, data));
    const wanted = sessionStorageGet('cube');
    const first = cubes.find((c) => c.name === wanted) ? wanted : cubes[0].name;
    this.cubeSelect.value = first;
    this.openCube(first);
  }

  destroy() {
    this.stopEvents?.();
    this.grid?.destroy();
    clearTimeout(this.reloadTimer);
    clearTimeout(this.reachTimer);
    if (this.keyHandler) document.removeEventListener('keydown', this.keyHandler);
  }

  #button(label, fn, title) {
    return h('button', { type: 'button', title, onclick: fn }, label);
  }

  // ---------------------------------------------------------------- opening a cube

  async openCube(name) {
    sessionStorageSet('cube', name);
    const cube = this.ctx.model.cubes.find((c) => c.name === name);
    this.pivot = new PivotState(cube);
    this.generation = cube.generation;
    this.#message(null);
    const dims = cube.dimensions;
    // first dimension on the rows, the last on the columns (usually the measures), the rest are filters
    this.pivot.rowDim = dims[0];
    this.pivot.colDim = dims.length > 1 ? dims[dims.length - 1] : dims[0];
    try {
      for (const d of dims) await this.#loadRoots(d);
      this.pivot.filters = {};
      for (const d of this.pivot.filterDimensions) this.pivot.filters[d] = this.members.get(d)[0]?.name;
      await this.#setAxis('row', this.pivot.rowDim);
      await this.#setAxis('col', this.pivot.colDim);
    } catch (e) {
      this.#error(e);
      return;
    }
    this.#drawAxes();
    await this.load();
  }

  async #loadRoots(dim) {
    if (this.members.has(dim)) return;
    const r = await api.get(`/dimensions/${api.enc(dim)}/members${api.query({ limit: MEMBER_PAGE })}`);
    this.members.set(dim, r.members);
  }

  async #setAxis(which, dim) {
    const axis = new Axis(dim);
    await this.#loadRoots(dim);
    axis.setRoots(this.members.get(dim));
    if (which === 'row') this.pivot.rows = axis; else this.pivot.cols = axis;
    // a dimension whose only root is an aggregate starts expanded one level, so there is something to see
    if (axis.length === 1 && axis.canExpand(0)) await this.#expand(axis, 0);
  }

  async #expand(axis, index) {
    if (axis.needsChildren(index)) {
      const name = axis.at(index).member.name;
      const r = await api.get(`/dimensions/${api.enc(axis.dimension)}/members${api.query({ parent: name, limit: MEMBER_PAGE })}`);
      axis.setChildren(name, r.members);
    }
    return axis.expand(index);
  }

  // ---------------------------------------------------------------- loading and showing

  async load() {
    if (!this.pivot?.rows) return;
    this.busy = true;
    try {
      const answer = await api.post(`/cubes/${api.enc(this.pivot.cube.name)}/view`, this.pivot.request());
      this.generation = this.pivot.setAnswer(answer);
      this.#message(null);
      this.#show();
    } catch (e) {
      this.#error(e);
    } finally {
      this.busy = false;
    }
  }

  #show(keepScroll = true) {
    this.grid.setData(this.pivot.gridData(), { keepScroll });
    this.#updateStatus();
    this.grid.focus();
  }

  #updateStatus() {
    if (!this.pivot?.answer) return;
    const sel = this.grid.selection;
    const bits = [h('span', {}, `Generation ${this.generation}`)];
    if (!sel.isEmpty) {
      const a = sel.active;
      const cell = this.pivot.gridData().cell(a.row, a.col);
      const addr = this.pivot.cellAddress(a.row, a.col).join(' · ');
      bits.push(h('span', {}, addr));
      bits.push(h('span', {}, { base: 'Basiszelle', agg: 'Aggregat', rule: 'Regelzelle', error: 'Fehler' }[cell.kind] + (cell.writable ? ', beschreibbar' : ', gesperrt')));
      // an input on an aggregate is spread over base cells: say how many, before anyone types
      if (cell.kind === 'agg' && cell.writable) {
        const note = h('span', { class: 'reach' }, '…');
        bits.push(note);
        this.#reach(addr, a, note);
      }
      if (!sel.isSingle) {
        const s = this.pivot.summarize(sel.rect);
        bits.push(h('span', { class: 'sum' }, 'Summe ', h('b', {}, formatNumber(s.sum)), ` (${s.count} Zellen)`));
      }
    }
    render(this.status, ...bits);
  }

  /** Fills `note` with the number of base cells an input on the active cell reaches. */
  #reach(addr, active, note) {
    clearTimeout(this.reachTimer);
    const token = ++this.reachToken;
    this.reachTimer = setTimeout(async () => {
      try {
        const query = {};
        this.pivot.dimensions.forEach((d, i) => { query[d] = this.pivot.cellAddress(active.row, active.col)[i]; });
        const r = await api.get(`/cubes/${api.enc(this.pivot.cube.name)}/cell${api.query(query)}`);
        // an answer for a cell the user already left is of no use
        if (token !== this.reachToken || !r.area) return;
        const base = BigInt(r.area.base_cells);
        note.textContent = `wirkt auf ${base.toLocaleString('de-DE')} Basiszellen, ${r.area.filled_cells.toLocaleString('de-DE')} davon gefüllt`;
      } catch { note.textContent = ''; }
    }, 250);
  }

  #message(text, kind = '') {
    if (!text) {
      this.banner.className = 'hidden';
      render(this.banner);
      return;
    }
    this.banner.className = `banner ${kind}`;
    render(this.banner, h('span', {}, text));
  }

  #error(e) {
    this.#message(e.message || String(e), 'error');
  }

  // ---------------------------------------------------------------- axes and filters

  #drawAxes() {
    const p = this.pivot;
    const zone = (title, which, dims) => {
      const el = h('div', { class: 'zone', 'data-zone': which },
        h('h3', {}, title),
        h('div', { class: 'chips' }, dims.map((d) => this.#chip(d, which))));
      el.addEventListener('dragover', (e) => { e.preventDefault(); el.classList.add('drop'); });
      el.addEventListener('dragleave', () => el.classList.remove('drop'));
      el.addEventListener('drop', (e) => {
        e.preventDefault();
        el.classList.remove('drop');
        this.#moveDimension(e.dataTransfer.getData('text/plain'), which);
      });
      return el;
    };
    render(this.axes,
      zone('Zeilen', 'row', [p.rowDim]),
      zone('Spalten', 'col', [p.colDim]),
      zone('Filter', 'filter', p.filterDimensions));
  }

  #chip(dim, where) {
    const chip = h('span', { class: 'chip', draggable: 'true', title: 'Auf Zeilen, Spalten oder Filter ziehen' }, dim);
    chip.addEventListener('dragstart', (e) => { e.dataTransfer.setData('text/plain', dim); chip.classList.add('drag'); });
    chip.addEventListener('dragend', () => chip.classList.remove('drag'));
    if (where === 'filter') {
      const select = h('select', { 'aria-label': `Filter ${dim}`, onchange: () => { this.pivot.filters[dim] = select.value; this.load(); } });
      this.#fillFilter(select, dim);
      const search = h('input', { type: 'search', placeholder: 'suchen …', 'aria-label': `${dim} suchen`, onchange: async () => { await this.#searchInto(select, dim, search.value); search.value = ''; } });
      chip.append(select);
      chip.append(search);
    }
    return chip;
  }

  #fillFilter(select, dim) {
    const current = this.pivot.filters[dim];
    const members = this.members.get(dim) || [];
    const names = new Set(members.map((m) => m.name));
    if (current && !names.has(current)) select.append(h('option', { value: current }, current));
    for (const m of members) select.append(h('option', { value: m.name, selected: m.name === current ? true : null }, m.name));
    select.value = current;
  }

  /** The filter picker for a dimension with many members: search by name and alias. */
  async #searchInto(select, dim, text) {
    if (!text.trim()) return;
    try {
      const r = await api.get(`/dimensions/${api.enc(dim)}/members${api.query({ search: text.trim(), limit: 50 })}`);
      if (!r.members.length) return this.ctx.toast(`Kein Element mit „${text}“ in ${dim}.`);
      select.replaceChildren(...r.members.map((m) => h('option', { value: m.name }, m.name)));
      this.pivot.filters[dim] = r.members[0].name;
      select.value = r.members[0].name;
      this.load();
    } catch (e) {
      this.#error(e);
    }
  }

  async #moveDimension(dim, where) {
    const p = this.pivot;
    if (!dim || !p.dimensions.includes(dim)) return;
    try {
      if (where === 'row' && dim !== p.rowDim) {
        if (dim === p.colDim) p.colDim = p.rowDim;
        p.rowDim = dim;
      } else if (where === 'col' && dim !== p.colDim) {
        if (dim === p.rowDim) p.rowDim = p.colDim;
        p.colDim = dim;
      } else if (where === 'filter') {
        // a dimension leaves an axis by giving its place to a filter dimension
        const free = p.filterDimensions[0];
        if (!free || (dim !== p.rowDim && dim !== p.colDim)) return;
        if (dim === p.rowDim) p.rowDim = free; else p.colDim = free;
      } else {
        return;
      }
      for (const d of p.dimensions) if (!p.filters[d] && d !== p.rowDim && d !== p.colDim) p.filters[d] = (await this.#loadRootsReturn(d))[0]?.name;
      await this.#setAxis('row', p.rowDim);
      await this.#setAxis('col', p.colDim);
      this.#drawAxes();
      this.pivot.suppressEmpty = document.getElementById('sup')?.checked ?? false;
      await this.load();
      this.grid.setData(this.pivot.gridData(), { keepScroll: false });
    } catch (e) {
      this.#error(e);
    }
  }

  async #loadRootsReturn(dim) {
    await this.#loadRoots(dim);
    return this.members.get(dim);
  }

  async #toggle(axisName, shownIndex) {
    const p = this.pivot;
    const axis = axisName === 'row' ? p.rows : p.cols;
    // the grid may hide empty rows, so its index is not the axis index
    const index = (axisName === 'row' ? p.shown.rows : p.shown.cols)[shownIndex];
    try {
      if (axis.isExpanded(index)) axis.collapse(index);
      else if (!(await this.#expand(axis, index))) return;
      await this.load();
    } catch (e) {
      this.#error(e);
    }
  }

  // ---------------------------------------------------------------- editing

  async #edit(cells, text, info) {
    if (info?.refused) {
      const kind = info.cell.kind;
      this.ctx.toast(kind === 'rule' ? 'Diese Zelle wird von einer Regel berechnet.' : kind === 'agg' ? 'Auf diese Zelle darf nicht geschrieben werden (Rechte, Gewichte oder Regeln im Bereich).' : 'Diese Zelle ist gesperrt.', 'error');
      return;
    }
    const t = normalizeInput(text ?? '');
    if (!looksLikeInput(t)) {
      this.ctx.toast(`„${text}“ ist keine gültige Eingabe. Erlaubt: 500, #500, +50, -50, +5%, @Referenz:500 oder leer zum Löschen.`, 'error');
      return;
    }
    const writable = cells.filter(([r, c]) => this.pivot.gridData().cell(r, c).writable);
    if (writable.length < cells.length) this.ctx.toast(`${cells.length - writable.length} gesperrte Zelle(n) übersprungen.`);
    if (!writable.length) return;
    if (!(await this.#confirmReach(writable, t))) return;
    await this.#write(writable.map(([r, c]) => ({ row: r, col: c, text: t })));
  }

  /**
   * Asks before an input that reaches many cells: many selected cells, or one aggregate that
   * spreads over many base cells (a splash on a grand total can touch every cell of the cube).
   * The count comes from the server. If it cannot be determined, the user is asked rather than
   * written to silently.
   */
  async #confirmReach(cells, text) {
    let reach = cells.length;
    let known = true;
    const aggregates = cells.filter(([r, c]) => this.pivot.gridData().cell(r, c).kind === 'agg');
    if (aggregates.length) {
      try {
        reach = cells.length - aggregates.length;
        // one request per aggregate, but never more than a handful: beyond that the answer is "many"
        for (const [r, c] of aggregates.slice(0, 20)) {
          const names = this.pivot.cellAddress(r, c);
          const query = Object.fromEntries(this.pivot.dimensions.map((d, i) => [d, names[i]]));
          const res = await api.get(`/cubes/${api.enc(this.pivot.cube.name)}/cell${api.query(query)}`);
          reach += res.area ? Number(res.area.filled_cells) + (text.startsWith('#') ? Number(BigInt(res.area.base_cells) - BigInt(res.area.filled_cells)) : 0) : 1;
        }
        if (aggregates.length > 20) reach = Infinity;
      } catch {
        known = false;
      }
    }
    if (known && reach <= CONFIRM_ABOVE) return true;
    const what = text === '' ? 'leer (löschen)' : text;
    const how = !known ? 'Wie viele Zellen betroffen sind, konnte nicht ermittelt werden.' : reach === Infinity ? 'Sehr viele Zellen sind betroffen.' : `Bis zu ${reach.toLocaleString('de-DE')} Basiszellen werden geändert.`;
    return confirmDialog('Viele Zellen ändern', `„${what}“: ${how} Mit „Rückgängig“ lässt sich das zurücknehmen, solange niemand sonst die Zellen ändert.`, 'Schreiben');
  }

  /** Sends writes one by one in order; stops at the first problem. */
  async #write(writes) {
    const p = this.pivot;
    let changed = 0;
    for (const w of writes) {
      p.pending.set(`${w.row}:${w.col}`, { text: w.text === '' ? '' : w.text });
    }
    this.grid.refresh();
    for (const w of writes) {
      const key = `${w.row}:${w.col}`;
      try {
        const r = await api.post(`/cubes/${api.enc(p.cube.name)}/write`, { cell: p.cellAddress(w.row, w.col), value: w.text, generation: this.generation });
        this.generation = r.generation;
        changed += r.cells_changed;
        p.pending.delete(key);
      } catch (e) {
        // nothing after the failed write is sent, so none of it may be shown as written; the
        // cells show the real values again before the user is asked anything
        p.pending.clear();
        this.grid.refresh();
        changed += await this.#writeFailed(e, w);
        break;
      }
    }
    await this.load();
    if (changed) this.ctx.toast(`${changed.toLocaleString('de-DE')} Zelle(n) geändert.`, 'ok');
  }

  /** Handles a failed write. Returns how many cells were changed by a retry (0 if none). */
  async #writeFailed(e, w) {
    const p = this.pivot;
    if (e.isConflict && e.details) {
      p.conflicts.set(`${w.row}:${w.col}`, e.details);
      this.grid.refresh();
      const now = e.details.value === null || e.details.value === undefined ? 'leer' : formatNumber(e.details.value);
      const body = h('div', {}, h('p', {}, `„${e.details.cell}“ wurde inzwischen von ${e.details.user} geändert. Aktueller Wert: `, h('b', {}, now), '.'), h('p', { class: 'muted' }, `Ihre Eingabe: ${w.text === '' ? 'leer (löschen)' : w.text}`));
      const choice = await dialog('Änderung von jemand anderem', body, [
        { label: 'Meine Eingabe verwerfen', value: 'drop' },
        { label: 'Trotzdem überschreiben', value: 'overwrite', kind: 'primary' },
      ]);
      if (choice === 'overwrite') {
        try {
          // the view is refreshed first, so the write is checked against what the user decided on
          await this.load();
          const r = await api.post(`/cubes/${api.enc(p.cube.name)}/write`, { cell: p.cellAddress(w.row, w.col), value: w.text, generation: this.generation });
          this.generation = r.generation;
          return r.cells_changed;
        } catch (e2) {
          this.#error(e2);
        }
      }
      return 0;
    }
    this.ctx.toast(e.message, 'error');
    return 0;
  }

  async #paste(rows, rect) {
    const { writes, skipped } = pasteWrites(rows, rect, this.pivot.gridData());
    for (const w of writes) {
      if (!looksLikeInput(w.text)) {
        this.ctx.toast(`„${w.text}“ in der Einfügung ist keine gültige Eingabe. Es wurde nichts geschrieben.`, 'error');
        return;
      }
    }
    if (!writes.length) return this.ctx.toast('Keine beschreibbare Zelle im Einfügebereich.', 'error');
    if (skipped.length && !(await confirmDialog('Teilweise einfügen', `${skipped.length} Zelle(n) im Einfügebereich sind gesperrt oder liegen außerhalb und werden übersprungen. ${writes.length} werden geschrieben.`, 'Einfügen'))) return;
    if (!(await this.#confirmReach(writes.map((w) => [w.row, w.col]), 'Einfügung'))) return;
    await this.#write(writes);
  }

  async undo() {
    try {
      const r = await api.post(`/cubes/${api.enc(this.pivot.cube.name)}/undo`);
      this.generation = r.generation;
      this.ctx.toast(`Rückgängig: ${r.cells_changed} Zelle(n).`, 'ok');
      await this.load();
    } catch (e) {
      this.ctx.toast(e.isConflict ? `Nicht möglich: ${e.details?.user ?? 'jemand'} hat betroffene Zellen inzwischen geändert.` : e.message, 'error');
    }
  }

  async export(format) {
    try {
      const p = this.pivot;
      const area = Object.entries(p.filters).map(([d, m]) => `${d}=${m}`).join(',');
      const { blob, name } = await api.download(`/cubes/${api.enc(p.cube.name)}/export${api.query({ format, area, aggregates: true, rules: true })}`);
      const a = h('a', { href: '#', download: name });
      const url = URL.createObjectURL(blob);
      a.setAttribute('href', url);
      document.body.append(a);
      a.click();
      a.remove();
      setTimeout(() => URL.revokeObjectURL(url), 5000);
    } catch (e) {
      this.ctx.toast(e.message, 'error');
    }
  }

  // ---------------------------------------------------------------- live updates

  #onEvent(kind, data) {
    if (!this.pivot) return;
    const mine = data && data.cube && data.cube.toLowerCase() === this.pivot.cube.name.toLowerCase();
    if (kind === 'resync' || (kind === 'model' && mine)) {
      // structure changed or this client lagged behind: members and the view are stale
      this.members.clear();
      this.ctx.reloadModel().then(() => this.openCube(this.pivot.cube.name));
      return;
    }
    if (!mine || this.busy) return;
    if (data.user === this.ctx.me.user && data.generation <= this.generation) return; // our own change
    this.#message(`${data.user} hat ${data.cells_changed ?? 0} Zelle(n) geändert. Die Ansicht wird aktualisiert.`);
    clearTimeout(this.reloadTimer);
    this.reloadTimer = setTimeout(async () => {
      const before = this.pivot.answer;
      await this.load();
      this.#flashChanges(before);
      this.#message(null);
    }, 400);
  }

  /** Marks cells whose value differs from the previous answer. */
  #flashChanges(before) {
    const after = this.pivot.answer;
    if (!before || !after || before.values.length !== after.values.length) return;
    const shown = this.pivot.shown;
    const cells = [];
    shown.rows.forEach((r, i) => shown.cols.forEach((c, j) => {
      if (before.values[r]?.[c] !== after.values[r][c]) cells.push([i, j]);
    }));
    this.grid.flash(cells);
  }
}

function sessionStorageGet(key) {
  try { return sessionStorage.getItem(`neolap.${key}`); } catch { return null; }
}

function sessionStorageSet(key, value) {
  try { sessionStorage.setItem(`neolap.${key}`, value); } catch { /* storage may be blocked */ }
}
