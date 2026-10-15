// tools/bench/scenarios/quota.js — banc du quota de demandes Seerr dans la recherche (2026-10-15).
//
// Le vrai Jellyfin, le vrai Jellyfin Enhanced (JE 12.10) et l'app.js de « Mon compte » (candidat compte.js), avec un
// compte de banc ordinaire. Tout le proxy Seerr de JE (/JellyfinEnhanced/jellyseerr/*, /JellyfinEnhanced/tmdb/*) est
// servi PAR INTERCEPTION DANS CE NAVIGATEUR : aucune requête n'en sort, aucune vraie demande ne part, aucun compte
// Seerr n'est nécessaire. Le scénario fixe le quota (sans limite, puis 10 films et 10 saisons par 7 jours) et la réponse
// de chaque demande (succès 201, ou le 403 générique que JE 12.10 renvoie à la place du « Quota exceeded » de Seerr).
// Écritures bloquées par sécurité (réponse 204, notées) : tout POST/PUT/DELETE vers /JellyfinEnhanced/*, /gc-compte/
// et /gc-chat/ ; /gc-compte/api/route répond « non autorisé » (voie russe hors sujet ici).
//
// Contrôles, tous À L'ÉCRAN (getBoundingClientRect, élément visible dans la fenêtre, texte exact) :
//   (0) compte sans limite (admin) : aucune ligne, aucune pastille, 403 laissé à JE, aucun message après un succès ;
//   (a) la ligne de quota d'app.js en tête de la section Seerr des résultats (juste sous son titre, son TEXTE non
//       recouvert : points pris sur les lignes du texte), apparue au plus tard 75 s après un changement de quota (cache
//       d'app.js de 60 s, pas celui de 30 min de JE), mise à jour après une demande réussie et après chaque refus (avec
//       la date de libération) ;
//   (b) la pastille de quota de JE dans la fenêtre des saisons (places restantes, puis limite atteinte) ;
//   (c) le message d'app.js après une demande réussie : à l'écran, au-dessus de tout, sans capter le toucher
//       (pointer-events: none), et sans recouvrir le message de JE mesuré AU REPOS (fin de sa glissade) avec un titre
//       long (plus de 64 px de haut) ; aussi après un film ;
//   (d) la fenêtre « Limite de demandes atteinte » de JE, en français, après le 403 générique : 11 saisons demandées
//       pour 10 restantes, 1 saison quand il n'en reste plus, 1 film quand il n'en reste plus (fenêtre de film) ;
//   (e) le 6e argument (TVDB saisi à la main) arrive à la fonction d'origine de JE (espion posé AVANT la surcouche
//       d'app.js) et dans le corps de la demande interceptée ;
//   (f) aucune requête vers le proxy Seerr de JE n'est sortie du navigateur (chaque POST de demande vu par le
//       navigateur a reçu la réponse du banc ; aucune réponse de ces chemins ne vient du réseau) ;
//   (g) fenêtre des saisons rouverte PENDANT le message : message caché, « Annuler » et « Demander » atteignables
//       (rien par-dessus leur centre), et un vrai toucher (clic sur bureau et télé) sur « Annuler » ferme la fenêtre ;
//   (h) série entière (requestMedia 'tv', chemin de JE pour une série sans saison numérotée) : 403 laissé tel quel
//       s'il reste des places, réécrit seulement quand les saisons sont épuisées.
// Captures : /out/quota-<appareil>-<étape>.png.
//
// Usage : tools/bench/bench.sh --timeout 600 --candidate DIR tools/bench/scenarios/quota.js desktop phone tv
//         (DIR/compte.js = l'app.js à éprouver ; sans candidat, c'est celui que sert la production)
// Contre-épreuve : compte.js = app.js d'avant le quota (git show <rév>:crates/homelabd/assets/compte/app.js) doit
// ÉCHOUER sur (a), (c), (d), (e), (g) et (h) ; celui d'avant la revue du 15/10 (message à bottom:84px, sans
// pointer-events) sur (c), (g) et (h).
'use strict';
const { env, DEVICES, sleep, launch, login, onScreen, checker, injectCandidates } = require('/bench/bench.js');

const DEV = env.benchDevice.replace(/[^a-z0-9-]/gi, '_');
const TOUCH = ['phone', 'iphone', 'android', 'tablet'].includes(env.device);
const SHOT = (n) => `/out/quota-${DEV}-${n}.png`;
const QUERY = 'zzbancquota';
const TV_ID = 990001;
const MOVIE_ID = 990002;
const TVDB = 424242;   // « saisi à la main » dans la fenêtre (la série factice n'a pas d'identifiant TVDB)
const DAY = 86400e3;

// ---------------------------------------------------------------------------------------------- données factices
const T0 = Date.now();
const iso = (ms) => new Date(ms).toISOString().replace('Z', '0000Z');  // format .NET « o » que renvoie JE
// date de libération des saisons : après-demain 10:00 UTC (12:00 à Paris en heure d'été, 11:00 en hiver)
const RESET_TV = (() => { const d = new Date(T0 + 2 * DAY); d.setUTCHours(10, 0, 0, 0); return d.getTime(); })();
// côté du quota comme le relaie JE (GET /api/v1/user/<id>/quota de Seerr + nextResetAt)
function side(used, resetAt) {
  const s = { days: 7, limit: 10, used, remaining: 10 - used, restricted: used >= 10 };
  if (used > 0 && resetAt) s.nextResetAt = iso(resetAt);
  return s;
}
const MOVIE4 = side(4, T0 + 3 * DAY);
// date de libération des films : après-demain 09:00 UTC
const RESET_MOV = (() => { const d = new Date(T0 + 2 * DAY); d.setUTCHours(9, 0, 0, 0); return d.getTime(); })();
const UNL_SIDE = { days: 7, limit: 0, used: 0, remaining: 0, restricted: false };  // compte sans limite (admin)
const Q = {
  UNL: { movie: UNL_SIDE, tv: UNL_SIDE },
  A: { movie: MOVIE4, tv: side(0) },                    // places restantes
  B: { movie: MOVIE4, tv: side(2, T0 + 7 * DAY) },      // après la demande réussie de 2 saisons
  D: { movie: MOVIE4, tv: side(10, RESET_TV) },         // saisons épuisées
  F: { movie: side(10, RESET_MOV), tv: side(10, RESET_TV) },  // films épuisés aussi
  F2: { movie: side(9, RESET_MOV), tv: side(10, RESET_TV) },  // après la demande d'un film (une place libérée avant)
  H: { movie: MOVIE4, tv: side(2, T0 + 7 * DAY) },      // série entière demandée avec 8 places : 403 d'une autre cause
};
function parisWhen(ms) {
  const d = new Date(ms);
  return d.toLocaleDateString('fr-FR', { day: '2-digit', month: '2-digit', timeZone: 'Europe/Paris' }) + ' à ' +
    d.toLocaleTimeString('fr-FR', { hour: '2-digit', minute: '2-digit', timeZone: 'Europe/Paris' });
}
const W = ' (sur 7 jours glissants)';
const TXT = {
  A: `Il te reste 6 films sur 10 et 10 saisons sur 10 à demander${W}.`,
  B: `Il te reste 6 films sur 10 et 8 saisons sur 10 à demander${W}.`,
  D: `Il te reste 6 films sur 10 et 0 saison sur 10 à demander${W} — nouvelle demande de saison possible le ${parisWhen(RESET_TV)}.`,
  toastB: `Demande envoyée. Il te reste 6 films sur 10 et 8 saisons sur 10 à demander${W}.`,
  over11: `Tu demandes 11 saisons, il t'en reste 10 sur 10${W} : demande moins de saisons, ou attends qu'une place se libère. (Quota exceeded)`,
  full: 'Tu as déjà demandé 10 saisons sur 10 ces 7 derniers jours. (Quota exceeded)',
  F: `Il te reste 0 film sur 10 et 0 saison sur 10 à demander${W} — nouvelle demande de film possible le ${parisWhen(RESET_MOV)} ; ` +
    `nouvelle demande de saison possible le ${parisWhen(RESET_TV)}.`,
  filmFull: 'Tu as déjà demandé 10 films sur 10 ces 7 derniers jours. (Quota exceeded)',
  toastFilm: `Demande envoyée. Il te reste 1 film sur 10 et 0 saison sur 10 à demander${W} — nouvelle demande de saison possible le ${parisWhen(RESET_TV)}.`,
};
// réponse de JE 12.10 (ProxyJellyseerrRequest) à un compte non admin pour TOUT 403 de Seerr, quota compris
const JE_403 = { error: true, code: 'Forbidden', httpStatus: 403, message: 'Seerr declined the request. Ask your administrator to check your account permissions.' };

// titre long exprès : le message de JE (« Demandé N saison(s) de <titre> ») passe sur 3 lignes ou plus (constat de revue)
const TV_ITEM = { id: TV_ID, mediaType: 'tv', name: 'Série du banc au titre volontairement très long pour mesurer les messages (quota)',
  originalName: 'Serie du banc', firstAirDate: '2010-01-08',
  posterPath: null, backdropPath: null, overview: 'Série factice du banc de quota : aucune demande réelle.', voteAverage: 7.1,
  popularity: 1, originalLanguage: 'fr', genreIds: [] };
const MOVIE_ITEM = { id: MOVIE_ID, mediaType: 'movie', title: 'Film du banc (quota)', originalTitle: 'Film du banc', releaseDate: '2020-02-14',
  posterPath: null, backdropPath: null, overview: 'Film factice du banc de quota.', voteAverage: 6.4, popularity: 1, originalLanguage: 'fr', genreIds: [] };
const SEARCH = { page: 1, totalPages: 1, totalResults: 2, results: [TV_ITEM, MOVIE_ITEM] };
// 11 saisons + les épisodes spéciaux (saison 0, jamais cochée par « Sélectionner toutes les saisons »)
const TV_DETAILS = { id: TV_ID, name: TV_ITEM.name, posterPath: null, backdropPath: null, externalIds: {},
  seasons: Array.from({ length: 12 }, (_, n) => ({ id: 880000 + n, seasonNumber: n, name: n ? `Saison ${n}` : 'Épisodes spéciaux',
    episodeCount: n ? 10 : 2, airDate: `${2010 + n}-01-08` })) };
const SONARR = [{ id: 0, name: 'Sonarr (banc)', isDefault: true, is4k: false, activeProfileId: 7, activeDirectory: '/data/TV Shows' }];
const RADARR = [{ id: 0, name: 'Radarr (banc)', isDefault: true, is4k: false, activeProfileId: 7, activeDirectory: '/data/Movies' }];
const PROFILES = [{ id: 7, name: 'FR-friendly H.264' }];

// ---------------------------------------------------------------------------------------------- interception
const M = { quota: Q.UNL, reply: null, afterOk: null, posts: [], quotaGets: 0, other: [], blocked: [] };
function mock(r) {
  if (r.isInterceptResolutionHandled()) return;
  let u;
  try { u = new URL(r.url()); } catch (e) { return; }
  const path = u.pathname, meth = r.method();
  // r.respond est appelé SANS attente : la requête est marquée traitée avant le gestionnaire de bench.js
  const json = (status, body) => { r.respond({ status, contentType: 'application/json; charset=utf-8', body: JSON.stringify(body) }).catch(() => {}); };
  const none = () => { r.respond({ status: 204, body: '' }).catch(() => {}); };
  const sm = /\/JellyfinEnhanced\/jellyseerr(\/.*)$/.exec(path);
  if (sm) {
    const sp = sm[1];
    if (meth === 'POST' && sp === '/request') {
      let body = null;
      try { body = JSON.parse(r.postData() || 'null'); } catch (e) { body = '(illisible)'; }
      const reply = M.reply || 'forbidden';  // demande imprévue : refusée
      M.reply = null;
      M.posts.push({ t: Date.now(), body, reply });
      if (reply === 'ok') {
        if (M.afterOk) { M.quota = M.afterOk; M.afterOk = null; }
        return json(201, { id: 990000 + M.posts.length, status: 1, media: { id: 1, tmdbId: body && body.mediaId, status: 2 } });
      }
      return json(403, JE_403);
    }
    if (meth !== 'GET') { M.blocked.push(`${meth} jellyseerr${sp}`); return json(404, { error: true, code: 'bench' }); }
    if (sp === '/user-status') return json(200, { active: true, userFound: true, jellyseerrUserId: '990', reason: 'linked' });
    if (sp === '/quota') { M.quotaGets += 1; return json(200, M.quota); }
    if (sp === '/search') {
      const q = u.searchParams.get('query') || '';
      return json(200, q === QUERY ? SEARCH : { page: 1, totalPages: 1, totalResults: 0, results: [] });
    }
    let x;
    if ((x = /^\/movie\/(\d+)$/.exec(sp))) return json(200, { id: +x[1], title: MOVIE_ITEM.title, collection: null });
    if (sp === '/radarr') return json(200, RADARR);
    if (/^\/radarr\/\d+$/.test(sp)) return json(200, { profiles: PROFILES, rootFolders: [{ id: 1, path: '/data/Movies' }] });
    if (sp === '/sonarr') return json(200, SONARR);
    if (/^\/sonarr\/lookup\/\d+$/.test(sp)) return json(200, []);
    if (/^\/sonarr\/\d+$/.test(sp)) return json(200, { profiles: PROFILES, rootFolders: [{ id: 1, path: '/data/TV Shows' }] });
    if (sp === '/overrideRule') return json(200, []);
    if (sp === '/settings/partial-requests') return json(200, { partialRequestsEnabled: true, enableSpecialEpisodes: true });
    if (/^\/tv\/\d+$/.test(sp)) return json(200, TV_DETAILS);
    if (/^\/tv\/\d+\/season\/\d+$/.test(sp)) return json(200, { episodes: [{ airDate: '2010-01-08' }] });
    if (M.other.length < 40) M.other.push(sp.slice(0, 80));
    return json(404, { error: true, code: 'bench', message: 'non simulé par le banc' });
  }
  if (/\/JellyfinEnhanced\/tmdb\//.test(path)) return json(200, /\/genres\//.test(path) ? [] : {});
  const write = meth !== 'GET' && meth !== 'HEAD';
  if (write && /\/JellyfinEnhanced\//.test(path)) { M.blocked.push(`${meth} ${path.replace(/^.*\/JellyfinEnhanced/, 'JE')}`.slice(0, 80)); return none(); }
  if (meth === 'GET' && /\/gc-compte\/api\/route$/.test(path)) return json(200, { allowed: false, items: [] });
  if (write && /\/gc-(compte|chat)\//.test(path)) { M.blocked.push(`${meth} ${path}`.slice(0, 80)); return none(); }
}

// (e) espion sur les fonctions d'ORIGINE de JE, posé dès qu'elles existent, donc avant la surcouche d'app.js
// (wrapRequests, toutes les 2 s) ; `beforeGc` dit si c'était bien le cas
function spyInit() {
  window.__bancSpy = { calls: [], beforeGc: null };
  const iv = setInterval(() => {
    const api = window.JellyfinEnhanced && window.JellyfinEnhanced.jellyseerrAPI;
    if (!api || typeof api.requestMedia !== 'function' || typeof api.requestTvSeasons !== 'function') return;
    clearInterval(iv);
    window.__bancSpy.beforeGc = !api.__gcRu;
    ['requestMedia', 'requestTvSeasons'].forEach((fn) => {
      const orig = api[fn];
      api[fn] = function () {
        const a = arguments;
        window.__bancSpy.calls.push({ fn, n: a.length, tvdb: a.length > 5 ? a[5] : '(absent)', seasons: fn === 'requestTvSeasons' ? a[1] : null });
        return orig.apply(this, a);
      };
    });
  }, 20);
}

// page réglée comme newPage() de bench.js, mais avec NOTRE gestionnaire posé AVANT celui des candidats
async function makePage(browser) {
  const p = await browser.newPage();
  const d = DEVICES[env.device] || DEVICES.desktop;
  if (d.ua) await p.setUserAgent(d.ua);
  await p.setViewport(d.viewport);
  await p.emulateTimezone('Europe/Paris');
  if (env.layout) await p.evaluateOnNewDocument((l) => { try { localStorage.setItem('layout', l); } catch (e) { /* refusé */ } }, env.layout);
  await p.evaluateOnNewDocument(spyInit);
  p.gcErrors = [];
  p.gcDialogs = [];
  p.on('pageerror', (e) => p.gcErrors.push(String(e.message).slice(0, 200)));
  p.on('dialog', (dl) => { p.gcDialogs.push(dl.type() + ': ' + dl.message().slice(0, 80)); dl.dismiss().catch(() => {}); });
  await p.setBypassServiceWorker(true);
  await p.setRequestInterception(true);
  p.on('request', mock);
  p.gcCandidates = await injectCandidates(p);
  // sans candidat (ou --no-inject), personne d'autre ne laisse passer les requêtes non simulées
  if (!p.gcCandidates) p.on('request', (r) => { if (!r.isInterceptResolutionHandled()) r.continue().catch(() => {}); });
  return p;
}

// (f) ce que voit le navigateur (session CDP distincte) : une réponse venue du réseau porte l'adresse du serveur
async function watchNetwork(p) {
  const net = { posts: 0, leaks: [], remote: 0, seerr: 0 };
  const cdp = p.createCDPSession ? await p.createCDPSession() : await p.target().createCDPSession();
  await cdp.send('Network.enable');
  const SEERR = /\/JellyfinEnhanced\/(jellyseerr|tmdb)\//;
  cdp.on('Network.requestWillBeSent', (e) => {
    if (e.request.method === 'POST' && /\/JellyfinEnhanced\/jellyseerr\/request(\?|$)/.test(e.request.url)) net.posts += 1;
  });
  cdp.on('Network.responseReceived', (e) => {
    const fromNet = !!e.response.remoteIPAddress;
    if (SEERR.test(e.response.url)) { net.seerr += 1; if (fromNet) net.leaks.push(new URL(e.response.url).pathname.slice(0, 80)); } else if (fromNet) net.remote += 1;
  });
  return net;
}

// ---------------------------------------------------------------------------------------------- mesures à l'écran
// premier élément visible du sélecteur ramené dans la fenêtre, puis mesuré par onScreen()
async function seen(p, sel, ms = 20000) {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    const there = await p.evaluate((s) => {
      const el = [...document.querySelectorAll(s)].find((e) => { const r = e.getBoundingClientRect(); return r.width > 0 && r.height > 0; });
      if (!el) return false;
      const r = el.getBoundingClientRect();
      if (r.top < 0 || r.bottom > innerHeight) el.scrollIntoView({ block: 'center' });
      return true;
    }, sel).catch(() => false);
    if (there && await onScreen(p, sel)) return true;
    await sleep(400);
  }
  return false;
}
const box = (r) => [r.left, r.top, r.width, r.height].map(Math.round);
// ligne de quota de la section Seerr AFFICHÉE : texte, place (juste sous le titre, avant les cartes), dans la fenêtre
async function lineState(p, scroll = true) {
  return p.evaluate((sc) => {
    const sec = [...document.querySelectorAll('.jellyseerr-section')].find((s) => s.offsetParent !== null);
    if (!sec) return { section: false };
    const line = sec.querySelector('.gc-quota');
    if (!line) return { section: true, line: false };
    if (sc) { const r0 = line.getBoundingClientRect(); if (r0.top < 0 || r0.bottom > innerHeight) line.scrollIntoView({ block: 'center' }); }
    const r = line.getBoundingClientRect(), s = sec.getBoundingClientRect(), cs = getComputedStyle(line);
    const title = sec.querySelector('.sectionTitle'), card = sec.querySelector('.jellyseerr-card');
    const t = title && title.getBoundingClientRect(), k = card && card.getBoundingClientRect();
    // non recouvert = le TEXTE : 3 points sur chaque ligne du texte (pas la marge de la ligne, que la rangée
    // transparente des cartes de la télé chevauche à gauche : faux échec du 15/10)
    const rg = document.createRange();
    rg.selectNodeContents(line);
    const pts = [];
    [...rg.getClientRects()].filter((q) => q.width > 4 && q.height > 4)
      .forEach((q) => [0.25, 0.5, 0.75].forEach((f) => pts.push([q.left + q.width * f, q.top + q.height / 2])));
    const miss = pts.filter(([x, y]) => { const e = document.elementFromPoint(x, y); return !(e && line.contains(e)); });
    return {
      section: true, line: true, text: line.textContent, box: [r.left, r.top, r.width, r.height].map(Math.round),
      shown: r.width > 0 && r.height >= 10 && cs.display !== 'none' && cs.visibility !== 'hidden' && +cs.opacity > 0,
      inViewport: r.top >= 0 && r.bottom <= innerHeight && r.left >= 0 && r.right <= innerWidth,
      // juste sous le titre : élément suivant, sans chevauchement visible (marge haute de -.2em) ni trou
      underTitle: !!t && line.previousElementSibling === title && r.top >= t.bottom - 6 && r.top - t.bottom <= 24,
      gap: t ? Math.round(r.top - t.bottom) : null,
      beforeCards: !k || r.bottom <= k.top + 2,
      inSection: r.left >= s.left - 1 && r.right <= s.right + 1 && r.top >= s.top - 1 && r.bottom <= s.bottom + 1,
      free: pts.length > 0 && miss.length === 0, points: pts.length, recouverts: miss.slice(0, 6).map((q) => q.map(Math.round)),
    };
  }, scroll).catch((e) => ({ error: String(e.message).slice(0, 120) }));
}
async function waitLine(p, text, ms) {
  const end = Date.now() + ms;
  let st;
  do {
    st = await lineState(p);
    if (st.line && st.text === text && st.shown && st.inViewport) { await sleep(300); return await lineState(p); }
    await sleep(500);
  } while (Date.now() < end);
  return st;
}
const lineOk = (st, text) => !!(st && st.line && st.text === text && st.shown && st.inViewport && st.underTitle && st.beforeCards && st.inSection && st.free);

// pastille de JE dans la fenêtre des saisons ouverte
async function chipState(p, ms = 15000) {
  const sel = '.jellyseerr-season-modal.show .jellyseerr-quota-chip';
  const vis = await seen(p, sel, ms);
  const st = await p.evaluate((s) => {
    const el = document.querySelector(s);
    if (!el) return { chip: false };
    const r = el.getBoundingClientRect();
    return { chip: true, text: el.innerText.replace(/\s+/g, ' ').trim(), cls: el.className, box: [r.left, r.top, r.width, r.height].map(Math.round) };
  }, sel).catch(() => ({ chip: false }));
  return { ...st, vis };
}
// dialogue Jellyfin (Dashboard.alert) ouvert par JE
async function dialogState(p) {
  return p.evaluate(() => {
    const d = [...document.querySelectorAll('.dialogContainer .formDialog, .dialogContainer .dialog')]
      .find((e) => { const r = e.getBoundingClientRect(); return r.width > 0 && r.height > 0; });
    if (!d) return { open: false };
    const t = d.querySelector('.formDialogHeaderTitle'), x = d.querySelector('.text');
    const r = d.getBoundingClientRect(), xr = x && x.getBoundingClientRect();
    return {
      open: true, title: t ? t.textContent.trim() : null, text: x ? x.innerText : d.innerText.slice(0, 600),
      box: [r.left, r.top, r.width, r.height].map(Math.round),
      textShown: !!xr && xr.width > 0 && xr.height > 0 && xr.top >= 0 && xr.bottom <= innerHeight && xr.left >= 0 && xr.right <= innerWidth,
    };
  }).catch(() => ({ open: false }));
}
async function waitDialog(p, ms = 20000) {
  const end = Date.now() + ms;
  while (Date.now() < end) { const d = await dialogState(p); if (d.open && d.text) { await sleep(500); return dialogState(p); } await sleep(400); }
  return dialogState(p);
}
async function closeDialog(p) {
  await p.evaluate(() => {
    const d = [...document.querySelectorAll('.dialogContainer .formDialog, .dialogContainer .dialog')]
      .find((e) => { const r = e.getBoundingClientRect(); return r.width > 0 && r.height > 0; });
    const b = d && (d.querySelector('.formDialogFooter .btnOption') || d.querySelector('button'));
    if (b) b.click();
  }).catch(() => {});
  const end = Date.now() + 8000;
  while (Date.now() < end) { if (!(await dialogState(p)).open) return true; await sleep(300); }
  return false;
}
async function toastState(p) {
  return p.evaluate(() => {
    const t = document.getElementById('gc-quota-toast');
    if (!t) return { toast: false };
    const r = t.getBoundingClientRect(), cs = getComputedStyle(t);
    // au-dessus de tout : le message ne capte aucun toucher, il est rendu « touchable » le temps de la mesure (synchrone)
    const pe = t.style.pointerEvents;
    t.style.pointerEvents = 'auto';
    const free = [[0.5, 0.5], [0.1, 0.25], [0.9, 0.25], [0.1, 0.75], [0.9, 0.75]]
      .every(([fx, fy]) => { const e = document.elementFromPoint(r.left + r.width * fx, r.top + r.height * fy); return !!e && t.contains(e); });
    t.style.pointerEvents = pe;
    // messages de JE (« Demandé N saison(s)… », en bas à droite) AU REPOS seulement : fin de la glissade, entiers à l'écran
    const je = [...document.querySelectorAll('.jellyfin-enhanced-toast')].map((e) => e.getBoundingClientRect())
      .filter((b) => b.width > 0 && b.height > 0 && b.left >= 0 && b.right <= innerWidth + 1 && b.top >= 0 && b.bottom <= innerHeight + 1);
    const cross = je.some((b) => b.left < r.right && b.right > r.left && b.top < r.bottom - 1 && b.bottom > r.top + 1);
    return {
      toast: true, text: t.textContent, box: [r.left, r.top, r.width, r.height].map(Math.round),
      jeToasts: je.map((b) => [b.left, b.top, b.width, b.height].map(Math.round)), overJe: cross,
      jeMaxH: Math.round(Math.max(0, ...je.map((b) => b.height))),
      pointerEvents: cs.pointerEvents,
      shown: r.width > 0 && r.height >= 10 && cs.display !== 'none' && cs.visibility !== 'hidden' && +cs.opacity > 0,
      inViewport: r.top >= 0 && r.bottom <= innerHeight && r.left >= 0 && r.right <= innerWidth,
      free,
    };
  }).catch(() => ({ toast: false }));
}
// `withJe` : attendre aussi un message de JE au repos (demande de saisons : JE affiche le sien)
async function waitToast(p, text, withJe, ms = 12000) {
  const end = Date.now() + ms;
  let st;
  do {
    st = await toastState(p);
    if (st.toast && st.shown && st.text === text && (!withJe || st.jeToasts.length)) { await sleep(400); return toastState(p); }
    await sleep(250);
  } while (Date.now() < end);
  return st;
}
const toastOk = (ts, text) => !!(ts && ts.toast && ts.shown && ts.inViewport && ts.free && ts.text === text);
// fenêtre de demande ouverte : le message est-il caché, et le centre de ses boutons atteignable (styles réels) ?
async function windowState(p) {
  return p.evaluate(() => {
    const t = document.getElementById('gc-quota-toast');
    const out = { toastInDom: !!t, toastShown: false, toastBox: null, buttons: [] };
    if (t) {
      const r = t.getBoundingClientRect(), cs = getComputedStyle(t);
      out.toastBox = [r.left, r.top, r.width, r.height].map(Math.round);
      out.toastShown = r.width > 0 && r.height > 0 && cs.display !== 'none' && cs.visibility !== 'hidden' && +cs.opacity > 0;
    }
    document.querySelectorAll('.jellyseerr-season-modal.show .jellyseerr-modal-footer button').forEach((b) => {
      const r = b.getBoundingClientRect(), hit = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
      out.buttons.push({
        text: b.textContent.trim().slice(0, 40), box: [r.left, r.top, r.width, r.height].map(Math.round),
        reach: !!hit && b.contains(hit), hit: hit ? String(hit.id || hit.className || hit.tagName).slice(0, 50) : null,
        inViewport: r.width > 0 && r.top >= 0 && r.bottom <= innerHeight && r.left >= 0 && r.right <= innerWidth,
      });
    });
    return out;
  }).catch((e) => ({ error: String(e.message).slice(0, 120) }));
}
const jeToastTexts = (p) => p.evaluate(() => [...document.querySelectorAll('.jellyfin-enhanced-toast')].map((e) => e.innerText.trim())).catch(() => []);
async function gone(p, sel, ms = 5000) {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (!(await p.$(sel))) return true; await sleep(250); }
  return false;
}

// ---------------------------------------------------------------------------------------------- gestes
async function closeSeasonModal(p) {
  if (!(await p.$('.jellyseerr-season-modal'))) return true;
  await p.keyboard.press('Escape');
  return gone(p, '.jellyseerr-season-modal');
}
async function closeMoreInfo(p) {
  if (!(await p.$('.je-more-info-modal'))) return true;
  await p.keyboard.press('Escape');
  return gone(p, '.je-more-info-modal');
}
// survol (bureau, télé) ou toucher (téléphone) de la carte, puis « Demander » : fenêtre des saisons ou de film. Deux
// essais : une carte remplacée par un nouveau rendu de JE, ou un toucher qui ouvre « Plus d'infos » (fermé, puis
// nouvel essai).
// `quick` : mesure pendant un message de 7 s (attente à 100 ms près, sans attendre les réglages avancés)
async function waitOn(p, sel, ms, step) {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (await onScreen(p, sel)) return true; await sleep(step); }
  return false;
}
async function openRequestModal(p, type, id, quick = false) {
  const step = quick ? 100 : 500;
  await closeSeasonModal(p);
  await closeMoreInfo(p);
  const card = `.jellyseerr-card[data-banc="${type}"]`;
  const tries = [];
  for (let attempt = 1; attempt <= 2; attempt++) {
    const found = await p.evaluate((ty, tid) => {
      document.querySelectorAll('[data-banc]').forEach((e) => e.removeAttribute('data-banc'));
      const c = [...document.querySelectorAll('.jellyseerr-section .jellyseerr-card')]
        .find((k) => k.offsetParent !== null && k.querySelector(`.jellyseerr-more-info-link[data-media-type="${ty}"][data-tmdb-id="${tid}"]`));
      if (!c) return false;
      c.setAttribute('data-banc', ty);
      c.scrollIntoView({ block: 'center', inline: 'center' });
      return true;
    }, type, id).catch(() => false);
    if (!found) { tries.push('carte introuvable'); await sleep(1500); continue; }
    await sleep(700);  // geste après la fin du défilement (JE prend un toucher < 100 ms après un défilement pour un arrêt de lancer)
    if (TOUCH) await p.tap(`${card} .cardImageContainer`).catch(() => {}); else await p.hover(`${card} .cardScalable`).catch(() => {});
    const btn = `${card} .jellyseerr-overview .jellyseerr-request-button`;
    if (await waitOn(p, btn, 6000, step)) {
      const label = await p.$eval(btn, (b) => b.textContent.trim()).catch(() => '');
      if (TOUCH) await p.tap(btn).catch(() => {}); else await p.click(btn).catch(() => {});
      const ok = await waitOn(p, '.jellyseerr-season-modal.show .jellyseerr-season-content', 20000, step);
      if (ok && quick) return { ok, label, attempt };
      if (ok) {
        // réglages avancés (bloc caché par app.js, mais JE exige qu'ils soient remplis)
        const f = type === 'tv' ? '#tv-folder' : '#movie-folder';
        const filled = await p.waitForFunction((s) => { const x = document.querySelector(`.jellyseerr-season-modal.show ${s}`); return !!(x && x.value); },
          { timeout: 15000 }, f).then(() => true, () => false);
        await sleep(400);
        return { ok: filled, label, attempt, why: filled ? null : 'dossier des réglages avancés jamais rempli' };
      }
      tries.push('fenêtre absente de l\'écran');
    } else tries.push(await p.$('.je-more-info-modal') ? '« Plus d\'infos » ouvert au lieu du bouton' : 'bouton « Demander » de la carte absent de l\'écran');
    await closeMoreInfo(p);
    await closeSeasonModal(p);
  }
  return { ok: false, why: tries.join(' ; ') };
}
const openSeriesModal = (p, quick) => openRequestModal(p, 'tv', TV_ID, quick);
// saisons cochées dans la fenêtre : 'all' (« Sélectionner toutes les saisons ») ou liste de numéros ; TVDB saisi.
// JE redessine la liste des saisons toutes les 10 s (refreshModalInterval) : un clic sur une case remplacée échoue
// (« Node is detached », vu le 15/10 sur la contre-épreuve) ; jusqu'à 3 essais, saisons relues à la fin.
async function prepareOnce(p, seasons) {
  const M_ = '.jellyseerr-season-modal.show';
  if (!(await seen(p, `${M_} #jellyseerr-tvdb-id`, 10000))) return { ok: false, why: 'champ TVDB absent' };
  await p.$eval(`${M_} #jellyseerr-tvdb-id`, (i) => { i.value = ''; });
  await p.type(`${M_} #jellyseerr-tvdb-id`, String(TVDB));
  if (!(await seen(p, `${M_} .jellyseerr-season-item[data-season-number="1"]`, 10000))) return { ok: false, why: 'liste des saisons absente' };
  const all = `${M_} #jellyseerr-select-all-seasons`;
  const isAll = async () => p.$eval(all, (x) => x.checked).catch(() => null);
  const checked = () => p.$$eval(`${M_} .jellyseerr-season-item .jellyseerr-season-checkbox:checked`, (xs) => xs.map((x) => +x.dataset.seasonNumber));
  if (seasons === 'all') {
    if (!(await isAll())) await p.click(all);
  } else {
    if ((await checked()).length) { if (!(await isAll())) await p.click(all); await p.click(all); }  // tout décocher
    for (const n of seasons) await p.click(`${M_} .jellyseerr-season-item[data-season-number="${n}"] .jellyseerr-season-checkbox`);
  }
  // réglages avancés (bloc caché par app.js, mais JE exige qu'ils soient remplis)
  const filled = await p.waitForFunction((m) => { const f = document.querySelector(`${m} #tv-folder`); return !!(f && f.value); }, { timeout: 15000 }, M_).then(() => true, () => false);
  const got = await checked();
  const want = seasons === 'all' ? Array.from({ length: 11 }, (_, k) => k + 1) : seasons;
  const tvdb = await p.$eval(`${M_} #jellyseerr-tvdb-id`, (i) => i.value).catch(() => '');
  const ok = filled && JSON.stringify(got) === JSON.stringify(want) && tvdb === String(TVDB);
  return { ok, got, why: ok ? null : (filled ? 'saisons ou TVDB inattendus' : 'dossier des réglages avancés jamais rempli') };
}
async function prepareRequest(p, seasons) {
  let r = null;
  for (let attempt = 1; attempt <= 3; attempt++) {
    try { r = await prepareOnce(p, seasons); } catch (e) { r = { ok: false, why: String(e.message).slice(0, 120) }; }
    r.attempt = attempt;
    if (r.ok) return r;
    await sleep(500);
  }
  return r;
}
async function submit(p, reply, afterOk) {
  const n0 = M.posts.length;
  M.reply = reply; M.afterOk = afterOk || null;
  await p.click('.jellyseerr-season-modal.show .jellyseerr-modal-button-primary');
  const end = Date.now() + 15000;
  while (Date.now() < end && M.posts.length === n0) await sleep(200);
  if (M.posts.length > n0) return M.posts[M.posts.length - 1];
  M.reply = null; M.afterOk = null;  // aucune demande partie : rien ne doit s'appliquer à la suivante
  return null;
}
const spy = (p) => p.evaluate(() => window.__bancSpy || null).catch(() => null);

// ---------------------------------------------------------------------------------------------- déroulé
(async () => {
  const c = checker('quota');
  const b = await launch();
  let code = 1;
  let p = null, net = null;
  const extra = {};
  try {
    p = await makePage(b);
    net = await watchNetwork(p);
    console.log(`info   app.js éprouvé : ${p.gcCandidates ? 'candidat compte.js (--candidate)' : 'celui que sert la production'}`);
    await login(p);
    c.check('connexion du compte de banc', true);
    const ready = await p.waitForFunction(() => window.__gcAccount === true && !!(window.JellyfinEnhanced && window.JellyfinEnhanced.jellyseerrAPI),
      { timeout: 60000 }).then(() => true, () => false);
    c.check('app.js de Mon compte et Jellyfin Enhanced chargés', ready);

    // recherche : la section Seerr (résultats simulés) à l'écran
    await p.evaluate((q) => { location.hash = '#/search?query=' + encodeURIComponent(q); }, QUERY);
    await p.waitForSelector('#searchPage #searchTextInput', { timeout: 30000 });
    await sleep(1500);
    if ((await p.$eval('#searchPage #searchTextInput', (i) => i.value).catch(() => '')) !== QUERY) {
      await p.click('#searchPage #searchTextInput', { clickCount: 3 });
      await p.type('#searchPage #searchTextInput', QUERY, { delay: 40 });
    }
    c.check('section Seerr de la recherche à l\'écran (2 cartes simulées)', await seen(p, '.jellyseerr-section .jellyseerr-card', 40000),
      await p.$$eval('.jellyseerr-section .jellyseerr-card', (x) => x.length).catch(() => 0));
    const wrapped = await p.waitForFunction(() => window.JellyfinEnhanced.jellyseerrAPI.__gcRu === true, { timeout: 10000 }).then(() => true, () => false);
    c.check('surcouche d\'app.js posée sur l\'API de JE (__gcRu)', wrapped);
    const s0 = await spy(p);
    c.check('espion posé sur les fonctions d\'origine de JE AVANT la surcouche d\'app.js', !!(s0 && s0.beforeGc === true), s0);

    // (0) compte sans limite (admin) : app.js n'ajoute rien, JE garde la main
    await sleep(8000);
    let ln = await lineState(p, false);
    c.check('(0) sans limite : aucune ligne de quota (8 s)', ln.section === true && ln.line === false, ln);
    await p.screenshot({ path: SHOT('0-sans-limite') });
    let o = await openSeriesModal(p);
    c.check('(0) fenêtre des saisons ouverte depuis la carte de la série', o.ok, o);
    await sleep(2000);
    c.check('(0) aucune pastille de quota de JE', !(await onScreen(p, '.jellyseerr-season-modal.show .jellyseerr-quota-chip')));
    let pr = await prepareRequest(p, [1]);
    c.check('(0) saison 1 cochée et TVDB saisi', pr.ok && JSON.stringify(pr.got) === '[1]', pr);
    let post = await submit(p, 'forbidden');
    c.check('(0) demande d\'une saison interceptée (403 générique)', !!post && post.reply === 'forbidden', post && post.body);
    await sleep(2500);
    let dg = await dialogState(p);
    const jt = await jeToastTexts(p);
    c.check('(0) 403 laissé à JE : pas de fenêtre de quota, message générique de JE', !dg.open && jt.some((x) => /Seerr declined the request/.test(x)), { dg, jt });
    post = await submit(p, 'ok');
    c.check('(0) demande réussie interceptée (201 simulé)', !!post && post.reply === 'ok', post && post.body);
    await sleep(3000);
    c.check('(0) aucun message « Demande envoyée » pour un compte sans limite', !(await p.$('#gc-quota-toast')));
    await closeSeasonModal(p);

    // (a) quota posé (10 films et 10 saisons par 7 jours) : la ligne apparaît au plus tard 75 s après (cache d'app.js de
    // 60 s ; le cache de JE, 30 min, ne doit pas la figer)
    M.quota = Q.A;
    ln = await waitLine(p, TXT.A, 80000);
    c.check('(a) ligne de quota sous le titre de la section Seerr, à l\'écran, places restantes (≤ 75 s après le changement)', lineOk(ln, TXT.A), ln);
    await p.screenshot({ path: SHOT('1-ligne') });

    // (b) pastille de quota dans la fenêtre des saisons
    o = await openSeriesModal(p);
    c.check('fenêtre des saisons ouverte depuis la carte de la série', o.ok, o);
    let chip = await chipState(p);
    c.check('(b) pastille de quota de la fenêtre des saisons à l\'écran (0 sur 10)', chip.vis && /^Séries\s*:\s*0 sur 10 utilisés au cours des 7 derniers jours/.test(chip.text || ''), chip);
    await p.screenshot({ path: SHOT('2-pastille') });

    // (d) 11 saisons pour 10 restantes → 403 générique de JE → fenêtre « Limite de demandes atteinte » en français
    pr = await prepareRequest(p, 'all');
    c.check('11 saisons cochées (toutes sauf les spéciaux) et TVDB saisi', pr.ok && pr.got.length === 11 && !pr.got.includes(0), pr);
    post = await submit(p, 'forbidden');
    let pb = (post && post.body) || {};
    c.check('demande de 11 saisons interceptée (403 générique de JE 12.10)', !!post && post.reply === 'forbidden' &&
      Array.isArray(pb.seasons) && pb.seasons.length === 11, pb);
    dg = await waitDialog(p);
    await p.screenshot({ path: SHOT('3-refus-11-saisons') });
    c.check('(d) fenêtre « Limite de demandes atteinte » à l\'écran, message français (11 demandées, 10 restantes)',
      dg.open && dg.textShown && dg.title === 'Limite de demandes atteinte' && (dg.text || '').split(/\n+/)[0].trim() === TXT.over11, dg);
    c.check('(e) 6e argument (TVDB saisi) dans le corps de la demande refusée', pb.tvdbId === TVDB, pb);
    await closeDialog(p);

    // (c) demande réussie de 2 saisons : message d'app.js ; (e) TVDB transmis à la fonction d'origine
    o = (await p.$('.jellyseerr-season-modal.show')) ? { ok: true } : await openSeriesModal(p);
    c.check('fenêtre des saisons disponible pour la 2e demande', o.ok, o);
    pr = await prepareRequest(p, [1, 2]);
    c.check('saisons 1 et 2 cochées et TVDB saisi', pr.ok && JSON.stringify(pr.got) === '[1,2]', pr);
    post = await submit(p, 'ok', Q.B);
    pb = (post && post.body) || {};
    c.check('demande de 2 saisons interceptée (201 simulé)', !!post && post.reply === 'ok' && JSON.stringify(pb.seasons) === '[1,2]', pb);
    const ts = await waitToast(p, TXT.toastB, true);
    const tToast = Date.now(), tm = {};
    if (ts.toast) await p.screenshot({ path: SHOT('4-message') });
    tm.capture = Date.now() - tToast;
    c.check('(c) message après la demande réussie à l\'écran, au-dessus de tout, avec le quota restant', toastOk(ts, TXT.toastB), ts);
    c.check('(c) le message ne capte aucun toucher (pointer-events: none)', ts.pointerEvents === 'none', ts);
    c.check('(c) message de JE au repos mesuré, titre long (plus de 64 px de haut)', !!(ts.jeToasts && ts.jeToasts.length && ts.jeMaxH > 64), ts);
    c.check('(c) le message ne recouvre pas celui de JE (« Demandé 2 saison(s)… »)', !!(ts.toast && ts.jeToasts && ts.jeToasts.length && !ts.overJe), ts);
    c.check('(e) 6e argument (TVDB saisi) dans le corps de la demande réussie', pb.tvdbId === TVDB, pb);
    const s1 = await spy(p);
    const okCalls = (s1 && s1.calls || []).filter((k) => k.fn === 'requestTvSeasons');
    c.check('(e) la fonction d\'origine requestTvSeasons de JE reçoit les 6 arguments, TVDB en 6e (chaque demande)',
      okCalls.length >= 3 && okCalls.every((k) => k.n >= 6 && k.tvdb === TVDB), s1 && s1.calls);

    // (g) la fenêtre des saisons rouverte PENDANT le message (7 s) : rien ne gêne ses boutons. Quota des saisons épuisé
    // dès maintenant : la pastille de cette fenêtre le lit, et JE le garde en cache pour la suivante (30 min)
    M.quota = Q.D;
    o = await openSeriesModal(p, true);
    tm.fenetre = Date.now() - tToast;
    const ws = await windowState(p);
    ws.secondesApresMessage = Math.round((Date.now() - tToast) / 100) / 10;
    ws.ms = tm;
    c.check('(g) fenêtre des saisons rouverte pendant le message', o.ok, o);
    c.check('(g) message encore là (moins de 7 s) quand la fenêtre se rouvre : contrôle significatif', ws.toastInDom === true, ws);
    c.check('(g) message caché tant que la fenêtre est ouverte', ws.toastInDom === true && ws.toastShown === false, ws);
    c.check('(g) « Annuler » et « Demander » à l\'écran et atteignables (rien par-dessus leur centre)',
      ws.buttons && ws.buttons.length === 2 && ws.buttons.every((k) => k.reach && k.inViewport), ws);
    const cancel = '.jellyseerr-season-modal.show .jellyseerr-modal-button-secondary';
    const alive = !!(await p.$('#gc-quota-toast'));
    if (TOUCH) await p.tap(cancel).catch(() => {}); else await p.click(cancel).catch(() => {});
    const closed = await gone(p, '.jellyseerr-season-modal', 5000);
    await p.screenshot({ path: SHOT('5-apres-annuler') });
    c.check(`(g) un vrai ${TOUCH ? 'toucher' : 'clic'} sur « Annuler » ferme la fenêtre (message encore là au geste : ${alive ? 'oui' : 'non'})`, closed && alive, { closed, alive });
    await closeSeasonModal(p);
    ln = await waitLine(p, TXT.B, 12000);
    c.check('(a) ligne à jour après la demande réussie (8 saisons restantes)', lineOk(ln, TXT.B), ln);

    // saisons épuisées (posé avant (g)) : pastille « limite atteinte », refus d'une saison, ligne avec la date
    o = await openSeriesModal(p);
    c.check('fenêtre des saisons rouverte (saisons épuisées)', o.ok, o);
    chip = await chipState(p);
    c.check('(b) pastille « limite atteinte » à l\'écran (10 sur 10)', chip.vis && /jellyseerr-quota-chip-restricted/.test(chip.cls || '') &&
      /^Séries\s*:\s*10 sur 10 utilisés au cours des 7 derniers jours/.test(chip.text || '') && /Limite de demandes atteinte/.test(chip.text || ''), chip);
    await p.screenshot({ path: SHOT('6-pastille-epuise') });
    pr = await prepareRequest(p, [3]);
    c.check('saison 3 cochée et TVDB saisi', pr.ok && JSON.stringify(pr.got) === '[3]', pr);
    post = await submit(p, 'forbidden');
    c.check('demande d\'une saison interceptée (403 générique)', !!post && post.reply === 'forbidden', post && post.body);
    dg = await waitDialog(p);
    await p.screenshot({ path: SHOT('7-refus-epuise') });
    c.check('(d) fenêtre « Limite de demandes atteinte » à l\'écran, message français (saisons épuisées)',
      dg.open && dg.textShown && dg.title === 'Limite de demandes atteinte' && (dg.text || '').split(/\n+/)[0].trim() === TXT.full, dg);
    await closeDialog(p);
    await closeSeasonModal(p);
    ln = await waitLine(p, TXT.D, 12000);
    await p.screenshot({ path: SHOT('8-ligne-epuise') });
    c.check('(a) ligne « 0 saison » avec la date de libération (heure de Paris)', lineOk(ln, TXT.D), ln);

    // film par la fenêtre de film (le chemin de production, JellyseerrShowAdvanced) : refus, puis succès
    M.quota = Q.F;
    o = await openRequestModal(p, 'movie', MOVIE_ID);
    c.check('fenêtre de film ouverte depuis la carte du film', o.ok, o);
    post = await submit(p, 'forbidden');
    c.check('demande de film interceptée (403 générique)', !!post && post.reply === 'forbidden' && post.body && post.body.mediaType === 'movie', post && post.body);
    dg = await waitDialog(p);
    await p.screenshot({ path: SHOT('9-refus-film') });
    c.check('(d) fenêtre « Limite de demandes atteinte » à l\'écran, message français (films épuisés)',
      dg.open && dg.textShown && dg.title === 'Limite de demandes atteinte' && (dg.text || '').split(/\n+/)[0].trim() === TXT.filmFull, dg);
    await closeDialog(p);
    await closeSeasonModal(p);
    ln = await waitLine(p, TXT.F, 12000);
    c.check('(a) ligne « 0 film » et « 0 saison » avec les deux dates, juste après le refus (relue sans attendre)', lineOk(ln, TXT.F), ln);
    o = await openRequestModal(p, 'movie', MOVIE_ID);
    c.check('fenêtre de film rouverte pour la 2e demande', o.ok, o);
    post = await submit(p, 'ok', Q.F2);
    c.check('demande de film interceptée (201 simulé)', !!post && post.reply === 'ok' && post.body && post.body.mediaType === 'movie', post && post.body);
    const tf = await waitToast(p, TXT.toastFilm, false);
    await p.screenshot({ path: SHOT('10-message-film') });
    c.check('(c) message après le film demandé, à l\'écran, au-dessus de tout, sans capter le toucher', toastOk(tf, TXT.toastFilm) && tf.pointerEvents === 'none', tf);

    // (h) série entière (requestMedia 'tv') : un 403 n'est réécrit que si les saisons sont épuisées
    const whole = async (quota) => {
      M.quota = quota; M.reply = 'forbidden'; M.afterOk = null;
      return p.evaluate((id, tvdb) => window.JellyfinEnhanced.jellyseerrAPI.requestMedia(id, 'tv', {}, false, null, tvdb)
        .then(() => ({ ok: true }), (e) => ({ status: e && e.status, message: e && e.responseJSON && e.responseJSON.message })), TV_ID, TVDB)
        .catch((e) => ({ error: String(e.message).slice(0, 120) }));
    };
    let h = await whole(Q.H);
    let hb = (M.posts[M.posts.length - 1] || {}).body || {};
    c.check('(h) série entière refusée avec 8 places restantes : 403 laissé tel quel (pas un refus de quota)',
      h.status === 403 && h.message === JE_403.message && hb.seasons === 'all' && hb.tvdbId === TVDB, { h, body: hb });
    h = await whole(Q.D);
    hb = (M.posts[M.posts.length - 1] || {}).body || {};
    c.check('(h) série entière refusée, saisons épuisées : 403 réécrit en refus de quota français', h.status === 403 && h.message === TXT.full, { h, body: hb });
    await sleep(1500);
    await closeDialog(p);  // aucune fenêtre attendue (appel direct), par sûreté

    // (f) rien n'est sorti du navigateur vers le proxy Seerr
    await sleep(1000);
    c.check('(f) chaque demande vue par le navigateur a reçu la réponse du banc (aucune vers Seerr)',
      net.posts === M.posts.length && M.posts.length === 9, { navigateur: net.posts, banc: M.posts.length });
    c.check('(f) aucune réponse du proxy Seerr de JE venue du réseau (détecteur éprouvé sur les autres requêtes)',
      net.leaks.length === 0 && net.seerr > 0 && net.remote > 0, net);
    c.check('aucune fenêtre window.alert/confirm/prompt', p.gcDialogs.length === 0, p.gcDialogs);
    extra.posts = M.posts.map((k) => ({ reply: k.reply, body: k.body }));
  } catch (e) {
    c.check('déroulé du banc', false, String(e.message).slice(0, 300));
    if (p) await p.screenshot({ path: SHOT('erreur') }).catch(() => {});
  }
  try {
    const s = p ? await spy(p) : null;
    Object.assign(extra, {
      quota_gets: M.quotaGets, non_simules: M.other, ecritures_bloquees: M.blocked, reseau: net, espion: s,
      erreurs_js: p ? p.gcErrors.slice(0, 10) : [],
    });
    console.log('info   GET quota :', M.quotaGets, '· chemins non simulés :', JSON.stringify(M.other.slice(0, 10)),
      '· écritures bloquées :', JSON.stringify(M.blocked.slice(0, 10)));
    if (p && p.gcErrors.length) console.log('erreurs JavaScript de la page :', p.gcErrors.slice(0, 5));
  } catch (e) { /* bilan seulement */ }
  code = c.done(extra);
  await b.close();
  process.exit(code);
})();
