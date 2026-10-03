// SyncPlay après un saut (2026-10-03) : le groupe restait sur la roue de chargement, il fallait faire pause puis
// lecture. Journaux du 03/10, deux groupes, tous les membres sur Jellyfin Desktop 1.0.0 (lecteur mpv) :
//   - le serveur met le groupe « en attente » et attend que chacun réponde « prêt » ;
//   - jellyfin-web (PlaybackCore.scheduleSeek) relance la lecture, saute, puis attend l'événement « playing » du
//     lecteur pour se mettre en pause et répondre « prêt ». Avec mpv, cet événement arrive TROP TÔT (dès la reprise,
//     avant le saut : « prêt » envoyé en 60 ms avec l'ANCIENNE position, le serveur programme une pause « dans
//     1 392 s ») ou JAMAIS (au bout de 30 s le client ressaute sans répondre : groupe bloqué).
// Correctif, seulement dans Jellyfin Desktop : pause, PUIS saut, attente que le lecteur ait rejoint la cible (5 s
// près, 12 s au plus), puis « prêt » en pause à la cible — le serveur relance tout le groupe ensemble.
// Le vrai correctif est côté serveur dans Jellyfin 12.1 (PR #17797 et limite d'attente d'un membre) : ce script est
// provisoire. Il ne fait RIEN si la structure de jellyfin-web n'est pas celle attendue (autre version).
// Accès à SyncPlay : jellyfin-web n'expose rien en global ; le module central (« { Helper, Manager, PlayerFactory,
// Players } ») est retrouvé par le registre webpack (self.webpackChunk), d'après son code et non son numéro, qui
// change à chaque version. webpack ne crée un module qu'une fois : c'est le même Manager que celui de jellyfin-web.
// Essai sur navigateur : localStorage « gc-syncplay-force » = « 1 » applique le correctif hors Jellyfin Desktop.
// État lisible dans la console : window.__gcSyncPlay. Déposé dans JavaScript Injector (« Groscailloux SyncPlay »)
// par scripts/jellyfin-js-apply.py.
(function () {
  'use strict';

  var VERSION = 1;
  var prev = window.__gcSyncPlay;
  if (prev && prev.version >= VERSION) return;
  var ST = window.__gcSyncPlay = { version: VERSION, patched: false, reason: 'recherche', seeks: 0, last: null };

  var TICKS_PER_MS = 10000;
  // Saut terminé quand le lecteur est à 5 s de la cible : mpv peut s'arrêter sur l'image clé d'avant. Le « prêt »
  // annonce alors la CIBLE, pas la position lue : le serveur n'accepte un « prêt » en pause qu'à 500 ms près
  // (MaxPlaybackOffset) et renverrait sinon un saut, que mpv rejouerait à l'identique — boucle sans fin (16
  // « seeking to wrong position, correcting » en 3 jours, avant ce script).
  var ARRIVED_MS = 5000;
  var WAIT_MS = 12000; // au-delà, « prêt » quand même : jamais de groupe bloqué
  var POLL_MS = 250;
  var SEARCH_EVERY_MS = 2000;
  var SEARCH_FOR_MS = 5 * 60 * 1000;

  function forced() {
    try { return localStorage.getItem('gc-syncplay-force') === '1'; } catch (e) { return false; }
  }
  function desktop() {
    try {
      var ns = window.NativeShell;
      var name = ns && ns.AppHost && typeof ns.AppHost.appName === 'function' ? String(ns.AppHost.appName() || '') : '';
      return /Jellyfin (Desktop|Media Player)/i.test(name) || !!window.jmpInfo;
    } catch (e) { return false; }
  }
  function active() { return forced() || desktop(); }

  // __webpack_require__ par le registre des morceaux : un morceau vide dont la fonction d'exécution le reçoit
  var REQ = null;
  function webpackRequire() {
    if (REQ) return REQ;
    var chunks = window.webpackChunk;
    if (!chunks || typeof chunks.push !== 'function') return null;
    try {
      chunks.push([['gc-syncplay-' + VERSION + '-' + Date.now()], {}, function (r) { REQ = r; }]);
    } catch (e) { return null; }
    return REQ;
  }

  // module central de SyncPlay, déjà chargé par jellyfin-web : { Helper, Manager, PlayerFactory, Players }
  var CORE_ID = null;
  function findCore(req) {
    if (!req || !req.m) return null;
    var ids = CORE_ID ? [CORE_ID] : Object.keys(req.m);
    for (var i = 0; i < ids.length; i++) {
      var f = req.m[ids[i]];
      if (typeof f !== 'function') continue;
      var src;
      try { src = Function.prototype.toString.call(f); } catch (e) { continue; }
      if (src.length > 4000 || src.indexOf('Manager:') < 0 || src.indexOf('PlayerFactory:') < 0 || src.indexOf('Helper:') < 0) continue;
      var mod;
      try { mod = req(ids[i]); } catch (e) { continue; }
      var core = mod && (mod.default || mod);
      if (core && core.Manager && typeof core.Manager.getPlaybackCore === 'function' && core.Helper) {
        CORE_ID = ids[i];
        return core;
      }
    }
    return null;
  }

  function currentMs(wrapper) {
    try {
      if (!wrapper) return Promise.resolve(NaN);
      if (typeof wrapper.currentTimeAsync === 'function') {
        return Promise.resolve(wrapper.currentTimeAsync()).then(Number, function () { return NaN; });
      }
      return Promise.resolve(Number(wrapper.currentTime()));
    } catch (e) { return Promise.resolve(NaN); }
  }

  // « prêt », en pause, à la position donnée (même requête que PlaybackCore.sendBufferingRequest)
  function readyAt(core, ticks) {
    try {
      var api = core.manager.getApiClient();
      var item = core.manager.getQueueCore().getCurrentPlaylistItemId();
      var when = core.timeSyncCore.localDateToRemote(new Date());
      api.requestSyncPlayReady({ When: when.toISOString(), PositionTicks: Math.round(ticks), IsPlaying: false, PlaylistItemId: item });
      return true;
    } catch (e) { return false; }
  }

  function patch(pc) {
    if (pc.__gcSyncPlay >= VERSION) return true;
    var needed = ['scheduleSeek', 'sendBufferingRequest', 'clearScheduledCommand', 'localPause', 'localSeek'];
    for (var i = 0; i < needed.length; i++) {
      if (typeof pc[needed[i]] !== 'function') { ST.reason = 'méthode absente : ' + needed[i]; return false; }
    }
    if (!pc.timeSyncCore || typeof pc.timeSyncCore.remoteDateToLocal !== 'function' || !pc.manager) {
      ST.reason = 'horloge ou gestionnaire absents'; return false;
    }
    var original = pc.scheduleSeek;
    pc.scheduleSeek = function (playAtTime, positionTicks) {
      if (!active()) return original.apply(this, arguments);
      var core = this;
      core.clearScheduledCommand();
      var token = (core.__gcSeekToken || 0) + 1;
      core.__gcSeekToken = token;
      var run = function () {
        if (core.__gcSeekToken !== token) return;
        var t0 = Date.now();
        // pause PUIS saut : aucune reprise, donc aucun « playing » prématuré avec l'ancienne position
        core.localPause();
        core.localSeek(positionTicks);
        (function poll() {
          if (core.__gcSeekToken !== token) return; // un saut plus récent a pris la main
          var wrapper = core.manager.getPlayerWrapper && core.manager.getPlayerWrapper();
          currentMs(wrapper).then(function (ms) {
            if (core.__gcSeekToken !== token) return;
            var near = isFinite(ms) && Math.abs(ms * TICKS_PER_MS - positionTicks) <= ARRIVED_MS * TICKS_PER_MS;
            if (!near && Date.now() - t0 < WAIT_MS) { setTimeout(poll, POLL_MS); return; }
            ST.seeks++;
            ST.last = { at: new Date().toISOString(), waitedMs: Date.now() - t0, atTarget: near,
                        offsetMs: isFinite(ms) ? Math.round(ms - positionTicks / TICKS_PER_MS) : null };
            console.debug('[gc-syncplay] prêt après saut', ST.last);
            if (!readyAt(core, positionTicks)) core.sendBufferingRequest(false);
          });
        })();
      };
      var now = new Date();
      var at = core.timeSyncCore.remoteDateToLocal(playAtTime);
      if (at > now) core.scheduledCommandTimeout = setTimeout(run, at - now);
      else run();
    };
    pc.__gcSyncPlay = VERSION;
    return true;
  }

  var started = Date.now();
  (function search() {
    var core = findCore(webpackRequire());
    var pc = null;
    try { pc = core && core.Manager.getPlaybackCore(); } catch (e) { pc = null; }
    if (pc && patch(pc)) {
      ST.patched = true;
      ST.reason = active() ? 'actif' : 'en place (inactif hors Jellyfin Desktop)';
      return;
    }
    if (Date.now() - started > SEARCH_FOR_MS) { ST.reason = ST.reason === 'recherche' ? 'SyncPlay introuvable' : ST.reason; return; }
    setTimeout(search, SEARCH_EVERY_MS);
  })();
})();
