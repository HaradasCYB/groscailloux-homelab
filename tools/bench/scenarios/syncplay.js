// tools/bench/scenarios/syncplay.js — banc SyncPlay à deux navigateurs (2026-10-10) : combien de temps le groupe
// reste-t-il arrêté après un saut, selon l'attente maximale de gc-syncplay.js (WAIT_MS) ?
// Repris de backups/syncplay-20261003/sp_bench.js (un saut, deux comptes) et étendu après la séance du 10/10, où un
// membre sur Jellyfin Desktop 2.0.0-dev faisait attendre tout le groupe 12 s à chaque saut. Le candidat
// /repo/branding/jellyfin/gc-syncplay.js (l'arbre de travail) est ajouté au private.js servi aux DEUX navigateurs,
// forcé (localStorage « gc-syncplay-force »), et réglé par window.__gcSyncPlayTest ({ waitMs, noArrive }).
// Phases (PHASES, toutes par défaut) :
//   normal  sauts ordinaires sur ITEM : délai réel d'arrivée de chaque navigateur (attente maximale 12 s)
//   lent    la première requête vidéo de B après le saut est retardée de DELAYS ms (premier octet lent, comme un
//           fichier froid de la seedbox) ; pour chaque attente WAITS : un saut lointain. Coût d'un « prêt » envoyé
//           AVANT l'arrivée, contre celui d'une attente plus longue.
//   muet    B ne voit jamais son lecteur arriver (comme l'appli 2.0.0-dev du 10/10) : groupe arrêté = WAITS ; puis
//           une rafale de 5 sauts (le cas du 10/10)
//   seedbox sauts lointains sur ITEM2 (fichier de la seedbox, zones jamais lues) : délai réel d'arrivée à froid
//   depart  A lance ITEM seul PUIS crée le groupe, B le rejoint (démarrage du 10/10) ; puis relance depuis le groupe
// Chaque phase repart d'une lecture de groupe neuve (un lecteur web rechargé à 0 fausserait la suite, banc du 10/10).
// Mesures : état du groupe côté serveur (SyncPlay/List), vidéos des deux pages (« synchro » = les deux tournent
// vraiment : ni en pause, ni en cours de saut, HAVE_FUTURE_DATA, à 1,5 s l'une de l'autre), window.__gcSyncPlay.last.
// Les navigateurs du banc (Chromium de Linux) lisent en HLS remuxé : chaque saut lointain relance ffmpeg.
// Usage : tools/bench/bench.sh --accounts 2 --prefix zz_sp --item <id H.264 du VPS> --env ITEM2=<id seedbox>
//           --timeout 1200 tools/bench/scenarios/syncplay.js desktop
// Sorties : /out/syncplay.json (et le nom du groupe, pour lire le journal du serveur ensuite).
'use strict';
const fs = require('fs');
const path = require('path');
const { env, sleep, launch, login } = require('/bench/bench.js');

const CAND = fs.readFileSync('/repo/branding/jellyfin/gc-syncplay.js', 'utf8');
const num = (v, d) => (v || d).split(',').map(Number).filter((x) => x > 0);
const WAITS = num(process.env.WAITS, '3000,5000,8000');
const DELAYS = num(process.env.DELAYS, '4000');
const PHASES = (process.env.PHASES || 'normal,seedbox,lent,muet,depart').split(',');
const ITEM = env.item;
const ITEM2 = process.env.ITEM2 || '';
const GROUP = 'gc-banc-' + Date.now().toString(36);
const out = { group: GROUP, waits: WAITS, delays: DELAYS, phases: {}, console: { A: [], B: [] } };
const T0 = Date.now();

async function open(browser, user, pw, name) {
  const ctx = await browser.createBrowserContext(); // comptes séparés : jellyfin-web garde la session dans localStorage
  const p = await ctx.newPage();
  // messages SyncPlay de jellyfin-web et du script (ce qu'un vrai appareil ne nous dit pas)
  p.on('console', (m) => {
    const t = m.text();
    if (/SyncPlay|gc-syncplay/.test(t) && !/TimeSync|Ping/.test(t)) out.console[name].push(`${((Date.now() - T0) / 1000).toFixed(1)} ${t.slice(0, 300)}`);
  });
  await p.setViewport({ width: 640, height: 360 });
  p.delayOnce = 0;
  await p.evaluateOnNewDocument(() => {
    try { localStorage.setItem('gc-syncplay-force', '1'); } catch (e) { /* stockage refusé */ }
    window.__gcSyncPlayTest = { waitMs: 12000, noArrive: false };
  });
  await p.setBypassServiceWorker(true);
  await p.setRequestInterception(true);
  p.on('request', async (r) => {
    const u = r.url().split('?')[0];
    try {
      if (/\/JavaScriptInjector\/private\.js$/.test(u)) {
        const body = await (await fetch(r.url(), { headers: r.headers() })).text();
        return await r.respond({ status: 200, contentType: 'application/javascript; charset=utf-8', body: body + '\n;\n' + CAND });
      }
      // premier octet lent : la première requête du flux vidéo après le saut (jamais l'API : le « prêt » partirait
      // en retard)
      if (p.delayOnce > 0 && /\/videos\/[0-9a-f-]+\/(stream|hls)/i.test(u)) { const d = p.delayOnce; p.delayOnce = 0; await sleep(d); }
    } catch (e) { /* requête servie telle quelle */ }
    if (!r.isInterceptResolutionHandled()) r.continue().catch(() => {});
  });
  await login(p, user, pw);
  await p.waitForFunction(() => window.ApiClient && window.ApiClient.isWebSocketOpen && window.ApiClient.isWebSocketOpen(), { timeout: 30000 }).catch(() => {});
  await p.waitForFunction(() => window.__gcSyncPlay && window.__gcSyncPlay.version >= 2 && window.__gcSyncPlay.patched, { timeout: 60000 });
  return p;
}

const call = (p, method, url, body) => p.evaluate(async (method, url, body) => {
  const AC = window.ApiClient;
  const r = await fetch(AC.getUrl(url), { method, headers: { 'X-Emby-Token': AC.accessToken(), 'Content-Type': 'application/json' }, body: body ? JSON.stringify(body) : undefined });
  const t = await r.text();
  try { return { status: r.status, body: t ? JSON.parse(t) : null }; } catch (e) { return { status: r.status, body: t.slice(0, 200) }; }
}, method, url, body || null);
const video = (p) => p.evaluate(() => {
  const v = document.querySelector('video.htmlvideoplayer');
  return v ? { t: Math.round(v.currentTime * 10) / 10, paused: v.paused, seeking: v.seeking, ready: v.readyState } : null;
});
const gc = (p) => p.evaluate(() => (window.__gcSyncPlay ? window.__gcSyncPlay.last : null));
const setTest = (p, t) => p.evaluate((t) => Object.assign(window.__gcSyncPlayTest, t), t);
async function groupState(p) {
  const r = await call(p, 'GET', 'SyncPlay/List');
  const g = (r.body || []).find((x) => x.GroupName === GROUP);
  return g ? g.State : 'absent';
}

// Observe le groupe après un saut : premier passage en lecture, retours en attente, et moment où les deux vidéos
// tournent ensemble (écart < 1,5 s) près de la cible.
async function observe(A, Bp, target, maxMs = 40000, settleMs = 6000) {
  const t0 = Date.now();
  const r = { target, firstPlayingMs: null, waitingAgain: 0, syncedMs: null, states: [] };
  let prev = null, waited = false, played = false;
  while (Date.now() - t0 < maxMs) {
    const [g, va, vb] = await Promise.all([groupState(A), video(A), video(Bp)]);
    const dt = Date.now() - t0;
    if (g !== prev) {
      r.states.push(`${(dt / 1000).toFixed(1)}s ${g}`);
      if (g === 'Waiting') { if (played) r.waitingAgain++; waited = true; }
      if (g === 'Playing' && waited && !played) { played = true; r.firstPlayingMs = dt; }
      prev = g;
    }
    const runs = (v) => v && !v.paused && !v.seeking && v.ready >= 3;
    if (played && r.syncedMs === null && runs(va) && runs(vb) && Math.abs(va.t - vb.t) < 1.5 && Math.abs(va.t - target) < 30) r.syncedMs = dt;
    if (r.syncedMs !== null && dt > r.syncedMs + settleMs) break;
    await sleep(200);
  }
  [r.A, r.B] = [await gc(A), await gc(Bp)];
  [r.videoA, r.videoB] = [await video(A), await video(Bp)];
  return r;
}

let current = ITEM; // titre de la lecture de groupe en cours
async function seekAndObserve(A, Bp, delta, opts) {
  // les deux lecteurs doivent tourner avant le saut ; sinon (lecteur tombé) : lecture de groupe relancée, notée
  const runs = (v) => v && !v.paused && !v.seeking && v.ready >= 3;
  let restarted = false;
  for (let i = 0; i < 16 && !(runs(await video(A)) && runs(await video(Bp))); i++) await sleep(500);
  if (!(runs(await video(A)) && runs(await video(Bp)))) {
    console.log(`  lecteur arrêté avant le saut (A ${JSON.stringify(await video(A))} B ${JSON.stringify(await video(Bp))}) : relance`);
    await playQueue(A, Bp, current); await sleep(4000); restarted = true;
  }
  const v = await video(A);
  const target = Math.max(5, Math.round((v ? v.t : 0) + delta));
  const st = (await call(A, 'POST', 'SyncPlay/Seek', { PositionTicks: target * 1e7 })).status;
  const r = await observe(A, Bp, target, opts && opts.maxMs, opts && opts.settleMs);
  r.seek = st; r.restarted = restarted;
  const line = (x) => (x ? `${x.via} ${x.waitedMs} ms` : '—');
  console.log(`  saut → ${target} s : groupe en lecture après ${r.firstPlayingMs} ms, synchro ${r.syncedMs} ms, re-attentes ${r.waitingAgain} | A ${line(r.A)} | B ${line(r.B)}`);
  return r;
}

async function playQueue(A, Bp, item) {
  const st = (await call(A, 'POST', 'SyncPlay/SetNewQueue', { PlayingQueue: [item], PlayingItemPosition: 0, StartPositionTicks: 0 })).status;
  for (let i = 0; i < 90; i++) {
    await sleep(1000);
    const [va, vb] = [await video(A), await video(Bp)];
    if (va && vb && !va.paused && !vb.paused && va.t > 3 && vb.t > 3) return st;
  }
  throw new Error(`lecture de groupe non démarrée (${item}) : A ${JSON.stringify(await video(A))} B ${JSON.stringify(await video(Bp))}`);
}

(async () => {
  const b = await launch(['--autoplay-policy=no-user-gesture-required', '--mute-audio']);
  let code = 0;
  let A, Bp;
  try {
    A = await open(b, env.user, env.pw, 'A');
    Bp = await open(b, process.env.USER_NAME_2, process.env.PW_2, 'B');
    out.created = (await call(A, 'POST', 'SyncPlay/New', { GroupName: GROUP })).status;
    await sleep(2000);
    const g = ((await call(A, 'GET', 'SyncPlay/List')).body || []).find((x) => x.GroupName === GROUP);
    if (!g) throw new Error('groupe du banc introuvable');
    out.groupId = g.GroupId;
    for (let i = 0; i < 5; i++) {
      out.joined = (await call(Bp, 'POST', 'SyncPlay/Join', { GroupId: g.GroupId })).status;
      if (out.joined < 300) break;
      await sleep(2000);
    }
    await sleep(3000);
    const both = (t) => Promise.all([setTest(A, t), setTest(Bp, t)]);

    // lecture de groupe neuve, puis 4 s de lecture
    const fresh = async (item) => { current = item || ITEM; await both({ waitMs: 12000, noArrive: false }); await playQueue(A, Bp, current); await sleep(4000); };
    if (PHASES.includes('normal')) {
      console.log('== normal (attente maximale 12 s)');
      await fresh();
      out.phases.normal = [];
      for (const d of [30, 360, -180, 1200, 20, 900]) out.phases.normal.push(await seekAndObserve(A, Bp, d, { maxMs: 30000, settleMs: 4000 }));
    }
    if (PHASES.includes('lent')) {
      out.phases.lent = [];
      await fresh();
      for (const delay of DELAYS) {
        for (const w of WAITS) {
          console.log(`== lent : premier octet de B retardé de ${delay} ms, attente maximale ${w} ms`);
          await both({ waitMs: w, noArrive: false });
          Bp.delayOnce = delay;
          const r = await seekAndObserve(A, Bp, 600, { maxMs: 45000, settleMs: 8000 });
          Bp.delayOnce = 0;
          out.phases.lent.push({ delay, wait: w, ...r });
          await sleep(3000);
        }
      }
    }
    if (PHASES.includes('muet')) {
      out.phases.muet = [];
      await fresh();
      for (const w of WAITS) {
        console.log(`== muet : B ne voit jamais son lecteur arriver, attente maximale ${w} ms`);
        await both({ waitMs: w, noArrive: false });
        await setTest(Bp, { noArrive: true });
        out.phases.muet.push({ wait: w, ...(await seekAndObserve(A, Bp, 120, { maxMs: 30000, settleMs: 4000 })) });
        await setTest(Bp, { noArrive: false });
        await sleep(3000);
      }
      const w = WAITS[Math.min(1, WAITS.length - 1)];
      console.log(`== muet : rafale de 5 sauts (1,5 s d'écart), attente maximale ${w} ms`);
      await both({ waitMs: w, noArrive: false });
      await setTest(Bp, { noArrive: true });
      for (let i = 0; i < 4; i++) {
        const v = await video(A);
        await call(A, 'POST', 'SyncPlay/Seek', { PositionTicks: Math.round(((v ? v.t : 0) + 10) * 1e7) });
        await sleep(1500);
      }
      out.phases.rafale = { wait: w, ...(await seekAndObserve(A, Bp, 10, { maxMs: 30000, settleMs: 4000 })) };
      await setTest(Bp, { noArrive: false });
    }
    if (PHASES.includes('seedbox') && ITEM2) {
      console.log('== seedbox : sauts lointains à froid (attente maximale 12 s)');
      try {
        await fresh(ITEM2);
        out.phases.seedbox = [];
        for (const d of [900, 1500, -1200, 2400, 600]) out.phases.seedbox.push(await seekAndObserve(A, Bp, d, { maxMs: 40000, settleMs: 4000 }));
      } catch (e) {
        out.seedboxError = String((e && e.message) || e); // la phase suivante tourne quand même
        console.log('ERREUR seedbox : ' + out.seedboxError);
        code = 1;
      }
    }
    if (PHASES.includes('depart')) {
      console.log('== depart : A lance seul, PUIS crée le groupe, B rejoint');
      await call(A, 'POST', 'SyncPlay/Leave'); await call(Bp, 'POST', 'SyncPlay/Leave');
      await sleep(3000);
      const dep = { group: GROUP + '-d', fromS: ((Date.now() - T0) / 1000).toFixed(1) };
      const devA = await A.evaluate(() => window.ApiClient.deviceId());
      const sess = ((await call(A, 'GET', `Sessions?deviceId=${encodeURIComponent(devA)}`)).body || [])[0];
      dep.play = (await call(A, 'POST', `Sessions/${sess.Id}/Playing?playCommand=PlayNow&itemIds=${ITEM}`)).status;
      for (let i = 0; i < 40; i++) { await sleep(1000); const v = await video(A); if (v && !v.paused && v.t > 5) break; }
      await sleep(5000);
      dep.before = { A: await video(A), B: await video(Bp) };
      dep.created = (await call(A, 'POST', 'SyncPlay/New', { GroupName: dep.group })).status;
      await sleep(1500);
      const g2 = ((await call(A, 'GET', 'SyncPlay/List')).body || []).find((x) => x.GroupName === dep.group);
      dep.groupId = g2 && g2.GroupId;
      dep.joined = g2 ? (await call(Bp, 'POST', 'SyncPlay/Join', { GroupId: g2.GroupId })).status : null;
      const t0 = Date.now();
      dep.timeline = [];
      let prev = '';
      const gs = async () => { const r = await call(A, 'GET', 'SyncPlay/List'); const x = (r.body || []).find((y) => y.GroupName === dep.group); return x ? x.State : 'absent'; };
      while (Date.now() - t0 < 40000) {
        const [s, va, vb] = await Promise.all([gs(), video(A), video(Bp)]);
        const line = `${s} | A ${va ? `${va.t}${va.paused ? ' ‖' : ' ▶'}` : '—'} | B ${vb ? `${vb.t}${vb.paused ? ' ‖' : ' ▶'}` : '—'}`;
        if (line.replace(/[\d.]+/g, '') !== prev) { dep.timeline.push(`${((Date.now() - t0) / 1000).toFixed(1)}s ${line}`); prev = line.replace(/[\d.]+/g, ''); }
        await sleep(500);
      }
      dep.after40s = { A: await video(A), B: await video(Bp), state: await gs() };
      console.log(`  au bout de 40 s : ${dep.after40s.state}, A ${JSON.stringify(dep.after40s.A)}, B ${JSON.stringify(dep.after40s.B)}`);
      // relance depuis le groupe (ce qui a débloqué la séance du 10/10)
      dep.relaunch = (await call(A, 'POST', 'SyncPlay/SetNewQueue', { PlayingQueue: [ITEM], PlayingItemPosition: 0, StartPositionTicks: 0 })).status;
      const t1 = Date.now();
      for (let i = 0; i < 60; i++) {
        await sleep(500);
        const [va, vb] = [await video(A), await video(Bp)];
        if (va && vb && !va.paused && !vb.paused && va.t > 1 && vb.t > 1) { dep.relaunchPlayingMs = Date.now() - t1; break; }
      }
      console.log(`  relance depuis le groupe : les deux lisent après ${dep.relaunchPlayingMs} ms`);
      out.phases.depart = dep;
    }
  } catch (e) {
    out.error = String((e && e.message) || e);
    console.log('ERREUR : ' + out.error);
    code = 1;
  }
  try { if (A) await call(A, 'POST', 'SyncPlay/Leave'); if (Bp) await call(Bp, 'POST', 'SyncPlay/Leave'); } catch (e) { /* pages fermées */ }
  try { fs.writeFileSync(path.join(env.out, 'syncplay.json'), JSON.stringify(out, null, 1)); } catch (e) { /* /out absent */ }
  console.log(JSON.stringify({ group: out.group, groupId: out.groupId, departGroupId: out.phases.depart && out.phases.depart.groupId, error: out.error || null }));
  await b.close();
  process.exit(code);
})();
