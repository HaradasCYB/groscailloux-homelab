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
  function passwordLink() {
    api('POST', '/api/password').then(function (r) {
      var w = window.open(r.url, '_blank', 'noopener');
      if (!w) note('Ouvre ce lien : ' + r.url, 'ok');
    }).catch(function (e) { note(e.message, 'err'); });
  }
  function load() {
    return api('GET', '/api/me').then(function (d) { S.data = d; render(d); })
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
    var b = h('button', { type: 'button', class: 'headerButton headerButtonRight paper-icon-button-light gc-acc-btn', title: 'Mon compte', 'aria-label': 'Mon compte', onclick: toggle }, [icon()]);
    var user = right.querySelector('.headerUserButton');
    if (user) right.insertBefore(b, user); else right.appendChild(b);
  }
  mount();
  setInterval(mount, TV ? 3000 : 1000);
})();
