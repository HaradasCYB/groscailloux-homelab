// tools/bench/lib/bench.js — outils communs des scénarios lancés par tools/bench/bench.sh (2026-10-08, lot 4).
// Repris des bancs de backups/ (header_dump.js, modern_ui.js, tv_home_lite.js) : appareils, scripts candidats
// injectés DANS CE NAVIGATEUR seulement (jamais en production), connexion, mesure à l'écran.
// Règle (CLAUDE.md, 03/10) : un élément se contrôle par sa TAILLE À L'ÉCRAN (getBoundingClientRect), jamais par sa
// seule présence dans la page — le tchat était « présent » dans l'en-tête caché de la 12.1.
'use strict';
const fs = require('fs');
const path = require('path');
const puppeteer = require('puppeteer');

const UA = {
  iphone: 'Mozilla/5.0 (iPhone; CPU iPhone OS 18_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.5 Mobile/15E148 Safari/604.1',
  android: 'Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/147.0.0.0 Mobile Safari/537.36',
  ipad: 'Mozilla/5.0 (iPad; CPU OS 18_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.5 Mobile/15E148 Safari/604.1',
  tv: 'Mozilla/5.0 (Web0S; Linux/SmartTV) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.6099.270 Safari/537.36 WebAppManager',
};
const MOBILE = { isMobile: true, hasTouch: true, deviceScaleFactor: 2 };
// appareil → agent (null = celui du navigateur) et fenêtre
const DEVICES = {
  desktop: { ua: null, viewport: { width: +(process.env.WIDTH || 1600), height: +(process.env.HEIGHT || 900) } },
  phone: { ua: UA.iphone, viewport: { width: 390, height: 844, ...MOBILE } },
  iphone: { ua: UA.iphone, viewport: { width: 390, height: 844, ...MOBILE } },
  android: { ua: UA.android, viewport: { width: 412, height: 915, ...MOBILE } },
  tablet: { ua: UA.ipad, viewport: { width: 820, height: 1180, ...MOBILE } },
  tv: { ua: UA.tv, viewport: { width: 1920, height: 1080 } },
};
// scripts JavaScript Injector exécutés AVANT la connexion (RequiresAuthentication: false → public.js)
const PUBLIC_SCRIPTS = ['gc-tv.js', 'gc-lang.js', 'gc-socket.js'];

const env = {
  jf: process.env.JF_URL,
  user: process.env.USER_NAME,
  pw: process.env.PW,
  userId: process.env.USER_ID,
  device: process.env.DEVICE || 'desktop',
  layout: process.env.LAYOUT || '',
  benchDevice: process.env.BENCH_DEVICE || process.env.DEVICE || 'desktop',
  item: process.env.ITEM || '',
  candidateDir: process.env.NO_INJECT === '1' ? '' : (process.env.CANDIDATE_DIR || ''),
  out: '/out',
  repo: '/repo',
};
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function launch(extraArgs = []) {
  return puppeteer.launch({ args: ['--no-sandbox', '--lang=fr-FR', ...extraArgs] });
}

// Candidats du dossier CANDIDATE_DIR : chat.js → /gc-chat/app.js, compte.js → /gc-compte/app.js, gc-*.js ajoutés au
// private.js (ou public.js) servi, groscailloux-tv.css à la place du CustomCss, deployed-*.txt = texte exact d'une
// version déployée à retirer du private.js (scripts qui ne reprennent pas la main par VERSION).
function candidates() {
  const dir = env.candidateDir;
  if (!dir || !fs.existsSync(dir)) return null;
  const files = fs.readdirSync(dir).sort();
  const read = (f) => fs.readFileSync(path.join(dir, f), 'utf8');
  const js = (pub) => files.filter((f) => /^gc-.*\.js$/.test(f) && PUBLIC_SCRIPTS.includes(f) === pub).map(read).join('\n;\n');
  return {
    chat: files.includes('chat.js') ? read('chat.js') : null,
    compte: files.includes('compte.js') ? read('compte.js') : null,
    css: files.includes('groscailloux-tv.css') ? read('groscailloux-tv.css') : null,
    privateJs: js(false),
    publicJs: js(true),
    deployed: files.filter((f) => /^deployed-.*\.txt$/.test(f)).map((f) => [f, read(f)]),
  };
}

async function injectCandidates(page) {
  const c = candidates();
  if (!c) return false;
  const JS = 'application/javascript; charset=utf-8';
  await page.setBypassServiceWorker(true);
  await page.setRequestInterception(true);
  page.on('request', async (r) => {
    const u = r.url().split('?')[0];
    try {
      if (c.chat && /\/gc-chat\/app\.js$/.test(u)) return await r.respond({ status: 200, contentType: JS, body: c.chat });
      if (c.compte && /\/gc-compte\/app\.js$/.test(u)) return await r.respond({ status: 200, contentType: JS, body: c.compte });
      const inj = /\/JavaScriptInjector\/(private|public)\.js$/.exec(u);
      if (inj && (inj[1] === 'private' ? c.privateJs || c.deployed.length : c.publicJs)) {
        let body = await (await fetch(r.url(), { headers: r.headers() })).text();
        if (inj[1] === 'private') {
          for (const [f, old] of c.deployed) {
            if (body.includes(old)) body = body.replace(old, `/* ${f} retiré par le banc */`);
            else console.log(`banc : ${f} introuvable dans private.js`);
          }
        }
        return await r.respond({ status: 200, contentType: JS, body: body + '\n;\n' + (inj[1] === 'private' ? c.privateJs : c.publicJs) });
      }
      if (c.css && /\/Branding\/Configuration$/i.test(u)) {
        const j = await (await fetch(r.url(), { headers: r.headers() })).json();
        j.CustomCss = c.css;
        return await r.respond({ status: 200, contentType: 'application/json', body: JSON.stringify(j) });
      }
      if (c.css && /\/Branding\/Css$/i.test(u)) return await r.respond({ status: 200, contentType: 'text/css', body: c.css });
    } catch (e) { /* requête servie telle quelle */ }
    if (!r.isInterceptResolutionHandled()) r.continue();
  });
  return true;
}

// Nouvelle page réglée pour l'appareil (agent, fenêtre, mise en page jellyfin-web) avec les candidats éventuels.
async function newPage(browser, device = env.device, layout = env.layout) {
  const p = await browser.newPage();
  const d = DEVICES[device] || DEVICES.desktop;
  if (d.ua) await p.setUserAgent(d.ua);
  await p.setViewport(d.viewport);
  if (layout) await p.evaluateOnNewDocument((l) => { try { localStorage.setItem('layout', l); } catch (e) { /* stockage refusé */ } }, layout);
  p.gcErrors = [];
  p.on('pageerror', (e) => p.gcErrors.push(String(e.message).slice(0, 200)));
  p.gcCandidates = await injectCandidates(p);
  return p;
}

// Connexion par le formulaire manuel (tous les comptes sont cachés de l'écran de connexion).
async function login(page, user = env.user, pw = env.pw) {
  await page.goto(`${env.jf}/web/`, { waitUntil: 'networkidle2', timeout: 90000 });
  await page.waitForSelector('#txtManualName', { visible: true, timeout: 40000 });
  await page.type('#txtManualName', user);
  await page.type('#txtManualPassword', pw);
  await page.keyboard.press('Enter');
  await page.waitForFunction(() => /home/.test(location.hash), { timeout: 90000 });
}

// Boîtes À L'ÉCRAN des éléments d'un sélecteur : [{x, y, w, h}] (w = h = 0 : caché).
async function boxes(page, selector) {
  return page.evaluate((s) => [...document.querySelectorAll(s)].map((el) => {
    const r = el.getBoundingClientRect(); const cs = getComputedStyle(el);
    const shown = r.width > 0 && r.height > 0 && cs.visibility !== 'hidden' && cs.display !== 'none';
    return { x: Math.round(r.left), y: Math.round(r.top), w: shown ? Math.round(r.width) : 0, h: shown ? Math.round(r.height) : 0 };
  }), selector);
}
// au moins un élément visible et dans la fenêtre
async function onScreen(page, selector) {
  const vp = page.viewport();
  return (await boxes(page, selector)).some((b) => b.w > 0 && b.h > 0 && b.x < vp.width && b.y < vp.height && b.x + b.w > 0 && b.y + b.h > 0);
}
// attend qu'un élément soit visible à l'écran (false au bout de ms)
async function waitOnScreen(page, selector, ms = 30000) {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (await onScreen(page, selector)) return true; await sleep(500); }
  return false;
}

// Résultat lisible : une ligne par contrôle, un JSON dans /out, code de sortie 1 au premier échec.
function checker(name) {
  const results = [];
  return {
    check(label, ok, detail) {
      results.push({ label, ok: !!ok, detail: detail === undefined ? null : detail });
      console.log(`${ok ? 'OK    ' : 'ÉCHEC '} ${label}${detail !== undefined && !ok ? ' — ' + JSON.stringify(detail).slice(0, 300) : ''}`);
      return !!ok;
    },
    done(extra = {}) {
      const failed = results.filter((r) => !r.ok).length;
      const file = path.join(env.out, `${name}-${env.benchDevice.replace(/[^a-z0-9-]/gi, '_')}.json`);
      try { fs.writeFileSync(file, JSON.stringify({ name, device: env.benchDevice, results, ...extra }, null, 1)); } catch (e) { /* /out absent */ }
      console.log(`${failed ? 'ÉCHEC' : 'RÉUSSI'} : ${results.length - failed}/${results.length} contrôle(s)`);
      return failed ? 1 : 0;
    },
  };
}

module.exports = { env, DEVICES, UA, sleep, launch, newPage, login, boxes, onScreen, waitOnScreen, checker, injectCandidates };
