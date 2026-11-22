/*
 * units.js — a shared inch/millimetre display preference.
 *
 * Apps store lengths in their own canonical unit and convert only at the UI
 * edge; this module holds the user's display choice ('in' | 'mm', default
 * 'mm') plus the conversion and formatting helpers.
 *
 * The choice is kept in a `units` cookie scoped to the parent domain
 * (a.example.com and b.example.com share it), so sister apps on the same
 * domain follow one toggle. localStorage is a fallback where cookies can't
 * be set (file://). No dependencies, no build step: load it with a plain
 * <script> and use `window.Units`. Also loadable in Node for tests.
 */
(function (root) {
  'use strict';

  const MM_PER_IN = 25.4;
  const KEY = 'units';
  const UNITS = ['in', 'mm'];
  const DEFAULT = 'mm';
  const DECIMALS = 5;
  const listeners = [];

  /** Cookie domain shared by sibling subdomains, or '' (host-only) for localhost/IPs. */
  function parentDomain(hostname) {
    const host = String(hostname || '').toLowerCase().replace(/\.$/, '');
    if (!host || !host.includes('.') || /^[\d.]+$/.test(host) || host.includes(':')) return '';
    return '.' + host.split('.').slice(-2).join('.');
  }

  const valid = (u) => (UNITS.includes(u) ? u : null);

  function readCookie() {
    if (typeof document === 'undefined') return null;
    const m = document.cookie.match(/(?:^|;\s*)units=([^;]*)/);
    return m ? valid(decodeURIComponent(m[1])) : null;
  }

  function readStorage() {
    try { return valid(root.localStorage && root.localStorage.getItem('plom.' + KEY)); } catch { return null; }
  }

  let current = readCookie() || readStorage() || DEFAULT;

  function get() { return current; }

  function set(unit) {
    const u = valid(unit);
    if (!u) return;
    if (typeof document !== 'undefined') {
      const domain = parentDomain(root.location && root.location.hostname);
      document.cookie = `${KEY}=${u}; path=/; max-age=31536000; SameSite=Lax`
        + (domain ? `; domain=${domain}` : '')
        + (root.location && root.location.protocol === 'https:' ? '; Secure' : '');
    }
    try { root.localStorage && root.localStorage.setItem('plom.' + KEY, u); } catch {}
    changed(u);
  }

  function changed(u) {
    if (u === current) return;
    current = u;
    listeners.slice().forEach((cb) => { try { cb(u); } catch (e) { console.error(e); } });
  }

  /** Call cb(unit) whenever the unit changes — here or in a sibling app (picked up on focus). */
  function onChange(cb) { listeners.push(cb); return () => listeners.splice(listeners.indexOf(cb), 1); }

  const round = (v) => {
    const f = 10 ** DECIMALS;
    return Math.round(Number(v) * f) / f;
  };
  /** Display string: rounded to 5 decimals, trailing zeros trimmed ("1.5875", "14"). */
  function format(v) {
    const n = round(v);
    return Number.isFinite(n) ? String(n === 0 ? 0 : n) : '';
  }

  const inToMm = (v) => Number(v) * MM_PER_IN;
  const mmToIn = (v) => Number(v) / MM_PER_IN;
  // Canonical → display and display → canonical, for apps storing inches or mm.
  const fromIn = (v, u = current) => (u === 'mm' ? inToMm(v) : Number(v));
  const toIn = (v, u = current) => (u === 'mm' ? mmToIn(v) : Number(v));
  const fromMm = (v, u = current) => (u === 'in' ? mmToIn(v) : Number(v));
  const toMm = (v, u = current) => (u === 'in' ? inToMm(v) : Number(v));
  const label = (u = current) => u;
  /** Input step for the current unit, e.g. step(0.0625, 0.5). */
  const step = (inStep, mmStep, u = current) => (u === 'in' ? inStep : mmStep);

  const CSS = '.units-toggle{display:inline-flex;border:1px solid currentColor;border-radius:6px;overflow:hidden;opacity:.85;font-size:12px;line-height:1}'
    + '.units-toggle button{border:0;background:transparent;color:inherit;padding:5px 9px;cursor:pointer;font:inherit}'
    + '.units-toggle button[aria-pressed=true]{background:rgba(127,127,127,.3);font-weight:700}';

  /** Render an "in | mm" segmented toggle into `el`. */
  function mountToggle(el) {
    if (!el || typeof document === 'undefined') return;
    if (!document.getElementById('units-toggle-css')) {
      const style = document.createElement('style');
      style.id = 'units-toggle-css';
      style.textContent = CSS;
      document.head.prepend(style); // first, so app styles can override
    }
    el.classList.add('units-toggle');
    el.setAttribute('role', 'group');
    el.setAttribute('aria-label', 'Units');
    el.title = 'Units for every length (shared across sites on this domain)';
    el.innerHTML = UNITS.map((u) => `<button type="button" data-unit="${u}">${u}</button>`).join('');
    const paint = () => el.querySelectorAll('button').forEach((b) => b.setAttribute('aria-pressed', String(b.dataset.unit === current)));
    el.addEventListener('click', (e) => { const b = e.target.closest('button[data-unit]'); if (b) set(b.dataset.unit); });
    onChange(paint);
    paint();
  }

  if (typeof root.addEventListener === 'function') {
    const sync = () => { const u = readCookie() || readStorage(); if (u) changed(u); };
    root.addEventListener('focus', sync);
    root.addEventListener('storage', (e) => { if (e.key === 'plom.' + KEY) sync(); });
    if (typeof document !== 'undefined') document.addEventListener('visibilitychange', () => { if (!document.hidden) sync(); });
  }

  const Units = {
    MM_PER_IN, DEFAULT, UNITS, get, set, onChange, format, round, label, step,
    inToMm, mmToIn, fromIn, toIn, fromMm, toMm, mountToggle, parentDomain,
  };
  if (typeof module === 'object' && module.exports) module.exports = Units;
  root.Units = Units;
})(typeof window !== 'undefined' ? window : globalThis);
