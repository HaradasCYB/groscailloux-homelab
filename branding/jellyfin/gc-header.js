// « Groscailloux En-tête » : la nouvelle interface de Jellyfin 12.1 (barre React `header.MuiAppBar-root`, celle des
// navigateurs, de Jellyfin Desktop et des applis mobiles) retrouve ce que l'ancienne avait (2026-10-04) :
//   - le logo « Groscailloux TV » à la place de l'icône et du nom du serveur ;
//   - sur l'accueil, une rangée d'onglets Accueil, Favoris, Découvrir, Demandes, Calendrier sous la barre, comme
//     l'ancien en-tête. Découvrir/Demandes/Calendrier passent par les boutons de Jellyfin Enhanced (onglets 2 à 4 de
//     l'accueil) ; leurs icônes et le lien « Favoris » de la barre sont cachés là où la rangée les remplace ;
//   - les libellés restés en anglais : la barre est dessinée AVANT le chargement du français et ne se redessine
//     qu'à un changement de largeur de la fenêtre (« Favorites », « Search », « User Menu »…), et quelques
//     infobulles de Jellyfin Enhanced n'ont pas de traduction (« Requests », « Calendar ») ;
//   - la cloche NotifySync, qui s'accroche à l'ancien en-tête (gardé dans la page mais caché), déplacée dans la
//     barre visible, juste après le tchat. NotifySync ne la recrée pas tant qu'elle existe (`#netflix-bell`).
//   - PC (v4) : la rangée d'onglets monte au centre de la barre quand elle y tient ;
//   - téléphone (v3, 04/10) : la barre ne déborde plus sur ☰, la rangée d'onglets est resserrée, défile avec un
//     fondu au bord et amène l'onglet actif au centre ; le bandeau d'annonce du tchat passe sous les menus.
// Rien ne change dans l'ancienne interface ni sur les télés : tout dépend de la barre moderne visible, sauf les
// libellés anglais des boutons de la bannière Media Bar (« Play » → « Lire »), traduits partout.
// Ne retire jamais un élément dessiné par React (il le retirerait lui-même ensuite et planterait) : on cache,
// on déplace nos propres éléments, on change des textes et des attributs.
// Déposé dans JavaScript Injector (« Groscailloux En-tête », privé) par scripts/jellyfin-js-apply.py.
(function () {
  'use strict';
  // une version plus récente (déploiement, essai d'un banc) prend la main : celle-ci s'arrête au tour suivant
  var VERSION = 4;
  if ((window.__gcHeaderV || 0) >= VERSION) return;
  window.__gcHeaderV = VERSION;

  var LOGO = '/web/banner-light.gc.png'; // servi par Jellyfin Enhanced (nos images de marque), quel que soit le nom
  /* anglais → français, pour les seuls textes connus de la barre et du panneau de Jellyfin Enhanced */
  var FR = {
    'Favorites': 'Favoris', 'More': 'Plus', 'Search': 'Rechercher', 'Cast to Device': 'Diffuser sur l’appareil',
    'User Menu': 'Menu utilisateur', 'Open Menu': 'Ouvrir le menu', 'Close Menu': 'Fermer le menu',
    'Requests': 'Demandes', 'Calendar': 'Calendrier', 'Recommandations Seerr': 'Découvrir', 'Recommendations': 'Découvrir',
    'Discover': 'Découvrir', 'More from Jellyfin Enhanced': 'Plus', 'Random item': 'Élément aléatoire',
    // libellés de Jellyfin Enhanced sans traduction (menu ☰, menu du compte, réglages)
    'More from JE': 'Plus', 'Enhanced Panel': 'Réglages Enhanced', 'Bookmarks': 'Signets', 'Modular Home': 'Accueil personnalisé',
    'Jellyfin Enhanced - User Settings': 'Jellyfin Enhanced : réglages'
  };
  var ATTRS = ['aria-label', 'title', 'data-header-label'];
  /* boutons de la bannière Media Bar 3.0 : libellés en anglais écrits en dur, aucun réglage de langue (04/10) */
  var MEDIABAR = { 'Play': 'Lire', 'Details': 'Infos', 'Favorite': 'Favori' };
  var ICON = {
    home: 'M10 20v-6h4v6h5v-8h3L12 3 2 12h3v8z',
    fav: 'M16.5 3c-1.74 0-3.41.81-4.5 2.09C10.91 3.81 9.24 3 7.5 3 4.42 3 2 5.42 2 8.5c0 3.78 3.4 6.86 8.55 11.54L12 21.35l1.45-1.32C18.6 15.36 22 12.28 22 8.5 22 5.42 19.58 3 16.5 3zm-4.4 15.55l-.1.1-.1-.1C7.14 14.24 4 11.39 4 8.5 4 6.5 5.5 5 7.5 5c1.54 0 3.04.99 3.57 2.36h1.87C13.46 5.99 14.96 5 16.5 5c2 0 3.5 1.5 3.5 3.5 0 2.89-3.14 5.74-7.9 10.05z'
  };
  /* onglets : 0 et 1 = jellyfin-web ; les suivants = pages de Jellyfin Enhanced, dans l'ordre de ses boutons */
  var TABS = [
    { label: 'Accueil', href: '#/home', tab: 0, icon: ICON.home },
    { label: 'Favoris', href: '#/home?tab=1', tab: 1, icon: ICON.fav },
    { label: 'Découvrir', je: 'je-native-tab-link-recommendations' },
    { label: 'Demandes', je: 'je-native-tab-link-requests' },
    { label: 'Calendrier', je: 'je-native-tab-link-calendar' }
  ];
  var S = { nav: null, links: [] };

  var CSS_ID = 'gc-header-css-' + VERSION;
  function css() {
    if (document.getElementById(CSS_ID)) return;
    var logo = 'header.MuiAppBar-root .MuiToolbar-root > .MuiStack-root > a[href="#/"]';
    var st = document.createElement('style');
    st.id = CSS_ID;
    st.textContent = [
      logo + '{font-size:0!important;min-width:0!important}',
      logo + ' > .MuiButton-startIcon{display:none!important}',
      logo + '::after{content:"";display:block;width:176px;height:25px;background:url("' + LOGO + '") left center/contain no-repeat}',
      'nav.gc-tabs{display:flex;flex-wrap:nowrap!important;justify-content:center;gap:.3em;padding:0 .8em .6em;overflow-x:auto;scrollbar-width:none;-webkit-overflow-scrolling:touch}',
      'nav.gc-tabs::-webkit-scrollbar{display:none}',
      // notre logo sous 900 px, là où jellyfin-web ne dessine ni le sien ni les bibliothèques (placeLogo)
      // order : toujours après ☰ et Retour, que React insère Retour avant ou après lui
      'header.MuiAppBar-root .MuiToolbar-root:has(> a.gc-logo) > .MuiBox-root{order:2}',
      'a.gc-logo{order:1;flex:0 0 auto;display:block;width:176px;height:25px;margin-left:8px;background:url("' + LOGO + '") left center/contain no-repeat}',
      'a.gc-logo[hidden]{display:none!important}',
      'nav.gc-tabs[hidden],nav.gc-tabs a[hidden]{display:none!important}',
      'nav.gc-tabs a{display:inline-flex;align-items:center;gap:.45em;flex:0 0 auto;padding:.42em 1.05em;border-radius:.55em;' +
        'background:rgba(28,30,35,.78);border:1px solid rgba(255,255,255,.1);color:#f2f3f5;font-family:inherit;font-size:15px;' +
        'font-weight:500;line-height:1.2;text-decoration:none;white-space:nowrap;cursor:pointer}',
      'nav.gc-tabs a:hover{background:rgba(60,63,70,.92)}',
      'nav.gc-tabs a.gc-on{background:#fff;color:#0b0b0c;border-color:#fff}',
      'nav.gc-tabs a:focus-visible{outline:2px solid #8fd3fb;outline-offset:2px}',
      'nav.gc-tabs svg{width:18px;height:18px;fill:currentColor;flex:0 0 auto}',
      // bandeau d'annonce du tchat : sous la barre réelle (plus haute avec la rangée d'onglets), pas à 5,2 em, et SOUS
      // le menu latéral et les menus de la barre (MUI : barre 1100, tiroir 1200, menus 1300) — à 99999 il cachait
      // « Accueil » et « Favoris » du menu ☰. Téléphone en paysage : en bas, plutôt que sur le titre de la bannière.
      'html.gc-modern .gc-banner{top:calc(var(--gc-header-h,48px) + .6em)!important;z-index:1050!important}',
      '@media (max-height:500px){html.gc-modern .gc-banner{top:auto!important;bottom:calc(.6em + env(safe-area-inset-bottom,0px))}}',
      // téléphone : la barre contenait plus de boutons qu'elle n'a de place (Jellyfin Enhanced, tchat, cloche, SyncPlay,
      // Lire sur, Rechercher, Mon compte) ; le surplus débordait à GAUCHE, sur le bouton ☰ (seul accès aux
      // bibliothèques). Les boutons de Jellyfin Enhanced (⋯, Aléatoire) sont retirés sous 600 px — Découvrir, Demandes et
      // Calendrier restent dans la rangée d'onglets —, les icônes passent à 40 px, ☰ reste au-dessus de tout, et
      // SyncPlay est caché sur les plus petits écrans (sous 360 px, ou 380 px avec le bouton Retour des applis).
      '@media (max-width:600px){' +
        'html.gc-modern header.MuiAppBar-root #je-header-buttons-group{display:none!important}' +
        'html.gc-modern header.MuiAppBar-root .je-header-actions-host>.MuiIconButton-root,html.gc-modern header.MuiAppBar-root .je-header-actions-host>.headerButton{width:40px!important;min-width:40px!important;height:44px!important;padding:8px!important;flex:0 0 auto!important}' +
        'html.gc-modern header.MuiAppBar-root .MuiToolbar-root>button:first-child{position:relative;z-index:2;flex-shrink:0}' +
        'html.gc-modern header.MuiAppBar-root .je-header-actions-host{overflow-x:clip}' +
      '}',
      '@media (max-width:359px){html.gc-modern header.MuiAppBar-root .je-header-actions-host>[aria-controls="app-sync-play-menu"]{display:none!important}}',
      '@media (max-width:380px){html.gc-modern header.MuiAppBar-root .MuiToolbar-root:has(>button~button) .je-header-actions-host>[aria-controls="app-sync-play-menu"]{display:none!important}}',
      // là où la rangée d'onglets est affichée, ses doublons de la barre disparaissent
      'body.gc-home header.MuiAppBar-root .MuiStack-root > a[href="#/home?tab=1"],body.gc-home #je-native-tabs-group{display:none!important}',
      // téléphone : rangée plus serrée, icônes retirées sous 480 px ; elle défile quand même sous ~430 px (fondu au bord)
      '@media (max-width:600px){nav.gc-tabs{justify-content:flex-start;gap:6px;padding:0 10px 6px}nav.gc-tabs a{font-size:14px;padding:8px 12px;min-height:36px;box-sizing:border-box}}',
      '@media (max-width:480px){nav.gc-tabs svg{display:none}nav.gc-tabs a{font-size:13.5px;padding:8px 11px}}',
      // téléphone en paysage : la barre fixe et la rangée prenaient 89 px sur 390
      '@media (max-height:500px) and (pointer:coarse){nav.gc-tabs{gap:6px;padding:0 10px 5px}nav.gc-tabs a{font-size:13.5px;padding:5px 12px;min-height:0}nav.gc-tabs svg{width:15px;height:15px}}',
      // PC : rangée montée dans la barre, au centre, quand la place libre le permet (placeInline)
      'header.MuiAppBar-root > nav.gc-tabs.gc-inline{position:absolute;padding:0;align-items:center;justify-content:flex-start;overflow:visible;-webkit-mask-image:none;mask-image:none}',
      // version compacte, si la normale ne tient pas dans la barre (beaucoup de bibliothèques, écran de 1600 px)
      'header.MuiAppBar-root > nav.gc-tabs.gc-compact{gap:.25em}',
      'header.MuiAppBar-root > nav.gc-tabs.gc-compact a{font-size:14px;padding:.34em .8em}',
      'header.MuiAppBar-root > nav.gc-tabs.gc-compact svg{display:none}',
      // fondu au bord qui cache encore des onglets (classes posées par edges())
      'nav.gc-tabs.gc-fr{-webkit-mask-image:linear-gradient(90deg,#000 calc(100% - 32px),transparent);mask-image:linear-gradient(90deg,#000 calc(100% - 32px),transparent)}',
      'nav.gc-tabs.gc-fl{-webkit-mask-image:linear-gradient(90deg,transparent,#000 32px);mask-image:linear-gradient(90deg,transparent,#000 32px)}',
      'nav.gc-tabs.gc-fl.gc-fr{-webkit-mask-image:linear-gradient(90deg,transparent,#000 32px,#000 calc(100% - 32px),transparent);mask-image:linear-gradient(90deg,transparent,#000 32px,#000 calc(100% - 32px),transparent)}'
    ].join('\n');
    document.head.appendChild(st);
  }

  /* barre de la nouvelle interface, seulement si elle est affichée */
  function modernBar() {
    var bar = document.querySelector('header.MuiAppBar-root');
    return bar && bar.getClientRects().length ? bar : null;
  }
  /* boîte des boutons de droite (Rechercher, Lire sur, SyncPlay), même règle que le tchat et Mon compte */
  function rightBox(bar) {
    var k = bar.querySelector('a[href="#/search"], [aria-controls="app-remote-play-menu"], [aria-controls="app-sync-play-menu"]');
    var user = bar.querySelector('[aria-controls="app-user-menu"]');
    return k ? k.parentElement : user && user.parentElement && user.parentElement.previousElementSibling;
  }
  /* onglet d'accueil affiché (0, 1, 2…), ou -1 hors de l'accueil */
  function homeTab() {
    var h = location.hash || '';
    if (h === '' || h === '#' || h === '#/') return 0;
    if (!/^#\/home(\.html)?(\?|$)/.test(h)) return -1;
    var m = /[?&]tab=(\d+)/.exec(h);
    return m ? +m[1] : 0;
  }
  /* index d'onglet d'une page Jellyfin Enhanced : 2 + sa place parmi ses boutons (ordre de ses onglets) */
  function jeIndex(id) {
    var group = document.getElementById('je-native-tabs-group');
    if (!group) return -1;
    var btns = group.querySelectorAll('button[id^="je-native-tab-link-"]');
    for (var i = 0; i < btns.length; i++) if (btns[i].id === id) return 2 + i;
    return -1;
  }

  function svg(path) {
    var ns = 'http://www.w3.org/2000/svg', s = document.createElementNS(ns, 'svg'), p = document.createElementNS(ns, 'path');
    s.setAttribute('viewBox', '0 0 24 24'); s.setAttribute('aria-hidden', 'true');
    p.setAttribute('d', path); s.appendChild(p);
    return s;
  }
  function buildTabs() {
    var nav = document.createElement('nav');
    nav.className = 'gc-tabs';
    nav.setAttribute('aria-label', 'Accueil');
    nav.addEventListener('scroll', edges, { passive: true });
    S.links = TABS.map(function (t) {
      var a = document.createElement('a');
      a.href = t.href || '#/home';
      if (t.icon) a.appendChild(svg(t.icon));
      a.appendChild(document.createTextNode(t.label));
      // pages de Jellyfin Enhanced : l'adresse suffit (`#/home?tab=N`, posée par paintTabs) ; cliquer son bouton ne
      // marche plus quand il l'a replié dans son menu « ⋯ » (écran de 1366 px, 04/10)
      nav.appendChild(a);
      return { def: t, el: a };
    });
    return nav;
  }
  /* fondu à gauche/à droite tant que des onglets dépassent de ce côté */
  function edges() {
    var n = S.nav; if (!n) return;
    var m = n.scrollWidth - n.clientWidth;
    n.classList.toggle('gc-fl', m > 2 && n.scrollLeft > 2);
    n.classList.toggle('gc-fr', m > 2 && n.scrollLeft < m - 2);
  }
  /* sous 900 px (MUI md), jellyfin-web ne dessine ni son logo ni les bibliothèques : notre logo après ☰ (ou après le
     bouton Retour des applis), seulement s'il reste la place (tablette en portrait, téléphone en paysage ; pas un
     téléphone en portrait). On n'insère et ne cache que notre propre lien. */
  function placeLogo(bar) {
    var tb = bar.querySelector('.MuiToolbar-root'); if (!tb) return;
    var btns = tb.querySelectorAll(':scope > button'), ham = btns.length ? btns[btns.length - 1] : null, a = S.logo;
    if (!ham || tb.querySelector('.MuiStack-root > a[href="#/"]')) { if (a) a.hidden = true; return; }
    if (!a) { a = S.logo = document.createElement('a'); a.className = 'gc-logo'; a.href = '#/home'; a.setAttribute('aria-label', 'Accueil'); }
    // clé : largeur, nombre d'éléments ET nombre d'éléments affichés (sur l'accueil, des boutons sont cachés par CSS)
    var els = tb.querySelectorAll('a,button'), shown = 0;
    for (var j = 0; j < els.length; j++) if (els[j] !== a && els[j].offsetWidth) shown++;
    var key = innerWidth + ':' + els.length + ':' + shown;
    if (a.previousElementSibling === ham && key === S.logoKey) return; // rien n'a bougé : pas de remesure
    S.logoKey = key;
    if (a.previousElementSibling !== ham) ham.parentNode.insertBefore(a, ham.nextSibling);
    a.hidden = true; // mesure sans lui (même tour, rien n'est dessiné entre-temps)
    var hr = ham.getBoundingClientRect().right, min = innerWidth;
    for (var i = 0; i < els.length; i++) {
      if (els[i] === a || els[i] === ham) continue;
      var r = els[i].getBoundingClientRect();
      if (r.width > 0 && r.left >= hr - 1 && r.left < min) min = r.left;
    }
    a.hidden = min - hr < 192;
  }
  function paintTabs(bar, tab) {
    if (!S.nav) S.nav = buildTabs();
    if (S.nav.parentElement !== bar) bar.appendChild(S.nav);
    S.nav.hidden = tab < 0;
    var onEl = null;
    S.links.forEach(function (l) {
      var idx = l.def.je ? jeIndex(l.def.je) : l.def.tab;
      if (l.def.je) {
        l.el.hidden = idx < 0; // onglet absent chez Jellyfin Enhanced : pas d'onglet
        if (idx >= 0) l.el.href = '#/home?tab=' + idx;
      }
      var on = idx === tab;
      l.el.classList.toggle('gc-on', on);
      if (on) { onEl = l.el; l.el.setAttribute('aria-current', 'page'); } else l.el.removeAttribute('aria-current');
    });
    // onglet actif amené au centre de la rangée quand il change (ou quand la largeur change) : jamais à chaque tour,
    // pour ne pas contrarier un défilement du membre
    var w = S.nav.clientWidth;
    if (onEl && w && (onEl !== S.onEl || w !== S.navW)) {
      var nr = S.nav.getBoundingClientRect(), r = onEl.getBoundingClientRect();
      S.nav.scrollLeft += (r.left + r.width / 2) - (nr.left + nr.width / 2);
    }
    S.onEl = w ? onEl : null; S.navW = w;
    edges();
  }

  /* PC (04/10) : la rangée d'onglets prenait une 2ᵉ ligne alors que le milieu de la barre était vide. Elle y monte,
     centrée dans la fenêtre (ou dans l'espace libre), si elle tient avec 24 px de marge entre les bibliothèques et les
     boutons de droite ; sinon elle reste sous la barre (téléphone, fenêtre étroite, beaucoup de bibliothèques). La
     mesure ne dépend pas de la place de la rangée (elle est hors de la barre d'outils) : pas de va-et-vient. */
  var INLINE_MARGIN = 16;
  function placeInline(bar) {
    var nav = S.nav, tb = bar.querySelector('.MuiToolbar-root');
    if (!nav || nav.hidden || !tb) return;
    var links = [], all = nav.querySelectorAll('a');
    for (var i = 0; i < all.length; i++) if (!all[i].hidden) links.push(all[i]);
    if (!links.length) return;
    function width() { return links[links.length - 1].getBoundingClientRect().right - links[0].getBoundingClientRect().left; }
    var rb = rightBox(bar), left = 0, right = innerWidth;
    var els = tb.querySelectorAll('a, button, .headerButton, #netflix-bell');
    for (var j = 0; j < els.length; j++) {
      var r = els[j].getBoundingClientRect();
      if (r.width < 1) continue;
      var after = rb && (rb.contains(els[j]) || (rb.compareDocumentPosition(els[j]) & Node.DOCUMENT_POSITION_FOLLOWING));
      if (after) right = Math.min(right, r.left); else left = Math.max(left, r.right);
    }
    // rien n'a bougé (largeur, bibliothèques, boutons, onglets, polices) : rien à refaire
    var key = innerWidth + ':' + Math.round(left) + ':' + Math.round(right) + ':' + links.length + ':' + Math.round(width());
    if (key === S.inlineKey) return;
    var room = right - left - 2 * INLINE_MARGIN;
    nav.classList.remove('gc-compact');
    var w = width(), compact = false;
    if (w > room) { nav.classList.add('gc-compact'); w = width(); compact = w <= room; if (!compact) nav.classList.remove('gc-compact'); }
    S.inlineKey = innerWidth + ':' + Math.round(left) + ':' + Math.round(right) + ':' + links.length + ':' + Math.round(width());
    var fits = w > 0 && w <= room, x = 0;
    if (fits) {
      x = innerWidth / 2 - w / 2;
      if (x < left + INLINE_MARGIN || x + w > right - INLINE_MARGIN) x = left + (right - left - w) / 2;
      x = Math.round(x - bar.getBoundingClientRect().left);
    }
    nav.classList.toggle('gc-inline', fits);
    nav.style.left = fits ? x + 'px' : '';
    nav.style.top = fits ? tb.offsetTop + 'px' : '';
    nav.style.height = fits ? tb.offsetHeight + 'px' : '';
    edges();
  }

  /* textes anglais connus → français : attributs, puis textes des liens et boutons (sans remplacer les nœuds) */
  function translate(scope) {
    if (!scope) return;
    var els = scope.querySelectorAll('[aria-label], [title], [data-header-label]');
    for (var i = 0; i < els.length; i++) {
      for (var j = 0; j < ATTRS.length; j++) {
        var v = els[i].getAttribute(ATTRS[j]);
        if (v && FR.hasOwnProperty(v)) els[i].setAttribute(ATTRS[j], FR[v]);
      }
    }
    var w = document.createTreeWalker(scope, NodeFilter.SHOW_TEXT, null), n;
    while ((n = w.nextNode())) {
      var t = n.nodeValue, k = t.trim();
      if (k && FR.hasOwnProperty(k)) n.nodeValue = t.replace(k, FR[k]);
    }
  }

  function translateMediaBar() {
    var els = document.querySelectorAll('#slides-container .play-text, #slides-container .action-label');
    for (var i = 0; i < els.length; i++) {
      var k = (els[i].textContent || '').trim();
      if (MEDIABAR.hasOwnProperty(k)) els[i].textContent = MEDIABAR[k];
    }
    // ligne d'infos : « 1 Season », « 3 Seasons » (texte seul, sans icône)
    var specs = document.querySelectorAll('#slides-container .spec-item');
    for (var j = 0; j < specs.length; j++) {
      if (specs[j].children.length) continue;
      var m = /^\s*(\d+)\s+Seasons?\s*$/.exec(specs[j].textContent || '');
      if (m) specs[j].textContent = m[1] + (+m[1] > 1 ? ' saisons' : ' saison');
    }
  }

  /* textes anglais des extensions hors de la barre : section de Jellyfin Enhanced du menu ☰, menu du compte, lien des
     réglages, langues audio des fiches (« Japanese » → « Japonais ») et taille des fichiers (« 1.2 GB » → « 1,2 Go ») */
  var LANGS = null;
  try { LANGS = new Intl.DisplayNames(['fr'], { type: 'language' }); } catch (e) { /* navigateur ancien */ }
  var UNITS = { Bytes: 'octets', KB: 'Ko', MB: 'Mo', GB: 'Go', TB: 'To', PB: 'Po' };
  function translateExtras() {
    translate(document.querySelector('.jellyfinEnhancedSection'));
    translate(document.getElementById('app-user-menu'));
    translate(document.getElementById('jellyfinEnhancedUserPrefsLink'));
    translate(document.getElementById('je-header-launcher-panel'));
    translate(document.querySelector('.mainDrawer .pluginMenuOptions')); // ancienne interface : « Modular Home »
    if (LANGS) {
      var ls = document.querySelectorAll('.audio-language-item[data-lang]:not([data-gc-fr])');
      for (var i = 0; i < ls.length; i++) {
        var code = ls[i].getAttribute('data-lang'), name = null;
        ls[i].setAttribute('data-gc-fr', '');
        if (/^(zxx|und|mis|mul)$/i.test(code)) continue;
        try { name = LANGS.of(code); } catch (e) { /* code inconnu */ }
        if (!name || name.toLowerCase() === code.toLowerCase()) continue;
        for (var c = ls[i].firstChild; c; c = c.nextSibling)
          if (c.nodeType === 3 && c.nodeValue.trim()) { c.nodeValue = name.charAt(0).toUpperCase() + name.slice(1); break; }
      }
    }
    var fs = document.querySelectorAll('.mediaInfoItem-fileSize');
    for (var k = 0; k < fs.length; k++) {
      var tn = fs[k].lastChild, m = tn && tn.nodeType === 3 && /^\s*(\d+(?:\.\d+)?)\s*(Bytes|KB|MB|GB|TB|PB)\s*$/.exec(tn.nodeValue);
      if (m) tn.nodeValue = m[1].replace('.', ',') + ' ' + UNITS[m[2]];
    }
  }

  /* cloche NotifySync : dans la barre visible, juste après le tchat (sinon avant les boutons de Jellyfin) */
  function placeBell(box) {
    var bell = document.getElementById('netflix-bell');
    if (!bell || !box) return;
    var chat = null;
    for (var c = box.firstElementChild; c; c = c.nextElementSibling) if (c.classList.contains('gc-chat-btn')) { chat = c; break; }
    if (chat) { if (chat.nextElementSibling !== bell) box.insertBefore(bell, chat.nextSibling); return; }
    if (bell.parentElement === box) return;
    var first = null;
    for (var d = box.firstElementChild; d; d = d.nextElementSibling) if (d.classList.contains('MuiButtonBase-root')) { first = d; break; }
    box.insertBefore(bell, first);
  }

  var timer = null;
  function stop() {
    clearInterval(timer);
    window.removeEventListener('hashchange', tick);
    window.removeEventListener('scroll', scrolled);
    window.removeEventListener('resize', tick);
    if (S.nav && S.nav.parentElement) S.nav.parentElement.removeChild(S.nav);
    if (S.logo && S.logo.parentElement) S.logo.parentElement.removeChild(S.logo);
    var st = document.getElementById(CSS_ID);
    if (st) st.parentElement.removeChild(st);
  }

  function tick() {
    if (window.__gcHeaderV !== VERSION) { stop(); return; }
    if (document.hidden || !document.body) return;
    translateMediaBar(); // ancienne et nouvelle interface : la bannière est la même
    translateExtras();
    var bar = modernBar();
    var tab = bar ? homeTab() : -1;
    document.body.classList.toggle('gc-home', !!bar && tab >= 0);
    document.documentElement.classList.toggle('gc-modern', !!bar);
    if (!bar) { if (S.nav) S.nav.hidden = true; return; }
    css();
    paintTabs(bar, tab);
    placeLogo(bar);
    placeInline(bar);
    var h = Math.round(bar.getBoundingClientRect().height);
    if (h !== S.h) { S.h = h; document.documentElement.style.setProperty('--gc-header-h', h + 'px'); }
    translate(bar);
    placeBell(rightBox(bar));
  }

  /* page défilée : la barre devient opaque sur téléphone et tablette (règle dans groscailloux-tv.css) ; tout en haut
     de l'accueil, elle reste translucide sur la bannière */
  function scrolled() {
    var y = window.scrollY || (document.scrollingElement && document.scrollingElement.scrollTop) || 0;
    document.documentElement.classList.toggle('gc-scrolled', y > 8);
  }
  window.addEventListener('scroll', scrolled, { passive: true });
  window.addEventListener('hashchange', tick);
  window.addEventListener('resize', tick); // rotation : logo et rangée replacés sans attendre le tour suivant
  timer = setInterval(tick, 1000);
  tick();
})();
