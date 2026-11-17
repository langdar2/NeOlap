// The real PivotPage, driven in Node: a DOM stand-in, a controllable `fetch`, no browser. It holds
// what only the page does (not pivot-state.js): requests that overtake each other, and writes that
// go out one after the other while the user changes a filter.

import test from 'node:test';
import assert from 'node:assert/strict';

// ---- a DOM just big enough for dom.js, toast.js and the page ---------------------------------
globalThis.Node = class {};
class El extends globalThis.Node {
  constructor(tag) { super(); this.tag = tag; this.children = []; this.listeners = {}; this.className = ''; this.classList = { add() {}, remove() {}, toggle() {} }; this.value = ''; this.textContent = ''; this.dataset = {}; }
  setAttribute(k, v) { this[k] = v; }
  addEventListener(t, f) { (this.listeners[t] ||= []).push(f); }
  removeEventListener() {}
  append(...n) { this.children.push(...n); }
  appendChild(n) { this.children.push(n); return n; }
  replaceChildren(...n) { this.children = n; }
  remove() {}
  focus() {}
  querySelector() { return null; }
  querySelectorAll() { return []; }
  getBoundingClientRect() { return { width: 800, height: 400, top: 0, left: 0 }; }
  set innerHTML(_) { throw new Error('innerHTML must never be used'); }
}
globalThis.document = { createElement: (t) => new El(t), createTextNode: (t) => ({ text: t }), addEventListener() {}, removeEventListener() {}, body: new El('body'), activeElement: null };
globalThis.window = { addEventListener() {}, removeEventListener() {}, matchMedia: () => ({ matches: false }) };
globalThis.sessionStorage = { getItem: () => null, setItem() {} };
globalThis.ResizeObserver = class { observe() {} disconnect() {} };
globalThis.requestAnimationFrame = (f) => setTimeout(f, 0);
globalThis.FormData = class {};

// ---- a server stand-in whose answers the test releases in the order it wants ----------------
function server() {
  const log = [];
  const waiting = [];
  globalThis.fetch = (url, init) => new Promise((resolve) => {
    const body = init?.body ? JSON.parse(init.body) : undefined;
    const entry = { url: String(url), method: init?.method, body, resolve };
    log.push(entry);
    const reply = (json, status = 200) => resolve({ ok: status < 200 || status >= 300 ? false : true, status, json: async () => json });
    entry.reply = reply;
    if (/\/dimensions\//.test(entry.url)) {
      const m = (name, leaf = true) => ({ id: 0, name, aliases: [], level: 0, leaf, children: leaf ? 0 : 2 });
      const parent = new URL(entry.url, 'http://x').searchParams.get('parent');
      const dim = /dimensions\/([^/?]+)/.exec(entry.url)[1];
      const names = { years: ['Jan', 'Feb'], regions: ['Nord', 'Sued'], measures: ['Sales', 'Cost'] }[dim];
      return reply({ members: parent ? [] : names.map((n) => m(n)) });
    }
    if (/\/view$/.test(entry.url)) return waiting.push(entry); // the test decides when this answers
    if (/\/write$/.test(entry.url)) return waiting.push(entry);
    reply({});
  });
  return { log, waiting };
}

const answer = (generation, v) => ({ generation, values: [[v, v], [v, v]], kind: [['base', 'base'], ['base', 'base']], writable: [[true, true], [true, true]], rows: ['Nord', 'Sued'], cols: ['Sales', 'Cost'] });
const tick = () => new Promise((r) => setTimeout(r, 5));

async function page(srv) {
  const { PivotPage } = await import('../page-pivot.js');
  const cube = { name: 'sales', dimensions: ['years', 'regions', 'measures'], generation: 1 };
  const ctx = { model: { cubes: [cube], dimensions: [] }, can: () => true, onEvent: () => () => {}, toast() {} };
  const p = new PivotPage(ctx);
  // the parts of mount() that need layout are replaced; the pivot logic under test is the real one
  p.grid = { refresh() {}, destroy() {}, selection: { rect: { top: 0, left: 0, bottom: 0, right: 0 }, isEmpty: true }, focus() {}, setData() {}, editor: null, setGrid() {}, update() {}, setRowCount() {} };
  p.banner = new El('div');
  p.status = new El('div');
  p.axes = new El('div');
  p.host = new El('div');
  p.gridHost = new El('div');
  p.cubeSelect = new El('select');
  const opened = p.openCube('sales');
  // release the first view of the page itself
  for (let i = 0; i < 50 && !srv.waiting.length; i++) await tick();
  srv.waiting.shift().reply(answer(1, 1));
  await opened;
  srv.log.length = 0;
  return p;
}

test('an answer that arrives after a newer request is dropped, not shown', async () => {
  const srv = server();
  const p = await page(srv);
  p.pivot.filters.regions = 'Nord';
  const a = p.load(); // filter A
  await tick();
  p.pivot.filters.regions = 'Sued';
  const b = p.load(); // filter B
  await tick();
  const [first, second] = srv.waiting.splice(0);
  assert.equal(first.body.filters.regions, 'Nord');
  assert.equal(second.body.filters.regions, 'Sued');
  second.reply(answer(9, 222)); // B answers first
  await b;
  first.reply(answer(3, 111)); // A answers late
  await a;
  assert.equal(p.pivot.answer.values[0][0], 222, 'the view still shows the answer of filter B');
  assert.equal(p.generation, 9, 'and the generation of that answer, not the older one');
});

test('a paste that is in flight keeps its cube and cells when the filter changes', async () => {
  const srv = server();
  const p = await page(srv);
  p.pivot.filters.regions = 'Nord';
  p.pivot.setAnswer(answer(1, 1));
  // two rows pasted under filter Jan; the page sends them one request after the other
  const pasting = p.paste([['1'], ['2']], { top: 0, left: 0, bottom: 1, right: 0 });
  for (let i = 0; i < 50 && !srv.waiting.length; i++) await tick();
  const first = srv.waiting.shift();
  assert.deepEqual(first.body.cell, ['Jan', 'Nord', 'Sales']);
  p.pivot.filters.regions = 'Sued'; // the user changes the filter while request 1 is in flight
  first.reply({ generation: 2, cells_changed: 1 });
  for (let i = 0; i < 50 && !srv.waiting.length; i++) await tick();
  const second = srv.waiting.shift();
  assert.deepEqual(second.body.cell, ['Feb', 'Nord', 'Sales'], 'row 2 still goes to the filter the paste started under (Nord, not Sued)');
  second.reply({ generation: 3, cells_changed: 1 });
  for (let i = 0; i < 50 && srv.waiting.length === 0; i++) await tick();
  srv.waiting.splice(0).forEach((e) => e.reply(answer(3, 1))); // the reload after the writes
  await pasting;
});
