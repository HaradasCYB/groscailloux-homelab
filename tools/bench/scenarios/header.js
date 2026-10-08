// tools/bench/scenarios/header.js — banc de fumée de l'en-tête (2026-10-08, repris de header_dump.js du 03/10).
// Connexion d'un compte ordinaire, puis À L'ÉCRAN : une barre d'en-tête visible, le bouton du tchat et celui de Mon
// compte (sauf télé : pas de tchat), le menu ☰ visible et libre sur toute sa largeur, et ces boutons toujours là après
// un aller-retour sur la recherche. Lecture seule : aucun réglage touché, aucune lecture lancée. Capture : /out/header-<appareil>.png.
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
    // ☰ (seul accès aux bibliothèques sur téléphone) : balayage de TOUTE sa largeur tous les 3 px (comme mfix_home.js,
    // revue mobile du 04/10 : le débordement de la barre recouvrait ☰ en partie) ; chaque point doit tomber sur ☰.
    // 2026-10-08 (revue) : introuvable ou caché = ÉCHEC dans la nouvelle interface sous 900 px (MUI md : ni logo ni
    // bibliothèques dans la barre, ☰ est le seul accès) et sur téléphone ou tablette ; avant, le contrôle sautait ou
    // réussissait sans rien vérifier, et ne sondait que le centre du bouton. À 900 px et plus, la barre moderne n'a pas
    // de ☰ (bibliothèques dans la barre, vérifié au banc le 08/10) : non exigé.
    const menu = await p.evaluate(() => {
      const vis = (el) => { const r = el.getBoundingClientRect(); const cs = getComputedStyle(el); return r.width >= 1 && r.height >= 1 && cs.visibility !== 'hidden' && cs.display !== 'none'; };
      const bar = [...document.querySelectorAll('header.MuiAppBar-root')].find(vis);
      const m = bar
        ? bar.querySelector('.MuiToolbar-root > button:first-child') || bar.querySelector('button[aria-label="Open Menu"], button[aria-label="Ouvrir le menu"]')
        : document.querySelector('.skinHeader .mainDrawerButton');
      const res = { modern: !!bar, narrow: innerWidth < 900, found: !!m, visible: false };
      if (!m || !vis(m)) return res;
      const r = m.getBoundingClientRect();
      const y = Math.round(r.top + r.height / 2);
      let ok = 0, n = 0; const wrong = new Set();
      for (let x = Math.round(r.left) + 2; x < r.right - 1; x += 3) {
        n++;
        const e = document.elementFromPoint(x, y);  // null hors de la fenêtre : compté comme recouvert
        if (e && m.contains(e)) ok++;
        else wrong.add(e ? ((e.closest('button,a,[role=button]') || e).getAttribute('aria-label') || String(e.className || e.tagName).slice(0, 40)) : 'hors écran');
      }
      return { ...res, visible: true, box: [Math.round(r.left), Math.round(r.top), Math.round(r.width), Math.round(r.height)], hit: `${ok}/${n}`, free: n > 0 && ok === n, wrong: [...wrong].slice(0, 4) };
    });
    const touch = ['phone', 'iphone', 'android', 'tablet'].includes(env.device);
    if (!tv && ((menu.modern && menu.narrow) || touch || menu.visible)) {
      c.check('menu ☰ visible et libre sur toute sa largeur', menu.found && menu.visible && menu.free, menu);
    } else {
      console.log('info   ☰ non exigé ici (barre moderne de 900 px et plus, ancienne interface sur grand écran, télé) :',
        JSON.stringify(menu));
    }
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
