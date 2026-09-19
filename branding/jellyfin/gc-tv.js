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
