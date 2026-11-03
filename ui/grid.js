// Pivot grid: a self-contained component.
//
// It knows nothing about NeOlap's API. The owner gives it a `data` provider (headers and cell
// values) and gets events back (`select`, `edit`, `toggle`, `copy`, `paste`). Only the cells in
// the viewport are in the DOM (see grid-model.js for which ones); scrolling reuses them, so a view
// with thousands of rows stays as cheap as one with ten.
//
// Layout: a fixed corner, a header row for the columns, a header column for the rows, and a body
// that scrolls both. The headers follow the body's scroll position by transform.

import { GridSelection, offsetsFor, visibleRange, scrollToReveal, NAV_KEYS } from './grid-model.js';
import { h, formatNumber } from './dom.js';

const ROW_HEIGHT = 26;
const COL_WIDTH = 110;
const HEADER_WIDTH = 220;
const HEADER_HEIGHT = 30;

/**
 * @typedef {object} GridData
 * @property {number} rowCount
 * @property {number} colCount
 * @property {(row:number) => {label:string, depth:number, expandable:boolean, expanded:boolean, aggregate:boolean}} rowHeader
 * @property {(col:number) => {label:string, depth:number, expandable:boolean, expanded:boolean, aggregate:boolean}} colHeader
 * @property {(row:number, col:number) => {value:(number|string|null), kind:string, writable:boolean, changed?:boolean}} cell
 */

export class Grid {
  /** @param {HTMLElement} host */
  constructor(host, { onSelect, onEdit, onToggle, onCopy, onPaste, onContext } = {}) {
    this.host = host;
    this.handlers = { onSelect, onEdit, onToggle, onCopy, onPaste, onContext };
    this.selection = new GridSelection(0, 0);
    /** @type {GridData} */
    this.data = { rowCount: 0, colCount: 0, rowHeader: () => ({}), colHeader: () => ({}), cell: () => ({}) };
    this.rowOffsets = offsetsFor(0, () => ROW_HEIGHT);
    this.colOffsets = offsetsFor(0, () => COL_WIDTH);
    this.editor = null;
    this.cells = new Map(); // "row:col" → element
    this.rowHeaders = new Map();
    this.colHeaders = new Map();
    this.#build();
  }

  #build() {
    this.corner = h('div', { class: 'g-corner' });
    this.colHead = h('div', { class: 'g-colhead' });
    this.colHeadInner = h('div', { class: 'g-colhead-inner' });
    this.colHead.append(this.colHeadInner);
    this.rowHead = h('div', { class: 'g-rowhead' });
    this.rowHeadInner = h('div', { class: 'g-rowhead-inner' });
    this.rowHead.append(this.rowHeadInner);
    this.body = h('div', { class: 'g-body', tabindex: 0, role: 'grid', 'aria-label': 'Pivot' });
    this.bodyInner = h('div', { class: 'g-body-inner' });
    this.body.append(this.bodyInner);
    this.empty = h('div', { class: 'g-empty' });
    this.host.classList.add('g-host');
    this.host.replaceChildren(this.corner, this.colHead, this.rowHead, this.body, this.empty);
    this.body.addEventListener('scroll', () => this.#onScroll(), { passive: true });
    this.body.addEventListener('keydown', (e) => this.#onKey(e));
    this.body.addEventListener('mousedown', (e) => this.#onMouseDown(e));
    this.body.addEventListener('dblclick', (e) => this.#onDblClick(e));
    this.body.addEventListener('contextmenu', (e) => this.#onContext(e));
    this.body.addEventListener('paste', (e) => this.#onPaste(e));
    this.body.addEventListener('copy', (e) => this.#onCopy(e));
    this.rowHead.addEventListener('click', (e) => this.#onHeaderClick(e, 'row'));
    this.colHead.addEventListener('click', (e) => this.#onHeaderClick(e, 'col'));
    this.resizeObserver = new ResizeObserver(() => this.#layout());
    this.resizeObserver.observe(this.host);
  }

  destroy() {
    this.resizeObserver.disconnect();
  }

  /** Sets new data (a new view). Keeps the selection where it still fits. */
  setData(data, { keepScroll = true } = {}) {
    this.data = data;
    this.rowOffsets = offsetsFor(data.rowCount, () => ROW_HEIGHT);
    this.colOffsets = offsetsFor(data.colCount, () => COL_WIDTH);
    this.selection.resize(data.rowCount, data.colCount);
    this.#closeEditor(false);
    if (!keepScroll) {
      this.body.scrollTop = 0;
      this.body.scrollLeft = 0;
    }
    this.bodyInner.style.height = `${this.rowOffsets[data.rowCount]}px`;
    this.bodyInner.style.width = `${this.colOffsets[data.colCount]}px`;
    this.empty.textContent = data.rowCount === 0 || data.colCount === 0 ? (data.emptyText || '') : '';
    this.empty.classList.toggle('g-empty-on', data.rowCount === 0 || data.colCount === 0);
    this.cells.forEach((el) => el.remove());
    this.rowHeaders.forEach((el) => el.remove());
    this.colHeaders.forEach((el) => el.remove());
    this.cells.clear();
    this.rowHeaders.clear();
    this.colHeaders.clear();
    this.#layout();
  }

  /** Redraws the cells in view (after values changed), keeping everything else. */
  refresh() {
    this.cells.forEach((el, key) => {
      const [r, c] = key.split(':').map(Number);
      this.#paintCell(el, r, c);
    });
    this.#paintSelection();
  }

  /** Marks cells as changed for a moment (a live update from another user). */
  flash(cells) {
    for (const [r, c] of cells) {
      const el = this.cells.get(`${r}:${c}`);
      if (!el) continue;
      el.classList.add('g-flash');
      setTimeout(() => el.classList.remove('g-flash'), 1500);
    }
  }

  focus() {
    this.body.focus({ preventScroll: true });
  }

  get activeCell() {
    return this.selection.active;
  }

  // ---------------------------------------------------------------- layout and drawing

  #layout() {
    const width = this.host.clientWidth;
    const height = this.host.clientHeight;
    this.viewW = Math.max(0, width - HEADER_WIDTH);
    this.viewH = Math.max(0, height - HEADER_HEIGHT);
    this.#draw();
  }

  #onScroll() {
    this.colHeadInner.style.transform = `translateX(${-this.body.scrollLeft}px)`;
    this.rowHeadInner.style.transform = `translateY(${-this.body.scrollTop}px)`;
    this.#draw();
  }

  #draw() {
    const rr = visibleRange(this.rowOffsets, this.body.scrollTop, this.viewH);
    const cr = visibleRange(this.colOffsets, this.body.scrollLeft, this.viewW);
    this.#sync(this.rowHeaders, rr.first, rr.last, (r) => this.#makeRowHeader(r));
    this.#sync(this.colHeaders, cr.first, cr.last, (c) => this.#makeColHeader(c));
    const wanted = new Set();
    for (let r = rr.first; r <= rr.last; r++) {
      for (let c = cr.first; c <= cr.last; c++) wanted.add(`${r}:${c}`);
    }
    for (const [key, el] of this.cells) {
      if (!wanted.has(key)) {
        el.remove();
        this.cells.delete(key);
      }
    }
    for (const key of wanted) {
      if (this.cells.has(key)) continue;
      const [r, c] = key.split(':').map(Number);
      const el = h('div', { class: 'g-cell', role: 'gridcell' });
      el.dataset.row = r;
      el.dataset.col = c;
      el.style.transform = `translate(${this.colOffsets[c]}px, ${this.rowOffsets[r]}px)`;
      this.#paintCell(el, r, c);
      this.bodyInner.append(el);
      this.cells.set(key, el);
    }
    this.#paintSelection();
  }

  #sync(map, first, last, make) {
    for (const [i, el] of map) {
      if (i < first || i > last) {
        el.remove();
        map.delete(i);
      }
    }
    for (let i = first; i <= last; i++) {
      if (!map.has(i)) {
        const el = make(i);
        map.set(i, el);
      }
    }
  }

  #makeRowHeader(r) {
    const info = this.data.rowHeader(r);
    const el = this.#headerElement(info, 'row', r);
    el.style.transform = `translateY(${this.rowOffsets[r]}px)`;
    this.rowHeadInner.append(el);
    return el;
  }

  #makeColHeader(c) {
    const info = this.data.colHeader(c);
    const el = this.#headerElement(info, 'col', c);
    el.style.transform = `translateX(${this.colOffsets[c]}px)`;
    this.colHeadInner.append(el);
    return el;
  }

  #headerElement(info, axis, index) {
    const el = h('div', { class: `g-head g-head-${axis}`, title: info.label });
    el.dataset.index = index;
    if (info.aggregate) el.classList.add('g-aggregate');
    const indent = Math.min(info.depth || 0, 12);
    el.classList.add(`g-indent-${indent}`);
    const toggle = h('span', { class: 'g-toggle', 'aria-hidden': 'true' }, info.expandable ? (info.expanded ? '▾' : '▸') : '');
    if (info.expandable) toggle.dataset.toggle = '1';
    el.append(toggle, h('span', { class: 'g-label' }, info.label));
    return el;
  }

  #paintCell(el, r, c) {
    const cell = this.data.cell(r, c);
    const isNumber = typeof cell.value === 'number';
    el.textContent = cell.value === null || cell.value === undefined ? '' : isNumber ? formatNumber(cell.value, cell.digits ?? 2) : String(cell.value);
    el.className = 'g-cell';
    el.classList.add(`g-${cell.kind || 'agg'}`);
    if (isNumber && cell.value < 0) el.classList.add('g-negative');
    if (!isNumber && cell.value) el.classList.add('g-text');
    if (cell.writable) el.classList.add('g-writable');
    if (cell.conflict) el.classList.add('g-conflict');
    if (cell.pending) el.classList.add('g-pending');
  }

  #paintSelection() {
    for (const [key, el] of this.cells) {
      const [r, c] = key.split(':').map(Number);
      const inSel = this.selection.contains(r, c);
      const a = this.selection.active;
      el.classList.toggle('g-selected', inSel && !this.selection.isSingle);
      el.classList.toggle('g-active', r === a.row && c === a.col);
    }
    this.rowHeaders.forEach((el, r) => el.classList.toggle('g-head-on', r >= this.selection.rect.top && r <= this.selection.rect.bottom));
    this.colHeaders.forEach((el, c) => el.classList.toggle('g-head-on', c >= this.selection.rect.left && c <= this.selection.rect.right));
  }

  // ---------------------------------------------------------------- selection and scrolling

  #reveal(row, col) {
    this.body.scrollTop = scrollToReveal(this.rowOffsets, row, this.body.scrollTop, this.viewH);
    this.body.scrollLeft = scrollToReveal(this.colOffsets, col, this.body.scrollLeft, this.viewW);
  }

  #changed() {
    const a = this.selection.active;
    this.#reveal(a.row, a.col);
    this.#draw();
    this.handlers.onSelect?.(this.selection);
  }

  select(row, col, extend = false) {
    this.selection.select(row, col, extend);
    this.#changed();
  }

  #cellAt(e) {
    const el = e.target.closest?.('.g-cell');
    return el ? { row: Number(el.dataset.row), col: Number(el.dataset.col) } : null;
  }

  #onMouseDown(e) {
    if (e.button !== 0) return;
    const at = this.#cellAt(e);
    if (!at) return;
    if (this.editor) this.#closeEditor(true);
    this.selection.select(at.row, at.col, e.shiftKey);
    this.#changed();
    // dragging extends the selection until the button is released
    const move = (ev) => {
      const hit = document.elementFromPoint(ev.clientX, ev.clientY)?.closest?.('.g-cell');
      if (!hit) return;
      this.selection.select(Number(hit.dataset.row), Number(hit.dataset.col), true);
      this.#paintSelection();
    };
    const up = () => {
      document.removeEventListener('mousemove', move);
      document.removeEventListener('mouseup', up);
      this.handlers.onSelect?.(this.selection);
    };
    document.addEventListener('mousemove', move);
    document.addEventListener('mouseup', up);
  }

  #onDblClick(e) {
    const at = this.#cellAt(e);
    if (at) this.startEdit();
  }

  #onContext(e) {
    const at = this.#cellAt(e);
    if (!at) return;
    e.preventDefault();
    if (!this.selection.contains(at.row, at.col)) this.select(at.row, at.col);
    this.handlers.onContext?.(at, e);
  }

  #onHeaderClick(e, axis) {
    const head = e.target.closest('.g-head');
    if (!head) return;
    const index = Number(head.dataset.index);
    if (e.target.closest('[data-toggle]')) {
      this.handlers.onToggle?.(axis, index);
      return;
    }
    // clicking a header selects the whole row or column
    if (axis === 'row') {
      this.selection.select(index, 0);
      this.selection.select(index, this.data.colCount - 1, true);
    } else {
      this.selection.select(0, index);
      this.selection.select(this.data.rowCount - 1, index, true);
    }
    this.#changed();
    this.focus();
  }

  // ---------------------------------------------------------------- keyboard and editing

  #onKey(e) {
    if (this.editor) return; // the editor handles its own keys
    const mod = e.ctrlKey || e.metaKey;
    if (mod && (e.key === 'c' || e.key === 'v' || e.key === 'x' || e.key === 'a' || e.key === 'z')) {
      if (e.key === 'a') {
        e.preventDefault();
        this.selection.select(0, 0);
        this.selection.select(this.data.rowCount - 1, this.data.colCount - 1, true);
        this.#changed();
      }
      return; // copy, paste and undo are browser/app events
    }
    if (e.key === 'F2' || (e.key === 'Enter' && e.altKey)) {
      e.preventDefault();
      this.startEdit();
      return;
    }
    if (e.key === 'Delete' || e.key === 'Backspace') {
      e.preventDefault();
      this.handlers.onEdit?.(this.#rectCells(), '');
      return;
    }
    if (NAV_KEYS.has(e.key)) {
      const page = Math.max(1, Math.floor(this.viewH / ROW_HEIGHT) - 1);
      if (this.selection.move(e.key, { shift: e.shiftKey, ctrl: mod, page })) {
        e.preventDefault();
        this.#changed();
      }
      return;
    }
    // typing starts editing and replaces the content, like in a spreadsheet
    if (e.key.length === 1 && !mod && !e.altKey) {
      e.preventDefault();
      this.startEdit(e.key);
    }
  }

  /** The cells of the selection as `[row, col]` pairs. */
  #rectCells() {
    const r = this.selection.rect;
    const out = [];
    for (let row = r.top; row <= r.bottom; row++) for (let col = r.left; col <= r.right; col++) out.push([row, col]);
    return out;
  }

  get selectedCells() {
    return this.#rectCells();
  }

  /** Opens the editor on the active cell. `initial` replaces the content (typing), else it is edited. */
  startEdit(initial) {
    if (this.selection.isEmpty) return;
    const { row, col } = this.selection.active;
    const cell = this.data.cell(row, col);
    if (!cell.writable) {
      this.handlers.onEdit?.([[row, col]], null, { refused: true, cell });
      return;
    }
    const el = this.cells.get(`${row}:${col}`);
    if (!el) return;
    const text = initial !== undefined ? initial : cell.edit ?? (typeof cell.value === 'number' ? String(cell.value) : '');
    const input = h('input', { class: 'g-editor', type: 'text', value: text, spellcheck: false, autocomplete: 'off', 'aria-label': 'Wert eingeben' });
    el.classList.add('g-editing');
    el.replaceChildren(input);
    this.editor = { input, row, col, el };
    input.addEventListener('keydown', (ev) => this.#onEditorKey(ev));
    input.addEventListener('blur', () => this.#closeEditor(true));
    input.focus();
    if (initial === undefined) input.select();
    else input.setSelectionRange(text.length, text.length);
  }

  #onEditorKey(e) {
    if (e.key === 'Escape') {
      e.preventDefault();
      this.#closeEditor(false);
      this.focus();
    } else if (e.key === 'Enter' || e.key === 'Tab') {
      e.preventDefault();
      const shift = e.shiftKey;
      const key = e.key;
      this.#closeEditor(true);
      this.focus();
      this.selection.move(key, { shift });
      this.#changed();
    }
  }

  #closeEditor(commit) {
    const ed = this.editor;
    if (!ed) return;
    this.editor = null; // first: blur fires while removing the input
    const text = ed.input.value;
    ed.el.classList.remove('g-editing');
    this.#paintCell(ed.el, ed.row, ed.col);
    if (commit) this.handlers.onEdit?.([[ed.row, ed.col]], text);
  }

  // ---------------------------------------------------------------- clipboard

  #onCopy(e) {
    if (this.editor) return;
    const r = this.selection.rect;
    const lines = [];
    for (let row = r.top; row <= r.bottom; row++) {
      const cols = [];
      for (let col = r.left; col <= r.right; col++) {
        const v = this.data.cell(row, col).value;
        cols.push(v === null || v === undefined ? '' : String(v));
      }
      lines.push(cols.join('\t'));
    }
    e.clipboardData.setData('text/plain', lines.join('\n'));
    e.preventDefault();
    this.handlers.onCopy?.(this.selection);
  }

  #onPaste(e) {
    if (this.editor) return;
    const text = e.clipboardData.getData('text/plain');
    if (!text) return;
    e.preventDefault();
    this.handlers.onPaste?.(parseClipboard(text), this.selection.rect);
  }
}

/** Spreadsheet text (tab-separated, one line per row) → rows of cells. A trailing newline is ignored. */
export function parseClipboard(text) {
  const lines = text.replace(/\r\n?/g, '\n').replace(/\n$/, '').split('\n');
  return lines.map((l) => l.split('\t'));
}
