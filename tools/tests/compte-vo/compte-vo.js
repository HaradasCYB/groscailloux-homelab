// tools/tests/compte-vo/compte-vo.js — filet « VO » de Mon compte face à la préférence native « Langue d'origine »
// (2026-10-08, lot 4).
//
// Le VRAI crates/homelabd/assets/compte/app.js tourne dans un vrai navigateur, sur une page simulée (adresse
// « banc.invalid », tout est servi par interception : aucun appel à Jellyfin, Jellyseerr ni homelabd). La page imite
// jellyfin-web pendant une lecture : un <video>, un ApiClient minimal, la session de l'appareil (titre lu, piste audio
// reçue), la fiche du titre (pistes, OriginalLanguage, série) et /gc-compte/api/original (langue TMDB).
// Ce qui est vérifié (homelab_core::vo_native) :
//   - piste reçue déjà la bonne (VO choisie par le serveur en « Langue d'origine », ou jpn) : AUCUNE commande et aucune
//     autre requête (ni série, ni TMDB) ;
//   - partie en VF : la langue d'origine vient de la fiche Jellyfin (OriginalLanguage, de la série pour un épisode) sans
//     appel TMDB ; repli TMDB seulement pour une fiche sans OriginalLanguage ; anglais si l'origine est inconnue ;
//   - jamais pour un titre d'origine française, jamais l'audiodescription, jamais un doublage qui n'est pas la VO ;
//   - une seule tentative par titre ; rien en mode « fr ».
//
// Lancement : tools/bench/bench.sh --offline tools/tests/compte-vo/compte-vo.js
// Contre-épreuve : … --env MUTATE=1 (piste reçue ignorée et OriginalLanguage de la fiche ignorée) doit ÉCHOUER.
'use strict';
const fs = require('fs');
const { launch, checker, sleep } = require('/bench/bench.js');

const APP = fs.readFileSync(process.env.COMPTE_APP || '/repo/crates/homelabd/assets/compte/app.js', 'utf8');
const MUTATE = process.env.MUTATE === '1';
const SRC = MUTATE
  ? APP.replace('if (!startedInFrench(src.MediaStreams', 'if (false && !startedInFrench(src.MediaStreams')
    .replace('var v = x && x.OriginalLanguage;', 'var v = null;')
  : APP;
const ORIGIN = 'http://banc.invalid';
const TOKEN = 'jeton-de-banc';
const DESKTOP = 'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0 Safari/537.36';
// appareil « tv » du lanceur : agent webOS (le filet y tourne toutes les 5 s au lieu de 2,5 s)
const TV = 'Mozilla/5.0 (Web0S; Linux/SmartTV) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.6099.270 Safari/537.36 WebAppManager';
const UA_PAGE = process.env.DEVICE === 'tv' ? TV : DESKTOP;

// pistes et fiches simulées (index Jellyfin 12.1 : sous-titres externes d'abord, puis vidéo, audio…)
const V = { Type: 'Video', Index: 0, Codec: 'hevc' };
const A = (i, lang, title) => ({ Type: 'Audio', Index: i, Language: lang, Title: title || null, DisplayTitle: title || lang });
function item({ id, ol = null, series = null, tmdb = '100', streams }) {
  return {
    Id: id, Name: id, Type: series ? 'Episode' : 'Movie', OriginalLanguage: ol, SeriesId: series,
    ProviderIds: tmdb ? { Tmdb: tmdb } : {}, MediaSources: [{ Id: id, MediaStreams: [V, ...streams] }],
  };
}
function seriesItem({ id, ol = null, tmdb = '200' }) {
  return { Id: id, Name: id, Type: 'Series', OriginalLanguage: ol, ProviderIds: tmdb ? { Tmdb: tmdb } : {} };
}

const PAGE = (mode) => `<!doctype html><html><head><meta charset="utf-8"><title>banc</title></head><body>
<video></video>
<script>
  localStorage.setItem('jellyfin_credentials', JSON.stringify({ Servers: [{ AccessToken: '${TOKEN}', UserId: 'u1' }] }));
  ${mode ? `localStorage.setItem('gc-lang-mode', '${mode}');` : ''}
  window.ApiClient = {
    getCurrentUserId: function () { return 'u1'; },
    deviceId: function () { return 'dev1'; },
    accessToken: function () { return '${TOKEN}'; },
    getUrl: function (p, q) { return location.origin + '/' + p + (q ? '?' + new URLSearchParams(q) : ''); }
  };
</script>
<script src="/gc-compte/app.js"></script>
</body></html>`;

// scène : { mode, items: {id: fiche}, playing: { id, audio } (piste reçue), tmdb: { 'movie:100': 'en' }, me }
async function openPage(browser, scene) {
  const ctx = browser.createBrowserContext ? await browser.createBrowserContext() : await browser.createIncognitoBrowserContext();
  const p = await ctx.newPage();
  await p.setUserAgent(UA_PAGE);
  await p.setViewport({ width: 1280, height: 800 });
  const seen = { sessions: 0, items: [], original: [], commands: [], other: [] };
  p.gcErrors = [];
  p.on('pageerror', (e) => p.gcErrors.push(String(e.message).slice(0, 200)));
  await p.setRequestInterception(true);
  p.on('request', async (r) => {
    const u = new URL(r.url());
    const json = (body, status = 200) => r.respond({ status, contentType: 'application/json', body: JSON.stringify(body) });
    if (u.origin !== ORIGIN) return r.respond({ status: 404, body: '' });
    if (u.pathname === '/web/') return r.respond({ status: 200, contentType: 'text/html; charset=utf-8', body: PAGE(scene.mode) });
    if (u.pathname === '/gc-compte/app.js') return r.respond({ status: 200, contentType: 'application/javascript; charset=utf-8', body: SRC });
    if (u.pathname === '/Sessions' && r.method() === 'GET') {
      seen.sessions += 1;
      const pl = scene.playing;
      return json([{ Id: 's1', DeviceId: 'dev1', UserId: 'u1', NowPlayingItem: { Id: pl.id },
        PlayState: { AudioStreamIndex: pl.audio, MediaSourceId: pl.id } },
      { Id: 's2', DeviceId: 'autre', UserId: 'u1', NowPlayingItem: { Id: 'ailleurs' }, PlayState: { AudioStreamIndex: 1 } }]);
    }
    const m = u.pathname.match(/^\/Users\/u1\/Items\/([^/]+)$/);
    if (m) {
      seen.items.push(m[1]);
      const it = scene.items[m[1]];
      return it ? json(it) : json({}, 404);
    }
    if (u.pathname === '/Sessions/s1/Command' && r.method() === 'POST') {
      seen.commands.push(JSON.parse(r.postData() || '{}'));
      return r.respond({ status: 204, body: '' });
    }
    if (u.pathname === '/gc-compte/api/original') {
      const key = `${u.searchParams.get('kind')}:${u.searchParams.get('tmdb')}`;
      seen.original.push(key);
      return json({ lang: (scene.tmdb || {})[key] || null });
    }
    if (u.pathname === '/gc-compte/api/me') return json({ language: scene.me || 'fr', devices: [], history: [] });
    if (u.pathname.startsWith('/gc-compte/api/')) { seen.other.push(u.pathname); return json({}); }
    seen.other.push(u.pathname);
    return r.respond({ status: 404, body: '' });
  });
  await p.goto(`${ORIGIN}/web/`, { waitUntil: 'domcontentloaded' });
  return { p, ctx, seen };
}

// attend la fin du premier passage du filet VO (source décidée, ou erreur), puis le temps d'envoyer la commande
async function settle(p, ms = 9000) {
  await p.waitForFunction(() => window.__gcVoSource !== undefined || window.__gcVoError !== undefined, { timeout: ms }).catch(() => {});
  await sleep(800);
  return p.evaluate(() => ({ source: window.__gcVoSource, orig: window.__gcVoOrig, switched: window.__gcVoSwitched, error: window.__gcVoError }));
}
const idx = (cmds) => cmds.map((c) => c.Name === 'SetAudioStreamIndex' && c.Arguments && c.Arguments.Index);

(async () => {
  const c = checker(MUTATE ? 'compte-vo-mutant' : 'compte-vo');
  const browser = await launch();
  let code = 1;
  // animé MULTi : VF par défaut, VO japonaise ; film anglais MULTi ; film français avec AD
  const ANIME = [A(1, 'fre'), A(2, 'jpn')];
  const FILM_EN = [A(1, 'fre'), A(2, 'eng')];
  try {
    // 1. « Langue d'origine » : le serveur a déjà choisi le japonais pour un épisode d'animé
    {
      const { p, ctx, seen } = await openPage(browser, {
        mode: 'vo', playing: { id: 'ep1', audio: 2 },
        items: { ep1: item({ id: 'ep1', series: 'se1', streams: ANIME }), se1: seriesItem({ id: 'se1', ol: 'ja' }) },
      });
      const s = await settle(p);
      c.check('VO reçue (jpn) · aucune commande', seen.commands.length === 0, seen.commands);
      c.check('VO reçue (jpn) · ni série ni TMDB demandées (seule la fiche lue)', JSON.stringify(seen.items) === '["ep1"]' && seen.original.length === 0, { items: seen.items, tmdb: seen.original });
      c.check('VO reçue (jpn) · issue « deja-vo »', s.source === 'deja-vo', s);
      c.check('VO reçue · aucune erreur JavaScript', p.gcErrors.length === 0 && !s.error, { js: p.gcErrors, vo: s.error });
      await ctx.close();
    }
    // 2. film anglais, le serveur a déjà choisi l'anglais
    {
      const { p, ctx, seen } = await openPage(browser, {
        mode: 'vo', playing: { id: 'm1', audio: 2 }, items: { m1: item({ id: 'm1', ol: 'en', streams: FILM_EN }) },
      });
      await settle(p);
      c.check('VO reçue (eng) · aucune commande, aucun appel TMDB', seen.commands.length === 0 && seen.original.length === 0, seen);
      await ctx.close();
    }
    // 3. partie en VF malgré une fiche remplie (compte gardé sur jpn, ou client touché par le bogue) : bascule sur
    // l'anglais d'après la fiche Jellyfin, sans TMDB
    {
      const { p, ctx, seen } = await openPage(browser, {
        mode: 'vo', playing: { id: 'm2', audio: 1 }, items: { m2: item({ id: 'm2', ol: 'en', streams: FILM_EN }) },
        tmdb: { 'movie:100': 'ja' },
      });
      const s = await settle(p);
      c.check('VF reçue, fiche « en » · SetAudioStreamIndex vers la piste anglaise', JSON.stringify(idx(seen.commands)) === '["2"]', seen.commands);
      c.check('VF reçue, fiche « en » · langue lue sur la fiche, pas chez TMDB', s.source === 'jellyfin' && s.orig === 'en' && seen.original.length === 0, { s, tmdb: seen.original });
      await ctx.close();
    }
    // 4. épisode : OriginalLanguage sur la série seulement
    {
      const { p, ctx, seen } = await openPage(browser, {
        mode: 'vo', playing: { id: 'ep2', audio: 1 },
        items: { ep2: item({ id: 'ep2', series: 'se2', streams: ANIME }), se2: seriesItem({ id: 'se2', ol: 'ja' }) },
      });
      const s = await settle(p);
      c.check('épisode en VF, série « ja » · bascule sur le japonais', JSON.stringify(idx(seen.commands)) === '["2"]', seen.commands);
      c.check('épisode en VF, série « ja » · série lue, TMDB non', JSON.stringify(seen.items) === '["ep2","se2"]' && seen.original.length === 0 && s.source === 'jellyfin', { items: seen.items, tmdb: seen.original, s });
      await ctx.close();
    }
    // 5. fiche sans OriginalLanguage : repli TMDB (film)
    {
      const { p, ctx, seen } = await openPage(browser, {
        mode: 'vo', playing: { id: 'm3', audio: 1 }, items: { m3: item({ id: 'm3', tmdb: '300', streams: [A(1, 'fre'), A(2, 'ita'), A(3, 'eng')] }) },
        tmdb: { 'movie:300': 'it' },
      });
      const s = await settle(p);
      c.check('fiche vide · repli TMDB « it » → piste italienne', JSON.stringify(idx(seen.commands)) === '["2"]' && s.source === 'tmdb', { cmd: seen.commands, s });
      c.check('fiche vide · TMDB interrogé une fois (film 300)', JSON.stringify(seen.original) === '["movie:300"]', seen.original);
      await ctx.close();
    }
    // 6. épisode sans OriginalLanguage nulle part : TMDB par la série
    {
      const { p, ctx, seen } = await openPage(browser, {
        mode: 'vo', playing: { id: 'ep3', audio: 1 },
        items: { ep3: item({ id: 'ep3', series: 'se3', tmdb: null, streams: ANIME }), se3: seriesItem({ id: 'se3', tmdb: '400' }) },
        tmdb: { 'tv:400': 'ja' },
      });
      await settle(p);
      c.check('épisode, rien sur la fiche · TMDB de la série (tv:400) → japonais', JSON.stringify(idx(seen.commands)) === '["2"]' && JSON.stringify(seen.original) === '["tv:400"]', { cmd: seen.commands, tmdb: seen.original });
      await ctx.close();
    }
    // 7. origine inconnue partout : l'anglais
    {
      const { p, ctx, seen } = await openPage(browser, {
        mode: 'vo', playing: { id: 'm4', audio: 1 }, items: { m4: item({ id: 'm4', tmdb: '500', streams: FILM_EN }) },
      });
      await settle(p);
      c.check('origine inconnue · anglais', JSON.stringify(idx(seen.commands)) === '["2"]', seen.commands);
      await ctx.close();
    }
    // 8. titre français (fiche « fr ») : jamais de bascule, même avec une piste anglaise et une AD
    {
      const { p, ctx, seen } = await openPage(browser, {
        mode: 'vo', playing: { id: 'm5', audio: 1 },
        items: { m5: item({ id: 'm5', ol: 'fr', streams: [A(1, 'fre', 'VFF'), A(2, 'fre', 'VFF AD'), A(3, 'eng')] }) },
      });
      await settle(p);
      c.check('origine française · aucune commande', seen.commands.length === 0, seen.commands);
      await ctx.close();
    }
    // 9. origine coréenne sans piste coréenne : le doublage anglais n'est pas une VO
    {
      const { p, ctx, seen } = await openPage(browser, {
        mode: 'vo', playing: { id: 'm6', audio: 1 }, items: { m6: item({ id: 'm6', ol: 'ko', streams: FILM_EN }) },
      });
      await settle(p);
      c.check('origine « ko » sans piste coréenne · aucune commande', seen.commands.length === 0, seen.commands);
      await ctx.close();
    }
    // 10. audiodescription anglaise avant la piste anglaise : jamais l'AD
    {
      const { p, ctx, seen } = await openPage(browser, {
        mode: 'vo', playing: { id: 'm7', audio: 1 },
        items: { m7: item({ id: 'm7', ol: 'en', streams: [A(1, 'fre'), A(2, 'eng', 'English Audio Description'), A(3, 'eng', 'English')] }) },
      });
      await settle(p);
      c.check('AD anglaise rangée avant · la piste anglaise normale est prise', JSON.stringify(idx(seen.commands)) === '["3"]', seen.commands);
      await ctx.close();
    }
    // 11. une seule tentative par titre (le membre est revenu en VF : respecté)
    {
      const { p, ctx, seen } = await openPage(browser, {
        mode: 'vo', playing: { id: 'm8', audio: 1 }, items: { m8: item({ id: 'm8', ol: 'en', streams: FILM_EN }) },
      });
      await settle(p);
      await sleep(UA_PAGE === TV ? 10500 : 5500); // deux tours de plus, la session dit toujours VF
      c.check('une seule tentative par titre', seen.commands.length === 1 && seen.items.filter((x) => x === 'm8').length === 1, { cmd: seen.commands.length, items: seen.items });
      await ctx.close();
    }
    // 12. mode « fr » : le filet ne fait rien, pas même lire la session
    {
      const { p, ctx, seen } = await openPage(browser, {
        mode: 'fr', playing: { id: 'm9', audio: 1 }, items: { m9: item({ id: 'm9', ol: 'en', streams: FILM_EN }) },
      });
      await sleep(6000);
      c.check('mode fr · aucune session lue, aucune commande', seen.sessions === 0 && seen.commands.length === 0, seen);
      await ctx.close();
    }
    // 13. mode inconnu sur l'appareil : demandé à /api/me (vo), puis le filet joue
    {
      const { p, ctx, seen } = await openPage(browser, {
        mode: null, me: 'vo', playing: { id: 'm10', audio: 1 }, items: { m10: item({ id: 'm10', ol: 'en', streams: FILM_EN }) },
      });
      await settle(p, UA_PAGE === TV ? 20000 : 12000);
      c.check('mode lu sur /api/me (vo) · bascule faite', JSON.stringify(idx(seen.commands)) === '["2"]', seen.commands);
      await ctx.close();
    }
    // 14. décisions pures (window.__gcVo)
    {
      const { p, ctx } = await openPage(browser, { mode: 'fr', playing: { id: 'x', audio: 1 }, items: {} });
      const r = await p.evaluate(() => {
        const g = window.__gcVo;
        return {
          ol: [g.olOf({ OriginalLanguage: ' JA ' }), g.olOf({ OriginalLanguage: '' }), g.olOf({}), g.olOf(null)],
          fr: [g.startedInFrench([{ Type: 'Audio', Index: 1, Language: 'fra' }], 1), g.startedInFrench([{ Type: 'Audio', Index: 1, Language: 'jpn' }], 1), g.startedInFrench([], 1)],
          pick3: g.pickOriginal([{ Type: 'Audio', Index: 1, Language: 'fre' }, { Type: 'Audio', Index: 2, Language: 'ger' }], 1, 'de'),
        };
      });
      c.check('olOf · minuscules, vide → null', JSON.stringify(r.ol) === '["ja",null,null,null]', r.ol);
      c.check('startedInFrench · fra oui, jpn non, piste absente non', JSON.stringify(r.fr) === '[true,false,false]', r.fr);
      c.check('pickOriginal · « de » trouve une piste « ger »', r.pick3 === 2, r.pick3);
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
