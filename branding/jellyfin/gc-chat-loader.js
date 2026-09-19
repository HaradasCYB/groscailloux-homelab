// Chargeur du tchat Groscailloux, déposé dans le plugin JavaScript Injector (script « Groscailloux Tchat »,
// « Requires authentication » coché). Le tchat lui-même est servi par homelabd sous /gc-chat/app.js.
(function () {
  if (window.__gcChatLoader) return;
  window.__gcChatLoader = true;
  // Téléviseurs (webOS, Tizen…) : pas de tchat — télécommande sans clavier, et il coûte des sondages (2026-09-19).
  if (/web0?s|webos|tizen|smart-?tv|netcast|viera|bravia|hbbtv|aft[a-z]|android\s?tv|googletv|crkey/i.test(navigator.userAgent || '')
      || document.documentElement.classList.contains('layout-tv')) return;
  var s = document.createElement('script');
  s.src = '/gc-chat/app.js';
  s.defer = true;
  document.head.appendChild(s);
})();
