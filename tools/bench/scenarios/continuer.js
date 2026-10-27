// tools/bench/scenarios/continuer.js — « Retirer de Continuer à regarder » (gc-continue.js) et section de Mon compte
// (2026-10-27). Le compte de banc reçoit une progression sur ITEMS (« film,épisode » : ses propres données, supprimées
// avec lui), puis À L'ÉCRAN :
//   - les deux vignettes dans « Continuer à regarder » ;
//   - menu « … » (bureau, télé) ou appui long + ⋯ de la sélection (téléphone) : notre entrée visible, celle de Jellyfin
//     Enhanced absente ; un appui retire la SÉRIE de la rangée, message « Annuler » affiché ;
//   - après rechargement : toujours retirée (stockage serveur), réponses /UserItems/Resume (fetch ET XHR) filtrées ;
//   - Mon compte : le titre listé, « Remettre » le fait revenir ;
//   - masquée de nouveau puis « relancée » (date de dernière lecture plus récente) : revient seule, sortie de la liste ;
//   - progression jamais touchée (PlaybackPositionTicks identique avant/après).
// Usage : tools/bench/bench.sh --candidate DIR --env ITEMS=<film>,<épisode> tools/bench/scenarios/continuer.js desktop phone tv
'use strict';
const { env, launch, newPage, login, boxes, onScreen, waitOnScreen, checker, sleep } = require('/bench/bench.js');

const [MOVIE, EPISODE] = (process.env.ITEMS || '').split(',');
const ROW = '.verticalSection.ContinueWatching';

async function cardOnScreen(p, id) {
  return p.evaluate((row, id) => {
    const c = document.querySelector(`${row} .card[data-id="${id}"]`);
    if (!c) return false;
    const r = c.getBoundingClientRect();
    return r.width > 0 && r.height > 0 && getComputedStyle(c).display !== 'none';
  }, ROW, id);
}
async function waitCard(p, id, present, ms = 30000) {
  const end = Date.now() + ms;
  while (Date.now() < end) { if ((await cardOnScreen(p, id)) === present) return true; await sleep(500); }
  return false;
}
async function api(p, fn, ...args) { return p.evaluate(fn, ...args); }
async function setProgress(p, id, ticks, date) {
  return api(p, async (id, ticks, date) => {
    const a = window.ApiClient;
    await a.ajax({ type: 'POST', url: a.getUrl(`UserItems/${id}/UserData`, { userId: a.getCurrentUserId() }), data: JSON.stringify({ PlaybackPositionTicks: ticks, LastPlayedDate: date, Played: false }), contentType: 'application/json' });
    return true;
  }, id, ticks, date);
}
async function position(p, id) {
  return api(p, async (id) => { const a = window.ApiClient; const r = await a.getJSON(a.getUrl('Items', { userId: a.getCurrentUserId(), ids: id, enableUserData: true })); return r.Items[0].UserData.PlaybackPositionTicks; }, id);
}
async function hiddenPrefs(p) {
  return api(p, async () => { const a = window.ApiClient; const r = await a.getJSON(a.getUrl('DisplayPreferences/gc-continue', { userId: a.getCurrentUserId(), client: 'groscailloux', banc: 1 })); return JSON.parse((r.CustomPrefs || {}).gcHidden || '{}'); });
}
async function goHome(p) {
  await p.reload({ waitUntil: 'domcontentloaded' });
  await p.waitForFunction(() => window.ApiClient && ApiClient.accessToken && ApiClient.accessToken(), { timeout: 60000 });
  await waitOnScreen(p, '.verticalSection.MyMedia .card', 60000);
  await sleep(2500);
}
// ouvre le menu d'une vignette et renvoie la boîte de notre entrée (null si absente)
async function openMenu(p, id, device) {
  if (!(await p.evaluate((row, id) => { const c = document.querySelector(`${row} .card[data-id="${id}"]`); if (c) c.scrollIntoView({ block: 'center', inline: 'center' }); return !!c; }, ROW, id))) return { err: 'vignette absente' };
  await sleep(600);
  if (device === 'phone') {
    const pt = await p.evaluate((row, id) => { const r = document.querySelector(`${row} .card[data-id="${id}"] .cardImageContainer`).getBoundingClientRect(); return [r.left + r.width / 3, r.top + r.height / 3]; }, ROW, id);
    await p.touchscreen.touchStart(pt[0], pt[1]); await sleep(900); await p.touchscreen.touchEnd(); await sleep(1200);
    const more = await boxes(p, '.selectionCommandsPanel .btnSelectionPanelOptions');
    if (!more.length || !more[0].w) return { err: 'pas de sélection multiple' };
    await p.tap('.selectionCommandsPanel .btnSelectionPanelOptions');
  } else if (device === 'tv') {
    // télé web : menu de la vignette = commande « menu » de jellyfin-web (contextmenu sur le conteneur de vignettes)
    const ok = await p.evaluate((row, id) => { const c = document.querySelector(`${row} .card[data-id="${id}"]`); if (!c) return false; c.focus(); c.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true })); return true; }, ROW, id);
    if (!ok) return { err: 'vignette absente' };
    await sleep(2500);
  } else {
    const sel = `${ROW} .card[data-id="${id}"]`;
    await p.hover(sel);
    await sleep(500);
    const b = await p.$(`${sel} button[data-action="menu"]`);
    if (!b) return { err: 'pas de bouton …' };
    await b.click();
  }
  await sleep(1200);
  return p.evaluate(() => {
    const sh = [...document.querySelectorAll('.actionSheet')].pop();
    if (!sh) return { err: 'pas de feuille' };
    const box = (s) => { const e = sh.querySelector(s); if (!e) return null; const r = e.getBoundingClientRect(); return r.width > 0 && r.height > 0 && getComputedStyle(e).display !== 'none' ? [Math.round(r.left), Math.round(r.top), Math.round(r.width), Math.round(r.height)] : null; };
    return { ours: box('[data-id="gc-continue-hide"]'), je: box('[data-id="remove-continue-watching"]') || box('[data-id="je-multiselect-remove"]'), items: [...sh.querySelectorAll('.actionSheetMenuItem')].filter((x) => x.offsetParent).map((x) => x.getAttribute('data-id')) };
  });
}

(async () => {
  const c = checker('continuer');
  const b = await launch();
  let code = 1;
  const device = env.device === 'tv' || env.layout === 'tv' ? 'tv' : (['phone', 'iphone', 'android'].includes(env.device) ? 'phone' : 'desktop');
  try {
    if (!MOVIE || !EPISODE) throw new Error('ITEMS=<film>,<épisode> attendu');
    const p = await newPage(b);
    const counts = { resume: 0, nextup: 0, prefs: 0, plays: 0, hss: 0 };
    p.on('request', (r) => {
      const u = r.url();
      if (/\/HomeScreen\/Section\/(ContinueWatching|NextUp)/.test(u)) counts.hss++;
      else if (/DisplayPreferences\/gc-continue/.test(u) && r.method() === 'GET' && !/banc=1/.test(u)) counts.prefs++;
      else if (/sortBy=DatePlayed/i.test(u)) counts.plays++;
    });
    await login(p);
    await p.waitForFunction(() => window.ApiClient && ApiClient.accessToken && ApiClient.accessToken(), { timeout: 60000 });
    const skips = () => api(p, async () => { const a = window.ApiClient; const r = await a.getJSON(a.getUrl('DisplayPreferences/usersettings', { userId: a.getCurrentUserId(), client: 'emby' })); const c = r.CustomPrefs || {}; return `${c.skipForwardLength}/${c.skipBackLength}`; });
    const skip0 = await skips();
    const now = Date.now();
    await setProgress(p, MOVIE, 6000000000, new Date(now - 3600e3).toISOString());
    await setProgress(p, EPISODE, 6000000000, new Date(now - 7200e3).toISOString());
    await goHome(p);
    c.check('script chargé', await p.evaluate(() => !!window.gcContinue));
    c.check('film et épisode à l\'écran dans Continuer à regarder', (await waitCard(p, MOVIE, true)) && (await waitCard(p, EPISODE, true)));
    const pos0 = await position(p, EPISODE);

    // retrait de l'épisode (= la série entière)
    const m = await openMenu(p, EPISODE, device);
    c.check('notre entrée visible dans le menu', !!(m && m.ours), m);
    c.check('entrée de Jellyfin Enhanced non affichée', !!m && !m.je, m);
    if (m && m.ours) {
      if (device === 'tv') await p.evaluate(() => document.querySelector('.actionSheet [data-id="gc-continue-hide"]').click());
      else if (device === 'phone') await p.tap('.actionSheet [data-id="gc-continue-hide"]');
      else await p.click('.actionSheet [data-id="gc-continue-hide"]');
    }
    c.check('épisode retiré de la rangée à l\'écran', await waitCard(p, EPISODE, false, 15000));
    c.check('film toujours là', await cardOnScreen(p, MOVIE));
    let sheetOpen = true;   // fermeture animée : jusqu'à 3 s
    for (let i = 0; i < 6 && sheetOpen; i++) { sheetOpen = await onScreen(p, '.actionSheet'); if (sheetOpen) await sleep(500); }
    c.check('feuille fermée', !sheetOpen, await boxes(p, '.actionSheet'));
    c.check('message avec « Annuler » à l\'écran', await onScreen(p, '.gc-cont-toast button'), await boxes(p, '.gc-cont-toast'));
    await p.screenshot({ path: `/out/continuer-retire-${env.benchDevice.replace(/[^a-z0-9-]/gi, '_')}.png` });
    if (device === 'phone') c.check('sélection multiple refermée', !(await onScreen(p, '.selectionCommandsPanel')));
    const pr = await hiddenPrefs(p);
    const keys = Object.keys(pr);
    c.check('stocké côté serveur sous l\'id de la SÉRIE, avec la date de dernière lecture du serveur', keys.length === 1 && keys[0] !== EPISODE && pr[keys[0]].t > 0, pr);
    c.check('progression intacte', (await position(p, EPISODE)) === pos0);

    await goHome(p);
    c.check('toujours retiré après rechargement', (await waitCard(p, MOVIE, true)) && !(await cardOnScreen(p, EPISODE)));
    const filt = await api(p, async (ep) => {
      const a = window.ApiClient; const u = a.getUrl('UserItems/Resume', { userId: a.getCurrentUserId(), limit: 10, includeItemTypes: 'Movie,Episode' });
      const viaFetch = (await a.getJSON(u)).Items.map((i) => i.Id);
      const viaXhr = await new Promise((res) => { const x = new XMLHttpRequest(); x.open('GET', u); x.setRequestHeader('Authorization', `MediaBrowser Token="${a.accessToken()}"`); x.onload = () => res(JSON.parse(x.responseText).Items.map((i) => i.Id)); x.send(); });
      const series = await a.getJSON(a.getUrl('Shows/NextUp', { userId: a.getCurrentUserId(), seriesId: 'x' })).then(() => 'ok', () => 'err');
      return { viaFetch: viaFetch.includes(ep), viaXhr: viaXhr.includes(ep), series };
    }, EPISODE);
    c.check('/UserItems/Resume filtré (fetch et XHR)', !filt.viaFetch && !filt.viaXhr, filt);

    // Mon compte : liste et « Remettre »
    if (await waitOnScreen(p, '.gc-acc-btn', 30000)) {
      await p.evaluate(() => document.querySelector('.gc-acc-btn').click());
      let row = null;
      for (let i = 0; i < 24 && !(row && row.btn); i++) {   // panneau et liste chargés (télé plus lente)
        await sleep(500);
        row = await p.evaluate(() => {
          const sec = document.querySelector('.gc-acc .gc-cont-list');
          if (!sec) return null;
          const b = sec.querySelector('.row button');
          const r = b && b.getBoundingClientRect();
          return { text: sec.textContent.slice(0, 200), btn: !!r && r.width > 0 && r.height > 0 };
        });
      }
      c.check('Mon compte : titre listé avec « Remettre » à l\'écran', !!(row && row.btn), row);
      await p.screenshot({ path: `/out/continuer-compte-${env.benchDevice.replace(/[^a-z0-9-]/gi, '_')}.png` });
      if (row && row.btn) {
        await p.evaluate(() => document.querySelector('.gc-acc .gc-cont-list .row button').click());
        await sleep(2500);
        c.check('« Remettre » vide la liste', Object.keys(await hiddenPrefs(p)).length === 0);
      }
      await p.evaluate(() => { const x = document.querySelector('.gc-acc .close'); if (x) x.click(); });
      await sleep(500);
      c.check('remis : de retour dans la rangée', await waitCard(p, EPISODE, true, 20000));
    } else c.check('bouton Mon compte à l\'écran', false);

    // retour automatique après une nouvelle lecture
    const m2 = await openMenu(p, EPISODE, device);
    if (m2 && m2.ours) {
      await p.evaluate(() => document.querySelector('.actionSheet [data-id="gc-continue-hide"]').click());
      await waitCard(p, EPISODE, false, 15000);
    }
    c.check('retiré une 2e fois', Object.keys(await hiddenPrefs(p)).length === 1);
    await setProgress(p, EPISODE, 6500000000, new Date().toISOString());
    await p.evaluate(() => { window.__gcContinue.at = 0; });   // sans attendre la minute de cache
    await goHome(p);
    c.check('relancé après le retrait : revient seul', await waitCard(p, EPISODE, true, 20000));
    let left = 1;
    for (let i = 0; i < 20 && left; i++) { left = Object.keys(await hiddenPrefs(p)).length; if (left) await sleep(500); }
    c.check('relancé : sorti de la liste', left === 0);
    const skip1 = await skips();
    c.check('réglages du lecteur (usersettings/emby) inchangés', skip0 === skip1, { avant: skip0, apres: skip1 });
    console.log('info   requêtes :', JSON.stringify(counts));
    // 4 chargements de page + 4 écritures (relecture avant chaque écriture) + la liste de Mon compte ≈ 9 lectures
    c.check('pas de rafale (lectures des préférences ≤ 12, dernières lectures ≤ 8 sur tout le banc)', counts.prefs <= 12 && counts.plays <= 8, counts);
    if (p.gcErrors.length) console.log('erreurs JavaScript de la page :', p.gcErrors.slice(0, 5));
    code = c.done({ counts, errors: p.gcErrors.slice(0, 10) });
  } catch (e) {
    c.check('déroulé du banc', false, String(e.message).slice(0, 200));
    code = c.done();
  } finally {
    await b.close();
  }
  process.exit(code);
})();
