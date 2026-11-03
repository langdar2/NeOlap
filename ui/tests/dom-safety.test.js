import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, readdirSync } from 'node:fs';
import { safeHref, formatNumber, formatTime } from '../dom.js';

test('links: only same-page and same-site relative targets pass', () => {
  for (const ok of ['/ok/path', './ok', '#anchor', 'sub/page?x=1', '/', '#']) assert.equal(safeHref(ok), true, ok);
  for (const bad of [
    'javascript:alert(1)', 'jAvAsCrIpT:alert(1)', 'JAVASCRIPT:1', ' javascript:alert(1)', 'java\tscript:alert(1)',
    'java\nscript:alert(1)', '\u0001javascript:1', 'vbscript:1', 'data:text/html,<script>1</script>',
    '//evil.example/x', '/\\evil.example/x', 'https://evil.example/', 'http://evil.example/', 'ftp://x/', 'mailto:a@b.c',
    '', ' ', null, undefined, 5, {},
  ]) assert.equal(safeHref(bad), false, JSON.stringify(bad));
});

test('no source file of the UI writes markup from data', () => {
  // The whole point of dom.js: nothing may parse strings as HTML. This guards the code base, so a
  // later change that reaches for innerHTML fails here instead of becoming a stored XSS.
  const dir = new URL('..', import.meta.url);
  const files = readdirSync(dir).filter((f) => f.endsWith('.js'));
  assert.ok(files.length >= 10);
  const forbidden = [/\.innerHTML\b/, /\.outerHTML\b/, /insertAdjacentHTML/, /document\.write/, /\beval\s*\(/, /new Function\s*\(/, /\.srcdoc\b/, /setAttribute\(\s*['"]style['"]/, /setAttribute\(\s*['"]on/i];
  const hits = [];
  for (const f of files) {
    const text = readFileSync(new URL(f, dir), 'utf8').split('\n');
    text.forEach((line, i) => {
      if (/^\s*(\/\/|\*|\/\*)/.test(line)) return; // comments may name what they forbid
      for (const re of forbidden) if (re.test(line)) hits.push(`${f}:${i + 1}: ${line.trim()}`);
    });
  }
  assert.deepEqual(hits, []);
});

test('the pages do not use inline styles or handlers in their markup', () => {
  const html = readFileSync(new URL('../index.html', import.meta.url), 'utf8');
  assert.doesNotMatch(html, /\sstyle\s*=/i, 'the CSP forbids inline styles');
  assert.doesNotMatch(html, /\son[a-z]+\s*=/i, 'the CSP forbids inline handlers');
  assert.doesNotMatch(html, /<script(?![^>]*\bsrc=)[^>]*>/i, 'the CSP forbids inline scripts');
});

test('numbers and times are shown as text, errors pass through unchanged', () => {
  assert.equal(formatNumber(null), '');
  assert.equal(formatNumber(undefined), '');
  assert.equal(formatNumber('#DIV0'), '#DIV0');
  assert.equal(formatNumber(1234.5, 2, 'en-US'), '1,234.5');
  assert.equal(formatNumber(1234.5678, 2, 'en-US'), '1,234.57');
  assert.equal(formatNumber(0, 2, 'en-US'), '0');
  assert.equal(formatNumber(-5, 2, 'en-US'), '-5');
  assert.match(formatTime(0), /^19(69|70)-\d\d-\d\d \d\d:\d\d$/);
});
