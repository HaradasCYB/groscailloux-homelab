/* Groscailloux Tchat : bulle dans l'en-tête de Jellyfin + panneau (salons, fil privé avec l'admin).
 * Chargé par JavaScript Injector depuis /gc-chat/app.js (servi par homelabd, même origine que Jellyfin).
 * Identité : jeton de session Jellyfin (ApiClient.accessToken()), vérifié côté serveur.
 * Aucun HTML venant d'un message n'est interprété : tout passe par textContent. */
(function () {
  'use strict';
  if (window.__gcChat) return;
  window.__gcChat = true;

  var API = '/gc-chat/api';
  var POLL_OPEN = 5000, POLL_CLOSED = 60000, ME_OPEN = 20000;
  var INTRO = {
    annonces: "Les nouvelles de Groscailloux. Seul l'admin publie ici.",
    entraide: "Une question, un souci de lecture ? Tout le monde peut répondre. Précise le titre et l'appareil.",
    discussion: 'Discussion libre entre membres.',
    prive: "Seuls toi et l'admin voyez cette conversation.",
    threads: 'Conversations privées des membres.',
    compose: 'Choisis un ou plusieurs membres : chacun reçoit le message dans sa conversation privée avec toi.'
  };

  var S = {
    token: null, me: null, open: false, tab: 'annonces', thread: null,
    cache: {}, lastPoll: 0, lastMe: 0, bannerClosed: {}, busy: false, el: {},
    composing: false, recipients: {}, members: null
  };

  /* ---------- utilitaires ---------- */
  function h(tag, attrs, kids) {
    var e = document.createElement(tag);
    if (attrs) Object.keys(attrs).forEach(function (k) {
      if (k === 'text') e.textContent = attrs[k];
      else if (k === 'class') e.className = attrs[k];
      else if (k.slice(0, 2) === 'on') e.addEventListener(k.slice(2), attrs[k]);
      else e.setAttribute(k, attrs[k]);
    });
    (kids || []).forEach(function (c) { if (c) e.appendChild(typeof c === 'string' ? document.createTextNode(c) : c); });
    return e;
  }
  function token() {
    try { return window.ApiClient && ApiClient.accessToken && ApiClient.accessToken(); } catch (e) { return null; }
  }
  function api(method, path, body) {
    var opts = { method: method, headers: { 'X-Emby-Token': S.token } };
    if (body) { opts.headers['Content-Type'] = 'application/json'; opts.body = JSON.stringify(body); }
    return fetch(API + path, opts).then(function (r) {
      return r.json().catch(function () { return {}; }).then(function (j) {
        if (!r.ok) { var e = new Error(j.error || ('HTTP ' + r.status)); e.status = r.status; throw e; }
        return j;
      });
    });
  }
  function when(ts) {
    var d = new Date(ts * 1000), now = new Date(), s = (now - d) / 1000;
    var hm = d.toLocaleTimeString('fr-FR', { hour: '2-digit', minute: '2-digit' });
    if (s < 60) return "à l'instant";
    if (s < 3600) return 'il y a ' + Math.floor(s / 60) + ' min';
    if (d.toDateString() === now.toDateString()) return hm;
    var y = new Date(now); y.setDate(now.getDate() - 1);
    if (d.toDateString() === y.toDateString()) return 'hier ' + hm;
    return d.toLocaleDateString('fr-FR', { day: 'numeric', month: 'short' }) + ' ' + hm;
  }
  /* texte → nœuds, liens http(s) cliquables, jamais de HTML */
  function rich(text) {
    var frag = document.createDocumentFragment(), re = /https?:\/\/[^\s<>"]+/g, last = 0, m;
    while ((m = re.exec(text))) {
      var url = m[0].replace(/[.,;:!?)\]]+$/, '');
      frag.appendChild(document.createTextNode(text.slice(last, m.index)));
      frag.appendChild(h('a', { href: url, target: '_blank', rel: 'noopener noreferrer', text: url }));
      last = m.index + url.length;
      re.lastIndex = last;
    }
    frag.appendChild(document.createTextNode(text.slice(last)));
    return frag;
  }
  function playing() {
    return /#\/?video/.test(location.hash) || !!document.querySelector('#videoOsdPage:not(.hide), .videoPlayerContainer:not(.hide)');
  }
  function channelKey() {
    if (S.tab === 'prive' && S.composing) return null;
    if (S.tab === 'prive') return S.me.user.moderator ? S.thread : S.me.private.key;
    return S.tab;
  }

  /* ---------- styles ---------- */
  function css() {
    if (document.getElementById('gc-chat-css')) return;
    var c = [
      '.gc-chat-btn[hidden],.gc-panel [hidden],.gc-badge[hidden]{display:none!important}',
      '.gc-chat-btn{position:relative}',
      '.gc-chat-btn svg{width:1.45em;height:1.45em;fill:currentColor;display:block}',
      '.gc-badge{position:absolute;top:.15em;right:.1em;min-width:1.25em;height:1.25em;padding:0 .3em;border-radius:1em;background:var(--accentColor,#2f8fff);color:#fff;font:700 .68em/1.25em Inter,system-ui,sans-serif;text-align:center;box-sizing:border-box}',
      '.gc-badge.dot{min-width:.6em;width:.6em;height:.6em;padding:0;top:.35em;right:.35em}',
      '.gc-panel{position:fixed;top:0;right:0;bottom:0;width:min(430px,100vw);z-index:100000;display:flex;flex-direction:column;background:#0b0f16;color:#e8edf5;border-left:1px solid #232b3a;box-shadow:-12px 0 40px rgba(0,0,0,.45);font:15px/1.45 Inter,system-ui,sans-serif;transform:translateX(105%);transition:transform .22s ease}',
      '.gc-panel.open{transform:none}',
      '@media (prefers-reduced-motion:reduce){.gc-panel{transition:none}}',
      '.gc-head{display:flex;align-items:center;gap:.6em;padding:.9em 1em .6em}',
      '.gc-head strong{font-size:1.1em;flex:1}',
      '.gc-x{background:none;border:0;color:#8d99ad;font-size:1.6em;line-height:1;cursor:pointer;padding:.1em .3em;border-radius:.3em}',
      '.gc-x:hover,.gc-x:focus-visible{color:#fff;outline:2px solid var(--accentColor,#2f8fff)}',
      '.gc-tabs{display:flex;flex-wrap:wrap;gap:.35em .3em;padding:0 .8em .6em}',
      '.gc-tab{flex:none;position:relative;background:#151b26;border:1px solid #232b3a;color:#c9d2df;border-radius:2em;padding:.35em .85em;font:inherit;font-size:.88em;cursor:pointer}',
      '.gc-tab[aria-selected=true]{background:var(--accentColor,#2f8fff);border-color:transparent;color:#fff}',
      '.gc-tab:focus-visible{outline:2px solid #fff}',
      '.gc-tab .gc-n{margin-left:.35em;background:rgba(255,255,255,.2);border-radius:1em;padding:0 .4em;font-size:.85em}',
      '.gc-intro{margin:0 1em .4em;color:#8d99ad;font-size:.85em}',
      '.gc-list{flex:1;overflow-y:auto;padding:.4em 1em 1em;display:flex;flex-direction:column;gap:.55em}',
      '.gc-more{align-self:center;background:none;border:1px solid #232b3a;color:#8d99ad;border-radius:2em;padding:.25em .9em;cursor:pointer;font:inherit;font-size:.82em}',
      '.gc-msg{max-width:88%;align-self:flex-start;background:#151b26;border:1px solid #1f2735;border-radius:.9em .9em .9em .25em;padding:.5em .75em}',
      '.gc-msg.me{align-self:flex-end;background:rgba(47,143,255,.16);border-color:rgba(47,143,255,.35);border-radius:.9em .9em .25em .9em}',
      '.gc-msg.ann{max-width:100%;align-self:stretch;border-left:3px solid var(--accentColor,#2f8fff)}',
      '.gc-meta{display:flex;align-items:baseline;gap:.5em;font-size:.78em;color:#8d99ad;margin-bottom:.15em}',
      '.gc-meta b{color:#e8edf5;font-size:1.05em}',
      '.gc-role{background:var(--accentColor,#2f8fff);color:#fff;border-radius:.3em;padding:0 .35em;font-size:.85em}',
      '.gc-body{white-space:pre-wrap;word-wrap:break-word;overflow-wrap:anywhere}',
      '.gc-body a{color:#8cc4ff}',
      '.gc-del{margin-left:auto;background:none;border:0;color:#8d99ad;cursor:pointer;font:inherit;font-size:.95em;opacity:.7}',
      '.gc-del:hover,.gc-del:focus-visible{opacity:1;color:#ff8a8a}',
      '.gc-confirm{margin-left:auto;display:inline-flex;gap:.4em;align-items:center;color:#e8edf5}',
      '.gc-confirm button{border:0;border-radius:.4em;padding:.15em .55em;font:inherit;font-size:.95em;cursor:pointer}',
      '.gc-yes{background:#d9534f;color:#fff}',
      '.gc-no{background:#232b3a;color:#e8edf5}',
      '.gc-confirm button:focus-visible{outline:2px solid #fff}',
      '.gc-gone{color:#6b7686;font-style:italic}',
      '.gc-empty{margin:auto;color:#6b7686;text-align:center;font-size:.9em;padding:2em 1em}',
      '.gc-thread{display:block;width:100%;text-align:left;background:#151b26;border:1px solid #232b3a;color:inherit;border-radius:.7em;padding:.6em .8em;cursor:pointer;font:inherit}',
      '.gc-thread small{display:block;color:#8d99ad;margin-top:.15em;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}',
      '.gc-new{display:block;width:100%;text-align:left;background:rgba(47,143,255,.12);border:1px dashed rgba(47,143,255,.55);color:#cfe4ff;border-radius:.7em;padding:.6em .8em;cursor:pointer;font:inherit;font-weight:600}',
      '.gc-new:focus-visible,.gc-thread:focus-visible,.gc-pick:focus-within{outline:2px solid var(--accentColor,#2f8fff)}',
      '.gc-search{width:100%;box-sizing:border-box;background:#151b26;color:#e8edf5;border:1px solid #232b3a;border-radius:.6em;padding:.5em .7em;font:inherit}',
      '.gc-pick{display:flex;gap:.6em;align-items:center;padding:.4em .55em;border-radius:.55em;cursor:pointer}',
      '.gc-pick:hover{background:#151b26}',
      '.gc-pick small{color:#8d99ad;margin-left:auto;font-size:.8em}',
      '.gc-back{align-self:flex-start;background:none;border:0;color:#8cc4ff;cursor:pointer;font:inherit;font-size:.88em;padding:0}',
      '.gc-form{border-top:1px solid #232b3a;padding:.7em .8em calc(.7em + env(safe-area-inset-bottom));display:flex;flex-direction:column;gap:.45em}',
      '.gc-row{display:flex;gap:.5em;align-items:flex-end}',
      '.gc-form textarea{flex:1;resize:none;min-height:2.6em;max-height:9em;background:#151b26;color:#e8edf5;border:1px solid #232b3a;border-radius:.7em;padding:.55em .7em;font:inherit}',
      '.gc-form textarea:focus{outline:none;border-color:var(--accentColor,#2f8fff)}',
      '.gc-send{background:var(--accentColor,#2f8fff);color:#fff;border:0;border-radius:.7em;padding:.6em 1em;font:inherit;font-weight:600;cursor:pointer}',
      '.gc-send:disabled{opacity:.5;cursor:default}',
      '.gc-opt{font-size:.85em;color:#c9d2df;display:flex;gap:.45em;align-items:center}',
      '.gc-note{font-size:.8em;color:#8d99ad;min-height:1em}',
      '.gc-note.err{color:#ff8a8a}',
      '.gc-banner{position:fixed;left:50%;top:5.2em;transform:translateX(-50%);z-index:99999;width:min(640px,calc(100vw - 2em));display:flex;gap:.8em;align-items:center;background:#0b0f16;color:#e8edf5;border:1px solid rgba(47,143,255,.5);border-left:4px solid var(--accentColor,#2f8fff);border-radius:.8em;padding:.7em .9em;box-shadow:0 10px 30px rgba(0,0,0,.45);font:14px/1.4 Inter,system-ui,sans-serif}',
      '.gc-banner p{margin:0;flex:1;overflow:hidden;display:-webkit-box;-webkit-line-clamp:2;-webkit-box-orient:vertical}',
      '.gc-banner button{flex:none;background:none;border:0;color:#8cc4ff;cursor:pointer;font:inherit}',
      '@media (max-width:600px){.gc-panel{width:100vw;border-left:0}.gc-banner{top:4.6em}}'
    ].join('\n');
    document.head.appendChild(h('style', { id: 'gc-chat-css', text: c }));
  }

  /* ---------- bulle ---------- */
  function icon() {
    var ns = 'http://www.w3.org/2000/svg', svg = document.createElementNS(ns, 'svg'), p = document.createElementNS(ns, 'path');
    svg.setAttribute('viewBox', '0 0 24 24'); svg.setAttribute('aria-hidden', 'true');
    p.setAttribute('d', 'M4 4h16a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H9l-4.3 3.4A.8.8 0 0 1 3.4 20.8V18.6A2 2 0 0 1 2 16.6V6a2 2 0 0 1 2-2zm3 5.2a1.3 1.3 0 1 0 0 2.6 1.3 1.3 0 0 0 0-2.6zm5 0a1.3 1.3 0 1 0 0 2.6 1.3 1.3 0 0 0 0-2.6zm5 0a1.3 1.3 0 1 0 0 2.6 1.3 1.3 0 0 0 0-2.6z');
    svg.appendChild(p);
    return svg;
  }
  function ensureButton() {
    var right = document.querySelector('.skinHeader .headerRight');
    if (!right) return;
    var b = S.el.btn;
    if (!b || !right.contains(b)) {
      b = h('button', { type: 'button', class: 'headerButton headerButtonRight paper-icon-button-light gc-chat-btn', title: 'Tchat', 'aria-label': 'Ouvrir le tchat', onclick: function () { toggle(); } }, [icon()]);
      S.el.badge = h('span', { class: 'gc-badge', hidden: '' });
      b.appendChild(S.el.badge);
      right.insertBefore(b, right.firstChild);
      S.el.btn = b;
    }
    b.hidden = playing();
  }
  function counts() {
    var me = S.me, strong = 0, soft = 0;
    me.channels.forEach(function (c) { if (c.key === 'discussion') soft += c.unread; else strong += c.unread; });
    strong += me.private.unread;
    return { strong: strong, soft: soft };
  }
  function paintBadge() {
    if (!S.el.badge || !S.me) return;
    var n = counts(), b = S.el.badge;
    b.hidden = !(n.strong || n.soft);
    b.classList.toggle('dot', !n.strong && n.soft > 0);
    b.textContent = n.strong ? (n.strong > 9 ? '9+' : String(n.strong)) : '';
    S.el.btn.setAttribute('aria-label', n.strong ? 'Ouvrir le tchat, ' + n.strong + ' non lu(s)' : 'Ouvrir le tchat');
  }

  /* ---------- panneau ---------- */
  function buildPanel() {
    if (S.el.panel) return;
    var list = h('div', { class: 'gc-list', 'aria-live': 'polite' });
    var ta = h('textarea', { id: 'gc-chat-input', rows: '1', placeholder: 'Écrire un message…', 'aria-label': 'Message' });
    var note = h('div', { class: 'gc-note' });
    var mail = h('input', { type: 'checkbox', id: 'gc-chat-mail' });
    var optText = document.createTextNode('Envoyer aussi par mail aux membres');
    var opt = h('label', { class: 'gc-opt', for: 'gc-chat-mail', hidden: '' }, [mail, optText]);
    var send = h('button', { type: 'submit', class: 'gc-send', text: 'Envoyer' });
    var form = h('form', { class: 'gc-form', onsubmit: function (e) { e.preventDefault(); post(); } }, [opt, h('div', { class: 'gc-row' }, [ta, send]), note]);
    ta.addEventListener('keydown', function (e) { if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) { e.preventDefault(); post(); } });
    ta.addEventListener('input', function () {
      ta.style.height = 'auto'; ta.style.height = Math.min(ta.scrollHeight, 180) + 'px';
      var max = S.me ? S.me.limits.max_chars : 2000, left = max - ta.value.length;
      if (left < 200) setNote(left >= 0 ? left + ' caractère(s) restant(s)' : 'Message trop long', left < 0);
      else if (!note.classList.contains('err')) setNote('');
    });
    var tabs = h('div', { class: 'gc-tabs', role: 'tablist' });
    var panel = h('div', { class: 'gc-panel', role: 'dialog', 'aria-label': 'Tchat Groscailloux', 'aria-modal': 'false' }, [
      h('div', { class: 'gc-head' }, [h('strong', { text: 'Tchat' }), h('button', { type: 'button', class: 'gc-x', 'aria-label': 'Fermer le tchat', text: '×', onclick: function () { toggle(false); } })]),
      tabs, h('p', { class: 'gc-intro' }), list, form
    ]);
    panel.addEventListener('keydown', function (e) { if (e.key === 'Escape') { e.stopPropagation(); toggle(false); } });
    document.body.appendChild(panel);
    S.el.panel = panel; S.el.tabs = tabs; S.el.list = list; S.el.ta = ta; S.el.note = note;
    S.el.form = form; S.el.opt = opt; S.el.optText = optText; S.el.mail = mail; S.el.send = send; S.el.intro = panel.querySelector('.gc-intro');
  }
  function setNote(t, isErr) { S.el.note.textContent = t || ''; S.el.note.classList.toggle('err', !!isErr); }
  function paintTabs() {
    var me = S.me, t = S.el.tabs;
    t.textContent = '';
    var defs = me.channels.map(function (c) { return { key: c.key, label: c.label, n: c.unread }; });
    defs.push({ key: 'prive', label: me.user.moderator ? 'Privé' : "Écrire à l'admin", n: me.private.unread });
    defs.forEach(function (d) {
      var b = h('button', { type: 'button', class: 'gc-tab', role: 'tab', 'aria-selected': String(S.tab === d.key), onclick: function () { S.tab = d.key; S.thread = null; S.composing = false; render(true); } }, [d.label]);
      if (d.n && S.tab !== d.key) b.appendChild(h('span', { class: 'gc-n', text: d.n > 99 ? '99+' : String(d.n) }));
      t.appendChild(b);
    });
  }
  function toggle(force) {
    S.open = typeof force === 'boolean' ? force : !S.open;
    buildPanel();
    S.el.panel.classList.toggle('open', S.open);
    if (S.open) {
      closeBanner(false);
      refreshMe().then(function () { render(true); });
    } else if (S.el.btn) S.el.btn.focus();
  }
  function composerFor(key) {
    var me = S.me, can;
    if (S.composing) {
      S.el.form.hidden = false; S.el.opt.hidden = false; S.el.mail.checked = false;
      S.el.optText.textContent = 'Envoyer aussi par mail';
      S.el.ta.placeholder = 'Message privé (chaque destinataire le reçoit séparément)…';
      return;
    }
    S.el.optText.textContent = 'Envoyer aussi par mail aux membres';
    S.el.ta.placeholder = 'Écrire un message…';
    if (S.tab === 'prive') can = !!key;
    else can = (me.channels.filter(function (c) { return c.key === key; })[0] || {}).can_post;
    S.el.form.hidden = !can;
    S.el.opt.hidden = !(me.user.moderator && key === 'annonces');
    if (!S.el.opt.hidden) S.el.mail.checked = false;
  }
  function render(reset) {
    if (!S.me || !S.el.panel) return;
    paintTabs();
    var key = channelKey(), list = S.el.list;
    S.el.intro.textContent = S.composing ? INTRO.compose : S.tab === 'prive' && S.me.user.moderator && !S.thread ? INTRO.threads : INTRO[S.tab === 'prive' ? 'prive' : S.tab];
    composerFor(key);
    if (S.composing) { renderCompose(); return; }
    if (S.tab === 'prive' && S.me.user.moderator && !S.thread) { renderThreads(); return; }
    if (reset) {
      list.textContent = '';
      if (S.tab === 'prive' && S.me.user.moderator) list.appendChild(h('button', { type: 'button', class: 'gc-back', text: '← Toutes les conversations', onclick: function () { S.thread = null; render(true); } }));
      load(key, null);
    }
  }
  function renderThreads() {
    var list = S.el.list;
    list.textContent = '';
    list.appendChild(h('button', { type: 'button', class: 'gc-new', text: '✉  Nouveau message privé', onclick: function () { S.composing = true; S.recipients = {}; render(true); } }));
    api('GET', '/private').then(function (j) {
      if (S.tab !== 'prive' || S.thread || S.composing) return;
      if (!j.threads.length) { list.appendChild(h('p', { class: 'gc-empty', text: 'Aucune conversation privée pour le moment.' })); return; }
      j.threads.forEach(function (t) {
        var title = t.member_name + (t.unread ? '  •  ' + t.unread + ' non lu(s)' : '');
        list.appendChild(h('button', { type: 'button', class: 'gc-thread', onclick: function () { S.thread = t.channel; S.threadName = t.member_name; render(true); } }, [
          h('b', { text: title }), h('small', { text: when(t.last_at) + ' · ' + (t.last_body || 'message supprimé') })
        ]));
      });
    }).catch(showErr);
  }
  function renderCompose() {
    var list = S.el.list;
    list.textContent = '';
    list.appendChild(h('button', { type: 'button', class: 'gc-back', text: '← Conversations', onclick: function () { S.composing = false; render(true); } }));
    var search = h('input', { type: 'search', id: 'gc-chat-search', class: 'gc-search', placeholder: 'Chercher un membre…', 'aria-label': 'Chercher un membre' });
    var box = h('div', { role: 'group', 'aria-label': 'Destinataires' });
    list.appendChild(search); list.appendChild(box);
    function paint(ms) {
      box.textContent = '';
      var q = search.value.trim().toLowerCase();
      ms.filter(function (m) { return !q || m.name.toLowerCase().indexOf(q) >= 0; }).forEach(function (m) {
        var cb = h('input', { type: 'checkbox', value: m.id });
        cb.checked = !!S.recipients[m.id];
        cb.addEventListener('change', function () { if (cb.checked) S.recipients[m.id] = m.name; else delete S.recipients[m.id]; count(); });
        box.appendChild(h('label', { class: 'gc-pick' }, [cb, h('span', { text: m.name }), m.has_email ? null : h('small', { text: 'pas de mail' })]));
      });
      if (!box.firstChild) box.appendChild(h('p', { class: 'gc-empty', text: 'Aucun membre trouvé.' }));
    }
    function count() {
      var n = Object.keys(S.recipients).length;
      setNote(n ? n + ' destinataire(s) : ' + Object.keys(S.recipients).map(function (k) { return S.recipients[k]; }).join(', ') : '');
    }
    search.addEventListener('input', function () { if (S.members) paint(S.members); });
    (S.members ? Promise.resolve({ members: S.members }) : api('GET', '/members')).then(function (j) {
      S.members = j.members;
      if (S.composing) { paint(j.members); count(); }
    }).catch(showErr);
  }
  function sendDirect() {
    var ids = Object.keys(S.recipients), text = S.el.ta.value.trim();
    if (!ids.length) { setNote('Choisis au moins un destinataire.', true); return; }
    if (!text) return;
    S.busy = true; S.el.send.disabled = true;
    api('POST', '/direct', { user_ids: ids, body: text, email: S.el.mail.checked }).then(function (j) {
      var mailed = S.el.mail.checked;
      S.el.ta.value = ''; S.el.ta.style.height = 'auto'; S.recipients = {}; S.composing = false;
      render(true);
      setNote('Envoyé à ' + j.sent + ' membre(s)' + (mailed ? ', et par mail à ceux qui ont une adresse.' : '.'));
    }).catch(showErr).then(function () { S.busy = false; S.el.send.disabled = false; });
  }
  function msgNode(m) {
    var me = S.me, mine = m.author_id === me.user.id;
    var box = h('div', { class: 'gc-msg' + (mine ? ' me' : '') + (m.channel === 'annonces' ? ' ann' : ''), 'data-id': String(m.id) });
    var meta = h('div', { class: 'gc-meta' }, [h('b', { text: m.author_name }), m.author_moderator ? h('span', { class: 'gc-role', text: 'admin' }) : null, h('span', { text: when(m.created_at) })]);
    var canDel = !m.deleted && (me.user.moderator || (mine && Date.now() / 1000 - m.created_at <= me.limits.delete_own_within_secs));
    if (canDel) meta.appendChild(h('button', { type: 'button', class: 'gc-del', title: 'Supprimer', 'aria-label': 'Supprimer ce message', text: '🗑', onclick: function (e) { askDelete(e.currentTarget, m.id); } }));
    box.appendChild(meta);
    var body = h('div', { class: 'gc-body' + (m.deleted ? ' gc-gone' : '') });
    if (m.deleted) body.textContent = 'Message supprimé.'; else body.appendChild(rich(m.body));
    box.appendChild(body);
    return box;
  }
  function load(key, before) {
    var c = S.cache[key] || (S.cache[key] = { last: 0, first: 0 });
    var q = '/messages?channel=' + encodeURIComponent(key) + '&limit=50' + (before ? '&before=' + before : '');
    return api('GET', q).then(function (j) {
      if (channelKey() !== key) return;
      var list = S.el.list, msgs = j.messages;
      var old = list.querySelector('.gc-more'); if (old) old.remove();
      var empty = list.querySelector('.gc-empty'); if (empty) empty.remove();
      if (before) {
        var anchor = list.querySelector('.gc-msg'), prevH = list.scrollHeight;
        msgs.forEach(function (m) { list.insertBefore(msgNode(m), anchor); });
        list.scrollTop += list.scrollHeight - prevH;
      } else {
        msgs.forEach(function (m) { list.appendChild(msgNode(m)); });
        if (!msgs.length) list.appendChild(h('p', { class: 'gc-empty', text: S.tab === 'prive' ? 'Écris ton message : il ne sera lu que par l\'admin.' : 'Aucun message pour le moment.' }));
        list.scrollTop = list.scrollHeight;
        c.last = msgs.length ? msgs[msgs.length - 1].id : 0;
      }
      if (msgs.length) c.first = msgs[0].id;
      if (msgs.length === j.limit) {
        var more = h('button', { type: 'button', class: 'gc-more', text: 'Messages plus anciens', onclick: function () { load(key, c.first); } });
        var back = list.querySelector('.gc-back');
        list.insertBefore(more, back ? back.nextSibling : list.firstChild);
      }
      markRead(key, c.last);
    }).catch(showErr);
  }
  function poll() {
    var key = channelKey();
    if (!key || (S.tab === 'prive' && S.me.user.moderator && !S.thread)) return Promise.resolve();
    var c = S.cache[key];
    if (!c) return Promise.resolve();
    return api('GET', '/messages?channel=' + encodeURIComponent(key) + '&after=' + c.last + '&limit=100').then(function (j) {
      if (channelKey() !== key || !j.messages.length) return;
      var list = S.el.list, stick = list.scrollHeight - list.scrollTop - list.clientHeight < 80;
      var empty = list.querySelector('.gc-empty'); if (empty) empty.remove();
      j.messages.forEach(function (m) { if (!list.querySelector('[data-id="' + m.id + '"]')) list.appendChild(msgNode(m)); });
      c.last = j.messages[j.messages.length - 1].id;
      if (stick) list.scrollTop = list.scrollHeight;
      markRead(key, c.last);
    }).catch(function () {});
  }
  function markRead(key, id) {
    if (!id) return;
    api('POST', '/read', { channel: key, last_id: id }).then(function () {
      if (S.el.banner && S.el.banner.dataset.channel === key) closeBanner(false);
      refreshMe();
    }).catch(function () {});
  }
  function post() {
    if (S.busy) return;
    if (S.composing) { sendDirect(); return; }
    var key = channelKey(), text = S.el.ta.value.trim();
    if (!text || !key) return;
    S.busy = true; S.el.send.disabled = true;
    var body = { channel: key, body: text };
    if (!S.el.opt.hidden && S.el.mail.checked) body.email_members = true;
    api('POST', '/messages', body).then(function (j) {
      S.el.ta.value = ''; S.el.ta.style.height = 'auto';
      setNote(body.email_members ? 'Publié, et envoyé par mail aux membres.' : '');
      var list = S.el.list, empty = list.querySelector('.gc-empty'); if (empty) empty.remove();
      list.appendChild(msgNode(j.message));
      list.scrollTop = list.scrollHeight;
      var c = S.cache[key] || (S.cache[key] = { last: 0, first: 0 });
      c.last = Math.max(c.last, j.message.id);
    }).catch(showErr).then(function () { S.busy = false; S.el.send.disabled = false; S.el.ta.focus(); });
  }
  /* confirmation dans le message lui-même : window.confirm() n'existe pas dans les WebView de l'appli iPhone
     et de Jellyfin Desktop (il y répond « non » sans rien afficher) */
  function askDelete(btn, id) {
    var box = h('span', { class: 'gc-confirm', role: 'group', 'aria-label': 'Confirmer la suppression' });
    var keep = h('button', { type: 'button', class: 'gc-no', text: 'Annuler' });
    var yes = h('button', { type: 'button', class: 'gc-yes', text: 'Supprimer' });
    function restore() { if (box.parentNode) box.replaceWith(btn); btn.focus(); }
    keep.addEventListener('click', restore);
    yes.addEventListener('click', function () { yes.disabled = true; keep.disabled = true; del(id, restore); });
    box.appendChild(h('span', { text: 'Supprimer ?' })); box.appendChild(yes); box.appendChild(keep);
    btn.replaceWith(box);
    yes.focus();
    setTimeout(restore, 8000); // sans réponse : on remet la corbeille
  }
  function del(id, onFail) {
    api('DELETE', '/messages/' + id).then(function () {
      var n = S.el.list.querySelector('[data-id="' + id + '"]');
      if (n) n.replaceWith(msgNode({ id: id, channel: channelKey(), author_id: '', author_name: n.querySelector('b').textContent, author_moderator: false, body: '', created_at: Date.now() / 1000, deleted: true }));
    }).catch(function (e) { if (onFail) onFail(); showErr(e); });
  }
  function showErr(e) {
    if (e && e.status === 401) { reset(); return; }
    if (S.el.note) setNote(e && e.message ? e.message.charAt(0).toUpperCase() + e.message.slice(1) + '.' : 'Erreur réseau.', true);
  }

  /* ---------- bannière d'annonce (accueil) ---------- */
  function banner() {
    var me = S.me, onHome = /#\/?(home|$)/.test(location.hash || '#/home');
    var item = null;
    if (me && me.latest_private && !S.bannerClosed['p' + me.latest_private.id]) item = { key: 'p' + me.latest_private.id, m: me.latest_private, tab: 'prive', channel: me.private.key, icon: '✉️', label: "Message de l'admin : " };
    else if (me && me.latest_announcement && !S.bannerClosed['a' + me.latest_announcement.id]) item = { key: 'a' + me.latest_announcement.id, m: me.latest_announcement, tab: 'annonces', channel: 'annonces', icon: '📢', label: 'Annonce : ' };
    if (!item || S.open || !onHome || playing()) { closeBanner(false); return; }
    if (S.el.banner && S.el.banner.dataset.key === item.key) return;
    closeBanner(false);
    var b = h('div', { class: 'gc-banner', role: 'status', 'data-key': item.key, 'data-channel': item.channel, 'data-msg': String(item.m.id) }, [
      h('span', { 'aria-hidden': 'true', text: item.icon }),
      h('p', {}, [h('b', { text: item.label }), item.m.body.replace(/\s+/g, ' ')]),
      h('button', { type: 'button', text: 'Lire', onclick: function () { S.tab = item.tab; S.thread = null; S.composing = false; toggle(true); } }),
      h('button', { type: 'button', 'aria-label': 'Masquer', text: '×', onclick: function () { S.bannerClosed[item.key] = true; closeBanner(true); } })
    ]);
    document.body.appendChild(b);
    S.el.banner = b;
  }
  function closeBanner(markAsRead) {
    var b = S.el.banner;
    if (!b) return;
    if (markAsRead) api('POST', '/read', { channel: b.dataset.channel, last_id: Number(b.dataset.msg) }).then(refreshMe).catch(function () {});
    b.remove(); S.el.banner = null;
  }

  /* ---------- cycle ---------- */
  function refreshMe() {
    return api('GET', '/me').then(function (me) {
      S.me = me; S.lastMe = Date.now();
      paintBadge();
      if (S.open && S.el.tabs) paintTabs();
      banner();
    });
  }
  function reset() {
    S.me = null; S.token = null; S.cache = {}; S.open = false; S.bannerClosed = {}; S.composing = false; S.members = null;
    if (S.el.btn) S.el.btn.remove();
    if (S.el.panel) S.el.panel.remove();
    closeBanner(false);
    S.el = {};
  }
  function loop() {
    var t = token();
    if (t !== S.token) { reset(); S.token = t; }
    if (!S.token) return;
    if (!S.me) {
      if (S.denied === S.token) return;
      refreshMe().then(function () { css(); ensureButton(); paintBadge(); }).catch(function (e) {
        if (e && (e.status === 401 || e.status === 403)) S.denied = S.token; // hors bêta ou session invalide : rien à afficher
      });
      return;
    }
    ensureButton();
    if (document.hidden) return;
    var now = Date.now();
    if (S.open) {
      if (playing()) { toggle(false); return; }
      if (now - S.lastPoll >= POLL_OPEN) { S.lastPoll = now; poll(); }
      if (now - S.lastMe >= ME_OPEN) refreshMe().catch(showErr);
    } else if (now - S.lastMe >= POLL_CLOSED) {
      refreshMe().catch(showErr);
    }
    banner();
  }
  window.addEventListener('hashchange', function () { if (S.me) { ensureButton(); banner(); } });
  setInterval(loop, 1000);
  loop();
})();
