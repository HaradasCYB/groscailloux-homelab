// Onboarding admin (GET /) : création d'un compte par POST /onboard (session d'administration).
// Inclus en ligne par web.rs (index_page) ; ne jamais écrire la balise de fin de script ici.
const form = document.getElementById('onboardForm');
const btn = document.getElementById('submitBtn');
const resultBox = document.getElementById('result');
const toast = document.getElementById('toast');
// La session d'administration (cookie) suffit ; l'ancien jeton gardé sur l'appareil est effacé.
try { localStorage.removeItem('onboardToken'); } catch {}
const esc = s => String(s ?? '').replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));

function showToast(msg) { toast.textContent = msg; toast.classList.add('visible'); setTimeout(() => toast.classList.remove('visible'), 2000); }
function copyToClipboard(text) { navigator.clipboard.writeText(text).then(() => showToast('Copié !'), () => showToast('Échec de la copie')); }

function renderSuccess(d) {
  const pwdLine = d.password ? `Password : ${d.password}\n` : '';
  const credsBlock = `Username : ${d.username}\nEmail    : ${d.email}\n${pwdLine}Lien     : ${d.link_url || '—'}\n\n🎬 Streaming  : ${d.jellyfin_url}\n🎯 Requêtes   : ${d.jellyseerr_url}`;
  const linkState = d.dry_run ? 'non émis (dry-run)' : (d.mail_sent ? 'envoyé par mail, valable 1 h' : 'NON envoyé — transmettre le lien ci-dessous à la main');
  resultBox.className = 'result visible success';
  resultBox.innerHTML = `
    <h3 class="result-title">${d.dry_run ? '🧪 Dry-run OK (aucun compte créé)' : '✓ Compte créé avec succès'}</h3>
    <div class="creds">
      <div><span class="key">Username</span> : ${esc(d.username)}</div>
      <div><span class="key">Email</span>    : ${esc(d.email)}</div>
      ${d.password ? `<div><span class="key">Password</span> : <span class="pwd">${esc(d.password)}</span></div>` : `<div><span class="key">Password</span> : <i>à définir par le membre via le lien</i></div>`}
      <div><span class="key">Lien</span>     : ${esc(linkState)}</div>
      ${d.link_url && !d.mail_sent ? `<div class="pwd" style="word-break:break-all">${esc(d.link_url)}</div>` : ''}
      <br>
      <div>🎬 <span class="key">Streaming</span>  : ${esc(d.jellyfin_url)}</div>
      <div>🎯 <span class="key">Requêtes</span>   : ${esc(d.jellyseerr_url)}</div>
    </div>
    <p class="note">${esc(d.log)}</p>
    <div class="actions">
      <button class="secondary" id="copyBtn" type="button">📋 Copier</button>
      <button class="secondary" id="newBtn" type="button">+ Nouveau compte</button>
    </div>`;
  document.getElementById('copyBtn').onclick = () => copyToClipboard(credsBlock);
  document.getElementById('newBtn').onclick = () => { form.reset(); resultBox.className = 'result'; resultBox.innerHTML = ''; document.getElementById('username').focus(); };
}
function renderError(message) {
  resultBox.className = 'result visible error';
  resultBox.innerHTML = `<h3 class="result-title">✗ Erreur</h3><p class="error-msg">${esc(message)}</p>`;
}
form.addEventListener('submit', async (e) => {
  e.preventDefault();
  btn.classList.add('loading'); btn.disabled = true;
  resultBox.className = 'result'; resultBox.innerHTML = '';
  const username = document.getElementById('username').value.trim();
  const email = document.getElementById('email').value.trim();
  try {
    const res = await fetch('/onboard', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ username, email }) });
    const data = await res.json();
    if (res.status === 401) {
      location.href = '/connexion?next=/';
      return;
    } else if (data.success) {
      renderSuccess(data);
    } else renderError(data.error || 'Erreur inconnue');
  } catch (err) { renderError('Erreur réseau : ' + err.message); }
  finally { btn.classList.remove('loading'); btn.disabled = false; }
});
