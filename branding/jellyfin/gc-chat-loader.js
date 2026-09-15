// Chargeur du tchat Groscailloux, déposé dans le plugin JavaScript Injector (script « Groscailloux Tchat »,
// « Requires authentication » coché). Le tchat lui-même est servi par homelabd sous /gc-chat/app.js.
(function () {
  if (window.__gcChatLoader) return;
  window.__gcChatLoader = true;
  var s = document.createElement('script');
  s.src = '/gc-chat/app.js';
  s.defer = true;
  document.head.appendChild(s);
})();
