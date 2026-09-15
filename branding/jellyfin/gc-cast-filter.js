// « Lire sur » (diffuser vers un autre appareil) : ne propose que ses propres appareils connectés au même
// réseau que l'appareil courant (même adresse publique = même box, donc même Wi-Fi). Un appareil en données
// mobiles ou ailleurs, et ceux des autres comptes, n'apparaissent pas. Adresse courante inconnue ou privée :
// seul l'appareil courant est gardé (en cas de doute, on cache).
// Déposé dans JavaScript Injector (« Groscailloux Lire sur ») par scripts/jellyfin-js-apply.py.
// Chromecast et AirPlay (cibles du navigateur) ne passent pas par ici.
(function (root) {
  'use strict';

  function normalize(ep) {
    if (!ep) return '';
    var ip = String(ep).trim().toLowerCase();
    var m = ip.match(/^\[([^\]]+)\](?::\d+)?$/); // [v6]:port
    if (m) ip = m[1];
    else if (/^\d+\.\d+\.\d+\.\d+:\d+$/.test(ip)) ip = ip.replace(/:\d+$/, ''); // v4:port
    if (ip.indexOf('::ffff:') === 0 && ip.indexOf('.') > 0) ip = ip.slice(7); // v4 dans v6
    return ip;
  }

  function isPrivate(ip) {
    if (!ip) return true;
    if (ip.indexOf(':') >= 0) return ip === '::1' || /^f[cd]/.test(ip) || /^fe80/.test(ip);
    var p = ip.split('.').map(Number);
    if (p.length !== 4 || p.some(isNaN)) return true;
    return p[0] === 10 || p[0] === 127 || (p[0] === 172 && p[1] >= 16 && p[1] <= 31) ||
      (p[0] === 192 && p[1] === 168) || (p[0] === 100 && p[1] >= 64 && p[1] <= 127) || (p[0] === 169 && p[1] === 254);
  }

  function prefix64(ip) {
    var parts = ip.split('::');
    var head = parts[0] ? parts[0].split(':') : [];
    var tail = parts.length > 1 && parts[1] ? parts[1].split(':') : [];
    var full = head.concat(new Array(Math.max(0, 8 - head.length - tail.length)).fill('0'), tail);
    return full.slice(0, 4).map(function (h) { return parseInt(h || '0', 16); }).join(':');
  }

  function sameNetwork(a, b) {
    a = normalize(a); b = normalize(b);
    if (!a || !b || isPrivate(a) || isPrivate(b)) return false;
    var v6a = a.indexOf(':') >= 0, v6b = b.indexOf(':') >= 0;
    if (v6a !== v6b) return false;
    return v6a ? prefix64(a) === prefix64(b) : a === b;
  }

  /* sessions de getSessions → celles à proposer (l'appareil courant est ensuite retiré par Jellyfin) */
  function filterSessions(sessions, myDeviceId, myUserId) {
    var list = sessions || [];
    var mine = list.filter(function (s) { return s.DeviceId === myDeviceId; })[0];
    return list.filter(function (s) {
      if (s.DeviceId === myDeviceId) return true;
      if (!mine || s.UserId !== myUserId) return false;
      return sameNetwork(s.RemoteEndPoint, mine.RemoteEndPoint);
    });
  }

  root.__gcCastFilter = { normalize: normalize, isPrivate: isPrivate, sameNetwork: sameNetwork, filterSessions: filterSessions };

  if (typeof window === 'undefined' || root !== window) return; // tests
  function patch() {
    var api = window.ApiClient;
    if (!api || typeof api.getSessions !== 'function') return false;
    var proto = Object.getPrototypeOf(api);
    var host = proto && typeof proto.getSessions === 'function' ? proto : api;
    if (host.__gcCastPatched) return true;
    var original = host.getSessions;
    host.getSessions = function (query) {
      var client = this, result = original.apply(this, arguments);
      if (!query || !query.ControllableByUserId || !result || typeof result.then !== 'function') return result;
      return result.then(function (sessions) {
        try {
          return filterSessions(sessions, client.deviceId(), client.getCurrentUserId());
        } catch (e) {
          window.__gcCastError = String(e && e.message || e);
          return (sessions || []).filter(function (s) { return s.DeviceId === client.deviceId(); });
        }
      });
    };
    host.__gcCastPatched = true;
    return true;
  }
  if (!patch()) {
    var tries = 0, t = setInterval(function () { if (patch() || ++tries > 120) clearInterval(t); }, 500);
  }
})(typeof window !== 'undefined' ? window : globalThis);
