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
// Rien ne change dans l'ancienne interface ni sur les télés : tout dépend de la barre moderne visible.
// Ne retire jamais un élément dessiné par React (il le retirerait lui-même ensuite et planterait) : on cache,
// on déplace nos propres éléments, on change des textes et des attributs.
// Déposé dans JavaScript Injector (« Groscailloux En-tête », privé) par scripts/jellyfin-js-apply.py.
(function () {
  'use strict';
  if (window.__gcHeader) return;
  window.__gcHeader = true;

  var LOGO = '/web/banner-light.gc.png'; // servi par Jellyfin Enhanced (nos images de marque), quel que soit le nom
  /* anglais → français, pour les seuls textes connus de la barre et du panneau de Jellyfin Enhanced */
  var FR = {
    'Favorites': 'Favoris', 'More': 'Plus', 'Search': 'Rechercher', 'Cast to Device': 'Diffuser sur l’appareil',
    'User Menu': 'Menu utilisateur', 'Open Menu': 'Ouvrir le menu', 'Close Menu': 'Fermer le menu',
    'Requests': 'Demandes', 'Calendar': 'Calendrier', 'Recommandations Seerr': 'Découvrir', 'Recommendations': 'Découvrir',
    'Discover': 'Découvrir', 'More from Jellyfin Enhanced': 'Plus', 'Random item': 'Élément aléatoire'
  };
  var ATTRS = ['aria-label', 'title', 'data-header-label'];
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

  function css() {
    if (document.getElementById('gc-header-css')) return;
    var logo = 'header.MuiAppBar-root .MuiToolbar-root > .MuiStack-root > a[href="#/"]';
    var st = document.createElement('style');
    st.id = 'gc-header-css';
    st.textContent = [
      logo + '{font-size:0!important;min-width:0!important}',
      logo + ' > .MuiButton-startIcon{display:none!important}',
      logo + '::after{content:"";display:block;width:176px;height:25px;background:url("' + LOGO + '") left center/contain no-repeat}',
      '.gc-tabs{display:flex;flex-wrap:nowrap!important;justify-content:center;gap:.5em;padding:.1em 1em .6em;overflow-x:auto;scrollbar-width:none;-webkit-overflow-scrolling:touch}',
      '.gc-tabs::-webkit-scrollbar{display:none}',
      '.gc-tabs[hidden]{display:none!important}',
      '.gc-tabs a{display:inline-flex;align-items:center;gap:.45em;flex:0 0 auto;padding:.42em 1.05em;border-radius:.55em;' +
        'background:rgba(28,30,35,.78);border:1px solid rgba(255,255,255,.1);color:#f2f3f5;font-family:inherit;font-size:15px;' +
        'font-weight:500;line-height:1.2;text-decoration:none;white-space:nowrap;cursor:pointer}',
      '.gc-tabs a:hover{background:rgba(60,63,70,.92)}',
      '.gc-tabs a.gc-on{background:#fff;color:#0b0b0c;border-color:#fff}',
      '.gc-tabs a:focus-visible{outline:2px solid #8fd3fb;outline-offset:2px}',
      '.gc-tabs svg{width:18px;height:18px;fill:currentColor;flex:0 0 auto}',
      // bandeau d'annonce du tchat : sous la barre réelle (plus haute avec la rangée d'onglets), pas à 5,2 em
      'html.gc-modern .gc-banner{top:calc(var(--gc-header-h,48px) + .6em)!important}',
      // là où la rangée d'onglets est affichée, ses doublons de la barre disparaissent
      'body.gc-home header.MuiAppBar-root .MuiStack-root > a[href="#/home?tab=1"],body.gc-home #je-native-tabs-group{display:none!important}',
      '@media (max-width:600px){.gc-tabs{justify-content:flex-start;padding:.1em .6em .5em}.gc-tabs a{font-size:14px;padding:.38em .85em}}'
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
  function paintTabs(bar, tab) {
    if (!S.nav) S.nav = buildTabs();
    if (S.nav.parentElement !== bar) bar.appendChild(S.nav);
    S.nav.hidden = tab < 0;
    S.links.forEach(function (l) {
      var idx = l.def.je ? jeIndex(l.def.je) : l.def.tab;
      if (l.def.je) {
        l.el.hidden = idx < 0; // onglet absent chez Jellyfin Enhanced : pas d'onglet
        if (idx >= 0) l.el.href = '#/home?tab=' + idx;
      }
      var on = idx === tab;
      l.el.classList.toggle('gc-on', on);
      if (on) l.el.setAttribute('aria-current', 'page'); else l.el.removeAttribute('aria-current');
    });
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

  function tick() {
    if (document.hidden || !document.body) return;
    var bar = modernBar();
    var tab = bar ? homeTab() : -1;
    document.body.classList.toggle('gc-home', !!bar && tab >= 0);
    document.documentElement.classList.toggle('gc-modern', !!bar);
    if (!bar) { if (S.nav) S.nav.hidden = true; return; }
    css();
    paintTabs(bar, tab);
    var h = Math.round(bar.getBoundingClientRect().height);
    if (h !== S.h) { S.h = h; document.documentElement.style.setProperty('--gc-header-h', h + 'px'); }
    translate(bar);
    translate(document.getElementById('je-header-launcher-panel'));
    placeBell(rightBox(bar));
  }

  window.addEventListener('hashchange', tick);
  setInterval(tick, 1000);
  tick();
})();
