  (function () {
    var q = new URLSearchParams(window.location.search);
    var msgs = {
      '1': "Merci d'indiquer le nom exact de ton compte avant d'envoyer.",
      rate: 'Doucement : réessaie dans quelques secondes.',
      id: "L'identifiant d'abonnement doit ressembler à I-XXXXXXXXXXXX.",
      nopaypal: "Le rattachement automatique n'est pas disponible : envoie une demande à l'administrateur.",
      paypal: "PayPal ne reconnaît pas cet identifiant. Vérifie-le, ou envoie une demande à l'administrateur.",
      status: "Cet abonnement n'est pas encore actif côté PayPal (statut : " + (q.get('s') || '?') + "). Réessaie dans quelques minutes.",
      compte: "Cet abonnement a été souscrit pour un autre nom de compte. Indique le même, ou écris à l'administrateur.",
      nocompte: 'Indique ton nom de compte pour rattacher cet abonnement.',
      inconnu: 'Aucun compte Groscailloux ne porte ce nom. Vérifie l\'orthographe.',
      deja: "Ce compte a déjà un abonnement PayPal actif, qui se renouvelle tout seul : ce nouvel abonnement n'a pas été rattaché et ne prolonge rien. Résilie-le dans ton compte PayPal → Paiements automatiques ; l'administrateur est prévenu s'il a déjà été débité.",
      interne: 'Erreur de notre côté, réessaie dans un instant ou écris à l\'administrateur.'
    };
    if (q.has('ok')) { document.getElementById('ok').hidden = false; }
    if (q.has('err')) { var e = document.getElementById('err'); e.textContent = msgs[q.get('err')] || msgs['1']; e.hidden = false; }
    if (q.has('sub')) { document.getElementById('subscription_id').value = q.get('sub'); }
    if (q.has('compte')) { document.getElementById('compte').value = q.get('compte'); }
  })();
