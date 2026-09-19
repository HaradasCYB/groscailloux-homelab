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
// Les vraies cibles (Cast fonctionnel, TV et Desktop du compte) ne sont jamais touchées.
// Déposé dans JavaScript Injector (« Groscailloux AirPlay ») par scripts/jellyfin-js-apply.py.
// Jamais de window.confirm/alert/prompt : ignorés par la WebView iPhone et Jellyfin Desktop.
(function (root) {
  'use strict';

  var HIDE_MS = 7000;
  var VIDEO_WAIT_MS = 8000;
  var EMPTY_NOTE = 'Aucun autre appareil connecté avec ce compte.';

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

  function currentVideo(doc) {
    var v = (doc || document).querySelector('video');
    return v && typeof v.webkitShowPlaybackTargetPicker === 'function' ? v : null;
  }

  root.__gcAirPlay = { isApple: isApple, castUnsupported: castUnsupported, EMPTY_NOTE: EMPTY_NOTE };
  if (typeof window === 'undefined' || root !== window) return; // tests

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

  function onAirPlay(sheet) {
    steps = []; step('tap');
    var v = currentVideo();
    if (v) {
      // dans le geste, avant tout : Apple l'exige
      var ok = tryPicker(v, 'live');
      closeSheet(sheet, function () {
        window.__gcAirPlayShown = ok ? 'picker' : 'hint';
        if (!ok) banner('AirPlay', 'Touche l’icône AirPlay dans le lecteur pour choisir l’écran.');
        report(ok ? 'ok' : 'picker-refused');
      });
      return;
    }
    var btn = playButton();
    var detail = btn ? null : slideDetailButton();
    step(btn ? 'play: button ' + String(btn.className || btn.tagName).slice(0, 40) : detail ? 'play: via slide detail page' : 'play: no button');
    closeSheet(sheet, function () {
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
      onAirPlay(sheet);
    }, true);
    return b;
  }

  var VERSION = 3;
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
      p.textContent = EMPTY_NOTE;
      scroller.parentNode.insertBefore(p, scroller);
    }
  }

  var mo = new MutationObserver(function (muts) {
    for (var i = 0; i < muts.length; i++) {
      var added = muts[i].addedNodes;
      for (var j = 0; j < added.length; j++) {
        var n = added[j];
        if (!n || n.nodeType !== 1) continue;
        var sheet = n.classList && n.classList.contains('actionSheet') ? n : n.querySelector && n.querySelector('.actionSheet');
        if (sheet) fixSheet(sheet);
      }
    }
  });
  mo.observe(document.documentElement, { childList: true, subtree: true });
})(typeof window !== 'undefined' ? window : globalThis);
