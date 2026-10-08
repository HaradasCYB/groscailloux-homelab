// tools/bench/scenarios/candidats.js — banc du lanceur lui-même (2026-10-08, lot 4) : l'injection de candidats marche-t-elle
// encore (après une montée de Jellyfin, de JavaScript Injector ou d'ElegantFin) ?
// Une page reçoit des candidats MARQUEURS écrits par le scénario dans le conteneur (un gc-*.js ajouté au private.js,
// compte.js = le vrai app.js de Mon compte + un marqueur, un CSS = le vrai calque + une variable) ; une seconde page,
// sans candidats, ne doit en voir aucun. Rien n'est écrit côté serveur, aucune lecture lancée.
// Usage : tools/bench/bench.sh tools/bench/scenarios/candidats.js desktop [phone …]
'use strict';
const fs = require('fs');
const path = require('path');
const { env, launch, newPage, login, waitOnScreen, checker } = require('/bench/bench.js');

const DIR = '/tmp/candidats-banc';
const MARK = 'gc-banc-marqueur';

function writeCandidates() {
  fs.mkdirSync(DIR, { recursive: true });
  fs.writeFileSync(path.join(DIR, 'gc-zz-marqueur.js'), 'window.__gcBancPrivate = 1;\n');
  fs.writeFileSync(path.join(DIR, 'compte.js'),
    fs.readFileSync('/repo/crates/homelabd/assets/compte/app.js', 'utf8') + '\n;window.__gcBancCompte = 1;\n');
  fs.writeFileSync(path.join(DIR, 'groscailloux-tv.css'),
    fs.readFileSync('/repo/branding/jellyfin/groscailloux-tv.css', 'utf8') + `\n:root { --${MARK}: 1; }\n`);
}

async function markers(p) {
  return p.evaluate((m) => ({
    private: window.__gcBancPrivate === 1,
    compte: window.__gcBancCompte === 1,
    css: getComputedStyle(document.documentElement).getPropertyValue('--' + m).trim() === '1',
  }), MARK);
}

(async () => {
  const c = checker('candidats');
  const b = await launch();
  let code = 1;
  try {
    // 1. avec candidats
    writeCandidates();
    env.candidateDir = DIR;
    const p = await newPage(b);
    c.check('candidats trouvés par le banc', p.gcCandidates === true);
    await login(p);
    // Mon compte est chargé par un script de l'injecteur après l'accueil : son bouton fait foi
    c.check('bouton Mon compte à l\'écran (app.js candidat)', await waitOnScreen(p, '.gc-acc-btn', 40000));
    const m = await markers(p);
    c.check('gc-*.js candidat exécuté (ajouté au private.js)', m.private, m);
    c.check('compte.js candidat servi à la place de /gc-compte/app.js', m.compte, m);
    c.check('CSS candidat appliqué à la place du calque', m.css, m);
    await p.close();
    // 2. sans candidats : la production telle quelle (contexte neuf : pas de session ni de cache de la 1re page)
    env.candidateDir = '';
    const ctx = b.createBrowserContext ? await b.createBrowserContext() : await b.createIncognitoBrowserContext();
    const q = await newPage(ctx);
    await login(q);
    await waitOnScreen(q, '.gc-acc-btn', 40000);
    const n = await markers(q);
    c.check('sans candidats · aucun marqueur (ce que sert la production)', !n.private && !n.compte && !n.css, n);
    code = c.done({ avec: m, sans: n });
  } catch (e) {
    c.check('déroulé du banc', false, String(e.message).slice(0, 200));
    code = c.done();
  } finally {
    await b.close();
  }
  process.exit(code);
})();
