// « Mon compte » Groscailloux — servi par homelabd sous /gc-compte/app.js (même origine que Jellyfin).
// Bouton dans l'en-tête de jellyfin-web → panneau : état de l'abonnement, échéance, paiement, appareils
// connectés (déconnexion), changement de mot de passe, code de parrainage, historique. Téléviseurs :
// lecture seule, fermeture par la touche Retour. Jamais de window.confirm/alert (WebView iOS).
(function () {
  if (window.__gcAccount) return;
  window.__gcAccount = true;
  var TV = /web0?s|webos|tizen|smart-?tv|netcast|viera|bravia|hbbtv|aft[a-z]|android\s?tv|googletv|crkey/i
    .test(navigator.userAgent || '') || document.documentElement.classList.contains('layout-tv');
  var BASE = '/gc-compte';
  var S = { open: false, data: null, el: {} };

  function h(tag, attrs, kids) {
    var e = document.createElement(tag);
    if (attrs) Object.keys(attrs).forEach(function (k) {
      if (k === 'class') e.className = attrs[k];
      else if (k === 'text') e.textContent = attrs[k];
      else if (k.indexOf('on') === 0) e.addEventListener(k.slice(2), attrs[k]);
      else if (attrs[k] != null) e.setAttribute(k, attrs[k]);
    });
    (kids || []).forEach(function (c) { if (c) e.appendChild(typeof c === 'string' ? document.createTextNode(c) : c); });
    return e;
  }
  function token() {
    try {
      var c = JSON.parse(localStorage.getItem('jellyfin_credentials') || '{}');
      var s = (c.Servers || []).filter(function (x) { return x.AccessToken; })[0];
      return s ? s.AccessToken : null;
    } catch (e) { return null; }
  }
  function api(method, path, body) {
    var t = token();
    return fetch(BASE + path, {
      method: method, credentials: 'same-origin',
      headers: t ? { 'X-Emby-Token': t, 'Content-Type': 'application/json' } : { 'Content-Type': 'application/json' },
      body: body ? JSON.stringify(body) : undefined
    }).then(function (r) { return r.json().then(function (j) { if (!r.ok) throw new Error(j.error || ('HTTP ' + r.status)); return j; }); });
  }
  function css() {
    if (document.getElementById('gc-account-css')) return;
    var st = document.createElement('style'); st.id = 'gc-account-css';
    st.textContent = [
      '.gc-acc-btn svg{width:1.45em;height:1.45em;fill:currentColor;display:block}',
      '.gc-acc-wrap{position:fixed;inset:0;z-index:100000;display:flex;align-items:flex-start;justify-content:center;padding:4.5em 1em 1em;background:rgba(0,0,0,.55)}',
      '.gc-acc{width:min(34rem,100%);max-height:calc(100vh - 6em);overflow:auto;background:#141a22;color:#e8eef5;border:1px solid #2b3642;border-radius:14px;padding:1.1em 1.2em;font:15px/1.45 system-ui,sans-serif;box-shadow:0 20px 60px rgba(0,0,0,.5)}',
      '.gc-acc h2{margin:0 0 .3em;font-size:1.25em}.gc-acc h3{margin:1em 0 .4em;font-size:1em;color:#9fb3c8;text-transform:uppercase;letter-spacing:.04em}',
      '.gc-acc .st{display:inline-block;padding:.15em .6em;border-radius:999px;font-weight:600;font-size:.9em}',
      '.gc-acc .st.active,.gc-acc .st.offered,.gc-acc .st.exempt{background:#1d5d3a;color:#c9f7dc}.gc-acc .st.trial{background:#2a4d7a;color:#d3e7ff}.gc-acc .st.grace{background:#7a5a1a;color:#ffe7b3}.gc-acc .st.suspended{background:#7a2a2a;color:#ffd0d0}.gc-acc .st.unknown{background:#3a3f47;color:#dfe5ec}',
      '.gc-acc .row{display:flex;justify-content:space-between;gap:1em;padding:.45em 0;border-bottom:1px solid #222b35}.gc-acc .row:last-child{border-bottom:0}',
      '.gc-acc .muted{color:#9fb3c8;font-size:.92em}.gc-acc .btn{display:inline-block;padding:.5em .9em;border-radius:8px;border:1px solid #3a4b5e;background:#1e2a38;color:#e8eef5;cursor:pointer;font:inherit;text-decoration:none}',
      '.gc-acc .btn.primary{background:#0a6fb0;border-color:#0a6fb0;color:#fff}.gc-acc .btn:focus-visible{outline:2px solid #8fd3fb;outline-offset:2px}',
      '.gc-acc .close{float:right;background:none;border:0;color:#9fb3c8;font-size:1.4em;cursor:pointer;line-height:1}',
      '.gc-acc code{background:#0f151c;padding:.15em .45em;border-radius:6px;font-size:1.05em;letter-spacing:.08em}',
      '.gc-acc .note{margin:.6em 0 0;padding:.6em .8em;border-radius:8px;background:#1b2430;font-size:.92em}',
      '.gc-acc .err{background:#4a1f1f}.gc-acc .ok{background:#1f4a2c}',
      '.gc-acc ul{margin:.3em 0;padding-left:1.2em}.gc-acc li{margin:.15em 0}',
      '@media (max-width:600px){.gc-acc-wrap{padding:3.8em .5em .5em}.gc-acc{padding:.9em}}'
    ].join('\n');
    document.head.appendChild(st);
  }
  function icon() {
    var s = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
    s.setAttribute('viewBox', '0 0 24 24'); s.setAttribute('aria-hidden', 'true');
    var p = document.createElementNS('http://www.w3.org/2000/svg', 'path');
    p.setAttribute('d', 'M20 4H4c-1.1 0-2 .9-2 2v12c0 1.1.9 2 2 2h16c1.1 0 2-.9 2-2V6c0-1.1-.9-2-2-2zm0 14H4V6h16v12zM9 8a2.5 2.5 0 1 0 0 5 2.5 2.5 0 0 0 0-5zm-4 8.5c0-1.7 2.7-2.5 4-2.5s4 .8 4 2.5V17H5v-.5zM14 9h5v1.5h-5zm0 3h5v1.5h-5zm0 3h3v1.5h-3z');
    s.appendChild(p); return s;
  }
  function daysText(d) { if (d == null) return ''; if (d <= 0) return 'aujourd’hui'; if (d === 1) return 'demain'; return 'dans ' + d + ' jours'; }

  function render(d) {
    var box = S.el.box; box.innerHTML = '';
    box.appendChild(h('button', { class: 'close', type: 'button', 'aria-label': 'Fermer', onclick: close }, ['×']));
    box.appendChild(h('h2', { text: 'Mon compte' }));
    box.appendChild(h('p', { class: 'muted', text: d.name }));
    var st = h('span', { class: 'st ' + d.status, text: d.status_label });
    var line = h('div', { class: 'row' }, [h('span', { text: 'Abonnement' }), st]);
    box.appendChild(line);
    if (d.expires_text) {
      var lbl = d.status === 'trial' ? 'Fin de l\u2019essai' : (d.status === 'grace' ? '\u00c9ch\u00e9ance d\u00e9pass\u00e9e le' : (d.status === 'offered' ? 'Offert jusqu\u2019au' : 'Actif jusqu\u2019au'));
      box.appendChild(h('div', { class: 'row' }, [h('span', { text: lbl }), h('span', { text: d.expires_text + (d.days_left != null ? ' (' + daysText(d.days_left) + ')' : '') })]));
    }
    if (d.status === 'grace') box.appendChild(h('p', { class: 'note', text: 'Ton accès reste ouvert ' + d.grace_days + ' jours après l’échéance. Renouvelle pour éviter la coupure.' }));
    if ((d.status === 'offered' && !d.expires_text) || d.status === 'exempt') box.appendChild(h('p', { class: 'note', text: 'Ton accès est offert : rien à faire.' }));
    if (d.status === 'unknown') box.appendChild(h('p', { class: 'note', text: 'Ton compte est actif. Pour qu’il le reste sans intervention, tu peux enregistrer un abonnement ci-dessous.' }));

    if (d.can_pay && !(d.status === 'offered' || d.status === 'exempt')) {
      box.appendChild(h('h3', { text: 'Paiement' }));
      var payTxt = d.paypal ? 'Ton abonnement PayPal se renouvelle tout seul chaque mois (' + d.price_text + '). Pour l’arrêter : ton compte PayPal → Paiements automatiques.' :
        (d.status === 'trial' ? 'Après l’essai, l’accès continue avec un abonnement à ' + d.price_text + ', sans engagement.' : 'Abonnement à ' + d.price_text + ', sans engagement, activé automatiquement dès le paiement.');
      box.appendChild(h('p', { class: 'muted', text: payTxt }));
      if (!d.paypal && !TV) box.appendChild(h('p', null, [h('a', { class: 'btn primary', href: d.pay_url, target: '_blank', rel: 'noopener', text: d.status === 'trial' || d.status === 'suspended' || d.status === 'grace' ? 'M’abonner' : 'Enregistrer mon abonnement' })]));
      if (!d.paypal && TV) box.appendChild(h('p', { class: 'muted', text: 'Depuis un téléphone ou un ordinateur : ' + d.pay_url }));
    }

    if (d.referral_days > 0) {
      box.appendChild(h('h3', { text: 'Parrainage' }));
      box.appendChild(h('p', null, ['Ton code : ', h('code', { text: d.referral_code }), '. Un ami l’indique à son inscription : au premier paiement, vous gagnez ' + d.referral_days + ' jours chacun.']));
      if (d.signup_url) box.appendChild(h('p', { class: 'muted', text: 'Inscription : ' + d.signup_url }));
    }

    box.appendChild(h('h3', { text: 'Appareils connectés' }));
    if (!d.devices.length) box.appendChild(h('p', { class: 'muted', text: 'Aucune session ouverte.' }));
    d.devices.forEach(function (dev) {
      var right = TV ? h('span', { class: 'muted', text: dev.playing ? 'en lecture' : '' })
        : h('button', { class: 'btn', type: 'button', onclick: function () { logout(dev, this); }, text: 'Déconnecter' });
      box.appendChild(h('div', { class: 'row' }, [h('span', null, [dev.name + ' ', h('span', { class: 'muted', text: dev.client + (dev.playing ? ' — lit ' + dev.playing : '') })]), right]));
    });

    if (!TV) {
      box.appendChild(h('h3', { text: 'Sécurité' }));
      box.appendChild(h('p', null, [h('button', { class: 'btn', type: 'button', onclick: passwordLink, text: 'Changer mon mot de passe' }), ' ', h('span', { class: 'muted', text: 'Un lien valable une heure s’ouvre dans un nouvel onglet.' })]));
    }

    box.appendChild(h('h3', { text: 'Langue de lecture' }));
    var langSel = h('select', { class: 'btn', 'aria-label': 'Langue de lecture', onchange: function () { setLanguage(this.value, this); } });
    [['fr', 'Fran\u00e7ais quand il existe (VF, sinon VO sous-titr\u00e9e)'], ['vo', 'Toujours en VO, sous-titres fran\u00e7ais']].forEach(function (o) {
      var op = h('option', { value: o[0], text: o[1] }); if (d.language === o[0]) op.selected = true; langSel.appendChild(op);
    });
    box.appendChild(h('p', null, [langSel]));
    box.appendChild(h('p', { class: 'muted', text: 'Appliqu\u00e9 \u00e0 la prochaine lecture, sur tous tes appareils.' }));

    // Taille des sous-titres : réglage jellyfin-web propre à CET appareil (localStorage
    // `<userId>-localplayersubtitleappearance3`, champ textSize) ; l'ASS garde ses propres styles.
    box.appendChild(h('h3', { text: 'Taille des sous-titres' }));
    var sizeSel = h('select', { class: 'btn', 'aria-label': 'Taille des sous-titres', onchange: function () { setSubtitleSize(this.value); } });
    var cur = subtitleSize();
    [['normal', 'Normale'], ['large', 'Grande'], ['extralarge', 'Tr\u00e8s grande']].forEach(function (o) {
      var op = h('option', { value: o[0], text: o[1] }); if (cur === o[0]) op.selected = true; sizeSel.appendChild(op);
    });
    box.appendChild(h('p', null, [sizeSel]));
    box.appendChild(h('p', { class: 'muted', text: 'Pour cet appareil, sous-titres SRT ; les sous-titres ASS des anim\u00e9s gardent leur propre style.' }));

    if (d.history && d.history.length) {
      box.appendChild(h('h3', { text: 'Historique' }));
      var ul = h('ul');
      d.history.slice(0, 8).forEach(function (e) { ul.appendChild(h('li', { class: 'muted', text: e.date + ' — ' + e.detail })); });
      box.appendChild(ul);
    }
    S.el.note = h('div', { hidden: '' }); box.appendChild(S.el.note);
    if (TV) box.appendChild(h('p', { class: 'muted', text: 'Touche Retour pour fermer.' }));
  }
  function note(text, cls) {
    var n = S.el.note; if (!n) return;
    n.className = 'note ' + (cls || ''); n.textContent = text; n.hidden = false;
    setTimeout(function () { n.hidden = true; }, 8000);
  }
  function logout(dev, btn) {
    btn.disabled = true;
    api('DELETE', '/api/devices/' + encodeURIComponent(dev.device_id)).then(function () { note('Appareil déconnecté.', 'ok'); return load(); })
      .catch(function (e) { btn.disabled = false; note(e.message, 'err'); });
  }
  var SUB_KEY = 'localplayersubtitleappearance3';
  function userId() {
    try { var c = JSON.parse(localStorage.getItem('jellyfin_credentials') || '{}'); var s = (c.Servers || []).find(function (x) { return x.AccessToken; }); return s && s.UserId; } catch (e) { return null; }
  }
  function subtitleAppearance() {
    var u = userId(); if (!u) return {};
    try { return JSON.parse(localStorage.getItem(u + '-' + SUB_KEY) || '{}') || {}; } catch (e) { return {}; }
  }
  function subtitleSize() { return subtitleAppearance().textSize || 'normal'; }
  // Tailles de jellyfin-web (htmlVideoPlayer) : elles sont posées en style **inline** sur .videoSubtitlesInner au
  // moment où le lecteur crée l'élément de sous-titres. Changer le réglage pendant une lecture ne bougeait donc
  // rien tant qu'on ne changeait pas de piste (d'où le détour par le sous-titre secondaire, qui en affiche deux).
  // Une règle de feuille de style `!important` l'emporte sur un style inline : la taille s'applique aussitôt.
  var SIZE_EM = { smaller: '.8em', small: 'inherit', normal: '1.36em', large: '1.72em', larger: '2em', extralarge: '2.2em' };
  function applySubtitleSize() {
    var em = SIZE_EM[subtitleSize()], el = document.getElementById('gc-sub-size');
    if (!em) { if (el && el.parentNode) el.parentNode.removeChild(el); return; }
    var css = '.videoSubtitlesInner,.videoSecondarySubtitlesInner{font-size:' + em + ' !important}'
      + 'video::cue{font-size:' + em + ' !important}';
    if (!el) { el = document.createElement('style'); el.id = 'gc-sub-size'; (document.head || document.documentElement).appendChild(el); }
    if (el.textContent !== css) el.textContent = css;
  }
  function setSubtitleSize(v) {
    var u = userId(); if (!u) { note('Appareil non reconnu.', 'err'); return; }
    var a = subtitleAppearance(); a.textSize = v;
    try {
      localStorage.setItem(u + '-' + SUB_KEY, JSON.stringify(a));
      applySubtitleSize();
      note('Taille appliqu\u00e9e, m\u00eame en cours de lecture.', 'ok');
    } catch (e) { note('Impossible d\u2019enregistrer sur cet appareil.', 'err'); }
  }
  // Par défaut « Grande » sur un appareil qui n'a jamais choisi (le SRT est petit dans jellyfin-web) ; un choix
  // existant, y compris dans Réglages → Sous-titres de Jellyfin, n'est jamais touché.
  (function defaultSubtitleSize() {
    var u = userId(); if (!u) return;
    try {
      var k = u + '-' + SUB_KEY, raw = localStorage.getItem(k);
      if (raw !== null && raw !== '') return;
      localStorage.setItem(k, JSON.stringify({ textSize: 'large' }));
    } catch (e) { /* stockage indisponible : rien */ }
  })();
  applySubtitleSize();
  function setLanguage(mode, sel) {
    sel.disabled = true;
    api('POST', '/api/language', { mode: mode }).then(function () { rememberMode(mode); note('Langue enregistr\u00e9e.', 'ok'); sel.disabled = false; })
      .catch(function (e) { note(e.message, 'err'); sel.disabled = false; });
  }
  function passwordLink() {
    api('POST', '/api/password').then(function (r) {
      var w = window.open(r.url, '_blank', 'noopener');
      if (!w) note('Ouvre ce lien : ' + r.url, 'ok');
    }).catch(function (e) { note(e.message, 'err'); });
  }
  function load() {
    return api('GET', '/api/me').then(function (d) { S.data = d; rememberMode(d.language); render(d); })
      .catch(function (e) { S.el.box.innerHTML = ''; S.el.box.appendChild(h('button', { class: 'close', type: 'button', onclick: close }, ['×'])); S.el.box.appendChild(h('p', { text: 'Mon compte indisponible : ' + e.message })); });
  }
  function open() {
    if (S.open) return; S.open = true; css();
    var wrap = h('div', { class: 'gc-acc-wrap', role: 'dialog', 'aria-label': 'Mon compte', onclick: function (ev) { if (ev.target === wrap) close(); } });
    var box = h('div', { class: 'gc-acc', tabindex: '-1' }, [h('p', { class: 'muted', text: 'Chargement…' })]);
    wrap.appendChild(box); document.body.appendChild(wrap);
    S.el.wrap = wrap; S.el.box = box; box.focus();
    load();
  }
  function close() { if (!S.open) return; S.open = false; if (S.el.wrap) S.el.wrap.remove(); S.el = {}; }
  function toggle() { S.open ? close() : open(); }
  document.addEventListener('keydown', function (ev) {
    if (!S.open) return;
    if (ev.key === 'Escape' || ev.keyCode === 27 || ev.keyCode === 461 || ev.keyCode === 10009 || ev.keyCode === 8) { ev.preventDefault(); ev.stopPropagation(); close(); }
  }, true);

  function mount() {
    if (document.querySelector('.gc-acc-btn')) return;
    var right = document.querySelector('.skinHeader .headerRight');
    if (!right) return;
    if (!token()) return;
    css(); // sinon le SVG n'a ni taille ni couleur avant le premier clic (icône invisible après un rechargement)
    var b = h('button', { type: 'button', class: 'headerButton headerButtonRight paper-icon-button-light gc-acc-btn', title: 'Mon compte', 'aria-label': 'Mon compte', onclick: toggle }, [icon()]);
    var user = right.querySelector('.headerUserButton');
    if (user) right.insertBefore(b, user); else right.appendChild(b);
  }
  /* Mode « VO » (2026-09-25). Jellyfin n'accepte qu'une langue audio préférée par compte : le serveur pose le
     japonais (animés, sur tous les appareils) ; ici, pour un film ou une série qui n'est PAS un animé, si la
     lecture part sur la piste française alors qu'une piste d'origine existe (anglais d'abord, jamais
     l'audiodescription, jamais pour un titre d'origine française), on bascule dessus par la commande SetAudioStreamIndex envoyée à sa propre session.
     Une seule tentative par titre : un changement manuel du membre est respecté. (JoJo, parti en VF.) */
  var LANG_KEY = 'gc-lang-mode';
  function rememberMode(m) { try { if (m) localStorage.setItem(LANG_KEY, m); } catch (e) { /* stockage bloqué */ } }
  function langMode() { try { return localStorage.getItem(LANG_KEY); } catch (e) { return null; } }
  var FR = /^(fre|fra|fr)$/i;
  // TMDB donne la langue d'origine en ISO 639-1, les pistes sont en ISO 639-2 (deux formes pour certaines)
  var ISO2 = { en: ['eng'], ja: ['jpn'], ko: ['kor'], es: ['spa'], de: ['ger', 'deu'], it: ['ita'], zh: ['chi', 'zho'],
    cn: ['chi', 'zho'], pt: ['por'], ru: ['rus'], hi: ['hin'], nl: ['dut', 'nld'], sv: ['swe'], da: ['dan'],
    no: ['nor', 'nob'], pl: ['pol'], tr: ['tur'], ar: ['ara'], th: ['tha'], fi: ['fin'], he: ['heb'], id: ['ind'] };
  /* Piste à prendre si la lecture est partie en français (testable sans navigateur). `orig` = langue d'origine
     ISO 639-1 (null si inconnue). Origine française → rien ; origine connue → sa piste, sinon rien (un doublage
     anglais d'un film coréen n'est pas une VO) ; origine inconnue → l'anglais. Jamais l'audiodescription. */
  function pickOriginal(streams, current, orig) {
    if (orig && FR.test(orig)) return null;
    var audio = (streams || []).filter(function (x) { return x.Type === 'Audio'; });
    var cur = audio.filter(function (x) { return x.Index === current; })[0];
    if (!cur || !FR.test(cur.Language || '')) return null;
    var ok = audio.filter(function (x) {
      var t = ((x.Title || '') + ' ' + (x.DisplayTitle || '')).toLowerCase();
      return !FR.test(x.Language || '') && !/descript|audiodesc|\bad\b/.test(t);
    });
    var want = orig ? (ISO2[orig] || [orig]).concat([orig]) : ['eng', 'en'];
    var pick = ok.filter(function (x) { return want.indexOf((x.Language || '').toLowerCase()) >= 0; })[0];
    return pick ? pick.Index : null;
  }
  window.__gcVo = { pickOriginal: pickOriginal };
  var VO = { done: {}, busy: false, asked: false };
  function voTick() {
    if (VO.busy || !document.querySelector('video')) return;
    var mode = langMode();
    if (!mode) {
      if (!VO.asked && token()) { VO.asked = true; api('GET', '/api/me').then(function (d) { rememberMode(d.language); }).catch(function () {}); }
      return;
    }
    if (mode !== 'vo') return;
    var AC = window.ApiClient;
    if (!AC || !AC.getCurrentUserId || !AC.deviceId) return;
    VO.busy = true;
    var uid = AC.getCurrentUserId(), hdr = { 'X-Emby-Token': AC.accessToken(), 'Content-Type': 'application/json' };
    fetch(AC.getUrl('Sessions', { ControllableByUserId: uid }), { headers: hdr }).then(function (r) { return r.json(); })
      .then(function (ss) {
        var me = (ss || []).filter(function (x) { return x.DeviceId === AC.deviceId() && x.NowPlayingItem; })[0];
        if (!me || me.PlayState == null || me.PlayState.AudioStreamIndex == null) return;
        var id = me.NowPlayingItem.Id;
        if (VO.done[id]) return;
        VO.done[id] = true;
        var getJson = function (path) { return fetch(AC.getUrl(path), { headers: hdr }).then(function (r) { return r.json(); }); };
        return getJson('Users/' + uid + '/Items/' + id).then(function (it) {
          // langue d'origine : fiche du film, ou de la série pour un épisode
          var tmdbOf = function (x) { return x && x.ProviderIds && (x.ProviderIds.Tmdb || x.ProviderIds.tmdb); };
          var withSeries = it.SeriesId ? getJson('Users/' + uid + '/Items/' + it.SeriesId) : Promise.resolve(it);
          return withSeries.then(function (owner) {
            var tmdb = tmdbOf(owner);
            if (!tmdb) return null;
            return api('GET', '/api/original?kind=' + (it.SeriesId ? 'tv' : 'movie') + '&tmdb=' + encodeURIComponent(tmdb))
              .then(function (r) { return r.lang || null; }).catch(function () { return null; });
          }).then(function (orig) {
            window.__gcVoOrig = orig;
            var src = (it.MediaSources || []).filter(function (m) { return m.Id === me.PlayState.MediaSourceId; })[0] || (it.MediaSources || [])[0] || {};
            var target = pickOriginal(src.MediaStreams, me.PlayState.AudioStreamIndex, orig);
            if (target == null) return;
            window.__gcVoSwitched = target;
            return fetch(AC.getUrl('Sessions/' + me.Id + '/Command'), { method: 'POST', headers: hdr,
              body: JSON.stringify({ Name: 'SetAudioStreamIndex', Arguments: { Index: String(target) } }) });
          });
        });
      })
      .catch(function (e) { window.__gcVoError = String(e && e.message || e); })
      .then(function () { VO.busy = false; });
  }
  setInterval(voTick, TV ? 5000 : 2500);

  mount();
  // la taille est aussi relue à chaque tour : un changement fait dans Réglages → Sous-titres de Jellyfin suit
  setInterval(function () { mount(); applySubtitleSize(); }, TV ? 3000 : 1000);

  // ---- Onglet Demandes (Jellyfin Enhanced) : barre d'avancement sous chaque demande en cours -------------
  var REQ = { data: null, at: 0, timer: null };
  function reqCss() {
    if (document.getElementById('gc-req-css')) return;
    var st = document.createElement('style'); st.id = 'gc-req-css';
    st.textContent = [
      '.gc-req{display:block;width:100%;flex:0 0 100%;box-sizing:border-box;margin:.45em 0 .2em;font-size:.86em;color:#c9d4df}',
      '.gc-req .bar{height:6px;border-radius:999px;background:rgba(255,255,255,.12);overflow:hidden;margin:.3em 0}',
      '.gc-req .bar i{display:block;height:100%;border-radius:999px;background:linear-gradient(90deg,#3b8fd9,#6fd0ff);transition:width .6s}',
      '.gc-req.search .bar i{background:linear-gradient(90deg,#8a8f99,#b9c0cc)}.gc-req.import .bar i{background:linear-gradient(90deg,#3fae6b,#7ee0a2)}.gc-req.available .bar i{background:#3fae6b}',
      // jusqu'à 3 lignes (un libellé explicatif était coupé à « … » sur une seule ligne, 2026-09-25)
      '.gc-req .txt{white-space:normal;overflow:hidden;display:-webkit-box;-webkit-line-clamp:3;-webkit-box-orient:vertical;line-height:1.3}'
    ].join('\n');
    document.head.appendChild(st);
  }
  function reqCards() { return document.querySelectorAll('.je-request-card'); }
  function reqFetch() {
    if (Date.now() - REQ.at < 25000 && REQ.data) return Promise.resolve(REQ.data);
    return api('GET', '/api/requests').then(function (d) { REQ.data = d; REQ.at = Date.now(); return d; });
  }
  function reqPaint() {
    var cards = reqCards(); if (!cards.length) return;
    reqCss();
    reqFetch().then(function (d) {
      var byKey = {}, byPoster = {}, byTitle = {};
      (d.requests || []).forEach(function (r) {
        byKey[r.media_type + ':' + r.tmdb_id] = r;
        if (r.poster) byPoster[String(r.poster).split('/').pop()] = r;
        [r.title, r.original_title].forEach(function (t) { if (t) byTitle[(t + '|' + (r.year || '')).toLowerCase()] = r; });
      });
      cards.forEach(function (card) {
        var r = null;
        var el = card.querySelector('[data-tmdb-id]');
        if (el) {
          var type = el.getAttribute('data-media-type') || '';
          var tmdb = el.getAttribute('data-tmdb-id');
          r = byKey[(type === 'tv' ? 'tv' : 'movie') + ':' + tmdb] || byKey['tv:' + tmdb] || byKey['movie:' + tmdb];
        }
        if (!r) { var img = card.querySelector('img.je-request-poster, img'); if (img && img.src) r = byPoster[img.src.split('/').pop()] || null; }
        if (!r) {
          var tt = ((card.querySelector('.je-request-title') || {}).textContent || '').trim().toLowerCase();
          var yy = ((card.querySelector('.je-request-year') || {}).textContent || '').replace(/[()]/g, '').trim();
          r = byTitle[tt + '|' + yy] || byTitle[tt + '|'] || null;
        }
        var old = card.querySelector('.gc-req');
        if (!r || r.stage === 'available') { if (old) old.remove(); return; }
        var box = old || h('div', { class: 'gc-req' }, [h('div', { class: 'bar' }, [h('i')]), h('div', { class: 'txt' })]);
        box.className = 'gc-req ' + r.stage;
        box.querySelector('.bar i').style.width = r.percent + '%';
        box.querySelector('.txt').textContent = TV ? (r.percent + ' %') : r.label;
        box.querySelector('.txt').title = r.label; // texte complet au survol
        // sous la ligne « membre • date » (.je-request-meta est une rangée flex : y entrer la superposerait au texte),
        // dans la colonne .je-request-info
        if (!old) { var meta = card.querySelector('.je-request-meta'); if (meta) meta.insertAdjacentElement('afterend', box); else (card.querySelector('.je-request-info') || card).appendChild(box); }
      });
    }).catch(function () {});
  }
  setInterval(reqPaint, TV ? 6000 : 3000);
})();
