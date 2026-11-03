// Administration: users, roles, area grants, tokens and backup.

import * as api from './api.js';
import { h, render } from './dom.js';
import { confirmDialog, dialog, promptDialog } from './toast.js';

const ROLES = ['reader', 'planner', 'modeler', 'admin'];
const ROLE_NAMES = { reader: 'Leser', planner: 'Planer', modeler: 'Modellierer', admin: 'Admin' };

export class AdminPage {
  constructor(ctx) {
    this.ctx = ctx;
  }

  mount(host) {
    this.users = h('div', {});
    this.grants = h('div', {});
    render(host,
      h('div', { class: 'toolbar' },
        h('button', { type: 'button', class: 'primary', onclick: () => this.#addUser() }, 'Benutzer anlegen'),
        h('button', { type: 'button', onclick: () => this.#addGrant() }, 'Recht erteilen'),
        h('button', { type: 'button', onclick: () => this.#backup() }, 'Backup erstellen')),
      h('h2', {}, 'Benutzer'), this.users,
      h('h2', {}, 'Rechte'),
      h('p', { class: 'muted' }, 'Planer und Leser sehen nur Cubes mit einem Recht. Ein Bereichsrecht begrenzt das Schreiben; wer einen Cube lesen darf, liest ihn vollständig. Modellierer und Admins brauchen keine Rechte.'),
      this.grants);
    this.reload();
  }

  destroy() {}

  async reload() {
    try {
      const [u, g] = await Promise.all([api.get('/admin/users'), api.get('/admin/grants')]);
      this.#drawUsers(u.users);
      this.#drawGrants(g.grants);
    } catch (e) {
      this.ctx.toast(e.message, 'error');
    }
  }

  #drawUsers(users) {
    const me = this.ctx.me.user.toLowerCase();
    const rows = users.map((u) => h('tr', {},
      h('td', {}, u.name, u.name.toLowerCase() === me ? h('span', { class: 'muted' }, ' (Sie)') : null),
      h('td', {}, h('select', { 'aria-label': `Rolle von ${u.name}`, onchange: (e) => this.#role(u.name, e.target.value) },
        ROLES.map((r) => h('option', { value: r, selected: r === u.role ? true : null }, ROLE_NAMES[r])))),
      h('td', {}, h('div', { class: 'row' },
        h('button', { type: 'button', onclick: () => this.#password(u.name) }, 'Passwort'),
        h('button', { type: 'button', onclick: () => this.#token(u.name) }, 'Token'),
        h('button', { type: 'button', class: 'danger', onclick: () => this.#remove(u.name) }, 'Löschen')))));
    render(this.users, h('table', { class: 'list' }, h('thead', {}, h('tr', {}, ['Name', 'Rolle', ''].map((t) => h('th', {}, t)))), h('tbody', {}, rows)));
  }

  #drawGrants(grants) {
    if (!grants.length) return render(this.grants, h('p', { class: 'muted' }, 'Noch keine Rechte erteilt.'));
    const rows = grants.map((g) => h('tr', {},
      h('td', {}, g.user), h('td', {}, g.cube), h('td', {}, { read: 'lesen', write: 'schreiben', none: 'kein Zugriff' }[g.access]),
      h('td', {}, g.area.length ? g.area.map(([d, m]) => `${d}: ${m.join(', ')}`).join(' · ') : 'ganzer Cube'),
      h('td', {}, h('button', { type: 'button', class: 'danger', onclick: () => this.#revoke(g) }, 'Entziehen'))));
    render(this.grants, h('table', { class: 'list' }, h('thead', {}, h('tr', {}, ['Benutzer', 'Cube', 'Zugriff', 'Bereich', ''].map((t) => h('th', {}, t)))), h('tbody', {}, rows)));
  }

  async #run(fn, done) {
    try {
      await fn();
      if (done) this.ctx.toast(done, 'ok');
      await this.reload();
    } catch (e) {
      this.ctx.toast(e.message, 'error');
    }
  }

  async #addUser() {
    const r = await promptDialog('Benutzer anlegen', [
      { name: 'name', label: 'Name' }, { name: 'password', label: 'Passwort (mindestens 10 Zeichen)', type: 'password' },
      { name: 'role', label: 'Rolle', type: 'select', options: ROLES, value: 'planner' }], 'Anlegen');
    if (r) await this.#run(() => api.post('/admin/users', { name: r.name.trim(), password: r.password, role: r.role }), 'Benutzer angelegt.');
  }

  async #role(name, role) {
    await this.#run(() => api.patch(`/admin/users/${api.enc(name)}`, { role }), 'Rolle geändert.');
  }

  async #password(name) {
    const r = await promptDialog(`Neues Passwort für ${name}`, [{ name: 'password', label: 'Passwort (mindestens 10 Zeichen)', type: 'password' }], 'Ändern');
    if (r) await this.#run(() => api.patch(`/admin/users/${api.enc(name)}`, { password: r.password }), 'Passwort geändert. Alle Sitzungen und Tokens des Benutzers sind beendet.');
  }

  async #remove(name) {
    if (await confirmDialog(`${name} löschen?`, 'Der Benutzer, seine Sitzungen und Tokens werden gelöscht. Seine Einträge im Journal bleiben.', 'Löschen')) {
      await this.#run(() => api.del(`/admin/users/${api.enc(name)}`), 'Benutzer gelöscht.');
    }
  }

  async #token(name) {
    try {
      const r = await api.post(`/admin/users/${api.enc(name)}/token`);
      const field = h('input', { type: 'text', readonly: true, value: r.token, 'aria-label': 'Token' });
      await dialog(`Token für ${name}`, h('div', {}, h('p', {}, 'Dieser Token wird nur jetzt angezeigt. Bewahren Sie ihn sicher auf.'), field, h('p', { class: 'muted' }, 'Verwendung: Authorization: Bearer <token>')), [{ label: 'Schließen', value: true, kind: 'primary' }]);
    } catch (e) {
      this.ctx.toast(e.message, 'error');
    }
  }

  async #addGrant() {
    const cubes = this.ctx.model.cubes.map((c) => c.name);
    const r = await promptDialog('Recht erteilen', [
      { name: 'user', label: 'Benutzer' }, { name: 'cube', label: 'Cube', type: 'select', options: cubes, value: cubes[0] },
      { name: 'access', label: 'Zugriff', type: 'select', options: ['write', 'read', 'none'], value: 'write' },
      { name: 'area', label: 'Bereich (leer = ganzer Cube), z. B. regions=Nord|Süd; version=Plan' }], 'Erteilen');
    if (!r) return;
    let area;
    try {
      area = parseArea(r.area);
    } catch (e) {
      return this.ctx.toast(e.message, 'error');
    }
    await this.#run(() => api.put('/admin/grants', { user: r.user.trim(), cube: r.cube, access: r.access, area }), 'Recht erteilt.');
  }

  async #revoke(g) {
    await this.#run(() => api.del('/admin/grants', { user: g.user, cube: g.cube, area: g.area }), 'Recht entzogen.');
  }

  async #backup() {
    await this.#run(async () => {
      const r = await api.post('/admin/backup');
      this.ctx.toast(`Backup geschrieben: ${r.file}`, 'ok');
    });
  }
}

/** `regions=Nord|Süd; version=Plan` → `[["regions", ["Nord","Süd"]], ["version", ["Plan"]]]`. */
export function parseArea(text) {
  return text.split(';').map((p) => p.trim()).filter(Boolean).map((part) => {
    const [dim, members] = part.split('=');
    if (!dim || !members || !members.trim()) throw new Error(`„${part}“ hat nicht die Form Dimension=Element|Element.`);
    return [dim.trim(), members.split('|').map((m) => m.trim()).filter(Boolean)];
  });
}
