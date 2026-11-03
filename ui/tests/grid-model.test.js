import test from 'node:test';
import assert from 'node:assert/strict';
import { offsetsFor, indexAt, visibleRange, scrollToReveal, GridSelection } from '../grid-model.js';

const uniform = (n, size = 20) => offsetsFor(n, () => size);

test('offsets are cumulative and indexAt finds the containing item', () => {
  const o = uniform(5, 10);
  assert.deepEqual([...o], [0, 10, 20, 30, 40, 50]);
  assert.equal(indexAt(o, 0), 0);
  assert.equal(indexAt(o, 9.99), 0);
  assert.equal(indexAt(o, 10), 1);
  assert.equal(indexAt(o, 49), 4);
  assert.equal(indexAt(o, 500), 4, 'beyond the end: the last item');
  assert.equal(indexAt(o, -5), 0);
  assert.equal(indexAt(uniform(0), 10), 0, 'empty grid');
});

test('indexAt works with items of different sizes', () => {
  const o = offsetsFor(4, (i) => [10, 100, 5, 40][i]);
  assert.equal(indexAt(o, 9), 0);
  assert.equal(indexAt(o, 10), 1);
  assert.equal(indexAt(o, 109), 1);
  assert.equal(indexAt(o, 110), 2);
  assert.equal(indexAt(o, 115), 3);
});

test('visibleRange draws the viewport plus overscan and never leaves the grid', () => {
  const o = uniform(1000, 20);
  assert.deepEqual(visibleRange(o, 0, 100, 0), { first: 0, last: 5 }, 'a viewport ending exactly on an item border touches the next one');
  assert.deepEqual(visibleRange(o, 0, 100, 3), { first: 0, last: 8 });
  assert.deepEqual(visibleRange(o, 10000, 100, 3), { first: 497, last: 508 });
  assert.deepEqual(visibleRange(o, 1e9, 100, 3), { first: 996, last: 999 }, 'scrolled past the end');
  assert.ok(visibleRange(uniform(0), 0, 100).last < 0, 'nothing to draw');
  assert.ok(visibleRange(o, 0, 0).last < 0, 'no viewport, nothing to draw');
});

test('a million rows need only a handful of drawn rows', () => {
  const o = uniform(1_000_000, 22);
  for (const scroll of [0, 123456, 11_000_000, 21_999_000]) {
    const r = visibleRange(o, scroll, 600, 3);
    assert.ok(r.last - r.first < 40, `scroll ${scroll}: ${r.last - r.first + 1} rows`);
    assert.ok(r.first <= indexAt(o, scroll) && r.last >= indexAt(o, scroll + 600));
  }
});

test('scrollToReveal moves as little as possible', () => {
  const o = uniform(100, 20);
  assert.equal(scrollToReveal(o, 5, 0, 100), 20, 'item 5 ends at 120, so the view must scroll to 20');
  assert.equal(scrollToReveal(o, 2, 0, 100), 0, 'already visible: no movement');
  assert.equal(scrollToReveal(o, 1, 100, 100), 20, 'above the view: align its top');
  assert.equal(scrollToReveal(o, 99, 0, 100), 1900);
});

test('selection stays inside the grid and arrows move one cell', () => {
  const s = new GridSelection(3, 4);
  assert.equal(s.move('ArrowUp'), true);
  assert.deepEqual(s.focus, { row: 0, col: 0 }, 'cannot leave at the top');
  s.move('ArrowDown'); s.move('ArrowRight'); s.move('ArrowRight');
  assert.deepEqual(s.focus, { row: 1, col: 2 });
  for (let i = 0; i < 10; i++) { s.move('ArrowDown'); s.move('ArrowRight'); }
  assert.deepEqual(s.focus, { row: 2, col: 3 });
  assert.equal(s.move('F5'), false, 'other keys are not handled');
});

test('shift with arrows extends a rectangle, a plain move collapses it', () => {
  const s = new GridSelection(5, 5);
  s.select(1, 1);
  s.move('ArrowDown', { shift: true });
  s.move('ArrowRight', { shift: true });
  assert.deepEqual(s.rect, { top: 1, bottom: 2, left: 1, right: 2 });
  assert.equal(s.contains(2, 2), true);
  assert.equal(s.contains(3, 2), false);
  assert.equal(s.isSingle, false);
  s.move('ArrowDown', { shift: true });
  s.move('ArrowLeft', { shift: true });
  s.move('ArrowLeft', { shift: true });
  assert.deepEqual(s.rect, { top: 1, bottom: 3, left: 0, right: 1 }, 'the anchor stays, the focus moves');
  s.move('ArrowUp');
  assert.equal(s.isSingle, true);
});

test('tab and enter move like in a spreadsheet, tab wraps to the next row', () => {
  const s = new GridSelection(3, 3);
  s.move('Tab'); s.move('Tab');
  assert.deepEqual(s.focus, { row: 0, col: 2 });
  s.move('Tab');
  assert.deepEqual(s.focus, { row: 1, col: 0 }, 'past the last column: first column of the next row');
  s.move('Tab', { shift: true });
  assert.deepEqual(s.focus, { row: 0, col: 2 }, 'shift+tab goes back');
  s.select(2, 2);
  s.move('Tab');
  assert.deepEqual(s.focus, { row: 2, col: 2 }, 'the last cell stays put');
  s.move('Enter');
  assert.deepEqual(s.focus, { row: 2, col: 2 });
  s.select(0, 1);
  s.move('Enter'); s.move('Enter');
  assert.deepEqual(s.focus, { row: 2, col: 1 });
  s.move('Enter', { shift: true });
  assert.deepEqual(s.focus, { row: 1, col: 1 });
});

test('inside a multi-cell selection tab and enter move the active cell around the rectangle', () => {
  const s = new GridSelection(5, 5);
  s.select(1, 1); s.select(2, 2, true); // rect rows 1-2, cols 1-2, focus (2,2)
  assert.deepEqual(s.active, { row: 2, col: 2 });
  s.move('Tab');
  assert.deepEqual(s.active, { row: 1, col: 1 }, 'wraps to the top-left of the rectangle');
  assert.deepEqual(s.rect, { top: 1, bottom: 2, left: 1, right: 2 }, 'the rectangle is kept');
  s.move('Tab');
  assert.deepEqual(s.active, { row: 1, col: 2 });
  s.move('Tab');
  assert.deepEqual(s.active, { row: 2, col: 1 }, 'tab continues in the next row of the rectangle');
  s.move('Tab', { shift: true });
  assert.deepEqual(s.active, { row: 1, col: 2 }, 'shift+tab goes back');
  s.move('Enter');
  assert.deepEqual(s.active, { row: 2, col: 2 });
  s.move('Enter');
  assert.deepEqual(s.active, { row: 1, col: 1 }, 'enter wraps to the next column of the rectangle');
  assert.equal(s.isSingle, false);
  s.move('ArrowDown');
  assert.equal(s.isSingle, true, 'an arrow collapses the selection to one cell');
  assert.equal(s.cursor, null);
});

test('selecting again resets the active cell to the focus', () => {
  const s = new GridSelection(5, 5);
  s.select(0, 0); s.select(2, 2, true);
  s.move('Tab');
  assert.notDeepEqual(s.active, s.focus);
  s.select(3, 3);
  assert.deepEqual(s.active, { row: 3, col: 3 });
});

test('home, end, page keys and the ctrl corners', () => {
  const s = new GridSelection(100, 6);
  s.select(50, 3);
  s.move('Home'); assert.deepEqual(s.focus, { row: 50, col: 0 });
  s.move('End'); assert.deepEqual(s.focus, { row: 50, col: 5 });
  s.move('Home', { ctrl: true }); assert.deepEqual(s.focus, { row: 0, col: 0 });
  s.move('End', { ctrl: true }); assert.deepEqual(s.focus, { row: 99, col: 5 });
  s.move('PageUp', { page: 25 }); assert.deepEqual(s.focus, { row: 74, col: 5 });
  s.move('PageDown', { page: 40 }); assert.deepEqual(s.focus, { row: 99, col: 5 }, 'clamped');
  s.select(10, 0);
  s.move('PageDown', { shift: true, page: 5 });
  assert.deepEqual(s.rect, { top: 10, bottom: 15, left: 0, right: 0 });
});

test('resize keeps the selection inside a smaller grid and an empty grid ignores keys', () => {
  const s = new GridSelection(10, 10);
  s.select(8, 9); s.select(9, 9, true);
  s.resize(4, 3);
  assert.deepEqual(s.focus, { row: 3, col: 2 });
  assert.deepEqual(s.anchor, { row: 3, col: 2 });
  s.resize(0, 0);
  assert.equal(s.isEmpty, true);
  assert.equal(s.move('ArrowDown'), false);
  s.select(5, 5);
  assert.deepEqual(s.focus, { row: 0, col: 0 });
});
