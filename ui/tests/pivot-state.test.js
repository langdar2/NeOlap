import test from 'node:test';
import assert from 'node:assert/strict';
import { Axis, PivotState, pasteWrites, normalizeInput, looksLikeInput } from '../pivot-state.js';

const m = (name, children = 0, leaf = children === 0) => ({ id: 0, name, aliases: [], level: 0, leaf, children });

function regions() {
  const a = new Axis('regions');
  a.setRoots([m('Total', 2, false)]);
  a.setChildren('Total', [m('Nord', 2, false), m('Sued')]);
  a.setChildren('Nord', [m('Hamburg'), m('Berlin')]);
  return a;
}

test('an axis starts with its roots and expands level by level', () => {
  const a = regions();
  assert.deepEqual(a.names, ['Total']);
  assert.equal(a.canExpand(0), true);
  assert.equal(a.expand(0), true);
  assert.deepEqual(a.names, ['Total', 'Nord', 'Sued']);
  assert.deepEqual(a.items.map((i) => i.depth), [0, 1, 1]);
  assert.equal(a.expand(1), true);
  assert.deepEqual(a.names, ['Total', 'Nord', 'Hamburg', 'Berlin', 'Sued']);
  assert.deepEqual(a.items.map((i) => i.depth), [0, 1, 2, 2, 1]);
  assert.equal(a.expand(1), false, 'already expanded');
  assert.equal(a.canExpand(2), false, 'a base member has no children');
});

test('collapsing removes everything below and forgets nested expansion', () => {
  const a = regions();
  a.expand(0); a.expand(1);
  assert.equal(a.collapse(0), true);
  assert.deepEqual(a.names, ['Total']);
  assert.equal(a.expanded.size, 0, 'Nord is collapsed too');
  a.expand(0);
  assert.deepEqual(a.names, ['Total', 'Nord', 'Sued'], 'Nord comes back collapsed');
  assert.equal(a.collapse(2), false, 'Sued was never expanded');
});

test('children are loaded on demand', () => {
  const a = new Axis('regions');
  a.setRoots([m('Total', 2, false)]);
  assert.equal(a.needsChildren(0), true);
  assert.equal(a.expand(0), false, 'cannot expand before the children are there');
  a.setChildren('Total', [m('Nord'), m('Sued')]);
  assert.equal(a.needsChildren(0), false);
  assert.equal(a.expand(0), true);
});

test('headers describe depth, expandability and aggregates', () => {
  const a = regions();
  a.expand(0);
  assert.deepEqual(a.header(0), { label: 'Total', depth: 0, expandable: true, expanded: true, aggregate: true });
  assert.deepEqual(a.header(2), { label: 'Sued', depth: 1, expandable: false, expanded: false, aggregate: false });
  assert.equal(a.header(99).label, '', 'out of range is harmless');
});

function pivot() {
  const p = new PivotState({ name: 'sales', dimensions: ['years', 'regions', 'measures'] });
  p.rowDim = 'regions';
  p.colDim = 'measures';
  p.filters = { years: 'Jan' };
  p.rows = regions();
  p.rows.expand(0);
  p.cols = new Axis('measures');
  p.cols.setRoots([m('Sales'), m('Cost')]);
  p.setAnswer({
    generation: 7,
    values: [[300, 200], [200, 150], [100, null]],
    kind: [['agg', 'agg'], ['agg', 'agg'], ['base', 'base']],
    writable: [[false, false], [false, false], [true, true]],
  });
  return p;
}

test('the view request names both axes, the filters and the rules flag', () => {
  const p = pivot();
  assert.deepEqual(p.request(), {
    rows: { dimension: 'regions', members: ['Total', 'Nord', 'Sued'] },
    cols: { dimension: 'measures', members: ['Sales', 'Cost'] },
    filters: { years: 'Jan' },
    suppress_empty: false,
    rules: true,
  });
  assert.deepEqual(p.filterDimensions, ['years']);
});

test('grid data combines headers, values, kinds and writability', () => {
  const d = pivot().gridData();
  assert.deepEqual([d.rowCount, d.colCount], [3, 2]);
  assert.equal(d.cell(0, 0).value, 300);
  assert.equal(d.cell(2, 1).value, null);
  assert.equal(d.cell(2, 0).kind, 'base');
  assert.equal(d.cell(2, 0).writable, true);
  assert.equal(d.cell(0, 0).writable, false);
  assert.equal(d.cell(2, 0).edit, '100');
  assert.equal(d.rowHeader(1).label, 'Nord');
});

test('suppressing empty rows and columns keeps the grid and the addresses aligned', () => {
  const p = pivot();
  p.setAnswer({ generation: 1, values: [[1, null], [null, null], [2, null]], kind: [['agg', 'agg'], ['agg', 'agg'], ['base', 'base']], writable: [[false, false], [false, false], [true, true]] });
  p.suppressEmpty = true;
  const d = p.gridData();
  assert.deepEqual([d.rowCount, d.colCount], [2, 1], 'Nord and the Cost column are empty');
  assert.equal(d.rowHeader(1).label, 'Sued');
  assert.deepEqual(p.cellAddress(1, 0), ['Jan', 'Sued', 'Sales'], 'the address follows the shown row, not the raw index');
  assert.deepEqual(p.positionOf('Sued', 'Sales'), [1, 0]);
  assert.equal(p.positionOf('Nord', 'Sales'), null, 'a hidden row has no position');
});

test('cell addresses list one member per dimension in cube order', () => {
  const p = pivot();
  assert.deepEqual(p.cellAddress(2, 1), ['Jan', 'Sued', 'Cost']);
  assert.deepEqual(p.cellAddress(0, 0), ['Jan', 'Total', 'Sales']);
});

test('pending edits show their text until the server answers; a new answer clears them', () => {
  const p = pivot();
  p.pending.set('2:0', { text: '150' });
  const c = p.gridData().cell(2, 0);
  assert.equal(c.value, '150');
  assert.equal(c.pending, true);
  p.setAnswer(p.answer);
  assert.equal(p.gridData().cell(2, 0).value, 100);
  p.pending.set('2:0', { text: 'x', error: 'no' });
  assert.equal(p.gridData().cell(2, 0).value, 100, 'a failed edit shows the real value again');
});

test('summarize adds the numbers of a rectangle and ignores empty and text cells', () => {
  const p = pivot();
  assert.deepEqual(p.summarize({ top: 0, bottom: 2, left: 0, right: 1 }), { sum: 950, count: 5 });
  assert.deepEqual(p.summarize({ top: 2, bottom: 2, left: 1, right: 1 }), { sum: 0, count: 0 });
});

test('input is cleaned up but numbers are not guessed at (the server reads them strictly)', () => {
  const cases = [
    ['500', '500'], ['  500 ', '500'], ['', ''], ['   ', ''],
    ['1 234', '1234'], ["1'234", '1234'], ['1_234', '1234'], ['1\u00a0234,5', '1234,5'], ['1\u202f234', '1234'],
    ['1.234,5', '1.234,5'], ['1,234.5', '1,234.5'], ['1,5', '1,5'],
    ['+50', '+50'], ['-50', '-50'], ['+5%', '+5%'], ['-3,5%', '-3,5%'], ['5%', '5%'],
    ['#500', '#500'], ['#1.000,5', '#1.000,5'], ['=-50', '=-50'], ['@Actual,2026:500', '@Actual,2026:500'],
    ['1,,5', '1,,5'], ['abc', 'abc'],
  ];
  for (const [input, want] of cases) assert.equal(normalizeInput(input), want, JSON.stringify(input));
});

test('pasting fills down and right from the top-left cell and skips what cannot be written', () => {
  const p = pivot();
  const d = p.gridData();
  const r = pasteWrites([['1', '2'], ['3', '4']], { top: 2, left: 0 }, d);
  assert.deepEqual(r.writes.map((w) => [w.row, w.col, w.text]), [[2, 0, '1'], [2, 1, '2']]);
  assert.deepEqual(r.skipped.map((s) => s.why), ['outside', 'outside'], 'rows below the grid');
  const r2 = pasteWrites([['9', '8']], { top: 0, left: 0 }, d);
  assert.equal(r2.writes.length, 0);
  assert.deepEqual(r2.skipped.map((s) => s.why), ['readonly', 'readonly'], 'aggregates are not writable');
  const r3 = pasteWrites([['1,5', ' #2.000,5 ']], { top: 2, left: 0 }, d);
  assert.deepEqual(r3.writes.map((w) => w.text), ['1,5', '#2.000,5'], 'passed on for the server to read, blanks removed');
});
