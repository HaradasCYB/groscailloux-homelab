// tools/bench/scenarios/duree.js — filtre « Durée » des bibliothèques de films (gc-duree.js), 2026-10-27.
// Lecture seule (le compte de banc ne pose qu'une progression sur ITEM, un film, pour le filtre combiné). À L'ÉCRAN :
//   - menu Filtres de la bibliothèque LIB (Films) : bloc « Durée » visible, juste avant « Réinitialiser les filtres » ;
//   - « Moins de 1 h 30 » puis « Moins de 2 h » : total affiché = nombre attendu (calculé à part sur la liste complète),
//     chaque vignette affichée sous la durée ;
//   - combiné au filtre « Continuer à regarder » (IsResumable) : seul ITEM reste (s'il fait moins de 2 h) ;
//   - pagination : page 2 = la suite, sans relire la liste complète ;
//   - « Réinitialiser les filtres » : total complet, cases décochées ;
//   - bibliothèque SERIES : pas de bloc « Durée ».
// Usage : tools/bench/bench.sh --candidate DIR --item <film> --env LIB=<films> --env SERIES=<séries> \
//           tools/bench/scenarios/duree.js desktop phone
'use strict';
const { env, launch, newPage, login, onScreen, waitOnScreen, checker, sleep } = require('/bench/bench.js');

const LIB = process.env.LIB, SERIES = process.env.SERIES;

async function openFilters(p) {
  await p.evaluate(() => { const pop = document.querySelector('#filter-popover'); if (pop) document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' })); });
  const ok = await p.evaluate(() => {
    for (const b of document.querySelectorAll('.MuiButtonGroup-root button')) {
      const k = Object.keys(b).find((x) => x.startsWith('__reactFiber$')); let f = b[k];
      for (let i = 0; f && i < 6; i++, f = f.return) { const pr = f.memoizedProps; if (pr && pr.setLibraryViewSettings && 'hasFilters' in pr && 'itemType' in pr) { b.click(); return true; } }
    }
    return false;
  });
  await sleep(1200);
  return ok;
}
async function closeFilters(p) { await p.keyboard.press('Escape'); await sleep(800); }
async function state(p) {
  return p.evaluate(() => {
    const vis = (e) => { const r = e.getBoundingClientRect(); return r.width > 0 && r.height > 0; };
    const cards = [...document.querySelectorAll('.card[data-id]')].filter(vis).map((c) => c.getAttribute('data-id'));
    const label = [...document.querySelectorAll('.MuiToolbar-root .MuiChip-label, .MuiToolbar-root .MuiChip-root')].map((e) => e.textContent).join(' | ');
    return { cards, label, served: (window.__gcDuree || {}).served, full: (window.__gcDuree || {}).full };
  });
}
async function pick(p, minutes) {
  // ouvre le bloc et coche (ou décoche) une durée
  await p.evaluate(() => { const b = document.querySelector('#gc-dur-header'); if (b && b.getAttribute('aria-expanded') !== 'true') b.click(); });
  await sleep(400);
  await p.evaluate((m) => { document.querySelector(`.gc-dur-body input[value="${m}"]`).click(); }, String(minutes));
  await sleep(3500);
}

(async () => {
  const c = checker('duree');
  const b = await launch();
  let code = 1;
  try {
    if (!LIB) throw new Error('--env LIB=<bibliothèque de films> attendu');
    const p = await newPage(b);
    await login(p);
    await p.waitForFunction(() => window.ApiClient && ApiClient.accessToken && ApiClient.accessToken(), { timeout: 60000 });
    // référence : toute la bibliothèque, durées comprises (une lecture à part, hors du script)
    const ref = await p.evaluate(async (lib, item) => {
      const a = window.ApiClient; const uid = a.getCurrentUserId();
      if (item) await a.ajax({ type: 'POST', url: a.getUrl(`UserItems/${item}/UserData`, { userId: uid }), data: JSON.stringify({ PlaybackPositionTicks: 6000000000, LastPlayedDate: new Date().toISOString() }), contentType: 'application/json' });
      const r = await a.getJSON(a.getUrl('Items', { userId: uid, parentId: lib, recursive: true, includeItemTypes: 'Movie', enableImages: false }));
      const rt = {}; r.Items.forEach((i) => { rt[i.Id] = i.RunTimeTicks || 0; });
      return { total: r.Items.length, rt };
    }, LIB, env.item);
    const lt = (m) => Object.values(ref.rt).filter((t) => t && t < m * 600000000).length;
    console.log(`info   référence : ${ref.total} films, ${lt(120)} sous 2 h, ${lt(90)} sous 1 h 30`);
    await p.evaluate((lib) => { location.hash = `#/movies?topParentId=${lib}&collectionType=movies&tab=0`; }, LIB);
    await waitOnScreen(p, '.MuiButtonGroup-root button', 40000); await waitOnScreen(p, '.card[data-id]', 40000); await sleep(2500);
    const s0 = await state(p);
    c.check('bibliothèque affichée, total complet', s0.label.includes(String(ref.total)), s0.label);

    c.check('menu Filtres ouvert', await openFilters(p));
    const pos = await p.evaluate(() => {
      const box = document.querySelector('#filter-popover .gc-dur'); if (!box) return null;
      const h = box.querySelector('#gc-dur-header').getBoundingClientRect();
      const next = box.nextElementSibling;
      return { visible: h.width > 0 && h.height > 0, beforeReset: !!next && next.tagName === 'BUTTON', text: box.textContent };
    });
    c.check('bloc « Durée » à l\'écran, juste avant « Réinitialiser les filtres »', !!(pos && pos.visible && pos.beforeReset && /Durée/.test(pos.text)), pos);
    await p.screenshot({ path: `/out/duree-menu-${env.benchDevice.replace(/[^a-z0-9-]/gi, '_')}.png` });

    await pick(p, 90);
    let s = await state(p);
    c.check(`moins de 1 h 30 : total ${lt(90)}`, s.label.includes(String(lt(90))) && !s.label.includes(String(ref.total)), s.label);
    c.check('chaque vignette affichée fait moins de 1 h 30', s.cards.length > 0 && s.cards.every((id) => ref.rt[id] && ref.rt[id] < 90 * 600000000), s.cards.filter((id) => !(ref.rt[id] < 90 * 600000000)).slice(0, 5));
    const badge = await p.evaluate(() => { const d = document.querySelector('.MuiButtonGroup-root .MuiBadge-dot'); return !!d && !d.classList.contains('MuiBadge-invisible'); });
    c.check('pastille du bouton Filtres allumée', badge);

    await pick(p, 120);
    s = await state(p);
    c.check(`moins de 2 h (remplace 1 h 30) : total ${lt(120)}`, s.label.includes(String(lt(120))), s.label);
    const checked = await p.evaluate(() => [...document.querySelectorAll('.gc-dur-body input')].map((i) => i.value + ':' + i.checked).join(','));
    c.check('un seul choix coché', checked === '120:true,90:false', checked);
    const full1 = s.full;

    // combiné : « Continuer à regarder » (IsResumable) du menu Filtres natif
    await p.evaluate(() => { const b = document.querySelector('#filtersStatus-header'); if (b) b.click(); });
    await sleep(800);
    const resumable = await p.evaluate(() => { const l = [...document.querySelectorAll('#filter-popover label')].find((x) => /Continuer|Reprendre/i.test(x.textContent)); if (!l) return false; l.querySelector('input').click(); return true; });
    await sleep(3500);
    s = await state(p);
    const expectOne = env.item && ref.rt[env.item] && ref.rt[env.item] < 120 * 600000000;
    if (resumable && env.item) c.check('combiné avec « Continuer à regarder »', expectOne ? (s.cards.length === 1 && s.cards[0] === env.item) : s.cards.length === 0, s.cards);
    else console.log('info   filtre combiné non testé (pas d\'option ou pas d\'ITEM)');
    if (resumable) { await p.evaluate(() => { const l = [...document.querySelectorAll('#filter-popover label')].find((x) => /Continuer|Reprendre/i.test(x.textContent)); l.querySelector('input').click(); }); await sleep(3000); }
    await closeFilters(p);

    // pagination : 100 par page dans jellyfin-web ; la page 2 vient de la liste gardée
    s = await state(p);
    if (lt(120) > 100) {
      const next = await p.evaluate(() => { const bs = [...document.querySelectorAll('.MuiToolbar-root button')].filter((b) => /next|suivant/i.test((b.getAttribute('title') || '') + (b.getAttribute('aria-label') || '') + b.innerHTML)); const b = bs.pop(); if (!b || b.disabled) return false; b.click(); return true; });
      await sleep(3500);
      const s2 = await state(p);
      c.check('page 2 : la suite de la liste filtrée', next && s2.label.includes(String(lt(120))) && s2.cards.length === lt(120) - 100 && s2.cards.every((id) => ref.rt[id] < 120 * 600000000), { label: s2.label, n: s2.cards.length });
      c.check('page 2 sans relire la liste complète', s2.full === s.full, { avant: s.full, apres: s2.full });
    } else console.log('info   une seule page : pagination non testée');
    console.log(`info   pages servies ${s.served}, listes complètes lues ${s.full} (dont ${full1} avant le filtre combiné)`);

    // réinitialisation native
    await openFilters(p);
    await p.evaluate(() => { const b = [...document.querySelectorAll('#filter-popover .MuiPopover-paper > button')].pop(); if (b) b.click(); });
    await sleep(3500);
    const after = await p.evaluate(() => [...document.querySelectorAll('.gc-dur-body input')].map((i) => i.checked));
    await closeFilters(p);
    s = await state(p);
    c.check('« Réinitialiser les filtres » : total complet', s.label.includes(String(ref.total)), s.label);
    c.check('« Réinitialiser les filtres » : cases décochées', after.every((x) => !x), after);

    if (SERIES) {
      await p.evaluate((lib) => { location.hash = `#/tv?topParentId=${lib}&collectionType=tvshows&tab=0`; }, SERIES);
      await waitOnScreen(p, '.card[data-id]', 40000); await sleep(2000);
      await openFilters(p);
      c.check('séries : pas de bloc « Durée »', !(await onScreen(p, '#filter-popover .gc-dur')) && (await onScreen(p, '#filter-popover .MuiAccordion-root')));
      await closeFilters(p);
    }
    if (p.gcErrors.length) console.log('erreurs JavaScript de la page :', p.gcErrors.slice(0, 5));
    c.check('aucune erreur JavaScript', p.gcErrors.length === 0, p.gcErrors.slice(0, 3));
    code = c.done({ errors: p.gcErrors.slice(0, 10) });
  } catch (e) {
    c.check('déroulé du banc', false, String(e.message).slice(0, 200));
    code = c.done();
  } finally {
    await b.close();
  }
  process.exit(code);
})();
