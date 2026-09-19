// Langue d'affichage : jellyfin-web la garde **dans l'appareil** (localStorage, clé `<userId>-language`, et
// `<userId>-datetimelocale` pour le format des dates), jamais sur le serveur — `userSettings.language()` lit avec
// `enableOnServer = false`. Sans cette clé, il prend la langue de l'appareil : un PC en anglais affichait « Home »,
// « Favorites », « Ends at 11:09 PM » (2026-09-19). Ce script POSE le français quand rien n'est choisi, et ne
// touche jamais à un choix explicite du membre (clé déjà présente, même « en-US »).
// Script PUBLIC de JavaScript Injector (« Groscailloux Langue ») : il s'exécute avant que l'appli lise ses réglages.
//   - Au chargement : pour chaque compte déjà connu de l'appareil (`jellyfin_credentials`), on pose la clé si elle
//     manque — la langue est lue au démarrage, donc dès cette ouverture-ci.
//   - Première connexion d'un compte sur cet appareil : le compte n'est connu qu'après l'écran de connexion ; on
//     pose alors la clé et on recharge la page une seule fois (garde dans sessionStorage).
// Jamais de window.confirm/alert/prompt dans les scripts injectés.
(function () {
  'use strict';
  if (window.__gcLangDone) return;
  window.__gcLangDone = true;
  var LANG = 'fr';
  var RELOAD_FLAG = 'gc-lang-reloaded';

  function has(key) {
    try { var v = localStorage.getItem(key); return v !== null && v !== ''; } catch (e) { return true; }
  }
  /* pose la langue pour un compte ; renvoie true si quelque chose a été écrit. Une langue déjà choisie (même
     sans format de date) = choix du membre : on ne touche à rien, pas même au format de date. */
  function apply(userId) {
    if (!userId || has(userId + '-language')) return false;
    try {
      localStorage.setItem(userId + '-language', LANG);
      if (!has(userId + '-datetimelocale')) localStorage.setItem(userId + '-datetimelocale', LANG);
      return true;
    } catch (e) { return false; /* stockage indisponible : on laisse la langue de l'appareil */ }
  }

  /* comptes déjà connus de cet appareil */
  try {
    var creds = JSON.parse(localStorage.getItem('jellyfin_credentials') || '{}');
    var servers = (creds && creds.Servers) || [];
    for (var i = 0; i < servers.length; i++) apply(servers[i].UserId);
  } catch (e) { /* pas de session mémorisée */ }

  /* première connexion sur cet appareil : dès que l'appli connaît le compte, poser la clé puis recharger une fois */
  var tries = 0;
  var t = setInterval(function () {
    if (++tries > 600) { clearInterval(t); return; } // 20 min, puis on laisse tomber
    var api = window.ApiClient;
    var uid = api && typeof api.getCurrentUserId === 'function' ? api.getCurrentUserId() : null;
    if (!uid) return;
    clearInterval(t);
    if (!apply(uid)) return;
    try {
      if (sessionStorage.getItem(RELOAD_FLAG) === uid) return;
      sessionStorage.setItem(RELOAD_FLAG, uid);
    } catch (e) { return; }
    window.location.reload();
  }, 2000);
})();
