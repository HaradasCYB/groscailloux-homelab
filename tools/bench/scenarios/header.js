// tools/bench/scenarios/header.js — banc de fumée de l'en-tête (2026-10-08, repris de header_dump.js du 03/10).
// Connexion d'un compte ordinaire, puis À L'ÉCRAN : une barre d'en-tête visible, le bouton du tchat et celui de Mon
// compte (sauf télé : pas de tchat), le menu ☰ cliquable (pas recouvert), et ces boutons toujours là après un aller-retour
// sur la recherche. Lecture seule : aucun réglage touché, aucune lecture lancée. Capture : /out/header-<appareil>.png.
// Usage : tools/bench/bench.sh tools/bench/scenarios/header.js desktop phone [desktop-legacy tv …]
'use strict';
const { env, launch, newPage, login, boxes, onScreen, waitOnScreen, checker, sleep } = require('/bench/bench.js');

(async () => {
  const c = checker('header');
  const b = await launch();
  let code = 1;
  try {
    const p = await newPage(b);
    await login(p);
    c.check('connexion du compte de banc', true);
    const tv = env.device === 'tv' || env.layout === 'tv';
    // nos boutons sont posés par des scripts chargés après l'accueil : jusqu'à 40 s
    const acc = await waitOnScreen(p, '.gc-acc-btn', 40000);
    const bar = await p.evaluate(() => {
      const vis = (el) => { const r = el.getBoundingClientRect(); return r.width > 0 && r.height > 0 && getComputedStyle(el).visibility !== 'hidden'; };
      const bars = [...document.querySelectorAll('header.MuiAppBar-root, .skinHeader')].filter(vis);
      return bars.map((x) => (x.matches('header.MuiAppBar-root') ? 'moderne' : 'ancienne'));
    });
    c.check('une barre d\'en-tête visible', bar.length > 0, bar);
    c.check('bouton Mon compte à l\'écran', acc, await boxes(p, '.gc-acc-btn'));
    if (!tv) c.check('bouton du tchat à l\'écran', await onScreen(p, '.gc-chat-btn'), await boxes(p, '.gc-chat-btn'));
    else c.check('pas de tchat sur télé', !(await onScreen(p, '.gc-chat-btn')), await boxes(p, '.gc-chat-btn'));
    // ☰ (seul accès aux bibliothèques sur téléphone) : l'élément au point de son centre doit être lui ou un enfant
    const menu = await p.evaluate(() => {
      const m = document.querySelector('header.MuiAppBar-root button[aria-label="Open Menu"], header.MuiAppBar-root button[aria-label="Ouvrir le menu"], .skinHeader .mainDrawerButton');
      if (!m) return null;
      const r = m.getBoundingClientRect();
      if (!r.width || !r.height) return { visible: false };
      const hit = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
      return { visible: true, free: !!hit && (hit === m || m.contains(hit)) };
    });
    if (menu) c.check('menu ☰ cliquable (rien par-dessus)', !menu.visible || menu.free, menu);
    await p.screenshot({ path: `/out/header-${env.benchDevice.replace(/[^a-z0-9-]/gi, '_')}.png` });
    await p.evaluate(() => { location.hash = '#/search'; });
    await sleep(4000);
    await p.goBack().catch(() => {});
    c.check('Mon compte toujours à l\'écran après la recherche', await waitOnScreen(p, '.gc-acc-btn', 15000));
    if (p.gcErrors.length) console.log('erreurs JavaScript de la page :', p.gcErrors.slice(0, 5));
    code = c.done({ bars: bar, candidates: p.gcCandidates, errors: p.gcErrors.slice(0, 10) });
  } catch (e) {
    c.check('déroulé du banc', false, String(e.message).slice(0, 200));
    code = c.done();
  } finally {
    await b.close();
  }
  process.exit(code);
})();
