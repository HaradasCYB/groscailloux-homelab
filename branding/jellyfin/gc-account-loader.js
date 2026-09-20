// Chargeur de « Mon compte » Groscailloux, déposé dans le plugin JavaScript Injector (script
// « Groscailloux Mon compte », « Requires authentication » coché). La page elle-même est servie par
// homelabd sous /gc-compte/app.js (route NPM sur l'adresse de Jellyfin, comme le tchat).
(function () {
  if (window.__gcAccountLoader) return;
  window.__gcAccountLoader = true;
  var s = document.createElement('script');
  s.src = '/gc-compte/app.js';
  s.defer = true;
  document.head.appendChild(s);
})();
