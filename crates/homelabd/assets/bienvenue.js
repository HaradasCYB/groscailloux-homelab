// Pages publiques /bienvenue et /premiers-pas : bouton « Copier » de l'adresse du serveur et contrôle du mot de passe.
// Inclus en ligne par web.rs (public_page) ; ne jamais écrire la balise de fin de script ici.
(function () {
  // bouton « Copier » de l'adresse du serveur (sélectionne le texte si le presse-papiers est refusé)
  document.querySelectorAll('.copy[data-copy]').forEach(function (b) {
    b.addEventListener('click', function () {
      var t = b.getAttribute('data-copy');
      var sel = function () { var c = document.getElementById('gc-host'); if (!c) return; var r = document.createRange(); r.selectNodeContents(c); var w = getSelection(); w.removeAllRanges(); w.addRange(r); };
      try { navigator.clipboard.writeText(t).then(function () { b.textContent = 'Copié'; }, sel); } catch (e) { sel(); }
    });
  });
})();
(function () {
  var f = document.getElementById('pw-form'); if (!f) return;
  var a = document.getElementById('pw'), b = document.getElementById('pw2'), e = document.getElementById('pw-err'), btn = document.getElementById('pw-btn');
  var name = (f.getAttribute('data-username') || '').toLowerCase();
  function check() {
    var p = a.value, msg = '';
    if (p.length < 8) msg = 'Au moins 8 caractères.';
    else if (name && p.toLowerCase().indexOf(name) !== -1) msg = 'Le mot de passe ne doit pas contenir ton pseudo.';
    else if (b.value && p !== b.value) msg = 'Les deux mots de passe ne sont pas identiques.';
    e.textContent = msg; e.hidden = !msg; btn.disabled = !!msg || !b.value;
    return !msg;
  }
  a.addEventListener('input', check); b.addEventListener('input', check);
  f.addEventListener('submit', function (ev) { if (!check() || a.value !== b.value) { ev.preventDefault(); return; } btn.disabled = true; btn.textContent = 'Enregistrement…'; });
  check();
})();
