// « Groscailloux Continuer » (27/10) : retirer un titre de « Continuer à regarder » et « À suivre », par compte.
// Script privé de JavaScript Injector. Runbook : docs/runbooks/jellyfin-interface.md § 11.
//
// Décision de l'admin : MASQUER seulement. Ni la progression, ni l'historique, ni « vu » ne sont touchés. Un titre
// masqué revient tout seul dès que le membre relance ou avance un épisode du titre APRÈS le masquage : on garde la
// date de dernière lecture du titre au moment du masquage (date du SERVEUR, jamais l'horloge de l'appareil) et on la
// compare à celle des lectures suivantes. Une série se masque entière (tous ses épisodes, dans les deux rangées).
//
// Stockage : DisplayPreferences du compte (id « gc-continue », client « groscailloux », CustomPrefs.gcHidden =
// {"<id série ou film>": {n: nom, y: type, t: dernière lecture en ms, s: date du masquage en ms}}) : côté serveur,
// donc le même sur tous les appareils du membre ; lu au plus une fois par minute, relu avant chaque écriture.
//
// Qui dessine les rangées : Home Screen Sections (`/HomeScreen/Section/ContinueWatching|NextUp`, par fetch) ; Continue
// Watching Deduplicator ne garde qu'un épisode par série en amont. Ailleurs (onglet Suggestions des bibliothèques,
// autres extensions) : `/UserItems/Resume` et `/Shows/NextUp` (sans `seriesId` : le bouton « À suivre » d'une fiche
// de série n'est jamais filtré). Le filtre s'applique aux RÉPONSES de ces seules requêtes GET, dans la page.
//
// Point d'entrée : « Retirer de Continuer à regarder » dans la feuille « … » d'une vignette de ces rangées, et dans
// le menu de la sélection multiple (appui long sur téléphone : la 12.1 y ouvre la sélection, pas la feuille).
// Gestion (liste, « Remettre ») : section de Mon compte (`window.gcContinue.accountSection`). Le retrait de Jellyfin
// Enhanced (masquage par épisode, sans page de gestion chez nous) est caché par notre CSS (réglage par compte chez JE).
// Règles : aucune boucle de requêtes (une lecture des préférences + une des dernières lectures par minute au plus,
// seulement si un titre est masqué), aucun nœud React touché (les vignettes de l'accueil sont dessinées par
// l'ancien moteur de jellyfin-web : on rafraîchit le conteneur, on ne retire rien), pas de window.confirm/alert.
(function () {
  'use strict';
  var VERSION = 1;
  if (window.__gcContinueV && window.__gcContinueV >= VERSION) return;
  window.__gcContinueV = VERSION;

  var PREF_ID = 'gc-continue', CLIENT = 'groscailloux', PREF_KEY = 'gcHidden';
  var TTL = 60000;            // préférences et dernières lectures relues au plus une fois par minute
  var READY_MAX = 4000;       // une rangée n'attend jamais plus de 4 s le chargement des préférences
  var MAX_ENTRIES = 200;      // au-delà, les masquages les plus anciens partent
  var RECENT = 100;           // lectures récentes examinées pour faire revenir un titre
  var LABEL = 'Retirer de Continuer à regarder';
  var ROWS = '.verticalSection.ContinueWatching, .verticalSection.NextUp, .verticalSection.ContinueWatchingNextUp';
  var TV = /web0?s|webos|tizen|smart-?tv|netcast|viera|bravia|hbbtv|aft[a-z]|android\s?tv|googletv|crkey/i
    .test(navigator.userAgent || '') || document.documentElement.classList.contains('layout-tv');

  var S = { uid: null, hidden: null, at: 0, loading: null, saving: null, pendingUnhide: {} };
  var cardItems = {};
  window.__gcContinue = S;

  function api() { return window.ApiClient; }
  function uid() { try { var a = api(); return a && a.getCurrentUserId ? a.getCurrentUserId() : null; } catch (e) { return null; } }
  function ms(iso) { var t = iso ? Date.parse(iso) : NaN; return isNaN(t) ? 0 : t; }
  function keyOf(it) {
    if (!it || !it.Id) return null;
    if ((it.Type === 'Episode' || it.Type === 'Season') && it.SeriesId) return it.SeriesId;
    return it.Id;
  }
  function count(o) { var n = 0; for (var k in o) if (Object.prototype.hasOwnProperty.call(o, k)) n++; return n; }

  // ------------------------------------------------------------------ préférences (serveur)
  function prefUrl() { return api().getUrl('DisplayPreferences/' + PREF_ID, { userId: uid(), client: CLIENT }); }
  function readPrefs() {
    return api().getJSON(prefUrl()).then(function (p) {
      var h = {};
      try { h = JSON.parse((p && p.CustomPrefs && p.CustomPrefs[PREF_KEY]) || '{}') || {}; } catch (e) { h = {}; }
      return { dto: p, hidden: h };
    });
  }
  // lecture fraîche → modification → écriture (jamais une liste de la veille écrite par-dessus celle d'un autre appareil)
  function update(mutate) {
    var run = function () {
      return readPrefs().then(function (r) {
        var h = r.hidden, changed = mutate(h);
        if (!changed) { S.hidden = h; S.at = Date.now(); return h; }
        var keys = Object.keys(h);
        if (keys.length > MAX_ENTRIES) {
          keys.sort(function (a, b) { return (h[a].s || 0) - (h[b].s || 0); });
          keys.slice(0, keys.length - MAX_ENTRIES).forEach(function (k) { delete h[k]; });
        }
        var dto = r.dto || {};
        dto.CustomPrefs = dto.CustomPrefs || {};
        dto.CustomPrefs[PREF_KEY] = JSON.stringify(h);
        return api().ajax({ type: 'POST', url: prefUrl(), data: JSON.stringify(dto), contentType: 'application/json' })
          .then(function () { S.hidden = h; S.at = Date.now(); return h; });
      });
    };
    S.saving = (S.saving || Promise.resolve()).then(run, run);
    return S.saving;
  }

  // dernières lectures du compte : un titre masqué relancé ou avancé depuis revient
  function recentPlays() {
    return api().getJSON(api().getUrl('Items', {
      userId: uid(), recursive: true, includeItemTypes: 'Movie,Episode', sortBy: 'DatePlayed', sortOrder: 'Descending',
      limit: RECENT, enableImages: false, enableUserData: true, enableTotalRecordCount: false
    })).then(function (r) { return (r && r.Items) || []; });
  }
  function replayed(h, items) {
    var out = {};
    items.forEach(function (it) {
      var k = keyOf(it), e = k && h[k], lp = it.UserData && ms(it.UserData.LastPlayedDate);
      if (e && lp && lp > (e.t || 0)) out[k] = true;
    });
    return out;
  }
  function unhideKeys(keys) {
    if (!keys.length) return Promise.resolve();
    return update(function (h) { var c = false; keys.forEach(function (k) { if (h[k]) { delete h[k]; c = true; } }); return c; });
  }

  function load(force) {
    var u = uid();
    if (!u || !api()) return Promise.resolve(null);
    if (S.uid !== u) { S.uid = u; S.hidden = null; S.at = 0; cardItems = {}; }
    if (!force && S.hidden && Date.now() - S.at < TTL) return Promise.resolve(S.hidden);
    if (S.loading) return S.loading;
    S.loading = readPrefs().then(function (r) {
      S.hidden = r.hidden; S.at = Date.now();
      if (!count(r.hidden)) return r.hidden;
      return recentPlays().then(function (items) {
        var back = Object.keys(replayed(r.hidden, items));
        return back.length ? unhideKeys(back) : r.hidden;
      }, function () { return r.hidden; });
    }).then(function (h) { S.loading = null; return h; }, function () { S.loading = null; S.at = Date.now(); return S.hidden; });
    return S.loading;
  }
  // une rangée attend les préférences au plus READY_MAX ; en cas d'échec, rien n'est filtré
  function ready() {
    return new Promise(function (res) {
      var done = false, fin = function () { if (!done) { done = true; res(); } };
      setTimeout(fin, READY_MAX);
      load(false).then(fin, fin);
    });
  }

  // ------------------------------------------------------------------ filtre des réponses
  var RX = /\/(HomeScreen\/Section\/(ContinueWatching|NextUp|ContinueWatchingNextUp)|UserItems\/Resume|Users\/[^/?]+\/Items\/Resume|Shows\/NextUp)(\?|$)/i;
  function match(url, method) {
    if (method && String(method).toUpperCase() !== 'GET') return false;
    url = String(url || '');
    if (!RX.test(url)) return false;
    if (/Shows\/NextUp/i.test(url) && /[?&]seriesId=/i.test(url)) return false;  // fiche d'une série : jamais
    return true;
  }
  function hiddenNow(it) {
    var k = keyOf(it), h = S.hidden, e = k && h && h[k];
    if (!e) return false;
    var lp = it.UserData && ms(it.UserData.LastPlayedDate);
    if (lp && lp > (e.t || 0)) {           // relancé depuis le masquage : il revient (et sort de la liste)
      if (!S.pendingUnhide[k]) { S.pendingUnhide[k] = true; unhideKeys([k]).then(function () { delete S.pendingUnhide[k]; }, function () { delete S.pendingUnhide[k]; }); }
      return false;
    }
    return true;
  }
  // renvoie le corps filtré, ou null s'il n'y a rien à changer
  function filterBody(j) {
    var arr = Array.isArray(j) ? j : (j && Array.isArray(j.Items) ? j.Items : null);
    if (!arr) return null;
    arr.forEach(function (it) {   // vignette → élément (l'id de la série n'est pas dans la vignette)
      if (it && it.Id) cardItems[it.Id] = { Id: it.Id, Type: it.Type, SeriesId: it.SeriesId, SeriesName: it.SeriesName, Name: it.Name };
    });
    if (!S.hidden || !count(S.hidden)) return null;
    var kept = arr.filter(function (it) { return !hiddenNow(it); });
    if (kept.length === arr.length) return null;
    if (Array.isArray(j)) return kept;
    var o = {}; for (var p in j) if (Object.prototype.hasOwnProperty.call(j, p)) o[p] = j[p];
    o.Items = kept;
    if (typeof o.TotalRecordCount === 'number') o.TotalRecordCount = Math.max(kept.length, o.TotalRecordCount - (arr.length - kept.length));
    return o;
  }
  S.filterBody = filterBody;

  // fetch : Home Screen Sections (ApiClient.getJSON → fetch)
  var oFetch = window.fetch;
  if (oFetch && !oFetch.__gcContinue) {
    var nf = function (input, init) {
      var url = typeof input === 'string' ? input : (input && input.url) || '';
      var method = (init && init.method) || (input && typeof input !== 'string' && input.method) || 'GET';
      if (!match(url, method)) return oFetch.apply(this, arguments);
      var p = oFetch.apply(this, arguments);
      return Promise.all([p, ready()]).then(function (r) {
        var resp = r[0];
        if (!resp || !resp.ok) return resp;
        return resp.clone().text().then(function (t) {
          var o = filterBody(JSON.parse(t));
          if (o === null) return resp;
          return new Response(JSON.stringify(o), { status: resp.status, statusText: resp.statusText, headers: { 'Content-Type': 'application/json; charset=utf-8' } });
        }).catch(function () { return resp; });
      });
    };
    nf.__gcContinue = true;
    window.fetch = nf;
  }

  // XMLHttpRequest : SDK de jellyfin-web (axios) pour /UserItems/Resume et /Shows/NextUp
  var XP = window.XMLHttpRequest && XMLHttpRequest.prototype;
  var dText = XP && Object.getOwnPropertyDescriptor(XP, 'responseText');
  var dResp = XP && Object.getOwnPropertyDescriptor(XP, 'response');
  if (XP && dText && dText.get && dResp && dResp.get && !XP.__gcContinue) {
    XP.__gcContinue = true;
    var xOpen = XP.open, xSend = XP.send;
    XP.open = function (method, url) {
      this.__gcCont = match(url, method);
      return xOpen.apply(this, arguments);
    };
    XP.send = function () {
      if (!this.__gcCont) return xSend.apply(this, arguments);
      var x = this, args = arguments, memo;
      var filtered = function () {   // corps filtré (chaîne ou objet selon responseType), ou undefined
        if (x.readyState !== 4 || x.status !== 200) return undefined;
        if (memo !== undefined) return memo.v;
        memo = { v: undefined };
        try {
          var rt = x.responseType;
          if (rt === 'json') { var o1 = filterBody(dResp.get.call(x)); if (o1 !== null) memo.v = o1; }
          else if (!rt || rt === 'text') { var o2 = filterBody(JSON.parse(dText.get.call(x))); if (o2 !== null) memo.v = JSON.stringify(o2); }
        } catch (e) { /* corps illisible : servi tel quel */ }
        return memo.v;
      };
      try {
        Object.defineProperty(x, 'responseText', { configurable: true, get: function () { var f = filtered(); return typeof f === 'string' ? f : dText.get.call(x); } });
        Object.defineProperty(x, 'response', { configurable: true, get: function () { var f = filtered(); return f !== undefined ? f : dResp.get.call(x); } });
      } catch (e) { return xSend.apply(x, args); }
      ready().then(function () { try { xSend.apply(x, args); } catch (e) { /* requête annulée entre-temps */ } });
    };
  }

  // ------------------------------------------------------------------ rangées de l'accueil
  // l'ancien moteur de jellyfin-web : notifyRefreshNeeded(true) relance fetchData (filtré) et redessine la rangée
  function refreshRows() {
    var secs = document.querySelectorAll(ROWS);
    for (var i = 0; i < secs.length; i++) {
      var ic = secs[i].querySelector('.itemsContainer');
      if (ic && typeof ic.notifyRefreshNeeded === 'function') { try { ic.notifyRefreshNeeded(true); } catch (e) { /* rangée détruite */ } }
    }
  }

  // date de la dernière lecture du titre, lue sur le SERVEUR (une série : son épisode lu le plus récemment)
  function lastPlayed(key, type) {
    var q = { userId: uid(), enableImages: false, enableUserData: true, enableTotalRecordCount: false, limit: 1 };
    if (type === 'Series') { q.parentId = key; q.recursive = true; q.includeItemTypes = 'Episode'; q.sortBy = 'DatePlayed'; q.sortOrder = 'Descending'; } else q.ids = key;
    return api().getJSON(api().getUrl('Items', q)).then(function (r) {
      var it = r && r.Items && r.Items[0];
      return it && it.UserData ? ms(it.UserData.LastPlayedDate) : 0;
    });
  }

  function hide(targets) {     // targets : [{key, n, y}]
    var now = Date.now();
    return Promise.all(targets.map(function (t) {
      return lastPlayed(t.key, t.y).then(function (lp) { t.t = lp; }, function () { t.t = now; });
    })).then(function () {
      return update(function (h) {
        targets.forEach(function (t) { h[t.key] = { n: t.n || '', y: t.y || '', t: t.t || 0, s: now }; });
        return true;
      });
    }).then(function () { refreshRows(); });
  }
  function restore(keys) {
    return unhideKeys(keys).then(function () { refreshRows(); });
  }

  // ------------------------------------------------------------------ message après un retrait
  function css() {
    if (document.getElementById('gc-continue-css')) return;
    var st = document.createElement('style'); st.id = 'gc-continue-css';
    st.textContent = '.gc-cont-toast{position:fixed;left:50%;bottom:1.5em;transform:translateX(-50%);z-index:1050;max-width:min(36em,calc(100vw - 2em));box-sizing:border-box;display:flex;align-items:center;gap:1em;padding:.7em 1em;border-radius:10px;background:#141a22;color:#e8eef5;border:1px solid #2b3642;font:15px/1.4 system-ui,sans-serif;box-shadow:0 8px 24px rgba(0,0,0,.5);pointer-events:none}'
      + '.gc-cont-toast button{pointer-events:auto;flex:none;background:none;border:1px solid #3a4b5e;border-radius:8px;color:#8fd3fb;font:inherit;padding:.35em .8em;cursor:pointer;min-height:36px}'
      + '.gc-cont-toast span{overflow-wrap:anywhere}'
      + '.gc-cont-list .row span.gc-cont-name{overflow-wrap:anywhere}'
      // retrait de Jellyfin Enhanced (par épisode, sans « Remettre » chez nous) : caché
      + '.actionSheet [data-id="remove-continue-watching"],.actionSheet [data-id="je-multiselect-remove"]{display:none!important}';
    document.head.appendChild(st);
  }
  var toastEl = null, toastTimer = null;
  function toast(text, undo) {
    css();
    if (toastEl && toastEl.parentNode) toastEl.parentNode.removeChild(toastEl);
    clearTimeout(toastTimer);
    var d = document.createElement('div'); d.className = 'gc-cont-toast'; d.setAttribute('role', 'status');
    var s = document.createElement('span'); s.textContent = text; d.appendChild(s);
    if (undo) {
      var b = document.createElement('button'); b.type = 'button'; b.textContent = 'Annuler';
      b.addEventListener('click', function () { b.disabled = true; undo().then(function () { close(); }, function () { b.disabled = false; }); });
      d.appendChild(b);
    }
    document.body.appendChild(d); toastEl = d;
    function close() { if (d.parentNode) d.parentNode.removeChild(d); if (toastEl === d) toastEl = null; }
    toastTimer = setTimeout(close, 7000);
  }
  function nameList(targets) {
    if (targets.length === 1) return '« ' + (targets[0].n || 'Ce titre') + ' » retiré de Continuer à regarder';
    return targets.length + ' titres retirés de Continuer à regarder';
  }
  function doHide(targets) {
    // un même titre peut apparaître deux fois (épisode en cours + suivant)
    var seen = {}; targets = targets.filter(function (t) { if (!t.key || seen[t.key]) return false; seen[t.key] = true; return true; });
    if (!targets.length) return;
    hide(targets).then(function () {
      toast(nameList(targets), function () { return restore(targets.map(function (t) { return t.key; })); });
    }, function () { toast('Impossible de retirer ce titre pour l’instant. Réessaie dans un moment.'); });
  }

  // ------------------------------------------------------------------ entrée dans les menus
  function rowOf(card) { var s = card && card.closest && card.closest('.verticalSection'); return s && s.matches(ROWS) ? s : null; }
  function targetOf(card) {
    var id = card.getAttribute('data-id'), type = card.getAttribute('data-type');
    if (!id) return null;
    var t = cardItems[id];   // la vignette d'un épisode ne porte pas l'id de sa série : il vient de la réponse
    if (t) return { key: keyOf(t), n: t.SeriesName || t.Name || '', y: t.SeriesId ? 'Series' : t.Type };
    if (type === 'Episode' || type === 'Season') return null;   // série inconnue : pas d'entrée plutôt qu'une erreur
    return { key: id, n: ((card.querySelector('.cardText') || {}).textContent || '').trim(), y: type };
  }

  var last = { card: null, ts: 0 };
  function remember(e) {
    var t = e.target, card = t && t.closest && t.closest('.card[data-id]');
    if (!card) return;
    if (e.type !== 'contextmenu' && !(t.closest('[data-action="menu"]'))) return;
    last.card = card; last.ts = Date.now();
  }
  document.addEventListener('mousedown', remember, true);
  document.addEventListener('click', remember, true);
  document.addEventListener('contextmenu', remember, true);
  document.addEventListener('keydown', function (e) {    // télécommande / clavier : la vignette qui a le focus
    var a = document.activeElement, card = a && a.closest && a.closest('.card[data-id]');
    if (card) { last.card = card; last.ts = Date.now(); }
  }, true);

  function itemButton(scroller, onClick) {
    var ref = scroller.querySelector('.actionSheetMenuItem');
    if (!ref) return null;
    var b = ref.cloneNode(true);
    b.setAttribute('data-id', 'gc-continue-hide');
    var ic = b.querySelector('.actionsheetMenuItemIcon, .material-icons');
    if (ic) {   // icône Material par classe : on garde les classes de structure, on change le nom de l'icône
      var keep = ['actionsheetMenuItemIcon', 'listItemIcon', 'listItemIcon-transparent', 'material-icons'];
      ic.className = ic.className.split(/\s+/).filter(function (c) { return keep.indexOf(c) >= 0; }).concat('visibility_off').join(' ');
      ic.textContent = '';
    }
    var tx = b.querySelector('.actionSheetItemText') || b.querySelector('.listItemBodyText');
    if (tx) tx.textContent = LABEL;
    var aside = b.querySelector('.actionSheetItemAsideText'); if (aside) aside.parentNode.removeChild(aside);
    // la classe actionSheetMenuItem est gardée : jellyfin-web ferme la feuille lui-même (historique compris)
    b.addEventListener('click', onClick);
    return b;
  }
  function onSheet(sheet) {
    var scroller = sheet.querySelector('.actionSheetScroller') || sheet;
    if (scroller.querySelector('[data-id="gc-continue-hide"]')) return;
    // sélection multiple (appui long sur téléphone) : toutes les vignettes cochées doivent être dans ces rangées
    if (scroller.querySelector('[data-id="selectall"]')) {
      var checked = document.querySelectorAll('.card .chkItemSelect:checked');
      var cards = [];
      for (var i = 0; i < checked.length; i++) { var c = checked[i].closest('.card[data-id]'); if (c) cards.push(c); }
      if (!cards.length || cards.some(function (c) { return !rowOf(c); })) return;
      var targets = cards.map(targetOf);
      if (targets.some(function (t) { return !t; })) return;
      var bm = itemButton(scroller, function () {
        doHide(targets);
        var closeSel = document.querySelector('.selectionCommandsPanel .btnCloseSelectionPanel');
        setTimeout(function () { if (closeSel && document.body.contains(closeSel)) closeSel.click(); }, 300);
      });
      if (bm) scroller.appendChild(bm);
      return;
    }
    // feuille « … » d'une vignette : seulement juste après un appui sur une vignette de ces rangées
    if (!(scroller.querySelector('[data-id="resume"]') || scroller.querySelector('[data-id="play"]'))) return;
    var card = last.card;
    if (!card || Date.now() - last.ts > 3000 || !rowOf(card)) return;
    last.card = null;
    var t = targetOf(card);
    if (!t) return;
    var b = itemButton(scroller, function () { doHide([t]); });
    if (!b) return;
    var anchor = scroller.querySelector('[data-id="moremediainfo"]');
    if (anchor) scroller.insertBefore(b, anchor); else scroller.appendChild(b);
  }
  // les feuilles d'actions sont ajoutées dans un .dialogContainer directement sous <body> : seuls les enfants du body
  // sont observés (aucun sous-arbre), le contrôle s'arrête dès qu'il n'y a pas de feuille
  function scan(node) {
    if (!node || node.nodeType !== 1) return;
    var sheet = node.classList.contains('actionSheet') ? node : node.querySelector && node.querySelector('.actionSheet');
    if (sheet) onSheet(sheet);
  }
  function observe() {
    if (!document.body || !window.MutationObserver) return;
    new MutationObserver(function (recs) {
      for (var i = 0; i < recs.length; i++) for (var j = 0; j < recs[i].addedNodes.length; j++) scan(recs[i].addedNodes[j]);
    }).observe(document.body, { childList: true });
  }
  function start() { css(); observe(); }
  if (document.body) start(); else document.addEventListener('DOMContentLoaded', start);

  // ------------------------------------------------------------------ Mon compte (gestion)
  function list() {
    return load(true).then(function (h) {
      h = h || {};
      return Object.keys(h).map(function (k) { return { key: k, n: h[k].n, y: h[k].y, s: h[k].s }; })
        .sort(function (a, b) { return (b.s || 0) - (a.s || 0); });
    });
  }
  // section ajoutée par Mon compte (crates/homelabd/assets/compte/app.js) : mêmes classes que le panneau
  function accountSection(tv) {
    css();
    var wrap = document.createElement('div'); wrap.className = 'gc-cont-list';
    var h3 = document.createElement('h3'); h3.textContent = 'Continuer à regarder'; wrap.appendChild(h3);
    var body = document.createElement('div'); wrap.appendChild(body);
    var p = function (cls, text) { var e = document.createElement('p'); e.className = cls; e.textContent = text; return e; };
    var help = (tv ? 'Pour retirer un titre de la rangée : menu de sa vignette, « ' : 'Pour retirer un titre de la rangée : « … » sur sa vignette (appui long sur téléphone), « ')
      + LABEL + ' ». Ta progression est gardée ; le titre revient dès que tu le relances.';
    function draw() {
      body.innerHTML = '';
      body.appendChild(p('muted', 'Chargement…'));
      list().then(function (items) {
        body.innerHTML = '';
        if (!items.length) { body.appendChild(p('muted', 'Aucun titre retiré. ' + help)); return; }
        items.forEach(function (it) {
          var row = document.createElement('div'); row.className = 'row';
          var name = document.createElement('span'); name.className = 'gc-cont-name';
          name.textContent = (it.n || 'Titre') + ' ';
          var kind = document.createElement('span'); kind.className = 'muted'; kind.textContent = it.y === 'Series' ? '(série)' : (it.y === 'Movie' ? '(film)' : '');
          name.appendChild(kind);
          var btn = document.createElement('button'); btn.type = 'button'; btn.className = 'btn'; btn.textContent = 'Remettre';
          btn.addEventListener('click', function () {
            btn.disabled = true;
            restore([it.key]).then(draw, function () { btn.disabled = false; btn.textContent = 'Réessayer'; });
          });
          row.appendChild(name); row.appendChild(btn); body.appendChild(row);
        });
        body.appendChild(p('muted', 'Un titre revient aussi tout seul dès que tu le relances.'));
      }, function () { body.innerHTML = ''; body.appendChild(p('muted', 'Liste indisponible pour l’instant.')); });
    }
    draw();
    return wrap;
  }

  window.gcContinue = { version: VERSION, list: list, restore: restore, accountSection: accountSection };
})();
