// tools/bench/lib/ua-hook.js — chargé avant tout scénario par NODE_OPTIONS=--require /bench/ua-hook.js (2026-10-08).
// Ajoute « GcBanc/1 » à l'agent utilisateur de TOUTES les pages du banc, y compris celles des anciens scénarios qui
// posent leur propre agent (iPhone, webOS…) : les journaux NPM se trient alors d'un `grep -v 'GcBanc/'` (revue :
// obs-bruit-journaux-bancs — 39 % des requêtes Jellyfin d'une semaine venaient des bancs, 92 % des 4xx).
// Les règles des scripts qui lisent l'agent (télé, iPhone, Android) ne sont pas touchées : seul un suffixe s'ajoute.
'use strict';
const Module = require('module');

const SUFFIX = ' GcBanc/1';
const mark = (ua) => (typeof ua === 'string' && !ua.includes('GcBanc/') ? ua + SUFFIX : ua);

function patchPage(proto) {
  if (!proto || proto.__gcBanc) return;
  const orig = proto.setUserAgent;
  proto.setUserAgent = function (ua, ...rest) {
    if (ua && typeof ua === 'object' && typeof ua.userAgent === 'string') ua = { ...ua, userAgent: mark(ua.userAgent) };
    return orig.call(this, mark(ua), ...rest);
  };
  proto.__gcBanc = true;
}

function wrapNewPage(target, browser) {
  if (!target || typeof target.newPage !== 'function' || target.__gcBancNewPage) return;
  const newPage = target.newPage.bind(target);
  target.newPage = async (...args) => {
    const page = await newPage(...args);
    patchPage(Object.getPrototypeOf(page));
    await page.setUserAgent(await browser.userAgent());
    return page;
  };
  target.__gcBancNewPage = true;
}

function patchPuppeteer(pp) {
  if (!pp || pp.__gcBanc || typeof pp.launch !== 'function') return pp;
  const launch = pp.launch.bind(pp);
  pp.launch = async (...args) => {
    const browser = await launch(...args);
    wrapNewPage(browser, browser);
    wrapNewPage(browser.defaultBrowserContext(), browser);
    for (const m of ['createBrowserContext', 'createIncognitoBrowserContext']) {
      if (typeof browser[m] !== 'function') continue;
      const create = browser[m].bind(browser);
      browser[m] = async (...a) => { const ctx = await create(...a); wrapNewPage(ctx, browser); return ctx; };
    }
    return browser;
  };
  pp.__gcBanc = true;
  return pp;
}

const load = Module._load;
Module._load = function (request, ...rest) {
  const m = load.call(this, request, ...rest);
  return request === 'puppeteer' || request === 'puppeteer-core' ? patchPuppeteer(m) : m;
};
