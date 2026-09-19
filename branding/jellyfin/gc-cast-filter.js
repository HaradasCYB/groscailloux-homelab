// « Lire sur » (diffuser vers un autre appareil) : ne propose que ses **propres** appareils connectés ; ceux
// des autres comptes n'apparaissent jamais. Jusqu'au 2026-09-19 il exigeait aussi la même adresse publique
// que l'appareil courant (« même box »), mais un iPhone derrière le Relais privé iCloud (ou un VPN) joint le
// serveur par deux adresses à la fois — la box et un relais — et sa propre TV disparaissait du menu. La
// condition réseau est retirée : le seul garde-fou qui compte est le compte.
// Déposé dans JavaScript Injector (« Groscailloux Lire sur ») par scripts/jellyfin-js-apply.py.
// Chromecast et AirPlay (cibles du navigateur) ne passent pas par ici ; « Google Cast non pris en charge »
// est normal hors Chrome/Android.
(function (root) {
  'use strict';

  /* sessions de getSessions → celles à proposer (l'appareil courant est ensuite retiré par Jellyfin) */
  function filterSessions(sessions, myDeviceId, myUserId) {
    return (sessions || []).filter(function (s) {
      return s.DeviceId === myDeviceId || (!!myUserId && s.UserId === myUserId);
    });
  }

  root.__gcCastFilter = { filterSessions: filterSessions };

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
