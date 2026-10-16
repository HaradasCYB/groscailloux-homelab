// Rend og-card.html en PNG 1200x630 (image d'aperçu des liens, docs/runbooks/npm.md § 2), dans l'image Chromium du banc :
//
//   sudo docker run --rm --cpus=1 --memory=1g -e NODE_PATH=/usr/src/app/node_modules \
//     -v /opt/homelab/branding/jellyfin/logo:/in:ro -v <dossier de sortie, inscriptible par l'uid 1000>:/out \
//     --entrypoint node <image zenika/alpine-chrome:with-puppeteer de tools/bench> /in/og-card-render.js
//
// Puis passer en RVB (PNG opaque, ~210 Ko au lieu de ~310 Ko) : python3 -I -c "from PIL import Image;
// Image.open('og-card.png').convert('RGB').save('og-card.png', optimize=True)". Archivo vient de Google Fonts (réseau
// requis) : vérifier dans la sortie que les polices chargées sont bien « Archivo », sinon Chromium en a pris une autre.
const puppeteer = require('puppeteer');
(async () => {
  const b = await puppeteer.launch({ headless: 'new', args: ['--no-sandbox', '--disable-dev-shm-usage'] });
  const p = await b.newPage();
  await p.setViewport({ width: 1200, height: 630, deviceScaleFactor: 1 });
  await p.goto('file:///in/og-card.html', { waitUntil: 'networkidle0', timeout: 30000 });
  await p.evaluate(() => document.fonts.ready);
  const fam = await p.evaluate(() => [...document.fonts].filter(f => f.status === 'loaded').map(f => f.family + ' ' + f.weight + ' ' + f.stretch));
  console.log('polices chargées :', JSON.stringify(fam));
  const el = await p.$('#og');
  await el.screenshot({ path: '/out/og-card.png', type: 'png' });
  await b.close();
})().catch(e => { console.error(e); process.exit(1); });
