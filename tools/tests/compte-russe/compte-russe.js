// tools/tests/compte-russe/compte-russe.js — voie russe de « Mon compte » (2026-10-08, lot 4 ; repris de
// backups/russe-20260926/ru_filter_test.js, qui exécutait app.js dans un faux `window` sans navigateur).
//
// Ici le VRAI crates/homelabd/assets/compte/app.js du dépôt tourne dans un VRAI navigateur, sur une page simulée
// (adresse « banc.invalid », tout est servi par interception : aucun appel à Jellyfin, Jellyseerr ni homelabd).
// Ce qui est vérifié (CLAUDE.md, « Voie russe ») : les réglages avancés d'une demande Jellyseerr ne partent QUE pour un
// dossier russe choisi par un compte autorisé ; sinon la demande part SANS réglages (profil et dossier automatiques).
// L'autorisation est celle que renvoie /gc-compte/api/route ; une erreur ou une réponse lente ne doit jamais laisser
// passer un dossier russe. Le bloc « Version » de la fenêtre de demande n'est visible que du compte autorisé, et
// jamais sur télé.
//
// Lancement : tools/bench/bench.sh --offline tools/tests/compte-russe/compte-russe.js
// Contre-épreuve : … --env MUTATE=1 (keepSettings forcé à « vrai » dans la copie servie) doit ÉCHOUER.
'use strict';
const fs = require('fs');
const { launch, checker, sleep } = require('/bench/bench.js');

const APP = fs.readFileSync(process.env.COMPTE_APP || '/repo/crates/homelabd/assets/compte/app.js', 'utf8');
const MUTATE = process.env.MUTATE === '1';
const SRC = MUTATE ? APP.replace(/function keepSettings\(settings\) \{/, 'function keepSettings(settings) { return true;') : APP;
const ORIGIN = 'http://banc.invalid';
const TOKEN = 'jeton-de-banc';
const DESKTOP = 'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0 Safari/537.36';
const TV = 'Mozilla/5.0 (Web0S; Linux/SmartTV) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.6099.270 Safari/537.36 WebAppManager';

// dossiers racine tels que Jellyseerr les propose (chemins fictifs : seule la fin compte pour app.js)
const ROOT = '/srv/media';
const ADV = (folder, profile = 7) => ({ serverId: 1, profileId: profile, rootFolder: `${ROOT}/${folder}`, tags: [] });

// Page simulée : un Jellyfin Enhanced minimal dont requestMedia / requestTvSeasons notent ce qu'ils reçoivent, la
// fenêtre « Demander » avec son bloc d'options avancées, les identifiants d'une session ouverte, puis app.js.
const PAGE = `<!doctype html><html><head><meta charset="utf-8"><title>banc</title></head><body>
<div class="jellyseerr-advanced-options" id="adv">
  <h3>Advanced</h3>
  <div class="jellyseerr-form-row"><label for="m-server">Server</label><select id="m-server"><option value="1">seedbox</option></select></div>
  <div class="jellyseerr-form-row"><label for="m-profile">Quality profile</label><select id="m-profile"><option value="7">FR</option></select></div>
  <div class="jellyseerr-form-row"><label for="m-folder">Root folder</label><select id="m-folder">
    <option value="">--</option>
    <option value="${ROOT}/Movies">Movies</option>
    <option value="${ROOT}/Russian Movies">Russian Movies</option>
    <option value="${ROOT}/Anime Movies">Anime Movies</option>
  </select></div>
</div>
<script>
  localStorage.setItem('jellyfin_credentials', JSON.stringify({ Servers: [{ AccessToken: '${TOKEN}', UserId: 'u1' }] }));
  window.__calls = [];
  window.JellyfinEnhanced = { jellyseerrAPI: {
    requestMedia: function () { window.__calls.push({ fn: 'requestMedia', t: Date.now(), args: Array.prototype.slice.call(arguments) }); return Promise.resolve({ ok: true }); },
    requestTvSeasons: function () { window.__calls.push({ fn: 'requestTvSeasons', t: Date.now(), args: Array.prototype.slice.call(arguments) }); return Promise.resolve({ ok: true }); }
  } };
</script>
<script src="/gc-compte/app.js"></script>
</body></html>`;

// route : { allowed } | 'error' (500) ; delay : réponse lente (ms)
// sans la fenêtre « Demander » (modal: false), seule une demande interroge /api/route
const NO_MODAL = PAGE.replace(/<div class="jellyseerr-advanced-options"[\s\S]*?<\/select><\/div>\n<\/div>\n/, '');

async function openPage(browser, { route, delay = 0, ua = DESKTOP, modal: withModal = true }) {
  const ctx = browser.createBrowserContext ? await browser.createBrowserContext() : await browser.createIncognitoBrowserContext();
  const p = await ctx.newPage();
  await p.setUserAgent(ua);
  await p.setViewport({ width: 1280, height: 800 });
  const seen = { route: 0, tokens: new Set(), other: [] };
  p.gcErrors = [];
  p.on('pageerror', (e) => p.gcErrors.push(String(e.message).slice(0, 200)));
  await p.setRequestInterception(true);
  p.on('request', async (r) => {
    const u = new URL(r.url());
    if (u.origin !== ORIGIN) return r.respond({ status: 404, body: '' });
    if (u.pathname === '/web/') return r.respond({ status: 200, contentType: 'text/html; charset=utf-8', body: withModal ? PAGE : NO_MODAL });
    if (u.pathname === '/gc-compte/app.js') return r.respond({ status: 200, contentType: 'application/javascript; charset=utf-8', body: SRC });
    if (u.pathname === '/gc-compte/api/route') {
      seen.route += 1;
      seen.tokens.add(r.headers()['x-emby-token'] || '(aucun)');
      if (delay) await sleep(delay);
      seen.routeAt = Date.now();
      if (route === 'error') return r.respond({ status: 500, contentType: 'application/json', body: '{"error":"banc"}' });
      return r.respond({ status: 200, contentType: 'application/json', body: JSON.stringify({ allowed: route.allowed, items: [] }) });
    }
    if (u.pathname.startsWith('/gc-compte/api/')) { seen.other.push(u.pathname); return r.respond({ status: 200, contentType: 'application/json', body: '{}' }); }
    return r.respond({ status: 404, body: '' });
  });
  await p.goto(`${ORIGIN}/web/`, { waitUntil: 'domcontentloaded' });
  // l'enveloppe est posée par un minuteur de 2 s
  await p.waitForFunction(() => window.JellyfinEnhanced.jellyseerrAPI.__gcRu === true, { timeout: 8000 });
  return { p, ctx, seen };
}

// envoie une demande par l'API enveloppée ; rend les arguments reçus par le Jellyfin Enhanced simulé
async function send(p, fn, args) {
  return p.evaluate(async (fn, args) => {
    const before = window.__calls.length;
    await window.JellyfinEnhanced.jellyseerrAPI[fn](...args);
    return window.__calls.slice(before);
  }, fn, args);
}
const sent = (calls) => (calls.length === 1 ? calls[0].args : null);
const advOf = (calls, i) => { const a = sent(calls); return a ? a[i] : undefined; };
const kept = (adv, folder) => !!adv && adv.rootFolder === `${ROOT}/${folder}`;
const empty = (adv) => !!adv && typeof adv === 'object' && Object.keys(adv).length === 0;

async function modal(p) {
  return p.evaluate(() => {
    const bl = document.getElementById('adv');
    const sel = document.getElementById('m-folder');
    return {
      display: getComputedStyle(bl).display,
      options: [...sel.options].map((o) => o.value.split('/').pop()),
      label: (document.querySelector('label[for="m-folder"]') || {}).textContent,
      hiddenRows: [...bl.querySelectorAll('.jellyseerr-form-row')].filter((r) => r.style.display === 'none').length,
      help: !!bl.querySelector('.gc-ru-help'),
    };
  });
}

(async () => {
  const c = checker(MUTATE ? 'compte-russe-mutant' : 'compte-russe');
  const browser = await launch();
  let code = 1;
  try {
    // 1. compte autorisé
    {
      const { p, ctx, seen } = await openPage(browser, { route: { allowed: true } });
      let r = await send(p, 'requestMedia', [101, 'movie', ADV('Russian Movies'), false, null]);
      c.check('autorisé · film, dossier « Russian Movies » → réglages envoyés', kept(advOf(r, 2), 'Russian Movies'), r);
      r = await send(p, 'requestTvSeasons', [102, [1, 2], ADV('Russian'), null, false]);
      c.check('autorisé · série, dossier « Russian » → réglages envoyés', kept(advOf(r, 2), 'Russian'), r);
      r = await send(p, 'requestTvSeasons', [103, [1], ADV('Russian/'), null, false]);
      c.check('autorisé · « Russian/ » (barre finale) → réglages envoyés', kept(advOf(r, 2), 'Russian/'), r);
      r = await send(p, 'requestMedia', [104, 'movie', ADV('Movies'), false, null]);
      c.check('autorisé · dossier classique « Movies » → AUCUN réglage', empty(advOf(r, 2)), r);
      r = await send(p, 'requestTvSeasons', [105, [1], ADV('Anime', 8), null, false]);
      c.check('autorisé · dossier « Anime » → AUCUN réglage (rangement automatique)', empty(advOf(r, 2)), r);
      r = await send(p, 'requestMedia', [106, 'movie', ADV('Russian Movies/../Movies'), false, null]);
      c.check('autorisé · « Russian Movies/../Movies » → AUCUN réglage', empty(advOf(r, 2)), r);
      r = await send(p, 'requestMedia', [107, 'movie', ADV('Not Russian'), false, null]);
      c.check('autorisé · « Not Russian » → AUCUN réglage', empty(advOf(r, 2)), r);
      r = await send(p, 'requestMedia', [108, 'movie', null, false, null]);
      c.check('autorisé · sans réglages (null) → objet vide', empty(advOf(r, 2)), r);
      const media = { id: 9 };
      r = await send(p, 'requestMedia', [109, 'movie', ADV('Russian Movies'), true, media]);
      const a = sent(r) || [];
      c.check('autorisé · les autres arguments passent tels quels (id, type, 4K, média)',
        a[0] === 109 && a[1] === 'movie' && a[3] === true && a[4] && a[4].id === 9, a);
      r = await send(p, 'requestTvSeasons', [110, [3, 4], ADV('Russian'), media, true]);
      const t = sent(r) || [];
      c.check('autorisé · requestTvSeasons : saisons, média et 4K intacts',
        t[0] === 110 && JSON.stringify(t[1]) === '[3,4]' && t[3] && t[3].id === 9 && t[4] === true, t);
      // la fenêtre « Demander » est adaptée par un minuteur d'une seconde
      await p.waitForFunction(() => document.getElementById('adv').classList.contains('gc-ru-ok'), { timeout: 5000 }).catch(() => {});
      const m = await modal(p);
      c.check('autorisé · bloc « Version » visible', m.display !== 'none', m);
      c.check('autorisé · choix réduit à Classique et Russe (Anime et vide retirés)',
        JSON.stringify(m.options) === '["Movies","Russian Movies"]', m.options);
      c.check('autorisé · serveur et profil masqués, libellé « Version », explication', m.hiddenRows === 2 && m.label === 'Version' && m.help, m);
      c.check('autorisé · jeton de la session envoyé à /api/route', seen.tokens.size === 1 && seen.tokens.has(TOKEN), [...seen.tokens].map((x) => (x === TOKEN ? 'jeton' : x)));
      c.check('autorisé · aucune erreur JavaScript', p.gcErrors.length === 0, p.gcErrors);
      await ctx.close();
    }
    // 2. autre membre (non autorisé)
    {
      const { p, ctx, seen } = await openPage(browser, { route: { allowed: false } });
      let r = await send(p, 'requestMedia', [201, 'movie', ADV('Russian Movies'), false, null]);
      c.check('non autorisé · dossier « Russian Movies » → AUCUN réglage', empty(advOf(r, 2)), r);
      r = await send(p, 'requestTvSeasons', [202, [1], ADV('Russian'), null, false]);
      c.check('non autorisé · dossier « Russian » → AUCUN réglage', empty(advOf(r, 2)), r);
      r = await send(p, 'requestMedia', [203, 'movie', ADV('Movies'), false, null]);
      c.check('non autorisé · dossier classique → AUCUN réglage', empty(advOf(r, 2)), r);
      await sleep(1500);
      const m = await modal(p);
      c.check('non autorisé · bloc des options avancées caché', m.display === 'none', m);
      c.check('non autorisé · /api/route interrogé une seule fois (la 1re réponse coupe les appels)', seen.route === 1, seen.route);
      await ctx.close();
    }
    // 3. API en erreur : jamais de réglages russes
    {
      const { p, ctx } = await openPage(browser, { route: 'error' });
      const r = await send(p, 'requestMedia', [301, 'movie', ADV('Russian Movies'), false, null]);
      c.check('API en erreur · dossier russe → AUCUN réglage (refus par défaut)', empty(advOf(r, 2)), r);
      await sleep(1500);
      c.check('API en erreur · bloc caché', (await modal(p)).display === 'none');
      await ctx.close();
    }
    // 4. réponse lente : l'autorisation est relue AVANT l'envoi (la demande attend la réponse)
    {
      const { p, ctx } = await openPage(browser, { route: { allowed: true }, delay: 1500, modal: false });
      const r = await send(p, 'requestMedia', [401, 'movie', ADV('Russian Movies'), false, null]);
      c.check('réponse lente, autorisé · la demande attend l\'autorisation puis garde ses réglages', kept(advOf(r, 2), 'Russian Movies'), r);
      await ctx.close();
    }
    {
      const { p, ctx } = await openPage(browser, { route: { allowed: false }, delay: 1500, modal: false });
      const r = await send(p, 'requestMedia', [402, 'movie', ADV('Russian Movies'), false, null]);
      c.check('réponse lente, non autorisé · AUCUN réglage', empty(advOf(r, 2)), r);
      await ctx.close();
    }
    // 4 bis. autorisation encore en route (fenêtre ouverte : sa propre lecture de /api/route répond à 3 s) : des
    // réglages russes ne partent jamais AVANT la réponse « autorisé » (aujourd'hui : la demande part sans réglages ;
    // attendre la réponse en vol serait aussi correct)
    {
      const { p, ctx, seen } = await openPage(browser, { route: { allowed: true }, delay: 3000 });
      const r = await send(p, 'requestMedia', [403, 'movie', ADV('Russian Movies'), false, null]);
      const adv = advOf(r, 2);
      const ok = empty(adv) || (kept(adv, 'Russian Movies') && seen.routeAt && r[0].t >= seen.routeAt);
      c.check('autorisation en route · aucun réglage russe envoyé avant la réponse « autorisé »', ok,
        { reçu: adv, envoyé_à: r[0] && r[0].t, réponse_à: seen.routeAt || null });
      await ctx.close();
    }
    // 5. télé : le bloc reste caché, même pour le compte autorisé
    {
      const { p, ctx } = await openPage(browser, { route: { allowed: true }, ua: TV });
      await sleep(2500);
      c.check('télé, autorisé · bloc des options avancées caché', (await modal(p)).display === 'none');
      await ctx.close();
    }
    code = c.done({ app_bytes: APP.length, mutant: MUTATE });
  } catch (e) {
    c.check('déroulé du banc', false, String(e.message).slice(0, 300));
    code = c.done();
  } finally {
    await browser.close();
  }
  process.exit(code);
})();
