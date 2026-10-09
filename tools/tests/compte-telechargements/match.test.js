// tools/tests/compte-telechargements/match.test.js — appariement « carte de l'onglet Téléchargements de Jellyfin
// Enhanced ↔ avertissement expliqué par homelabd » (2026-10-09).
//
// Prend les fonctions pures du VRAI crates/homelabd/assets/compte/app.js (entre « gc-dl-match:start » et
// « gc-dl-match:end ») et les fait tourner sous Node, sans navigateur ni réseau. Ce qui est vérifié :
//   - une carte d'épisode (« S01E02 - … ») trouve SON élément, et seulement le sien (source, titre, épisode) ;
//   - jamais d'appariement ambigu : deux éléments possibles, deux cartes identiques, une carte de série sans
//     épisode, une autre source → rien ;
//   - film : par le titre, l'année seulement pour départager ; pack de saison : tous les épisodes, même note ;
//   - titres comparés sans accents ni ponctuation ; texte court de la télé = première phrase.
//
// Lancement : node tools/tests/compte-telechargements/match.test.js
// Contre-épreuve : MUTATE=1 (l'unicité des candidats n'est plus exigée) doit ÉCHOUER.
'use strict';
const fs = require('fs');
const path = require('path');
const assert = require('assert');

const APP = process.env.COMPTE_APP || path.join(__dirname, '../../../crates/homelabd/assets/compte/app.js');
const src = fs.readFileSync(APP, 'utf8');
const a = src.indexOf('// gc-dl-match:start');
const b = src.indexOf('// gc-dl-match:end');
assert(a > 0 && b > a, 'marqueurs gc-dl-match introuvables dans app.js');
let code = src.slice(a, b);
if (process.env.MUTATE === '1') code = code.replace('return hits.length === 1 ? hits[0] : null;', 'return hits[0] || null;');
const { dlNorm, dlParse, dlMatch, dlAssign, dlShort } = new Function(
  code + '\nreturn { dlNorm: dlNorm, dlParse: dlParse, dlMatch: dlMatch, dlAssign: dlAssign, dlShort: dlShort };'
)();

const SONARR = 'https://cdn.example.invalid/svg/sonarr.svg Sonarr';
const RADARR = 'https://cdn.example.invalid/svg/radarr-light-hybrid-light.svg Radarr';
const note = (o) => Object.assign({ badge: 'Déjà disponible', text: 'Cet épisode est déjà disponible. Suite.' }, o);
const card = (title, subtitle, icon, extra) => dlParse(Object.assign({ title, subtitle, icon }, extra || {}));

let n = 0;
function t(name, fn) { fn(); n += 1; console.log('ok - ' + name); }

t('lecture des cartes', () => {
  assert.deepStrictEqual(card('Carrie', 'S01E02 - FOREIGN LANGUAGE', SONARR),
    { source: 'Sonarr', title: 'Carrie', season: 1, episode: 2, year: null, pack: null });
  assert.strictEqual(card('Le Film', '', RADARR).source, 'Radarr');
  assert.strictEqual(card('Le Film', '2024', RADARR).year, 2024);
  assert.strictEqual(card('X', 'S01E02', '').source, '');
  assert.deepStrictEqual(card('Show', 'Saison 2 (10 épisodes)', SONARR, { pack: true, range: 'E01-E10' }).pack,
    { season: 2, count: 10, from: 1, to: 10 });
  assert.strictEqual(card('Show', 'Saison 2', SONARR, { pack: true, range: '' }).pack, null);
});

t('carte d\'épisode : le bon élément', () => {
  const items = [
    note({ source: 'Sonarr', title: 'Carrie', episode: 'S01E02' }),
    note({ source: 'Sonarr', title: 'Carrie', episode: 'S01E03', badge: 'Non reconnu' }),
    note({ source: 'Radarr', title: 'Carrie', episode: null, badge: 'Sans source' }),
  ];
  assert.strictEqual(dlMatch(items, card('Carrie', 'S01E02 - FOREIGN LANGUAGE', SONARR)).badge, 'Déjà disponible');
  assert.strictEqual(dlMatch(items, card('Carrie', 'S01E03 - x', SONARR)).badge, 'Non reconnu');
  assert.strictEqual(dlMatch(items, card('Carrie', 'S01E04 - x', SONARR)), null);
  assert.strictEqual(dlMatch(items, card('Carrie', '', RADARR)).badge, 'Sans source');
  // E1050 (numérotation absolue d'un animé) et zéros non significatifs
  assert.strictEqual(dlMatch([note({ source: 'Sonarr', title: 'One Piece', episode: 'S01E1050' })],
    card('One Piece', 'S01E1050 - ...', SONARR)).episode, 'S01E1050');
});

t('jamais d\'appariement ambigu', () => {
  const dup = [note({ source: 'Sonarr', title: 'Show', episode: 'S01E02' }), note({ source: 'Sonarr', title: 'SHOW!', episode: 'S01E02', badge: 'Pas de vidéo' })];
  assert.strictEqual(dlMatch(dup, card('Show', 'S01E02 - x', SONARR)), null);
  // carte de série sans épisode : on ne devine pas
  assert.strictEqual(dlMatch([note({ source: 'Sonarr', title: 'Show', episode: null })], card('Show', '', SONARR)), null);
  // autre source
  assert.strictEqual(dlMatch([note({ source: 'Radarr', title: 'Show', episode: null })], card('Show', '', SONARR)), null);
  // deux films du même titre : l'année départage, sinon rien
  const films = [note({ source: 'Radarr', title: 'Dune', year: 1984 }), note({ source: 'Radarr', title: 'Dune', year: 2021, badge: 'Échec' })];
  assert.strictEqual(dlMatch(films, card('Dune', '', RADARR)), null);
  assert.strictEqual(dlMatch(films, card('Dune', '2021', RADARR)).badge, 'Échec');
  // deux cartes identiques à l'écran (VPS et seedbox) : aucune des deux
  const one = [note({ source: 'Sonarr', title: 'Show', episode: 'S01E02' })];
  const c = card('Show', 'S01E02 - x', SONARR);
  assert.deepStrictEqual(dlAssign(one, [c, c]), [null, null]);
  assert.strictEqual(dlAssign(one, [c, card('Show', 'S01E03 - x', SONARR)])[0].episode, 'S01E02');
});

t('pack de saison', () => {
  const eps = [1, 2, 3].map((e) => note({ source: 'Sonarr', title: 'Show', episode: 'S02E0' + e, badge: 'Pack incomplet', detail: 'd' }));
  const pack = card('Show', 'Saison 2 (3 épisodes)', SONARR, { pack: true, range: 'E01-E03' });
  const m = dlMatch(eps, pack);
  assert.strictEqual(m.badge, 'Pack incomplet');
  assert.strictEqual(m.detail, 'd');
  assert.strictEqual(dlMatch(eps.slice(0, 2), pack), null); // un épisode du pack n'est pas expliqué
  const mixed = eps.slice(0, 2).concat([note({ source: 'Sonarr', title: 'Show', episode: 'S02E03', badge: 'Échec' })]);
  assert.strictEqual(dlMatch(mixed, pack), null);
  const otherDetail = eps.slice(0, 2).concat([Object.assign({}, eps[2], { detail: 'autre' })]);
  assert.strictEqual(dlMatch(otherDetail, pack).detail, '');
  assert.strictEqual(dlMatch(eps, card('Show', 'Saison 2 (3 épisodes)', SONARR, { pack: true, range: 'E04-E06' })), null);
});

t('titres sans accents ni ponctuation, texte court', () => {
  assert.strictEqual(dlNorm('  Élite : saison spéciale! '), 'elite saison speciale');
  assert.strictEqual(dlMatch([note({ source: 'Sonarr', title: 'Élite', episode: 'S01E01' })], card('Elite', 'S01E01 - x', SONARR)).title, 'Élite');
  assert.strictEqual(dlShort('Première phrase : suite. Deuxième phrase.'), 'Première phrase : suite.');
  assert.strictEqual(dlShort('Une seule'), 'Une seule');
});

console.log(`${n} tests ok`);
