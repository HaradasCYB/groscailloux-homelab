// Aide à la lecture : Jellyfin n'adapte pas la qualité en cours de route (une seule qualité par session, pas
// d'ABR). Si la connexion faiblit, la lecture directe — qui exige le débit du fichier en continu — cale et le
// lecteur relance le flux en boucle (constaté le 2026-09-15 : 4,7 Mbit/s demandés, 2 à 4 Mbit/s disponibles).
// Ce script surveille la progression de l'image et, au troisième blocage en trois minutes, propose dans la
// page de passer en qualité réduite (2 Mbit/s). Le changement passe par le menu du lecteur (roue crantée →
// Qualité) : la lecture continue au même endroit. Jellyfin mémorise ce palier par appareil : sans rien faire, le
// membre y restait bloqué ensuite, même sur une bonne connexion (≈ 60 lectures réencodées à 1–3 Mbit/s en 10 jours,
// relevé du 2026-09-23). Le palier posé par ce script ne vaut donc que pour la lecture en cours : à la fin de la
// lecture (ou au chargement suivant), « Auto » est rétabli, sauf si le membre a changé la qualité lui-même entre-temps.
// Sur téléviseur, le bandeau se ferme seul et à la touche Retour, et les relevés sont espacés.
// Jamais de window.confirm/alert/prompt : ignorés par la WebView iPhone et Jellyfin Desktop.
// Déposé dans JavaScript Injector (« Groscailloux Qualité ») par scripts/jellyfin-js-apply.py.
(function (root) {
  'use strict';

  /* Téléviseurs (LG webOS, Tizen, Android TV…) : peu de puissance, et une télécommande sans pointeur.
     On surveille moins souvent, sans ombre portée, et le bandeau se ferme seul ou à la touche Retour. */
  var TV = (function () {
    try {
      if (typeof navigator === 'undefined') return false;
      if (/web0?s|webos|tizen|smart-?tv|netcast|viera|bravia|hbbtv|aft[a-z]|android\s?tv|googletv|crkey/i
        .test(navigator.userAgent || '')) return true;
      if (typeof window !== 'undefined' && window.matchMedia
        && matchMedia('(hover: none) and (pointer: none)').matches) return true;
      return (navigator.hardwareConcurrency || 8) <= 2;
    } catch (e) { return false; }
  })();

  var WINDOW_MS = 180000;   // fenêtre glissante : 3 minutes
  var MIN_STALLS = 3;       // blocages avant de proposer
  var COOLDOWN_MS = 15000;  // un même blocage n'est compté qu'une fois (≈ 30 s de gêne avant de proposer)
  var POLL_MS = TV ? 4000 : 2000;   // cadence de surveillance de l'image
  var HIDE_MS = TV ? 15000 : 25000; // le bandeau s'efface tout seul
  var START_GRACE_S = 10;   // les hésitations des 10 premières secondes ne comptent pas
  // Après un saut (barre de progression, « Passer l'intro », ±10 s) ou un (re)chargement du flux (reprise en milieu de
  // film, changement de piste, de sous-titres ou de qualité), la mise en tampon n'est pas un signe de connexion faible :
  // ces 8 s-là ne comptent pas (banc du 04/10 sous 12.1 : un seul saut comptait pour un blocage, trois sauts en
  // 3 minutes affichaient le bandeau à tort). Un vrai blocage qui dure au-delà est encore vu par le relevé de l'image.
  var QUIET_MS = 8000;
  var BITRATE = 2000000;    // 2 Mbit/s : ~3x de marge sur un 1080p en lecture directe

  /* L'image a-t-elle cessé d'avancer entre deux relevés ? hls.js absorbe les événements « waiting » :
     on regarde donc la progression réelle, c'est ce que le membre voit. (testable sans navigateur) */
  function isStall(prev, cur, minDelta) {
    if (!prev || !cur || cur.paused || cur.seeking || cur.ended) return false;
    if (cur.readyState < 2) return true;
    return cur.time - prev.time < (minDelta === undefined ? 0.2 : minDelta);
  }

  /* Blocages retenus dans la fenêtre, et faut-il proposer ? */
  function prune(stalls, now, windowMs) {
    return (stalls || []).filter(function (t) { return now - t < (windowMs || WINDOW_MS); });
  }

  function shouldOffer(stalls, now, opts) {
    var o = opts || {};
    return prune(stalls, now, o.windowMs).length >= (o.min || MIN_STALLS);
  }

  /* Choix dans le menu Qualité du lecteur (0 = Auto) : le plus haut palier sous le plafond, sinon le plus bas
     proposé. Les paliers dépendent du fichier (6 / 4 / 3 / 1,5 Mbit/s… pour un 1080p). */
  function pickBitrate(ids, cap) {
    var nums = (ids || []).map(Number).filter(function (n) { return n > 0; }).sort(function (a, b) { return b - a; });
    var under = nums.filter(function (n) { return n <= (cap || BITRATE); });
    if (under.length) return under[0];
    return nums.length ? nums[nums.length - 1] : null;
  }

  /* Mémoire du palier posé par ce script (localStorage de jellyfin-web, clés `maxbitrate-Video-<réseau>` et
     `enableautobitratebitrate-Video-<réseau>`, inchangées en 12.1). `snap` = clés qui portent NOTRE palier juste
     après le choix (`value`) — jamais celles d'un autre réseau, que le membre a pu fixer lui-même ; `restore` remet
     « Auto » pour chaque clé encore identique (le membre n'y a pas touché) et oublie la note. (testable sans navigateur) */
  var MARK = 'gc-quality-lowered';
  function bitrateKeys(store) {
    var out = [];
    for (var i = 0; i < store.length; i++) {
      var k = store.key(i);
      if (k && /^maxbitrate-Video-/.test(k)) out.push(k);
    }
    return out;
  }
  function snap(store, value) {
    var keys = bitrateKeys(store), m = {};
    keys.forEach(function (k) {
      var v = store.getItem(k);
      if (value === undefined || v === String(value)) m[k] = v;
    });
    store.setItem(MARK, JSON.stringify(m));
    return m;
  }
  /* jellyfin-web écrit ses clés après la fermeture de la feuille, de façon asynchrone : relevé quand notre palier
     y est (10 s au plus), au lieu d'un délai fixe qu'un appareil lent dépasserait (note prise trop tôt = ancienne
     valeur = « choix du membre » = jamais remis en Auto). */
  function hasValue(store, value) {
    return bitrateKeys(store).some(function (k) { return store.getItem(k) === String(value); });
  }
  function restore(store) {
    var raw = store.getItem(MARK);
    if (!raw) return 0;
    var m, n = 0;
    try { m = JSON.parse(raw) || {}; } catch (e) { m = {}; }
    Object.keys(m).forEach(function (k) {
      if (store.getItem(k) !== m[k]) return; // changé depuis : choix du membre, on n'y touche pas
      var auto = k.replace(/^maxbitrate-/, 'enableautobitratebitrate-');
      if (store.getItem(auto) !== 'true') { store.setItem(auto, 'true'); n++; }
    });
    store.removeItem(MARK);
    return n;
  }

  root.__gcQuality = { isStall: isStall, prune: prune, shouldOffer: shouldOffer, pickBitrate: pickBitrate,
    snap: snap, restore: restore, hasValue: hasValue, WINDOW_MS: WINDOW_MS, MIN_STALLS: MIN_STALLS, BITRATE: BITRATE,
    QUIET_MS: QUIET_MS };

  if (typeof window === 'undefined' || root !== window || !window.document) return; // tests

  function restoreAuto() {
    try { window.__gcQualityRestored = restore(window.localStorage); } catch (e) { /* stockage bloqué */ }
  }
  restoreAuto(); // un palier laissé par une lecture précédente (onglet fermé en pleine lecture)

  var stalls = [], offered = false, banner = null, hideTimer = null, keyHandler = null,
    watched = null, last = null, lastCount = 0, quietUntil = 0;

  function quiet() { quietUntil = Date.now() + QUIET_MS; }

  function style() {
    if (document.getElementById('gc-quality-style')) return;
    var css = [
      '.gc-q{position:fixed;left:50%;bottom:11vh;transform:translateX(-50%);z-index:100001;max-width:min(92vw,560px);',
      'display:flex;flex-wrap:wrap;align-items:center;gap:.6em;background:#0b0f16;color:#e8edf5;border:1px solid #232b3a;',
      'border-radius:.8em;box-shadow:0 12px 40px rgba(0,0,0,.5);padding:.75em 1em;font:15px/1.4 Inter,system-ui,sans-serif}',
      '.gc-q p{margin:0;flex:1 1 15em;min-width:12em}',
      '.gc-q small{display:block;color:#8d99ad;font-size:.82em;margin-top:.15em}',
      '.gc-q button{border:0;border-radius:.45em;padding:.4em .9em;font:inherit;cursor:pointer}',
      '.gc-q .gc-q-yes{background:var(--accentColor,#2f8fff);color:#fff;font-weight:600}',
      '.gc-q .gc-q-yes[disabled]{opacity:.6;cursor:default}',
      '.gc-q .gc-q-no{background:#232b3a;color:#e8edf5}',
      '.gc-q button:focus-visible{outline:2px solid #fff;outline-offset:2px}',
      // ordinateur et tablette (04/10, 12.1) : à 11vh du bas, le bandeau couvrait la rangée de boutons du lecteur
      // (Sous-titres, Favori, Minuterie, Aperçu des épisodes) pendant 25 s, dans les deux interfaces. En haut, sous la
      // barre Retour / SyncPlay / Diffuser ; sous les menus de Jellyfin (1300) et ses feuilles. Télé inchangée.
      TV ? '' : '@media (min-width:481px) and (min-height:501px){.gc-q{top:calc(env(safe-area-inset-top,0px) + 64px);bottom:auto;z-index:1200}}',
      // téléphone (04/10) : en bas, le bandeau couvrait les commandes du lecteur 12.1 (pause, audio, sous-titres,
      // barre de progression) pendant 25 s et captait les appuis. Portrait : dans la bande noire sous la barre du
      // haut ; paysage : une ligne compacte sous la barre. Sous les menus de Jellyfin (1300) et ses feuilles.
      '@media (max-width:480px){.gc-q{top:calc(env(safe-area-inset-top,0px) + 144px);bottom:auto;z-index:1200;left:.6em;right:.6em;transform:none;max-width:none}}',
      '@media (max-height:500px) and (orientation:landscape){.gc-q{top:calc(env(safe-area-inset-top,0px) + 56px);bottom:auto;z-index:1200;flex-wrap:nowrap;gap:.5em;padding:.45em .7em;font-size:13px;max-width:min(calc(100vw - 1.2em),640px)}',
      '.gc-q p{flex:1 1 auto;min-width:0}.gc-q small{display:none}.gc-q button{flex:none;white-space:nowrap;padding:.35em .75em}}',
      TV ? '.gc-q{box-shadow:none;font-size:17px}' : ''
    ].join('');
    var el = document.createElement('style');
    el.id = 'gc-quality-style';
    el.textContent = css;
    document.head.appendChild(el);
  }

  function close() {
    if (hideTimer) { clearTimeout(hideTimer); hideTimer = null; }
    if (keyHandler) { document.removeEventListener('keydown', keyHandler, true); keyHandler = null; }
    if (banner && banner.parentNode) banner.parentNode.removeChild(banner);
    banner = null;
  }

  /* Télécommande : Retour (webOS 461, Tizen 10009), Échap ou Retour arrière ferment le bandeau. */
  function onRemoteBack(e) {
    if (e.key === 'Escape' || e.key === 'Backspace' || e.key === 'GoBack'
      || e.keyCode === 461 || e.keyCode === 10009) close();
  }

  function waitFor(test, timeout) {
    return new Promise(function (resolve, reject) {
      var tries = 0, max = Math.ceil((timeout || 6000) / 200);
      var t = setInterval(function () {
        var got = test();
        if (got) { clearInterval(t); resolve(got); }
        else if (++tries >= max) { clearInterval(t); reject(new Error('délai dépassé')); }
      }, 200);
    });
  }

  /* Change la qualité par le menu du lecteur (roue crantée → Qualité) : c'est le chemin de l'application
     elle-même, donc la lecture repart au même endroit et le choix est mémorisé par l'appareil.
     (La commande SetMaxStreamingBitrate, elle, n'est pas gérée par le client web : « does not recognize ».) */
  function setBitrate() {
    var gear = document.querySelector('.btnVideoOsdSettings');
    if (!gear) return Promise.reject(new Error('lecteur introuvable'));
    gear.click();
    return waitFor(function () { return document.querySelector('.actionSheet [data-id="quality"]'); })
      .then(function (item) {
        item.click();
        return waitFor(function () { return document.querySelector('.actionSheet [data-id="0"]'); });
      })
      .then(function () {
        var items = [].slice.call(document.querySelectorAll('.actionSheet [data-id]'));
        var choice = pickBitrate(items.map(function (e) { return e.getAttribute('data-id'); }));
        var el = choice && items.filter(function (e) { return e.getAttribute('data-id') === String(choice); })[0];
        if (!el) throw new Error('palier introuvable');
        el.click();
        return choice;
      });
  }

  function offer() {
    offered = true;
    style();
    close();
    banner = document.createElement('div');
    banner.className = 'gc-q';
    banner.setAttribute('role', 'status');
    var p = document.createElement('p');
    p.textContent = 'Ta connexion ne tient pas la qualité maximale.';
    var s = document.createElement('small');
    s.textContent = 'Passer en qualité réduite ? La lecture continue où tu en es.';
    p.appendChild(s);
    var yes = document.createElement('button');
    yes.className = 'gc-q-yes';
    yes.textContent = 'Réduire la qualité';
    var no = document.createElement('button');
    no.className = 'gc-q-no';
    no.textContent = 'Ignorer';
    no.addEventListener('click', close);
    yes.addEventListener('click', function () {
      yes.disabled = true;
      yes.textContent = 'Un instant…';
      if (hideTimer) { clearTimeout(hideTimer); hideTimer = null; }
      setBitrate().then(function (choice) {
        stalls = [];
        quiet(); // la lecture repart (nouveau flux) : sa mise en tampon ne compte pas
        window.__gcQualityApplied = choice;
        // jellyfin-web écrit ses clés peu après le clic : relevé dès que notre palier y est (10 s au plus)
        var tries = 0;
        var wait = setInterval(function () {
          try {
            if (!hasValue(window.localStorage, choice) && ++tries < 40) return;
            clearInterval(wait);
            window.__gcQualitySnap = snap(window.localStorage, choice);
          } catch (e) { clearInterval(wait); /* stockage bloqué */ }
        }, 250);
        close();
      }).catch(function (e) {
        window.__gcQualityError = String((e && e.message) || e);
        s.textContent = 'À faire à la main : roue crantée du lecteur → Qualité → 1,5 Mbit/s.';
        yes.textContent = 'Réduire la qualité';
        yes.disabled = false;
        hideTimer = setTimeout(close, HIDE_MS);
      });
    });
    banner.appendChild(p);
    banner.appendChild(yes);
    banner.appendChild(no);
    document.body.appendChild(banner);
    keyHandler = onRemoteBack;
    document.addEventListener('keydown', keyHandler, true);
    if (TV) yes.focus(); // sans pointeur, le bouton doit être atteignable à la télécommande
    hideTimer = setTimeout(close, HIDE_MS);
  }

  function onStall() {
    var now = Date.now();
    // Les hésitations du démarrage (mise en tampon initiale) ne comptent pas, ni celles d'un saut ou d'un rechargement.
    if (!watched || !(watched.currentTime > START_GRACE_S)) return;
    if (watched.seeking || now < quietUntil) return;
    if (now - lastCount < COOLDOWN_MS) return; // un même blocage ne compte qu'une fois
    lastCount = now;
    stalls = prune(stalls.concat(now), now);
    window.__gcQualityStalls = stalls.length;
    if (!offered && shouldOffer(stalls, now)) offer();
  }

  function snapshot(video) {
    return { time: video.currentTime, paused: video.paused, seeking: video.seeking,
      ended: video.ended, readyState: video.readyState };
  }

  function watch(video) {
    if (!video || video === watched) return;
    watched = video;
    stalls = [];
    offered = false;
    last = null;
    close();
    quiet(); // reprise en milieu de film : la mise en tampon du départ ne compte pas
    video.addEventListener('seeking', quiet);
    video.addEventListener('seeked', quiet);
    video.addEventListener('loadstart', quiet);
    video.addEventListener('waiting', onStall);
    video.addEventListener('stalled', onStall);
    video.addEventListener('ended', close);
  }

  setInterval(function () {
    var video = document.querySelector('video');
    if (!video) {
      if (watched) { watched = null; stalls = []; offered = false; last = null; close(); restoreAuto(); }
      return;
    }
    watch(video);
    var cur = snapshot(video);
    if (isStall(last, cur)) onStall();
    last = cur;
  }, POLL_MS);
})(typeof window !== 'undefined' ? window : globalThis);
