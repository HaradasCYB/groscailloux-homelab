// « Groscailloux Durée » (27/10) : filtre « Durée » (moins de 2 h, moins de 1 h 30) dans le menu Filtres des
// bibliothèques de FILMS de la nouvelle interface 12.1 (Films, Films d'animation). Script privé de JavaScript Injector.
// Runbook : docs/runbooks/jellyfin-interface.md § 12.
//
// Le menu Filtres est un composant React (popover MUI `#filter-popover`) ; GET /Items n'a aucun paramètre de durée.
//   - L'entrée « Durée » est NOTRE bloc, inséré juste avant « Réinitialiser les filtres » (aucun nœud React retiré ni
//     déplacé ; seul le nôtre est replacé si React insère une rubrique après coup).
//   - Le choix est posé dans l'état React de la vue (`setLibraryViewSettings`, retrouvé par la fibre du menu) sous
//     `Filters.GcMaxRunTime` (minutes) : pastille du bouton Filtres, « Réinitialiser les filtres » (Filters = {}),
//     mémorisation par bibliothèque et nouvelle requête de la page viennent donc de jellyfin-web lui-même.
//   - La requête de la page (SDK → axios → XMLHttpRequest, `startIndex`/`limit`) est servie par nous quand le filtre
//     est posé : la liste complète de la même requête (mêmes filtres, même tri, sans pagination) est lue UNE fois,
//     gardée 2 min (4 listes au plus, vidées à toute écriture), filtrée sur `RunTimeTicks`, puis découpée à la page demandée avec le bon
//     total. La requête d'origine part avec `limit=1` (sa réponse est remplacée) ; en cas d'échec, elle part telle quelle.
// Limites : « Tout lire » et « Aléatoire » de la barre ignorent la durée ; ancienne interface (télé, « legacy ») : rien.
(function () {
  'use strict';
  var VERSION = 1;
  if (window.__gcDureeV && window.__gcDureeV >= VERSION) return;
  window.__gcDureeV = VERSION;

  var FKEY = 'GcMaxRunTime';
  var OPTS = [[120, 'Moins de 2 h'], [90, 'Moins de 1 h 30']];
  var TICKS_PER_MIN = 600000000;
  var CACHE_MS = 120000;
  var S = { cache: {}, state: {}, served: 0, full: 0 };
  window.__gcDuree = S;

  function fiber(el) {
    var ks = Object.keys(el);
    for (var i = 0; i < ks.length; i++) if (ks[i].indexOf('__reactFiber$') === 0) return el[ks[i]];
    return null;
  }
  // props du bouton Filtres (FilterButton de jellyfin-web) en remontant la fibre React d'un élément. Seuls le setter
  // (stable), viewType et parentId y sont lus : la fibre accrochée à un nœud peut être l'ancienne des deux copies de
  // React et ses props retarder d'un rendu (banc du 27/10 : passage de 1 h 30 à 2 h servi avec 1 h 30).
  function filterProps(el, depth) {
    var f = el && fiber(el);
    for (var i = 0; f && i < depth; i++, f = f.return) {
      var p = f.memoizedProps;
      if (p && typeof p.setLibraryViewSettings === 'function' && 'hasFilters' in p && 'itemType' in p) return p;
    }
    return null;
  }
  function pageProps(parentId) {
    var bs = document.querySelectorAll('.MuiButtonGroup-root button');
    for (var i = 0; i < bs.length; i++) {
      var p = filterProps(bs[i], 12);
      if (p && (!parentId || p.parentId === parentId)) return p;
    }
    return null;
  }
  // Durée en vigueur par bibliothèque : notre propre suivi (posée par nos cases, remise à 0 par « Réinitialiser les
  // filtres »), sinon la valeur mémorisée par jellyfin-web pour la vue (localStorage « movies - <id> », rechargement).
  function current(parentId) {
    if (!parentId) return 0;
    if (Object.prototype.hasOwnProperty.call(S.state, parentId)) return S.state[parentId];
    try {
      var v = JSON.parse(localStorage.getItem('movies - ' + parentId) || '{}');
      return (v && v.Filters && +v.Filters[FKEY]) || 0;
    } catch (e) { return 0; }
  }
  function setMax(p, v) {
    S.state[p.parentId] = v || 0;
    p.setLibraryViewSettings(function (s) {
      var F = {}, o = (s && s.Filters) || {};
      for (var k in o) if (Object.prototype.hasOwnProperty.call(o, k) && k !== FKEY) F[k] = o[k];
      if (v) F[FKEY] = v;
      var n = {}; for (var k2 in s) if (Object.prototype.hasOwnProperty.call(s, k2)) n[k2] = s[k2];
      n.StartIndex = 0; n.Filters = F;
      return n;
    });
  }

  // ------------------------------------------------------------------ bloc « Durée » du menu Filtres
  function css() {
    if (document.getElementById('gc-duree-css')) return;
    var st = document.createElement('style'); st.id = 'gc-duree-css';
    st.textContent = '.gc-dur-body{padding:8px 16px 12px;border-top:1px solid rgba(0,0,0,.125)}'
      + '.gc-dur-body label{display:flex;align-items:center;gap:10px;min-height:40px;cursor:pointer}'
      + '.gc-dur-body input{width:18px;height:18px;margin:0 2px;flex:none;cursor:pointer}';
    document.head.appendChild(st);
  }
  function build(paper) {
    var acc = paper.querySelector('.MuiAccordion-root');
    if (!acc) return null;
    var head = acc.querySelector('.MuiAccordion-heading'), sum = acc.querySelector('.MuiAccordionSummary-root');
    var cont = acc.querySelector('.MuiAccordionSummary-content'), typo = cont && cont.querySelector('p');
    var iw = acc.querySelector('.MuiAccordionSummary-expandIconWrapper');
    if (!head || !sum || !cont || !typo || !iw) return null;
    css();
    var strip = function (c) { return c.replace(/\bMui-expanded\b/g, '').replace(/\s+/g, ' ').trim(); };
    var box = document.createElement('div'); box.className = strip(acc.className) + ' gc-dur';
    var h3 = document.createElement('h3'); h3.className = strip(head.className);
    var btn = document.createElement('button'); btn.type = 'button'; btn.className = strip(sum.className);
    btn.id = 'gc-dur-header'; btn.setAttribute('aria-expanded', 'false'); btn.setAttribute('aria-controls', 'gc-dur-content');
    var sc = document.createElement('span'); sc.className = strip(cont.className);
    var pt = document.createElement('p'); pt.className = typo.className; pt.textContent = 'Durée'; sc.appendChild(pt);
    var wi = document.createElement('span'); wi.className = strip(iw.className);
    if (iw.firstElementChild) wi.appendChild(iw.firstElementChild.cloneNode(true));
    btn.appendChild(sc); btn.appendChild(wi); h3.appendChild(btn); box.appendChild(h3);
    var body = document.createElement('div'); body.className = 'gc-dur-body'; body.id = 'gc-dur-content'; body.hidden = true;
    body.setAttribute('role', 'region'); body.setAttribute('aria-labelledby', 'gc-dur-header');
    var inputs = OPTS.map(function (o) {
      var l = document.createElement('label'), i = document.createElement('input');
      i.type = 'checkbox'; i.value = String(o[0]);
      i.addEventListener('change', function () {
        var p = filterProps(paper, 30);
        if (!p) return;
        var v = i.checked ? o[0] : 0;
        inputs.forEach(function (x) { x.checked = !!v && +x.value === v; });   // un seul choix à la fois
        setMax(p, v);
      });
      l.appendChild(i); l.appendChild(document.createTextNode(o[1])); body.appendChild(l);
      return i;
    });
    box.appendChild(body);
    btn.addEventListener('click', function () {
      var open = body.hidden;
      body.hidden = !open;
      btn.setAttribute('aria-expanded', open ? 'true' : 'false');
      [box, btn, wi].forEach(function (e) { e.classList.toggle('Mui-expanded', open); });
    });
    box.__gcSync = function () { var p = filterProps(paper, 30), v = current(p && p.parentId); inputs.forEach(function (x) { x.checked = +x.value === v; }); };
    box.__gcSync();
    return box;
  }
  function resetButton(paper) {
    var kids = paper.children;
    for (var i = kids.length - 1; i >= 0; i--) if (kids[i].tagName === 'BUTTON') return kids[i];
    return null;
  }
  function place(paper, box) {
    var reset = resetButton(paper);
    if (reset && box.nextSibling !== reset) paper.insertBefore(box, reset);
    else if (!reset && box.parentNode !== paper) paper.appendChild(box);
  }
  function setup(pop) {
    var paper = pop.querySelector('.MuiPopover-paper');
    if (!paper || paper.querySelector('.gc-dur')) return;
    var p = filterProps(paper, 30);
    if (!p || p.viewType !== 'movies') return;          // films seulement (Films, Films d'animation)
    var box = build(paper);
    if (!box) return;
    place(paper, box);
    // React peut ajouter une rubrique (genres chargés après coup) : on replace seulement NOTRE bloc
    var mo = new MutationObserver(function () { if (!document.body.contains(paper)) { mo.disconnect(); return; } place(paper, box); setTimeout(box.__gcSync, 0); });
    mo.observe(paper, { childList: true });
    // « Réinitialiser les filtres » vide Filters : notre suivi passe à 0 AVANT la requête (notre écouteur est sous celui
    // de React, posé sur le conteneur du portail), puis nos cases suivent
    paper.addEventListener('click', function (e) {
      var r = resetButton(paper);
      if (r && r.contains(e.target) && !r.disabled) { S.state[p.parentId] = 0; setTimeout(box.__gcSync, 60); }
    });
  }
  function scan(node) {
    if (!node || node.nodeType !== 1) return;
    var pop = node.id === 'filter-popover' ? node : (node.querySelector && node.querySelector('#filter-popover'));
    if (pop) setup(pop);
  }
  function observe() {
    if (!document.body || !window.MutationObserver) return;
    // le popover est rendu dans un portail, enfant direct de <body> : aucun sous-arbre observé
    new MutationObserver(function (recs) {
      for (var i = 0; i < recs.length; i++) for (var j = 0; j < recs[i].addedNodes.length; j++) scan(recs[i].addedNodes[j]);
    }).observe(document.body, { childList: true });
  }
  if (document.body) observe(); else document.addEventListener('DOMContentLoaded', observe);

  // ------------------------------------------------------------------ requête de la page
  function params(url) {
    var q = url.indexOf('?'), out = [];
    if (q < 0) return out;
    url.slice(q + 1).split('&').forEach(function (kv) {
      if (!kv) return;
      var i = kv.indexOf('='), k = i < 0 ? kv : kv.slice(0, i), v = i < 0 ? '' : kv.slice(i + 1);
      out.push([k, v]);
    });
    return out;
  }
  function get(ps, name) { name = name.toLowerCase(); for (var i = 0; i < ps.length; i++) if (ps[i][0].toLowerCase() === name) return decodeURIComponent(ps[i][1]); return null; }
  function rebuild(url, ps) { return url.split('?')[0] + '?' + ps.map(function (p) { return p[0] + '=' + p[1]; }).join('&'); }
  function without(ps, names) { return ps.filter(function (p) { return names.indexOf(p[0].toLowerCase()) < 0; }); }

  // la requête d'une page de la vue Films avec le filtre posé → {full, small, start, limit, max}, sinon null
  function plan(method, url) {
    if (String(method).toUpperCase() !== 'GET' || !/\/Items\?/.test(url)) return null;
    var ps = params(url), parentId = get(ps, 'parentId');
    if (!parentId || get(ps, 'ids') || get(ps, 'startIndex') === null || !/(^|,)Movie(,|$)/.test(get(ps, 'includeItemTypes') || '')) return null;
    var p = pageProps(parentId);
    if (!p || p.viewType !== 'movies') return null;
    var max = current(parentId);
    if (!max) return null;
    var limit = get(ps, 'limit');
    var base = without(ps, ['startindex', 'limit', 'enabletotalrecordcount']);
    return {
      full: rebuild(url, base.concat([['enableTotalRecordCount', 'false']])),
      small: rebuild(url, base.concat([['startIndex', '0'], ['limit', '1']])),
      start: +get(ps, 'startIndex') || 0, limit: limit === null ? null : +limit, max: max
    };
  }
  function fullList(pl, headers) {
    var c = S.cache[pl.full];
    if (c && Date.now() - c.at < CACHE_MS) return Promise.resolve(c.items);
    S.full++;
    return oFetch(pl.full, { headers: headers, credentials: 'same-origin' }).then(function (r) {
      if (!r.ok) throw new Error('HTTP ' + r.status);
      return r.json();
    }).then(function (j) {
      var items = (j && j.Items) || [];
      // 4 listes gardées au plus (la plus ancienne part) : retour d'un filtre combiné, page suivante
      var ks = Object.keys(S.cache);
      if (ks.length >= 4) { ks.sort(function (a, b) { return S.cache[a].at - S.cache[b].at; }); delete S.cache[ks[0]]; }
      S.cache[pl.full] = { at: Date.now(), items: items };
      return items;
    });
  }
  function page(pl, items) {
    var lim = pl.max * TICKS_PER_MIN;
    var kept = items.filter(function (it) { return it && it.RunTimeTicks && it.RunTimeTicks < lim; });
    var slice = pl.limit === null ? kept.slice(pl.start) : kept.slice(pl.start, pl.start + pl.limit);
    return { Items: slice, TotalRecordCount: kept.length, StartIndex: pl.start };
  }

  var oFetch = window.fetch.bind(window);
  var XP = window.XMLHttpRequest && XMLHttpRequest.prototype;
  var dText = XP && Object.getOwnPropertyDescriptor(XP, 'responseText');
  var dResp = XP && Object.getOwnPropertyDescriptor(XP, 'response');
  if (!XP || !dText || !dText.get || !dResp || !dResp.get || XP.__gcDuree) return;
  XP.__gcDuree = true;
  var xOpen = XP.open, xSend = XP.send, xHeader = XP.setRequestHeader;
  XP.open = function (method, url) {
    if (String(method).toUpperCase() !== 'GET') S.cache = {};   // une écriture (vu, favori…) : liste à relire
    var pl = null;
    try { pl = plan(method, String(url)); } catch (e) { pl = null; }
    this.__gcDur = pl ? { pl: pl, args: Array.prototype.slice.call(arguments), headers: [] } : null;
    return xOpen.apply(this, arguments);
  };
  XP.setRequestHeader = function (k, v) {
    if (this.__gcDur) this.__gcDur.headers.push([k, v]);
    return xHeader.apply(this, arguments);
  };
  XP.send = function () {
    var st = this.__gcDur;
    if (!st) return xSend.apply(this, arguments);
    var x = this, args = arguments, hdrs = {};
    st.headers.forEach(function (h) { hdrs[h[0]] = h[1]; });
    fullList(st.pl, hdrs).then(function (items) {
      var body = JSON.stringify(page(st.pl, items)), obj = null;
      Object.defineProperty(x, 'responseText', { configurable: true, get: function () { return x.readyState === 4 ? body : dText.get.call(x); } });
      Object.defineProperty(x, 'response', { configurable: true, get: function () {
        if (x.readyState !== 4) return dResp.get.call(x);
        if (x.responseType === 'json') { if (!obj) obj = JSON.parse(body); return obj; }
        return x.responseType && x.responseType !== 'text' ? dResp.get.call(x) : body;
      } });
      // même requête en version minuscule (limit=1) : en-têtes rejoués après le nouvel open
      var a = st.args.slice(); a[1] = st.pl.small;
      x.__gcDur = null;
      xOpen.apply(x, a);
      st.headers.forEach(function (h) { xHeader.apply(x, h); });
      S.served++;
      xSend.apply(x, args);
    }).catch(function () {
      try { x.__gcDur = null; xSend.apply(x, args); } catch (e) { /* annulée entre-temps */ }
    });
  };
})();
