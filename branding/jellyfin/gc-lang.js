// Langue d'affichage : jellyfin-web la garde **dans l'appareil** (localStorage, clé `<userId>-language`, et
// `<userId>-datetimelocale` pour le format des dates), jamais sur le serveur — `userSettings.language()` lit avec
// `enableOnServer = false`. Sans cette clé, il prend la langue de l'appareil : un PC en anglais affichait « Home »,
// « Favorites », « Ends at 11:09 PM » (2026-09-19). Ce script POSE le français quand rien n'est choisi, et ne
// touche jamais à un choix explicite du membre (clé déjà présente, même « en-US »).
// Script PUBLIC de JavaScript Injector (« Groscailloux Langue ») : il s'exécute avant que l'appli lise ses réglages.
//   - Au chargement : pour chaque compte déjà connu de l'appareil (`jellyfin_credentials`), on pose la clé si elle
//     manque — la langue est lue au démarrage, donc dès cette ouverture-ci.
//   - Première connexion d'un compte sur cet appareil (2026-10-04) : la clé est posée DÈS la réponse du serveur à la
//     connexion (`/Users/AuthenticateByName`, Quick Connect), avant que l'appli ne la traite : elle charge alors le
//     français elle-même, sans rechargement. Avant, on posait la clé après coup puis on rechargeait la page : ~10 s de
//     redémarrage de l'appli, et un rechargement parti pendant la connexion la faisait redémarrer sur l'écran de
//     connexion (vu en mise en page « legacy » sous Jellyfin 12.1).
//   - Filet (réponse illisible, autre chemin de connexion) : clé posée et UN rechargement, jamais sur l'écran de
//     connexion ni avant que les identifiants soient enregistrés dans l'appareil (garde dans sessionStorage).
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

  /* première connexion : la réponse du serveur donne le compte (User.Id) ; clé posée avant que l'appli la lise */
  var AUTH = /\/Users\/(AuthenticateByName|AuthenticateWithQuickConnect)(\?|$)/i;
  function fromAuth(text) {
    try {
      var j = typeof text === 'string' ? JSON.parse(text) : text;
      if (j && j.User && j.User.Id) apply(j.User.Id);
    } catch (e) { /* réponse illisible : le filet plus bas prend le relais */ }
  }
  // XHR (SDK de jellyfin-web) : écouteur posé dans open(), donc avant ceux du SDK, et `readystatechange` (état 4)
  // passe avant `loadend`, où le SDK lit la réponse
  var xOpen = XMLHttpRequest.prototype.open;
  XMLHttpRequest.prototype.open = function (method, url) {
    if (AUTH.test(String(url))) {
      this.addEventListener('readystatechange', function () {
        if (this.readyState !== 4 || this.status !== 200) return;
        try {
          if (this.responseType === '' || this.responseType === 'text') fromAuth(this.responseText);
          else if (this.responseType === 'json') fromAuth(this.response);
        } catch (e) { /* filet */ }
      });
    }
    return xOpen.apply(this, arguments);
  };
  // fetch (jellyfin-apiclient) : l'appli ne reçoit la réponse qu'une fois la clé posée
  var oFetch = window.fetch;
  if (typeof oFetch === 'function') {
    window.fetch = function (input) {
      var url = typeof input === 'string' ? input : (input && input.url) || '';
      var p = oFetch.apply(this, arguments);
      if (!AUTH.test(url)) return p;
      return p.then(function (res) {
        if (!res || !res.ok) return res;
        return res.clone().text().then(function (t) { fromAuth(t); return res; }, function () { return res; });
      });
    };
  }

  /* filet : la clé manque encore alors que l'appli connaît le compte → la poser et recharger une seule fois,
     hors de l'écran de connexion et une fois les identifiants enregistrés (sinon l'appli redémarrait déconnectée) */
  function credsSaved(uid) {
    try {
      var c = JSON.parse(localStorage.getItem('jellyfin_credentials') || '{}');
      return (c.Servers || []).some(function (x) { return x.UserId === uid && x.AccessToken; });
    } catch (e) { return false; }
  }
  var tries = 0;
  var t = setInterval(function () {
    if (++tries > 1200) { clearInterval(t); return; } // 20 min, puis on laisse tomber
    var api = window.ApiClient;
    var uid = api && typeof api.getCurrentUserId === 'function' ? api.getCurrentUserId() : null;
    if (!uid) return;
    if (has(uid + '-language')) { clearInterval(t); return; } // posée à la connexion, ou choix du membre
    if (/^#\/(login|selectserver|startup|wizard)/i.test(location.hash) || !credsSaved(uid)) return;
    clearInterval(t);
    if (!apply(uid)) return;
    try {
      if (sessionStorage.getItem(RELOAD_FLAG) === uid) return;
      sessionStorage.setItem(RELOAD_FLAG, uid);
    } catch (e) { return; }
    window.location.reload();
  }, 1000);
})();
