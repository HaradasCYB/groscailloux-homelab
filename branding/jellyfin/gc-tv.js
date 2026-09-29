// Téléviseurs (appli LG webOS, Samsung Tizen… : elles chargent CE client web) — script PUBLIC de JavaScript
// Injector (« Groscailloux TV », sans authentification) : il s'exécute dès le chargement de la page, avant que
// Media Bar ne démarre et avant la connexion, contrairement aux scripts authentifiés (private.js).
// Sur un téléviseur (agent utilisateur ; jamais le nombre de cœurs, qui attraperait un vieux portable) :
//   - Media Bar ne démarre pas (aucune liste aléatoire, aucun backdrop, aucune bande-annonce YouTube) : le
//     bandeau et ses bandes-annonces sont ce qui pèse le plus sur une télé ; le calque CSS cache ce qu'il a déjà
//     posé (`#slides-container`, `.bar-loading`).
//   - En-tête : les boutons Aléatoire (Jellyfin Enhanced) et SyncPlay sont retirés — une télécommande n'en fait
//     rien et ils repoussaient Rechercher/Notifications/Profil. Rechercher, la cloche et le profil restent.
// Ailleurs (PC, téléphone, tablette) : ce script ne fait strictement rien.
// Le tchat, lui, se retire seul sur TV (gc-chat-loader.js et /gc-chat/app.js). Le reste (rangées de l'accueil,
// blocs des fiches) est du CSS `.layout-tv` dans branding/jellyfin/groscailloux-tv.css.
// Jamais de window.confirm/alert/prompt dans les scripts injectés.
(function () {
  'use strict';
  if (window.__gcTvDone) return;
  var ua = String(navigator.userAgent || '');
  var TV = /web0?s|webos|tizen|smart-?tv|netcast|viera|bravia|hbbtv|aft[a-z]|android\s?tv|googletv|crkey/i.test(ua);
  window.__gcTv = TV;
  if (!TV) return;
  window.__gcTvDone = true;

  /* LG webOS : jellyfin-web (webOS ≥ 4, `video.audioTracks` présent) change de piste audio DANS le lecteur de la
     télé, sans rien demander au serveur ; sur les LG vues, ce basculement ne fait rien : la télé garde la piste
     « par défaut » du fichier (27/09 : VO demandée sur Obsession, restée en VF).
     1. `audioTracks` est masqué sur la seule vidéo EN LECTURE (celle posée dans la page) : jellyfin-web ne peut
        plus basculer lui-même et redemande le flux au serveur avec la piste choisie. L'élément de test qui sert à
        construire le profil de lecture n'est jamais inséré dans la page : le profil ne change pas, la piste par
        défaut reste en lecture directe (masquer tout le prototype faisait remuxer aussi la VF par défaut, que
        Jellyfin juge « secondaire » dès qu'elle n'est pas la première du fichier).
     2. Toute demande de lecture d'une autre piste que celle par défaut part sans lecture directe : remux, image et
        son copiés, coût processeur négligeable. */
  if (/web0?s|webos/i.test(ua)) {
    var hideTracks = function (v) {
      if (v.__gcNoTracks) return;
      try { Object.defineProperty(v, 'audioTracks', { configurable: true, get: function () { return undefined; } }); v.__gcNoTracks = true; } catch (e) {}
    };
    var scan = function (n) {
      if (!n || n.nodeType !== 1) return;
      if (n.tagName === 'VIDEO') hideTracks(n);
      else if (n.querySelectorAll) { var vs = n.querySelectorAll('video'); for (var i = 0; i < vs.length; i++) hideTracks(vs[i]); }
    };
    var startVideoWatch = function () {
      scan(document.body);
      new MutationObserver(function (ms) { ms.forEach(function (m) { for (var i = 0; i < m.addedNodes.length; i++) scan(m.addedNodes[i]); }); })
        .observe(document.documentElement, { childList: true, subtree: true });
    };
    if (document.body) startVideoWatch(); else document.addEventListener('DOMContentLoaded', startVideoWatch);
    window.__gcNoAudioTracks = true;
    var defaultAudio = function (streams) {
      var a = (streams || []).filter(function (s) { return s.Type === 'Audio' && !s.IsExternal; });
      var d = a.filter(function (s) { return s.IsDefault; })[0] || a[0];
      return d ? d.Index : null;
    };
    window.__gcTvDefaultAudio = defaultAudio;
    // corps modifié (texte) si la piste demandée n'est pas celle que la télé jouerait en lecture directe
    var adjust = function (url, bodyText) {
      var m = /\/Items\/([0-9a-f-]{32,36})\/PlaybackInfo/i.exec(url || '');
      var AC = window.ApiClient;
      if (!m || typeof bodyText !== 'string' || !AC || !AC.getItem) return Promise.resolve(bodyText);
      var body; try { body = JSON.parse(bodyText || '{}'); } catch (e) { return Promise.resolve(bodyText); }
      var idx = body.AudioStreamIndex;
      if (idx == null) idx = (/[?&]AudioStreamIndex=(\d+)/i.exec(url) || [])[1];
      if (idx == null || body.EnableDirectPlay === false) return Promise.resolve(bodyText);
      return AC.getItem(AC.getCurrentUserId(), m[1].replace(/-/g, '')).then(function (it) {
        var src = (it.MediaSources || []).filter(function (s) { return !body.MediaSourceId || s.Id === body.MediaSourceId; })[0] || (it.MediaSources || [])[0];
        var d = src ? defaultAudio(src.MediaStreams) : null;
        if (d == null || Number(idx) === d) return bodyText;
        body.EnableDirectPlay = false;
        window.__gcTvForcedRemux = (window.__gcTvForcedRemux || 0) + 1;
        return JSON.stringify(body);
      }, function () { return bodyText; });
    };
    window.__gcTvAdjust = adjust;
    // jellyfin-web 10.11 : playbackManager passe par le SDK (axios → XMLHttpRequest)
    var xOpen = XMLHttpRequest.prototype.open, xSend = XMLHttpRequest.prototype.send;
    XMLHttpRequest.prototype.open = function (method, url) {
      this.__gcPI = /^post$/i.test(method) && /\/PlaybackInfo/i.test(String(url)) ? String(url) : null;
      return xOpen.apply(this, arguments);
    };
    XMLHttpRequest.prototype.send = function (data) {
      var x = this;
      if (!x.__gcPI) return xSend.apply(x, arguments);
      adjust(x.__gcPI, data).then(function (d) { xSend.call(x, d); }, function () { xSend.call(x, data); });
    };
    // ancien client (jellyfin-apiclient) : fetch
    var origFetch = window.fetch;
    window.fetch = function (input, init) {
      var url = typeof input === 'string' ? input : (input && input.url) || '';
      if (init && /^post$/i.test(init.method || '') && /\/PlaybackInfo/i.test(url)) {
        return adjust(url, init.body).then(function (d) { return origFetch(input, Object.assign({}, init, { body: d })); });
      }
      return origFetch.apply(this, arguments);
    };
  }

  /* Media Bar : neutralisé avant son démarrage (ses objets sont exposés en fin de slideshowpure.js) */
  function muteMediaBar() {
    var sp = window.slideshowPure;
    if (!sp) return false;
    try {
      if (sp.CONFIG) sp.CONFIG.enableTrailers = false;
      if (sp.SlideshowManager) {
        sp.SlideshowManager.loadSlideshowData = function () { return Promise.resolve(); };
        sp.SlideshowManager.initSlideshow = function () { return Promise.resolve(); };
      }
      if (sp.VisibilityObserver) sp.VisibilityObserver.init = function () {};
      if (sp.PageBackdrop && sp.PageBackdrop.init) sp.PageBackdrop.init = function () {};
      return true;
    } catch (e) { return false; }
  }
  if (!muteMediaBar()) {
    // le script Media Bar est normalement déjà passé ; au cas où l'ordre changerait
    var tries = 0;
    var t = setInterval(function () { if (muteMediaBar() || ++tries > 50) clearInterval(t); }, 100);
  }

  /* En-tête : retirer Aléatoire (icône casino, Jellyfin Enhanced) et SyncPlay dès qu'ils apparaissent */
  var HEADER_DROP = '.headerRight .headerSyncButton, .headerRight #randomItemButton, .headerRight .headerButton .material-icons.casino';
  function pruneHeader() {
    var found = document.querySelectorAll(HEADER_DROP);
    for (var i = 0; i < found.length; i++) {
      var el = found[i].classList.contains('material-icons') ? found[i].closest('.headerButton') : found[i];
      if (el && el.parentNode) el.parentNode.removeChild(el);
    }
    var slides = document.getElementById('slides-container');
    if (slides && slides.parentNode) slides.parentNode.removeChild(slides);
    var bar = document.querySelector('.bar-loading');
    if (bar && bar.parentNode) bar.parentNode.removeChild(bar);
  }
  function watch() {
    pruneHeader();
    var mo = new MutationObserver(function () { pruneHeader(); });
    mo.observe(document.body, { childList: true, subtree: true });
  }
  if (document.body) watch(); else document.addEventListener('DOMContentLoaded', watch);

  /* Home Screen Sections charge ses rangées par paquets, au défilement seulement (son gestionnaire exige
     scrollY > dernier scrollY). Avec les rangées inutiles cachées, l'accueil TV tient dans l'écran : on ne peut
     plus défiler, et les paquets suivants (Séries ajoutées, Anime, Collections…) ne viendraient jamais. Tant que
     la page n'est pas défilable et que HSS n'a pas fini, on appelle son gestionnaire comme l'aurait fait un
     défilement. Dès que la page dépasse l'écran, HSS reprend seul. */
  setInterval(function () {
    var m = window.HssPageMeta;
    if (!m || typeof m.ScrollHandler !== 'function' || m.Finished === true || m.IsLoading === true) return;
    if (document.documentElement.scrollHeight > window.innerHeight + m.ScrollThreshold) return;
    if (!document.querySelector('.homeSectionsContainer')) return;
    try { m.LastScrollHeight = -1; m.ScrollHandler(); } catch (e) { /* HSS absent ou changé : rien à faire */ }
  }, 1500);
})();
