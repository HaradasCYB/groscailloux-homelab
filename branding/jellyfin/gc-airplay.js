// « Jouer sur » : Google Cast n'existe que dans Chrome et l'appli Android. Partout ailleurs, jellyfin-web
// affiche quand même une note « (Google Cast non pris en charge) » sous le titre — un `<p class="actionSheetText">`,
// pas une entrée, la liste étant vide — seule chose visible quand aucun autre appareil du compte n'est
// connecté, et prise pour une panne (2026-09-19).
// Quand cette note est là (vérifié à l'exécution, pas d'après l'appareil) :
//   - iPhone, iPad, Mac : elle devient une entrée « AirPlay ».
//       · vidéo en cours → le sélecteur AirPlay d'Apple s'ouvre dans le geste (il exige une vidéo ET un geste) ;
//       · sinon → lecture du titre affiché (fiche : son bouton Lire ; accueil : la fiche du titre du bandeau,
//         puis son bouton Lire — le bouton du bandeau Media Bar est inopérant dans l'appli iPhone), puis, dès
//         que le lecteur a sa vidéo, tentative d'ouvrir le
//         sélecteur tant que l'activation du geste dure (WebKit la garde quelques secondes) ; si Apple refuse,
//         un rappel discret pointe l'icône AirPlay du lecteur. Le rappel n'est plus affiché d'office.
//   - ailleurs (Firefox, Jellyfin Desktop…) : la note est retirée.
//   - liste vide : une note explique qu'aucun autre appareil du compte n'est connecté.
// Fermeture : jellyfin-web 10.11 ferme un dialogue par « retour » (history.state.usr.dialogs[]) ; si ça ne
// suffit pas (WebView de l'appli iPhone, 2026-09-19), on rejoue popstate, puis on retire le dialogue.
// Ce qui s'est passé est envoyé au journal client de Jellyfin (ClientLog) pour diagnostiquer un vrai iPhone.
// Nouvelle interface de Jellyfin 12.1 (2026-10-04, VERSION 5) : « Lire sur » y est un menu MUI
// (#app-remote-play-menu) et non plus une feuille ; mêmes règles, voir fixMenu().
// Les vraies cibles (Cast fonctionnel, TV et Desktop du compte) ne sont jamais touchées.
// Déposé dans JavaScript Injector (« Groscailloux AirPlay ») par scripts/jellyfin-js-apply.py.
// Jamais de window.confirm/alert/prompt : ignorés par la WebView iPhone et Jellyfin Desktop.
(function (root) {
  'use strict';

  var HIDE_MS = 7000;
  var VIDEO_WAIT_MS = 8000;
  var EMPTY_NOTE = 'Aucun autre appareil connecté avec ce compte.';
  // Chrome sur Android n'a pas le SDK Cast web (il n'existe que sur ordinateur) : jellyfin-web écrit
  // « (Google Cast non pris en charge) », qu'on retirait pour laisser « aucun autre appareil » — message
  // trompeur, un membre a cru à une restriction de notre part (2026-09-22). On dit quoi faire à la place.
  var EMPTY_ANDROID = 'Depuis le navigateur Android, la diffusion n\u2019est pas possible. Installe l\u2019appli '
    + 'Jellyfin du Play Store : elle propose la diffusion vers une TV Chromecast ou Google TV.';

  function isApple(nav) {
    nav = nav || root.navigator || {};
    var ua = String(nav.userAgent || '');
    var touchMac = /Macintosh/.test(ua) && (nav.maxTouchPoints || 0) > 1; // iPad en mode « bureau »
    return /iPhone|iPad|iPod/.test(ua) || touchMac || (/Macintosh/.test(ua) && /AppleWebKit/.test(ua) && !/Chrome|Chromium|Edg\//.test(ua));
  }

  /* jellyfin-web n'écrit une note entre parenthèses que pour « Cast non pris en charge » */
  function castUnsupported(text) {
    return /\(.+\)/.test(String(text || ''));
  }

  function isAndroid(nav) {
    nav = nav || root.navigator || {};
    return /Android/.test(String(nav.userAgent || '')) && !isApple(nav);
  }

  /// Note affichée quand la feuille « Lire sur » est vide.
  function emptyNote(castUnsupportedHere, nav) {
    return castUnsupportedHere && isAndroid(nav) ? EMPTY_ANDROID : EMPTY_NOTE;
  }

  function currentVideo(doc) {
    var v = (doc || document).querySelector('video');
    return v && typeof v.webkitShowPlaybackTargetPicker === 'function' ? v : null;
  }

  /* Sous-titres en AirPlay (2026-09-29) : jellyfin-web ne déclare que des sous-titres « External », qu'il dessine
     lui-même par-dessus la vidéo ; AirPlay n'envoie à la télé que le flux HLS, donc sans sous-titres (Chainsaw Man,
     un membre). Déclarer aussi `vtt` en « Hls » (en tête) : pour une lecture en HLS (remux ou conversion), le serveur
     met les sous-titres DANS le flux (`#EXT-X-MEDIA TYPE=SUBTITLES`, la piste choisie en DEFAULT=YES), que l'iPhone
     affiche nativement et qu'AirPlay transmet. Aucune conversion vidéo. Lecture directe : inchangée. */
  function withHlsSubtitles(bodyText) {
    var b;
    try { b = JSON.parse(bodyText || '{}'); } catch (e) { return bodyText; }
    var prof = b && b.DeviceProfile;
    if (!prof) return bodyText;
    var subs = prof.SubtitleProfiles || [];
    if (subs.some(function (x) { return x && x.Method === 'Hls'; })) return bodyText;
    prof.SubtitleProfiles = [{ Format: 'vtt', Method: 'Hls' }].concat(subs);
    return JSON.stringify(b);
  }

  root.__gcAirPlay = {
    withHlsSubtitles: withHlsSubtitles,
    isApple: isApple,
    isAndroid: isAndroid,
    castUnsupported: castUnsupported,
    emptyNote: emptyNote,
    EMPTY_NOTE: EMPTY_NOTE,
    EMPTY_ANDROID: EMPTY_ANDROID
  };
  if (typeof window === 'undefined' || root !== window) return; // tests

  /* PlaybackInfo : XHR du SDK (jellyfin-web 10.11) et fetch (ancien client) — appareils Apple seulement */
  if (isApple() && !window.__gcHlsSubs) {
    window.__gcHlsSubs = true;
    var xOpen = XMLHttpRequest.prototype.open, xSend = XMLHttpRequest.prototype.send;
    XMLHttpRequest.prototype.open = function (method, url) {
      this.__gcSubPI = /^post$/i.test(method) && /\/PlaybackInfo/i.test(String(url));
      return xOpen.apply(this, arguments);
    };
    XMLHttpRequest.prototype.send = function (data) {
      if (this.__gcSubPI && typeof data === 'string') data = withHlsSubtitles(data);
      return xSend.call(this, data);
    };
    var oFetch = window.fetch;
    window.fetch = function (input, init) {
      var url = typeof input === 'string' ? input : (input && input.url) || '';
      if (init && /^post$/i.test(init.method || '') && /\/PlaybackInfo/i.test(url) && typeof init.body === 'string') {
        init = Object.assign({}, init, { body: withHlsSubtitles(init.body) });
      }
      return oFetch.call(this, input, init);
    };
  }

  /* --- journal : ce qui s'est passé, lisible côté serveur (jellyfin/config/log/upload_*.log) --- */
  var steps = [];
  function step(s) { steps.push(Math.round(performance.now()) + 'ms ' + s); window.__gcAirPlaySteps = steps.slice(); }
  function report(tag) {
    try {
      var api = window.ApiClient;
      if (!api || typeof api.getUrl !== 'function' || typeof api.accessToken !== 'function') return;
      var body = 'gc-airplay ' + tag + ' | ' + navigator.userAgent + '\n' + steps.join('\n') + '\n';
      fetch(api.getUrl('ClientLog/Document'), {
        method: 'POST',
        headers: { 'Content-Type': 'text/plain', 'X-Emby-Token': api.accessToken() },
        body: body
      }).catch(function () {});
    } catch (e) { /* le journal n'est jamais bloquant */ }
  }

  function banner(title, sub) {
    var old = document.querySelector('.gc-ap');
    if (old) old.remove();
    var el = document.createElement('div');
    el.className = 'gc-ap';
    el.setAttribute('role', 'status');
    el.style.cssText = 'position:fixed;left:50%;top:max(12px,env(safe-area-inset-top));transform:translateX(-50%);z-index:100001;max-width:92vw;' +
      'background:rgba(17,17,17,.96);color:#fff;padding:10px 14px;border-radius:12px;font:14px/1.35 system-ui,sans-serif;box-shadow:0 6px 24px rgba(0,0,0,.5)';
    var h = document.createElement('div'); h.style.fontWeight = '600'; h.textContent = title;
    var s = document.createElement('div'); s.style.cssText = 'opacity:.85;margin-top:3px'; s.textContent = sub;
    el.appendChild(h); el.appendChild(s);
    document.body.appendChild(el);
    setTimeout(function () { if (el.parentNode) el.remove(); }, HIDE_MS);
  }

  function dialogInHistory() {
    var st = null;
    try { st = window.history && window.history.state; } catch (e) { return false; }
    if (!st) return false;
    if (st.dialogId) return true;
    return !!(st.usr && Array.isArray(st.usr.dialogs) && st.usr.dialogs.length);
  }

  /* retire le dialogue nous-mêmes quand le retour n'a pas suffi : dialogue, fond, verrou de défilement */
  function forceRemove(sheet) {
    var dlg = sheet.closest('.dialogContainer') || sheet;
    var bd = document.querySelector('.dialogBackdropOpened') || document.querySelector('.dialogBackdrop');
    if (bd && bd.parentNode) bd.parentNode.removeChild(bd);
    if (dlg.parentNode) dlg.parentNode.removeChild(dlg);
    document.body.classList.remove('noScroll', 'dialogOpen', 'dialog-open');
    document.documentElement.classList.remove('noScroll');
  }

  function closeSheet(sheet, done) {
    var open = function () { return document.body.contains(sheet); };
    if (dialogInHistory()) { step('close: history.back'); window.history.back(); } else { step('close: no history entry'); }
    setTimeout(function () {
      if (!open()) { step('close: closed'); return done(); }
      step('close: popstate');
      try { window.dispatchEvent(new PopStateEvent('popstate', { state: window.history.state })); } catch (e) { /* ignore */ }
      setTimeout(function () {
        if (!open()) { step('close: closed after popstate'); return done(); }
        step('close: forced');
        forceRemove(sheet);
        done();
      }, 250);
    }, 250);
  }

  function visible(el) { return !!(el && el.offsetParent !== null); }

  /* bouton Lire natif de la fiche du média ; jamais les boutons des vignettes (inopérants sans survol) ni
     celui du bandeau Media Bar (.slide) : ce dernier envoie une commande « à distance » à sa propre session
     (POST /Sessions/{id}/Playing), sans effet dans l'appli iPhone (2026-09-19, journal client : « video: none ») */
  function playButton() {
    var sel = ['.btnPlay', '.detailButton-play', '.mainDetailButtons button[data-action="resume"]',
               '.mainDetailButtons button[data-action="play"]', '.detailPagePrimaryContainer button[data-action="play"]', '.btnPlayAll'];
    for (var i = 0; i < sel.length; i++) {
      var all = document.querySelectorAll(sel[i]);
      for (var j = 0; j < all.length; j++) if (visible(all[j]) && !all[j].closest('.card') && !all[j].closest('.slide')) return all[j];
    }
    return null;
  }

  /* titre affiché par le bandeau d'accueil (Media Bar) : on passe par sa fiche, dont le bouton Lire est
     celui de jellyfin-web. Retourne le bouton « Détails » de la diapositive visible, ou null. */
  function slideDetailButton() {
    var slide = document.querySelector('#slides-container .slide.active[data-item-id]') ||
                document.querySelector('#slides-container .slide[data-item-id]');
    if (!slide || !visible(slide)) return null;
    var b = slide.querySelector('.detail-button');
    return b && visible(b) ? b : null;
  }

  /* attend le bouton Lire natif après la navigation vers la fiche (même délai que la vidéo) */
  function whenPlayButton(cb) {
    var b = playButton();
    if (b) return cb(b);
    var mo = new MutationObserver(function () {
      var b2 = playButton();
      if (b2) { mo.disconnect(); clearTimeout(t); cb(b2); }
    });
    mo.observe(document.documentElement, { childList: true, subtree: true });
    var t = setTimeout(function () { mo.disconnect(); cb(null); }, VIDEO_WAIT_MS);
  }

  function tryPicker(v, where) {
    try { v.webkitShowPlaybackTargetPicker(); step('picker: shown (' + where + ')'); return true; }
    catch (e) { step('picker: refused (' + where + ') ' + (e && e.message || e)); window.__gcAirPlayError = String(e && e.message || e); return false; }
  }

  function whenVideo(cb) {
    var v = currentVideo();
    if (v) return cb(v);
    var mo = new MutationObserver(function () {
      var v2 = currentVideo();
      if (v2) { mo.disconnect(); clearTimeout(t); cb(v2); }
    });
    mo.observe(document.documentElement, { childList: true, subtree: true });
    var t = setTimeout(function () { mo.disconnect(); cb(null); }, VIDEO_WAIT_MS);
  }

  /* `close(done)` ferme ce qui a été touché : feuille de l'ancienne interface ou menu de la nouvelle */
  function onAirPlay(close) {
    steps = []; step('tap');
    var v = currentVideo();
    if (v) {
      // dans le geste, avant tout : Apple l'exige
      var ok = tryPicker(v, 'live');
      close(function () {
        window.__gcAirPlayShown = ok ? 'picker' : 'hint';
        if (!ok) banner('AirPlay', 'Touche l’icône AirPlay dans le lecteur pour choisir l’écran.');
        report(ok ? 'ok' : 'picker-refused');
      });
      return;
    }
    var btn = playButton();
    var detail = btn ? null : slideDetailButton();
    step(btn ? 'play: button ' + String(btn.className || btn.tagName).slice(0, 40) : detail ? 'play: via slide detail page' : 'play: no button');
    close(function () {
      if (!btn && !detail) {
        window.__gcAirPlayShown = 'hint';
        banner('AirPlay', 'Ouvre un film ou une série, puis choisis AirPlay dans « Lire sur ».');
        report('no-play-button');
        return;
      }
      if (btn) return startAndPick(btn);
      detail.click();
      window.__gcAirPlayShown = 'navigating';
      whenPlayButton(function (b2) {
        if (!b2) {
          step('play: detail page has no play button within ' + VIDEO_WAIT_MS + 'ms');
          window.__gcAirPlayShown = 'hint';
          banner('AirPlay', 'Appuie sur Lire, puis choisis AirPlay dans « Lire sur ».');
          report('no-play-button-after-nav');
          return;
        }
        step('play: button ' + String(b2.className || b2.tagName).slice(0, 40) + ' (after nav)');
        startAndPick(b2);
      });
    });
  }

  function startAndPick(btn) {
    btn.click();
    window.__gcAirPlayShown = 'started';
    whenVideo(function (video) {
      if (!video) {
        step('video: none within ' + VIDEO_WAIT_MS + 'ms');
        window.__gcAirPlayShown = 'hint';
        banner('AirPlay', 'La lecture n’a pas démarré : appuie sur Lire, puis sur l’icône AirPlay du lecteur.');
        report('no-video');
        return;
      }
      step('video: ready');
      if (tryPicker(video, 'after-play')) { window.__gcAirPlayShown = 'picker'; report('ok-after-play'); return; }
      window.__gcAirPlayShown = 'hint';
      banner('AirPlay', 'Touche l’icône AirPlay dans le lecteur pour choisir l’écran.');
      report('picker-refused-after-play');
    });
  }

  function airPlayItem(sheet) {
    var b = document.createElement('button');
    b.setAttribute('is', 'emby-button');
    b.type = 'button';
    b.className = 'listItem listItem-button actionSheetMenuItem emby-button';
    b.setAttribute('data-id', 'gc-airplay');
    var icon = document.createElement('span');
    icon.className = 'actionsheetMenuItemIcon listItemIcon listItemIcon-transparent material-icons airplay';
    icon.setAttribute('aria-hidden', 'true');
    var body = document.createElement('div');
    body.className = 'listItemBody actionsheetListItemBody';
    var txt = document.createElement('div');
    txt.className = 'listItemBodyText actionSheetItemText';
    txt.textContent = 'AirPlay';
    body.appendChild(txt);
    b.appendChild(icon); b.appendChild(body);
    b.addEventListener('click', function (e) {
      e.preventDefault(); e.stopPropagation(); e.stopImmediatePropagation();
      onAirPlay(function (done) { closeSheet(sheet, done); });
    }, true);
    return b;
  }

  var VERSION = 6;
  window.__gcAirPlayVersion = VERSION;
  /* « Jouer sur » = la feuille qui suit un appui sur le bouton Cast de l'en-tête (indépendant de la langue) */
  var lastCastTap = 0;
  document.addEventListener('click', function (e) {
    if (e.target && e.target.closest && e.target.closest('.headerCastButton')) lastCastTap = Date.now();
  }, true);

  /* une feuille d'actions vient d'apparaître : est-ce « Jouer sur » ? */
  function fixSheet(sheet) {
    if (sheet.__gcAirPlayV >= VERSION) return;
    var notes = sheet.querySelectorAll('.actionSheetText');
    var note = null;
    for (var i = 0; i < notes.length; i++) if (castUnsupported(notes[i].textContent)) { note = notes[i]; break; }
    var fromCast = Date.now() - lastCastTap < 2000;
    if (!note && !fromCast) return; // pas cette feuille : on ne touche à rien
    sheet.__gcAirPlayDone = true;
    sheet.__gcAirPlayV = VERSION;
    var scroller = sheet.querySelector('.actionSheetScroller');
    // une version plus ancienne de ce script (déjà déployée) a pu passer avant : on reprend la main.
    // Cast est « non pris en charge » si la note est là, ou si cette ancienne version l'avait déjà remplacée.
    var old = sheet.querySelectorAll('.actionSheetMenuItem[data-id="gc-airplay"]');
    var unsupported = !!note || old.length > 0;
    for (var k = 0; k < old.length; k++) old[k].remove();
    if (note) note.remove();
    if (unsupported) {
      if (isApple() && scroller) {
        scroller.insertBefore(airPlayItem(sheet), scroller.firstChild);
        window.__gcAirPlayShown = 'airplay';
      } else {
        window.__gcAirPlayShown = 'hidden';
      }
    }
    if (scroller && !scroller.querySelector('.actionSheetMenuItem') && !sheet.querySelector('.gc-ap-empty')) {
      var p = document.createElement('p');
      p.className = 'actionSheetText gc-ap-empty';
      p.textContent = emptyNote(unsupported);
      scroller.parentNode.insertBefore(p, scroller);
    }
  }

  /* --- Nouvelle interface (Jellyfin 12.1) : « Lire sur » est un menu MUI (#app-remote-play-menu), dessiné par React.
     Sa ligne grisée dit « Google Cast non pris en charge » (iPhone, Firefox, Jellyfin Desktop) ou « Aucune cible de
     casting disponible » (Chrome, dont Android où la diffusion web n'existe pas). Même règles que la feuille : AirPlay
     sur les appareils Apple, sinon une explication quand la liste est vide. Les lignes de React ne sont jamais
     retirées (React les retire lui-même à la fermeture) : cachées ou rédigées autrement ; l'entrée AirPlay est une
     copie de la ligne grisée, rendue active, pour garder le style du menu. --- */
  var UNSUPPORTED_RE = /google cast/i, NO_TARGET_RE = /aucune cible|no .*target/i;
  var AIRPLAY_PATH = 'M6 22h12l-6-6zM21 3H3c-1.1 0-2 .9-2 2v12c0 1.1.9 2 2 2h4v-2H3V5h18v12h-4v2h4c1.1 0 2-.9 2-2V5c0-1.1-.9-2-2-2z';

  function closeMenu(menu, done) {
    var bd = menu.querySelector('.MuiBackdrop-root');
    if (bd) { step('close: menu backdrop'); bd.click(); }
    // le menu reste dans la page une fois fermé (il y est dès le chargement) : on regarde s'il est encore affiché
    setTimeout(function () { step(menu.getClientRects().length && getComputedStyle(menu).visibility !== 'hidden' ? 'close: menu still shown' : 'close: menu closed'); done(); }, 300);
  }

  function muiAirPlayItem(menu, model) {
    var li = model.cloneNode(true);
    li.classList.remove('Mui-disabled');
    li.classList.add('gc-ap-mui');
    li.removeAttribute('aria-disabled');
    li.style.display = '';
    li.tabIndex = 0;
    var icon = li.querySelector('.MuiListItemIcon-root');
    if (!icon) {
      icon = document.createElement('div');
      icon.className = 'MuiListItemIcon-root';
      icon.style.cssText = 'min-width:36px;display:inline-flex;color:inherit';
      li.insertBefore(icon, li.firstChild);
    }
    var ns = 'http://www.w3.org/2000/svg', svg = document.createElementNS(ns, 'svg'), path = document.createElementNS(ns, 'path');
    svg.setAttribute('viewBox', '0 0 24 24'); svg.setAttribute('aria-hidden', 'true');
    svg.style.cssText = 'width:1.5rem;height:1.5rem;fill:currentColor';
    path.setAttribute('d', AIRPLAY_PATH); svg.appendChild(path);
    icon.innerHTML = ''; icon.appendChild(svg);
    var txt = li.querySelector('.MuiListItemText-primary') || li;
    txt.textContent = 'AirPlay';
    li.addEventListener('click', function (e) {
      e.preventDefault(); e.stopPropagation(); e.stopImmediatePropagation();
      onAirPlay(function (done) { closeMenu(menu, done); });
    }, true);
    return li;
  }

  /* idempotent : relancé à chaque changement du menu (les cibles arrivent après son ouverture) */
  function fixMenu(menu) {
    var list = menu.querySelector('ul[role="menu"]');
    if (!list) return;
    var unsupported = null, noTarget = null, real = 0;
    for (var li = list.firstElementChild; li; li = li.nextElementSibling) {
      if (li.classList.contains('gc-ap-mui')) continue;
      var txt = li.textContent || '';
      if (li.classList.contains('Mui-disabled') && UNSUPPORTED_RE.test(txt)) unsupported = li;
      else if (li.classList.contains('Mui-disabled') && (NO_TARGET_RE.test(txt) || txt === EMPTY_ANDROID || txt === EMPTY_NOTE)) noTarget = li;
      else if (li.getAttribute('role') === 'menuitem' && !li.classList.contains('Mui-disabled')) real++;
    }
    var placeholder = unsupported || noTarget;
    if (isApple() && unsupported) {
      unsupported.style.display = 'none';
      if (noTarget) noTarget.style.display = 'none';
      if (!list.querySelector('.gc-ap-mui')) {
        list.insertBefore(muiAirPlayItem(menu, unsupported), list.firstChild);
        window.__gcAirPlayShown = 'airplay';
      }
      return;
    }
    if (!placeholder) return;
    if (real > 0) { placeholder.style.display = 'none'; window.__gcAirPlayShown = 'hidden'; return; }
    var note = isAndroid() ? EMPTY_ANDROID : EMPTY_NOTE;
    var label = placeholder.querySelector('.MuiListItemText-primary') || placeholder;
    if (label.textContent !== note) {
      label.textContent = note;
      label.style.whiteSpace = 'normal';
      var ic = placeholder.querySelector('.MuiListItemIcon-root');
      if (ic) ic.style.display = 'none';
      window.__gcAirPlayShown = 'note';
    }
    // Android : c'est une consigne à lire, pas une entrée indisponible. MUI grise toute ligne .Mui-disabled
    // (opacity .38) : on rend l'opacité, la ligne reste inerte (pointer-events: none de .Mui-disabled).
    if (note === EMPTY_ANDROID) {
      placeholder.style.opacity = '1';
      label.style.color = '#c9d2df';
    }
  }

  function watchMenu(menu) {
    if (menu.__gcAirPlayV >= VERSION) return;
    menu.__gcAirPlayV = VERSION;
    window.__gcAirPlayMenus = (window.__gcAirPlayMenus || 0) + 1;
    fixMenu(menu);
    var mm = new MutationObserver(function () { fixMenu(menu); });
    // characterData : à l'ouverture, React réécrit le texte de la ligne grisée (nœud texte), sans ajouter de nœud
    mm.observe(menu, { childList: true, subtree: true, characterData: true });
  }

  var mo = new MutationObserver(function (muts) {
    for (var i = 0; i < muts.length; i++) {
      var added = muts[i].addedNodes;
      for (var j = 0; j < added.length; j++) {
        var n = added[j];
        if (!n || n.nodeType !== 1) continue;
        var sheet = n.classList && n.classList.contains('actionSheet') ? n : n.querySelector && n.querySelector('.actionSheet');
        if (sheet) fixSheet(sheet);
        var menu = n.id === 'app-remote-play-menu' ? n : n.querySelector && n.querySelector('#app-remote-play-menu');
        if (menu) watchMenu(menu);
      }
    }
  });
  mo.observe(document.documentElement, { childList: true, subtree: true });
  // le menu de la nouvelle interface existe déjà, caché, quand ce script arrive (après la connexion) : il n'est
  // jamais « ajouté » sous nos yeux, seulement affiché. Il est aussi recréé quand la barre est redessinée.
  var m0 = document.getElementById('app-remote-play-menu');
  if (m0) watchMenu(m0);
})(typeof window !== 'undefined' ? window : globalThis);
