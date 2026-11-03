// Pivot state: axes, expansion and the translation between the grid and the API.
//
// No DOM, no network. The page feeds in member lists and view answers and gets back what the grid
// shows; tests (ui/tests/) drive it with plain objects.

/** One member as the server describes it (`GET /dimensions/{d}/members`). */
// { id, name, aliases, level, leaf, children }

/**
 * An axis: the list of rows (or columns) shown. Starts with the roots of one dimension; a member
 * with children can be expanded (its children are inserted below it) and collapsed again.
 */
export class Axis {
  constructor(dimension) {
    this.dimension = dimension;
    /** Visible items: `{ member, depth }`. */
    this.items = [];
    /** Names of expanded members. */
    this.expanded = new Set();
    /** Children by parent name, as loaded. */
    this.children = new Map();
  }

  /** Sets the first level (the roots, or a chosen set of members). */
  setRoots(members) {
    this.items = members.map((member) => ({ member, depth: 0 }));
    this.expanded.clear();
  }

  get length() {
    return this.items.length;
  }

  at(index) {
    return this.items[index];
  }

  isExpanded(index) {
    const it = this.items[index];
    return !!it && this.expanded.has(it.member.name);
  }

  canExpand(index) {
    const it = this.items[index];
    return !!it && it.member.children > 0;
  }

  /** Whether the children of the member at `index` still have to be loaded. */
  needsChildren(index) {
    const it = this.items[index];
    return !!it && it.member.children > 0 && !this.children.has(it.member.name);
  }

  /** Provides the children of a member (loaded from the server). */
  setChildren(parentName, members) {
    this.children.set(parentName, members);
  }

  /** Expands the member at `index`; its children must be loaded (`needsChildren`). */
  expand(index) {
    const it = this.items[index];
    if (!it || this.expanded.has(it.member.name)) return false;
    const kids = this.children.get(it.member.name);
    if (!kids) return false;
    this.expanded.add(it.member.name);
    this.items.splice(index + 1, 0, ...kids.map((member) => ({ member, depth: it.depth + 1 })));
    return true;
  }

  /** Collapses the member at `index` and everything expanded below it. */
  collapse(index) {
    const it = this.items[index];
    if (!it || !this.expanded.has(it.member.name)) return false;
    let end = index + 1;
    while (end < this.items.length && this.items[end].depth > it.depth) end++;
    for (let i = index; i < end; i++) this.expanded.delete(this.items[i].member.name);
    this.items.splice(index + 1, end - index - 1);
    return true;
  }

  /** Names of the shown members, for the view request. */
  get names() {
    return this.items.map((it) => it.member.name);
  }

  /** What the grid draws for header `index`. */
  header(index) {
    const it = this.items[index];
    if (!it) return { label: '', depth: 0, expandable: false, expanded: false, aggregate: false };
    return {
      label: it.member.name,
      depth: it.depth,
      expandable: it.member.children > 0,
      expanded: this.expanded.has(it.member.name),
      aggregate: !it.member.leaf,
    };
  }
}

/**
 * The current view: two axes plus a member per remaining dimension (the filters). Holds the last
 * answer of the server and turns it into grid data.
 */
export class PivotState {
  constructor(cube) {
    this.cube = cube; // { name, dimensions: [names] }
    this.rowDim = null;
    this.colDim = null;
    /** dimension → member name */
    this.filters = {};
    this.rows = null; // Axis
    this.cols = null; // Axis
    this.answer = null; // last /view answer
    this.suppressEmpty = false;
    this.useRules = true;
    /** pending edits by "row:col" → { text, error } while the server has not answered */
    this.pending = new Map();
    this.conflicts = new Map();
  }

  get dimensions() {
    return this.cube.dimensions;
  }

  /** Dimensions that are on neither axis. */
  get filterDimensions() {
    return this.dimensions.filter((d) => d !== this.rowDim && d !== this.colDim);
  }

  /** Request body for `POST /cubes/{c}/view`. */
  request() {
    return {
      rows: { dimension: this.rowDim, members: this.rows.names },
      cols: { dimension: this.colDim, members: this.cols.names },
      filters: { ...this.filters },
      suppress_empty: false, // suppression is applied by the page, so rows and answer stay aligned
      rules: this.useRules,
    };
  }

  /** Takes a view answer. Returns the generation. */
  setAnswer(answer) {
    this.answer = answer;
    this.pending.clear();
    this.conflicts.clear();
    return answer.generation;
  }

  /** Indices of rows (or columns) that hold at least one value, for "suppress empty". */
  #nonEmpty(axis) {
    const a = this.answer;
    if (!a) return [];
    const rowsN = a.values.length;
    const colsN = rowsN ? a.values[0].length : 0;
    const out = [];
    if (axis === 'row') {
      for (let r = 0; r < rowsN; r++) if (a.values[r].some((v) => v !== null)) out.push(r);
    } else {
      for (let c = 0; c < colsN; c++) if (a.values.some((row) => row[c] !== null)) out.push(c);
    }
    return out;
  }

  /** Row and column indices (into the answer) that are shown. */
  get shown() {
    const a = this.answer;
    const rows = a ? a.values.length : 0;
    const cols = a && rows ? a.values[0].length : 0;
    if (!this.suppressEmpty) {
      return { rows: [...Array(rows).keys()], cols: [...Array(cols).keys()] };
    }
    return { rows: this.#nonEmpty('row'), cols: this.#nonEmpty('col') };
  }

  /** Data for the grid. */
  gridData() {
    const shown = this.shown;
    const a = this.answer;
    const st = this;
    return {
      rowCount: shown.rows.length,
      colCount: shown.cols.length,
      emptyText: a ? 'Keine Werte in diesem Ausschnitt.' : 'Lade …',
      rowHeader: (i) => st.rows.header(shown.rows[i]),
      colHeader: (i) => st.cols.header(shown.cols[i]),
      cell: (i, j) => {
        const r = shown.rows[i];
        const c = shown.cols[j];
        const key = `${i}:${j}`;
        const pend = st.pending.get(key);
        const raw = a.values[r][c];
        return {
          value: pend && !pend.error ? pend.text : raw,
          kind: a.kind[r][c],
          writable: a.writable[r][c],
          pending: !!pend && !pend.error,
          conflict: st.conflicts.has(key),
          edit: typeof raw === 'number' ? String(raw) : '',
        };
      },
    };
  }

  /**
   * The member names of the cell at grid position `(i, j)`: one per dimension of the cube, in
   * cube order (what `POST /write` wants).
   */
  cellAddress(i, j) {
    const shown = this.shown;
    const rowName = this.rows.at(shown.rows[i]).member.name;
    const colName = this.cols.at(shown.cols[j]).member.name;
    return this.dimensions.map((d) => (d === this.rowDim ? rowName : d === this.colDim ? colName : this.filters[d]));
  }

  /** Maps member names to grid positions, for live updates (which cells to flash). */
  positionOf(rowName, colName) {
    const shown = this.shown;
    const i = shown.rows.findIndex((r) => this.rows.at(r).member.name === rowName);
    const j = shown.cols.findIndex((c) => this.cols.at(c).member.name === colName);
    return i >= 0 && j >= 0 ? [i, j] : null;
  }

  /** Sum of the numeric values in a rectangle, for the status bar. */
  summarize(rect) {
    let sum = 0;
    let count = 0;
    const d = this.gridData();
    for (let r = rect.top; r <= rect.bottom; r++) {
      for (let c = rect.left; c <= rect.right; c++) {
        const v = d.cell(r, c).value;
        if (typeof v === 'number') {
          sum += v;
          count += 1;
        }
      }
    }
    return { sum, count };
  }
}

/**
 * Turns pasted spreadsheet text into writes: the block starts at the top-left cell of the selection
 * and fills down and right. Cells that are not writable are skipped and reported. Numbers with a
 * decimal comma are accepted (`1.234,5`), so a paste from a German spreadsheet works.
 */
export function pasteWrites(rows, topLeft, gridData) {
  const writes = [];
  const skipped = [];
  rows.forEach((line, dr) => {
    line.forEach((raw, dc) => {
      const r = topLeft.top + dr;
      const c = topLeft.left + dc;
      if (r >= gridData.rowCount || c >= gridData.colCount) {
        skipped.push({ row: r, col: c, why: 'outside' });
        return;
      }
      const cell = gridData.cell(r, c);
      if (!cell.writable) {
        skipped.push({ row: r, col: c, why: 'readonly' });
        return;
      }
      writes.push({ row: r, col: c, text: normalizeInput(raw) });
    });
  });
  return { writes, skipped };
}

/**
 * Cell text → the server's splash syntax. Spreadsheet formatting is cleaned up (blanks and `'`
 * as thousands separators go). Separators in the number itself are *not* guessed at: the server
 * reads `1.234,5`, `1,234.5` and `1234,5` strictly (core `number()`), so the text is passed on and
 * a malformed number is refused there with a clear message. Splash prefixes (`#`, `+`, `-`, `=`,
 * `@`) and a trailing `%` pass through unchanged.
 */
export function normalizeInput(raw) {
  // any kind of blank (also the no-break spaces spreadsheets use for thousands), ' and _ go
  return String(raw).replace(/[\s\u00a0\u202f'_]/g, '');
}

// A coarse check before a request is sent: it stops obvious nonsense ("abc") without a round trip.
// The server's parser (core `Input::parse`) has the last word and its message is shown, so this
// check must never be *stricter* than the server, only the same or more lenient. That is verified
// against the real parser by `tests/input-contract.mjs` (run by CI), not maintained by hand.
const NUMBERISH = String.raw`[\d.,]*\d[\d.,]*|[.,]\d[\d.,]*`;

/** Whether `text` could be a cell input (a cheap check before sending). */
export function looksLikeInput(text) {
  const t = text.trim();
  if (t === '') return true; // empty deletes
  // a reference names its members (which may themselves contain ':') and ends in `:<number>`
  if (t.startsWith('@')) return new RegExp(String.raw`^@.+:\s*[-+=]?(?:${NUMBERISH})$`).test(t);
  return new RegExp(String.raw`^(?:[#=]\s*[-+]?|[-+])?(?:${NUMBERISH})%?$`).test(t);
}
