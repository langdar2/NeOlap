// Grid model: everything about a pivot grid that is not drawing.
//
// Virtual scrolling (which rows and columns are visible), the selection and keyboard navigation
// live here, without any DOM access, so they can be tested with Node (ui/tests/). The renderer in
// grid.js only asks this model what to draw.

/** Cumulative offsets of equally or individually sized items; `offsets[i]` is the start of item i. */
export function offsetsFor(count, sizeOf) {
  const offsets = new Float64Array(count + 1);
  for (let i = 0; i < count; i++) offsets[i + 1] = offsets[i] + sizeOf(i);
  return offsets;
}

/** Index of the item that contains `position` (the last item if it lies beyond the end). */
export function indexAt(offsets, position) {
  const count = offsets.length - 1;
  if (count <= 0) return 0;
  if (position <= 0) return 0;
  if (position >= offsets[count]) return count - 1;
  let lo = 0;
  let hi = count - 1;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (offsets[mid] <= position) lo = mid;
    else hi = mid - 1;
  }
  return lo;
}

/**
 * The range of items to draw for a viewport: the visible ones plus `overscan` on each side, so
 * fast scrolling never shows a gap. Returns `{ first, last }` (inclusive), or `last < first` if
 * there is nothing to draw.
 */
export function visibleRange(offsets, scroll, viewport, overscan = 3) {
  const count = offsets.length - 1;
  if (count <= 0 || viewport <= 0) return { first: 0, last: -1 };
  const first = Math.max(0, indexAt(offsets, scroll) - overscan);
  const last = Math.min(count - 1, indexAt(offsets, scroll + viewport) + overscan);
  return { first, last };
}

/** Scroll position that brings item `index` fully into view with the least movement. */
export function scrollToReveal(offsets, index, scroll, viewport) {
  const start = offsets[index];
  const end = offsets[index + 1];
  if (start < scroll) return start;
  if (end > scroll + viewport) return Math.max(0, end - viewport);
  return scroll;
}

/** Keys the model handles; the renderer prevents the browser default for these. */
export const NAV_KEYS = new Set([
  'ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight', 'Tab', 'Enter',
  'Home', 'End', 'PageUp', 'PageDown',
]);

const clamp = (v, lo, hi) => Math.min(Math.max(v, lo), hi);

/**
 * Selection and navigation over a `rows × cols` grid. The selection is the rectangle between an
 * anchor and a focus cell. The *active* cell (the one that is edited) is normally the focus; inside
 * a multi-cell selection Tab and Enter move the active cell around the rectangle without changing
 * it, as in a spreadsheet.
 */
export class GridSelection {
  constructor(rows = 0, cols = 0) {
    this.rows = rows;
    this.cols = cols;
    this.anchor = { row: 0, col: 0 };
    this.focus = { row: 0, col: 0 };
    /** The active cell inside the rectangle; `null` means "the focus". */
    this.cursor = null;
  }

  /** The cell that is edited. */
  get active() {
    return this.cursor ?? this.focus;
  }

  /** The grid changed size (new view, expanded member): keep the selection inside it. */
  resize(rows, cols) {
    this.rows = rows;
    this.cols = cols;
    this.anchor = this.#inside(this.anchor);
    this.focus = this.#inside(this.focus);
    this.cursor = this.cursor && this.contains(this.#inside(this.cursor).row, this.#inside(this.cursor).col) ? this.#inside(this.cursor) : null;
  }

  #inside(c) {
    return {
      row: clamp(c.row, 0, Math.max(0, this.rows - 1)),
      col: clamp(c.col, 0, Math.max(0, this.cols - 1)),
    };
  }

  get isEmpty() {
    return this.rows === 0 || this.cols === 0;
  }

  select(row, col, extend = false) {
    const c = this.#inside({ row, col });
    this.focus = c;
    this.cursor = null;
    if (!extend) this.anchor = c;
  }

  get rect() {
    return {
      top: Math.min(this.anchor.row, this.focus.row),
      bottom: Math.max(this.anchor.row, this.focus.row),
      left: Math.min(this.anchor.col, this.focus.col),
      right: Math.max(this.anchor.col, this.focus.col),
    };
  }

  contains(row, col) {
    const r = this.rect;
    return row >= r.top && row <= r.bottom && col >= r.left && col <= r.right;
  }

  get isSingle() {
    return this.anchor.row === this.focus.row && this.anchor.col === this.focus.col;
  }

  /**
   * Moves the selection for a key like a spreadsheet does: arrows move one cell, Tab and Enter
   * move right and down (Shift reverses), Home/End go to the row's ends (Ctrl: the grid's
   * corners), PageUp/PageDown jump `page` rows. Shift with an arrow or page key extends the
   * rectangle. Returns whether the key was handled.
   */
  move(key, { shift = false, ctrl = false, page = 10 } = {}) {
    if (this.isEmpty || !NAV_KEYS.has(key)) return false;
    if ((key === 'Tab' || key === 'Enter') && !this.isSingle) {
      // inside a multi-cell selection Tab and Enter cycle within it, like in a spreadsheet
      this.#cycle(key, shift);
      return true;
    }
    let { row, col } = this.focus;
    let extend = false;
    switch (key) {
      case 'ArrowUp': row -= 1; extend = shift; break;
      case 'ArrowDown': row += 1; extend = shift; break;
      case 'ArrowLeft': col -= 1; extend = shift; break;
      case 'ArrowRight': col += 1; extend = shift; break;
      case 'Tab': col += shift ? -1 : 1; break;
      case 'Enter': row += shift ? -1 : 1; break;
      case 'Home': col = 0; if (ctrl) row = 0; extend = shift; break;
      case 'End': col = this.cols - 1; if (ctrl) row = this.rows - 1; extend = shift; break;
      case 'PageUp': row -= page; extend = shift; break;
      case 'PageDown': row += page; extend = shift; break;
      default: return false;
    }
    // Tab past the last column continues in the next row, before the first in the previous one
    if (key === 'Tab') {
      if (col >= this.cols && row < this.rows - 1) { col = 0; row += 1; }
      else if (col < 0 && row > 0) { col = this.cols - 1; row -= 1; }
    }
    this.select(row, col, extend);
    return true;
  }

  #cycle(key, shift) {
    const r = this.rect;
    let { row, col } = this.active;
    if (key === 'Tab') {
      col += shift ? -1 : 1;
      if (col > r.right) { col = r.left; row += 1; }
      if (col < r.left) { col = r.right; row -= 1; }
      if (row > r.bottom) row = r.top;
      if (row < r.top) row = r.bottom;
    } else {
      row += shift ? -1 : 1;
      if (row > r.bottom) { row = r.top; col += 1; }
      if (row < r.top) { row = r.bottom; col -= 1; }
      if (col > r.right) col = r.left;
      if (col < r.left) col = r.right;
    }
    this.cursor = { row, col }; // anchor and focus stay: the rectangle is kept
  }
}
