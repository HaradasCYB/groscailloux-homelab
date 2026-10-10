// SyncPlay après un saut (2026-10-03) : le groupe restait sur la roue de chargement, il fallait faire pause puis
// lecture. Journaux du 03/10, deux groupes, tous les membres sur Jellyfin Desktop 1.0.0 (lecteur mpv) :
//   - le serveur met le groupe « en attente » et attend que chacun réponde « prêt » ;
//   - jellyfin-web (PlaybackCore.scheduleSeek) relance la lecture, saute, puis attend l'événement « playing » du
//     lecteur pour se mettre en pause et répondre « prêt ». Avec mpv, cet événement arrive TROP TÔT (dès la reprise,
//     avant le saut : « prêt » envoyé en 60 ms avec l'ANCIENNE position, le serveur programme une pause « dans
//     1 392 s ») ou JAMAIS (au bout de 30 s le client ressaute sans répondre : groupe bloqué).
// Correctif, seulement dans Jellyfin Desktop : pause, PUIS saut, attente que le lecteur ait rejoint la cible (5 s
// près, WAIT_MS au plus), puis « prêt » en pause à la cible — le serveur relance tout le groupe ensemble.
// Le vrai correctif est côté serveur dans Jellyfin 12.1 (PR #17797 et limite d'attente d'un membre) : ce script est
// provisoire. Il ne fait RIEN si la structure de jellyfin-web n'est pas celle attendue (autre version).
// Accès à SyncPlay : jellyfin-web n'expose rien en global ; le module central (« { Helper, Manager, PlayerFactory,
// Players } ») est retrouvé par le registre webpack (self.webpackChunk), d'après son code et non son numéro, qui
// change à chaque version. webpack ne crée un module qu'une fois : c'est le même Manager que celui de jellyfin-web.
// VERSION 2 (2026-10-10) : séance du 10/10 à trois, dont un membre sur Jellyfin Desktop 2.0.0-dev (lecteur web de
// Chromium, pas mpv) : après chaque saut, son « prêt » n'arrivait qu'au bout des 12 s d'attente maximale, tout le
// groupe attendait. Le banc tools/bench/scenarios/syncplay.js a montré deux défauts sur le lecteur web :
//   - sa position annonce la cible dès le DÉBUT du saut (Chrome émet timeupdate en lançant le saut) : « prêt » en
//     0,3 s, image pas encore là, puis « en chargement » 3 s plus tard et nouvelle attente de tout le groupe ;
//   - un lecteur rechargé (position 0) n'arrive jamais : attente maximale à chaque saut.
// Désormais, sur le lecteur web, l'arrivée se lit sur l'élément <video> (saut fini, assez de données pour jouer, à
// la cible) ; la position du lecteur ne sert plus qu'à mpv (Jellyfin Desktop 1.x, currentTimeAsync), validé en
// séance. Attente maximale : 10 s pour le lecteur web, 5 s pour mpv (au lieu de 12 s ; choix au banc, voir WAIT_MS),
// et chaque attente maximale atteinte est envoyée au journal client du serveur (jellyfin/config/log/upload_*.log,
// lignes « gc-syncplay … ») pour comprendre l'appareil en cause.
// Essai sur navigateur : localStorage « gc-syncplay-force » = « 1 » applique le correctif hors Jellyfin Desktop ; le
// banc règle alors window.__gcSyncPlayTest ({ waitMs, noArrive }), lu à chaque saut, jamais sans ce drapeau.
// État lisible dans la console : window.__gcSyncPlay. Déposé dans JavaScript Injector (« Groscailloux SyncPlay »)
// par scripts/jellyfin-js-apply.py.
(function () {
  'use strict';

  var VERSION = 2;
  var prev = window.__gcSyncPlay;
  if (prev && prev.version >= VERSION) return;
  var ST = window.__gcSyncPlay = { version: VERSION, patched: false, reason: 'recherche', seeks: 0, last: null };

  var TICKS_PER_MS = 10000;
  // Saut terminé quand le lecteur est à 5 s de la cible : mpv peut s'arrêter sur l'image clé d'avant. Le « prêt »
  // annonce alors la CIBLE, pas la position lue : le serveur n'accepte un « prêt » en pause qu'à 500 ms près
  // (MaxPlaybackOffset) et renverrait sinon un saut, que mpv rejouerait à l'identique — boucle sans fin (16
  // « seeking to wrong position, correcting » en 3 jours, avant ce script).
  var ARRIVED_MS = 5000;
  // Attente maximale : au-delà, « prêt » quand même, jamais de groupe bloqué (12 s jusqu'au 10/10). Banc du 10/10
  // (tools/bench/scenarios/syncplay.js, backups/bench/20261010/syncplay/) : couper trop tôt coûte plus cher
  // qu'attendre — à 5 s, un lecteur web encore en chargement a perdu son saut (10 min de décalage, jamais rattrapé) ;
  // à 3 s, synchro retrouvée seulement après 11 s et une nouvelle attente ; à 8 s, arrivée propre à 7,3 s. Lecteur web
  // en HLS, saut lointain dans un fichier froid de la seedbox : 1,4 à 8,9 s (ffmpeg relancé). mpv (séances réelles du
  // 04 au 10/10) : 0,3 à 0,6 s, jamais plus.
  var WAIT_MS = 10000;       // lecteur web (navigateur, Jellyfin Desktop 2.x)
  var WAIT_NATIVE_MS = 5000; // mpv (Jellyfin Desktop 1.x)
  var POLL_MS = 250;
  var REPORTS_MAX = 5; // attentes maximales envoyées au journal client, par page
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
  function bench() {
    try { return forced() && window.__gcSyncPlayTest || null; } catch (e) { return null; }
  }

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

  // lecteur web (navigateurs, Jellyfin Desktop 2.x) : l'élément <video> dit lui-même si le saut est fini
  function videoState() {
    try {
      var v = document.querySelector('video.htmlvideoplayer');
      if (!v) return null;
      return { ms: v.currentTime * 1000, seeking: !!v.seeking, ready: v.readyState, paused: !!v.paused };
    } catch (e) { return null; }
  }

  function appName() {
    try {
      var ah = window.NativeShell && window.NativeShell.AppHost;
      return ah ? String(ah.appName() || '') + ' ' + String(ah.appVersion ? ah.appVersion() || '' : '') : '';
    } catch (e) { return ''; }
  }

  // ce que voyait le script quand l'attente maximale a été atteinte (journal client du serveur)
  var reports = 0;
  function report(info) {
    if (reports >= REPORTS_MAX) return;
    reports++;
    try {
      var api = window.ApiClient;
      if (!api || typeof api.getUrl !== 'function' || typeof api.accessToken !== 'function') return;
      var body = 'gc-syncplay v' + VERSION + ' attente maximale | ' + navigator.userAgent + ' | ' + appName() + '\n' +
        JSON.stringify(info) + '\n';
      fetch(api.getUrl('ClientLog/Document'), {
        method: 'POST',
        headers: { 'Content-Type': 'text/plain', 'X-Emby-Token': api.accessToken() },
        body: body
      }).catch(function () {});
    } catch (e) { /* le journal n'est jamais bloquant */ }
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
            var t = bench() || {};
            var native = !!(wrapper && typeof wrapper.currentTimeAsync === 'function');
            var waitMs = t.waitMs > 0 ? t.waitMs : native ? WAIT_NATIVE_MS : WAIT_MS;
            var near = function (x) {
              return isFinite(x) && Math.abs(x * TICKS_PER_MS - positionTicks) <= ARRIVED_MS * TICKS_PER_MS;
            };
            // lecteur natif (mpv : currentTimeAsync) : sa position ; lecteur web : son élément <video>, saut terminé et
            // HAVE_FUTURE_DATA (3), sans quoi il repartirait aussitôt « en chargement »
            var vs = native ? null : videoState();
            var via = null;
            if (!t.noArrive) {
              if (vs) via = !vs.seeking && vs.ready >= 3 && near(vs.ms) ? 'video' : null;
              else via = near(ms) ? 'lecteur' : null;
            }
            var waited = Date.now() - t0;
            if (!via && waited < waitMs) { setTimeout(poll, POLL_MS); return; }
            ST.seeks++;
            ST.last = { at: new Date().toISOString(), waitedMs: waited, atTarget: !!via, via: via || 'délai',
                        offsetMs: isFinite(ms) ? Math.round(ms - positionTicks / TICKS_PER_MS) : null };
            console.debug('[gc-syncplay] prêt après saut', ST.last);
            if (!via) {
              var type = '';
              try { type = String(wrapper && wrapper.constructor && wrapper.constructor.type || ''); } catch (e) { /* inconnu */ }
              report({ waitedMs: waited, waitMs: waitMs, targetMs: Math.round(positionTicks / TICKS_PER_MS),
                       playerMs: isFinite(ms) ? Math.round(ms) : String(ms), wrapper: type,
                       async: native,
                       video: vs && { ms: Math.round(vs.ms), seeking: vs.seeking, ready: vs.ready, paused: vs.paused },
                       seeks: ST.seeks, bench: !!t.noArrive });
            }
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
