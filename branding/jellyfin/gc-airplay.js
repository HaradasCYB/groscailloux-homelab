// « Jouer sur » : Google Cast n'existe que dans Chrome et l'appli Android. Partout ailleurs, jellyfin-web
// affiche quand même une note « (Google Cast non pris en charge) » sous le titre, seule chose visible quand
// aucun autre appareil du compte n'est connecté — prise pour une panne (2026-09-19). Vérifié en test : ce
// n'est pas une entrée de menu mais un `<p class="actionSheetText">`, la liste étant vide.
// Quand cette note est là (vérifié à l'exécution, pas d'après l'appareil) :
//   - iPhone, iPad, Mac : elle est remplacée par une entrée « AirPlay ». Une vidéo en cours → le sélecteur
//     AirPlay d'Apple s'ouvre (il exige une vidéo ET un geste, d'où l'appel dans le clic) ; sinon → on lance
//     la lecture du titre affiché si un bouton Lire est visible, et on rappelle où est l'icône AirPlay.
//   - ailleurs (Firefox, Jellyfin Desktop…) : la note est retirée.
//   - liste vide : une note explique qu'aucun autre appareil du compte n'est connecté.
// Les vraies cibles (Cast fonctionnel, TV et Desktop du compte) ne sont jamais touchées.
// Déposé dans JavaScript Injector (« Groscailloux AirPlay ») par scripts/jellyfin-js-apply.py.
// Jamais de window.confirm/alert/prompt : ignorés par la WebView iPhone et Jellyfin Desktop.
(function (root) {
  'use strict';

  var HIDE_MS = 12000;
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

  function banner(title, sub) {
    var old = document.querySelector('.gc-ap');
    if (old) old.remove();
    var el = document.createElement('div');
    el.className = 'gc-ap';
    el.setAttribute('role', 'status');
    el.style.cssText = 'position:fixed;left:50%;bottom:11vh;transform:translateX(-50%);z-index:100001;max-width:92vw;' +
      'background:#111;color:#fff;padding:12px 16px;border-radius:12px;font:15px/1.4 system-ui,sans-serif;box-shadow:0 6px 24px rgba(0,0,0,.5)';
    var h = document.createElement('div'); h.style.fontWeight = '600'; h.textContent = title;
    var s = document.createElement('div'); s.style.cssText = 'opacity:.85;margin-top:4px'; s.textContent = sub;
    el.appendChild(h); el.appendChild(s);
    document.body.appendChild(el);
    setTimeout(function () { if (el.parentNode) el.remove(); }, HIDE_MS);
  }

  /* jellyfin-web ouvre chaque dialogue avec une entrée d'historique — `history.state.usr.dialogs[]` en
     10.11 (routeur), `history.state.dialogId` avant — et le ferme sur « retour » : c'est la seule fermeture
     fiable (mesuré le 2026-09-19 : clic synthétique sur le fond et Escape n'ont aucun effet). Sans entrée
     d'historique, on retombe sur le fond et le bouton de fermeture, sans jamais toucher à l'historique. */
  function dialogInHistory() {
    var st = null;
    try { st = window.history && window.history.state; } catch (e) { return false; }
    if (!st) return false;
    if (st.dialogId) return true;
    return !!(st.usr && Array.isArray(st.usr.dialogs) && st.usr.dialogs.length);
  }

  function closeSheet(sheet) {
    if (dialogInHistory()) { window.history.back(); return; }
    var backdrop = document.querySelector('.dialogBackdropOpened') || document.querySelector('.dialogBackdrop');
    if (backdrop) backdrop.click();
    var dlg = sheet.closest('.dialogContainer') || sheet;
    var btn = dlg.querySelector('.btnCloseDialog');
    if (btn) btn.click();
  }

  function playFromPage() {
    var sel = ['.btnPlay', '.detailButton-play', 'button[data-action="resume"]', 'button[data-action="play"]', '.btnPlayAll'];
    for (var i = 0; i < sel.length; i++) {
      var b = document.querySelector(sel[i]);
      if (b && b.offsetParent !== null) { b.click(); return true; }
    }
    return false;
  }

  function onAirPlay(sheet) {
    var v = currentVideo();
    closeSheet(sheet);
    if (v) {
      try { v.webkitShowPlaybackTargetPicker(); window.__gcAirPlayShown = 'picker'; return; } catch (e) { window.__gcAirPlayError = String(e && e.message || e); }
    }
    var started = playFromPage();
    banner('AirPlay',
      started ? 'La lecture démarre : touche l’icône AirPlay dans le lecteur pour choisir l’écran.'
              : 'Lance la lecture, puis touche l’icône AirPlay dans le lecteur pour choisir l’écran.');
    window.__gcAirPlayShown = started ? 'started' : 'hint';
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

  /* une feuille d'actions vient d'apparaître : est-ce « Jouer sur » avec la note Cast ? */
  function fixSheet(sheet) {
    if (sheet.__gcAirPlayDone) return;
    var notes = sheet.querySelectorAll('.actionSheetText');
    var note = null;
    for (var i = 0; i < notes.length; i++) if (castUnsupported(notes[i].textContent)) { note = notes[i]; break; }
    if (!note) return; // pas cette feuille, ou Cast fonctionne ici : on ne touche à rien
    sheet.__gcAirPlayDone = true;
    var scroller = sheet.querySelector('.actionSheetScroller');
    if (isApple() && scroller) {
      note.remove();
      scroller.insertBefore(airPlayItem(sheet), scroller.firstChild);
      window.__gcAirPlayShown = 'airplay';
    } else {
      note.remove();
      window.__gcAirPlayShown = 'hidden';
    }
    if (scroller && !scroller.querySelector('.actionSheetMenuItem')) {
      var p = document.createElement('p');
      p.className = 'actionSheetText';
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
