  (function () {
    var input = document.getElementById('compte');
    function compte() { return (input.value || '').trim(); }
    function valid() { return /^[a-zA-Z0-9_-]{2,32}$/.test(compte()); }
    // compte déjà abonné (abonnement PayPal actif) : aucun bouton, pas de second prélèvement
    if (document.getElementById('premium').getAttribute('data-deja') === '1') return;
    if (!window.paypal) return;
    paypal.Buttons({
      style: { shape: 'rect', color: 'gold', layout: 'vertical', label: 'subscribe' },
      onInit: function (data, actions) {
        if (!valid()) actions.disable();
        input.addEventListener('input', function () { valid() ? actions.enable() : actions.disable(); });
      },
      onClick: function () {
        if (!valid()) { input.focus(); document.getElementById('who-hint').textContent = 'Indique d\'abord ton nom de compte (2 à 32 lettres, chiffres, tiret ou souligné).'; }
      },
      createSubscription: function (data, actions) {
        return actions.subscription.create({ plan_id: '{{PLAN_ID}}', custom_id: compte() });
      },
      onApprove: function (data) {
        document.getElementById('sub_id').value = data.subscriptionID || '';
        document.getElementById('sub_compte').value = compte();
        document.getElementById('linking').hidden = false;
        document.getElementById('link').submit();
      }
    }).render('#paypal-button-container');
  })();
