// « Groscailloux Socket unique » (05/10, déployé) : banc backups/jellyfin12-test-20261003/zz_spns/.
// Script PUBLIC de JavaScript Injector (RequiresAuthentication: false) : public.js est chargé en `defer` juste après
// main.jellyfin.bundle.js, donc avant que jellyfin-web n'exécute `window.ApiClient = …` (setLocalApiClient).
//
// Cause traitée : NotifySync 5.8.4.0 (compilation Jellyfin 12, installée le 05/10 à 08:45) ouvre son PROPRE websocket
// /socket?ApiKey=<jeton de la session> dès que `ApiClient.isWebSocketOpen()` est faux — toujours le cas en 12.x, où
// seul le SDK ouvre une socket. Même jeton = même session ; le serveur n'envoie un message de session qu'à UNE socket,
// la plus récemment active (WebSocketController.SendMessage : MaxBy(LastActivityDate)), et c'était le plus souvent
// celle de NotifySync, qui jette tout sauf LibraryChanged/UserDataChanged. GroupJoined/GroupLeft et les commandes
// SyncPlay étaient perdus : groupe créé affiché « à rejoindre », sans « Quitter ».
//
// En 12.x seulement (ApiClient.subscribe présent, branché sur le SDK par jellyfin-web) :
//   - `isWebSocketOpen()` répond vrai : NotifySync n'ouvre plus rien ;
//   - `sendMessage()` n'envoie que si l'ancienne socket existe vraiment (sinon `_webSocket.send` sur null) ;
//   - LibraryChanged/UserDataChanged reçus par le SDK sont redonnés en événement 'message' de l'ApiClient (cloche de
//     NotifySync et InPlayerEpisodePreview restent en temps réel).
// Sur 10.11 (retour arrière possible) : comportement d'origine.
(function () {
  'use strict';
  if (window.__gcOneSocket) return;
  var ST = window.__gcOneSocket = { version: 1, patched: false, bridged: false, relayed: 0 };
  var TYPES = ['LibraryChanged', 'UserDataChanged'];

  // jellyfin-web 12.x branche `subscribe` sur le SDK dès la création de l'ApiClient (« apiclientcreated »), avant
  // `window.ApiClient = …` ; l'ApiClient de 10.11 n'en a pas.
  function is12(api) { return typeof api.subscribe === 'function'; }

  function patch(api) {
    if (!api || api.__gcOneSocket || typeof api.isWebSocketOpen !== 'function') return;
    api.__gcOneSocket = true;
    var isOpen = api.isWebSocketOpen;
    var send = api.sendMessage;
    api.isWebSocketOpen = function () { return is12(this) ? true : isOpen.apply(this, arguments); };
    if (typeof send === 'function') {
      api.sendMessage = function () {
        var w = this._webSocket;
        if (!is12(this) || (w && w.readyState === 1)) return send.apply(this, arguments);
      };
    }
    ST.patched = true;
    var tries = 0;
    (function bridge() {
      if (typeof api.subscribe === 'function' && window.Events && typeof window.Events.trigger === 'function') {
        try {
          api.subscribe(TYPES, function (msg) {
            ST.relayed++;
            try { window.Events.trigger(api, 'message', [msg]); } catch (e) { /* écouteur d'une extension */ }
          });
          ST.bridged = true;
        } catch (e) { ST.error = String(e && e.message || e).slice(0, 120); }
        return;
      }
      if (++tries < 120) setTimeout(bridge, 500);
    })();
  }

  var current = window.ApiClient;
  try {
    Object.defineProperty(window, 'ApiClient', {
      configurable: true,
      enumerable: true,
      get: function () { return current; },
      set: function (v) { current = v; patch(v); }
    });
  } catch (e) {
    var n = 0;
    (function poll() { patch(window.ApiClient); if (++n < 120 && !ST.patched) setTimeout(poll, 250); })();
  }
  patch(current);
})();
