// Import page: pick a file, describe the mapping, preview, run, watch the job.

import * as api from './api.js';
import { h, render } from './dom.js';
import { confirmDialog } from './toast.js';

const SPEC_EXAMPLE = `name = "Import"
cube = "{cube}"
source = { delimiter = ";", decimal = "," }   # Trennzeichen und Dezimalzeichen der Datei

[map]
# Dimension = "Spaltenname in der Datei"
{dims}
{values}
[options]
mode = "overwrite"        # replace_area | add | overwrite
unknown_member = "reject" # reject | skip | create_under:Name
`;

export class ImportPage {
  constructor(ctx) {
    this.ctx = ctx;
    this.timer = null;
  }

  mount(host) {
    const cubes = this.ctx.model.cubes;
    if (!cubes.length) return render(host, h('div', { class: 'card' }, 'Es gibt keinen Cube.'));
    this.cube = h('select', { 'aria-label': 'Cube', onchange: () => this.#template() }, cubes.map((c) => h('option', { value: c.name }, c.name)));
    this.file = h('input', { type: 'file', accept: '.csv,.txt,.xlsx', 'aria-label': 'Datei' });
    this.spec = h('textarea', { class: 'editor-text', spellcheck: false, 'aria-label': 'Import-Spezifikation (TOML)' });
    this.result = h('div', { class: 'card hidden' });
    this.dry = h('button', { type: 'button', onclick: () => this.run(true) }, 'Vorschau');
    this.go = h('button', { type: 'button', class: 'primary', onclick: () => this.run(false) }, 'Importieren');
    render(host,
      h('div', { class: 'toolbar' }, h('label', {}, 'Cube ', this.cube), h('label', {}, 'Datei ', this.file), this.dry, this.go),
      h('div', { class: 'split-even' },
        h('div', {}, h('h3', {}, 'Spezifikation'), this.spec,
          h('p', { class: 'muted' }, 'Die Spezifikation ordnet die Spalten der Datei den Dimensionen zu. Sie ist wiederholbar: Mit „replace_area“ ergibt zweimaliges Laden dasselbe wie einmaliges.')),
        h('div', {}, h('h3', {}, 'Ergebnis'), this.result, h('p', { class: 'muted' }, 'Die Vorschau liest und prüft die Datei, ohne etwas zu ändern.'))));
    this.#template();
  }

  destroy() {
    clearTimeout(this.timer);
  }

  #template() {
    const c = this.ctx.model.cubes.find((x) => x.name === this.cube.value);
    // A file with one column per measure (the usual wide format) maps the measures under [values],
    // not under [map]; the measures dimension is the one of kind "Measures".
    const kinds = new Map(this.ctx.model.dimensions.map((d) => [d.name, d.kind]));
    const measures = c.dimensions.find((d) => kinds.get(d) === 'Measures');
    const dims = c.dimensions.filter((d) => d !== measures).map((d) => `${d} = "${d}"`).join('\n');
    const values = measures
      ? '[values]\n# Element der Kennzahl-Dimension "' + measures + '" = "Spaltenname in der Datei"\n# Sales = "Umsatz"\n'
      : '# Lange Tabelle: eine Spalte mit dem Wert, die Kennzahl steht in einer eigenen Spalte\n# value_column = "Wert"\n';
    this.spec.value = SPEC_EXAMPLE.replace('{cube}', c.name).replace('{dims}', dims).replace('{values}', values);
  }

  async run(dry) {
    const file = this.file.files[0];
    if (!file) return this.ctx.toast('Bitte zuerst eine Datei wählen.', 'error');
    if (!dry && !(await confirmDialog('Import ausführen?', `„${file.name}“ wird in „${this.cube.value}“ geladen. Der Import ist eine einzige Operation und lässt sich mit „Rückgängig“ zurücknehmen.`, 'Importieren'))) return;
    const form = new FormData();
    form.append('spec', this.spec.value);
    form.append('dry_run', String(dry));
    form.append('file', file, file.name);
    this.dry.disabled = this.go.disabled = true;
    this.#show(h('p', { class: 'muted' }, 'Wird hochgeladen …'));
    try {
      const { job } = await api.post('/imports', form);
      await this.#follow(job);
    } catch (e) {
      this.#show(h('div', { class: 'error' }, e.message));
    } finally {
      this.dry.disabled = this.go.disabled = false;
    }
  }

  async #follow(job) {
    for (;;) {
      const j = await api.get(`/jobs/${api.enc(job)}`);
      const pct = Math.min(100, Math.round(j.progress / 10) * 10);
      if (j.status === 'running') {
        this.#show(h('div', {}, h('p', {}, `${phaseName(j.phase)} …`), h('div', { class: 'progress', role: 'progressbar', 'aria-valuenow': String(j.progress) }, h('div', { class: `p-${pct}` }))));
        await new Promise((r) => { this.timer = setTimeout(r, 300); });
        continue;
      }
      return this.#report(j);
    }
  }

  #report(j) {
    const r = j.result;
    if (!r) return this.#show(h('div', { class: 'error' }, j.error || 'Der Import ist fehlgeschlagen.'));
    const line = (label, v) => h('tr', {}, h('td', {}, label), h('td', { class: 'num' }, String(v)));
    const rows = [
      line('Zeilen gelesen', r.rows_read), line('Zellen erzeugt', r.cells_created), line('Zellen geändert', r.cells_updated),
      line('Zellen gelöscht', r.cells_deleted), line('Unverändert', r.cells_unchanged), line('Zeilen abgelehnt', r.rows_rejected),
    ];
    const rejected = (r.rejected || []).slice(0, 50).map((x) => h('tr', {}, h('td', { class: 'num' }, String(x.line)), h('td', {}, x.reason)));
    this.#show(h('div', {},
      h('p', { class: j.status === 'failed' ? 'error' : 'ok' }, j.status === 'failed' ? `Nichts geladen: ${j.error}` : r.dry_run ? 'Vorschau, es wurde nichts geändert.' : 'Import abgeschlossen.'),
      h('table', { class: 'list' }, h('tbody', {}, rows)),
      r.members_created?.length ? h('p', {}, `Neue Elemente: ${r.members_created.join(', ')}`) : null,
      rejected.length ? h('div', {}, h('h3', {}, 'Abgelehnte Zeilen'), h('table', { class: 'list' }, h('thead', {}, h('tr', {}, h('th', {}, 'Zeile'), h('th', {}, 'Grund'))), h('tbody', {}, rejected)),
        r.rows_rejected > rejected.length ? h('p', { class: 'muted' }, `… und ${r.rows_rejected - rejected.length} weitere.`) : null) : null));
  }

  #show(node) {
    this.result.classList.remove('hidden');
    render(this.result, node);
  }
}

function phaseName(p) {
  return { queued: 'Wartet', reading: 'Liest und prüft', checking: 'Prüft', importing: 'Importiert', writing: 'Schreibt' }[p] || p;
}
